//! Canonical, fail-closed identities for collision-assessor files and shared resources.
//!
//! Physical worktree paths are never used as logical file identities. Callers must supply the
//! Git common directory discovered by the read-only census; linked worktrees therefore share one
//! repository identity while independent repositories remain separate. Filesystem aliases are
//! resolved through the operating system before a repository-relative key is issued.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const ACCEPTED_RESOURCE_NAMESPACES: &[&str] = &[
    "api",
    "app-data",
    "artifact",
    "audio",
    "browser",
    "capability",
    "connector",
    "database",
    "deployment",
    "filesystem",
    "host",
    "mutex",
    "policy",
    "port",
    "process",
    "protocol",
    "recovery",
    "release",
    "remote",
    "runtime",
    "schema",
    "service",
    "supabase",
    "tauri",
    "test",
    "trust-boundary",
    "ui",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    MissingGitIdentity,
    InvalidGitIdentity(String),
    RepositoryMismatch,
    InvalidWorktree(String),
    EmptyPath,
    AbsolutePath,
    PathEscape,
    AmbiguousGlob,
    InvalidWindowsName(String),
    UnresolvedPath(String),
    OutsideRepository,
    AmbiguousNewPathCase(String),
    MissingResourceNamespace,
    UnsupportedResourceNamespace(String),
    InvalidResource(String),
    AmbiguousPhysicalIdentity(String),
    UnexpectedPhysicalType,
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingGitIdentity => write!(formatter, "Git common-directory identity is missing"),
            Self::InvalidGitIdentity(reason) => write!(formatter, "invalid Git identity: {reason}"),
            Self::RepositoryMismatch => {
                write!(formatter, "worktree Git identity does not match the repository identity")
            }
            Self::InvalidWorktree(reason) => write!(formatter, "invalid worktree: {reason}"),
            Self::EmptyPath => write!(formatter, "logical path is empty"),
            Self::AbsolutePath => write!(formatter, "logical path must be repository-relative"),
            Self::PathEscape => write!(formatter, "logical path escapes the repository"),
            Self::AmbiguousGlob => write!(formatter, "logical path contains an unexpanded glob"),
            Self::InvalidWindowsName(name) => {
                write!(formatter, "logical path contains an ambiguous Windows name: {name}")
            }
            Self::UnresolvedPath(path) => write!(formatter, "cannot resolve filesystem alias: {path}"),
            Self::OutsideRepository => {
                write!(formatter, "filesystem alias resolves outside the worktree")
            }
            Self::AmbiguousNewPathCase(path) => write!(
                formatter,
                "a non-ASCII Windows path must exist before it can receive a collision identity: {path}"
            ),
            Self::MissingResourceNamespace => write!(formatter, "resource namespace is missing"),
            Self::UnsupportedResourceNamespace(namespace) => {
                write!(formatter, "unsupported resource namespace: {namespace}")
            }
            Self::InvalidResource(reason) => write!(formatter, "invalid resource identity: {reason}"),
            Self::AmbiguousPhysicalIdentity(reason) => {
                write!(formatter, "ambiguous physical filesystem identity: {reason}")
            }
            Self::UnexpectedPhysicalType => {
                write!(formatter, "physical path has an unexpected filesystem type")
            }
        }
    }
}

impl std::error::Error for IdentityError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RepositoryIdentity {
    common_git_dir: PathBuf,
    key: String,
}

impl RepositoryIdentity {
    pub fn common_git_dir(&self) -> &Path {
        &self.common_git_dir
    }

    pub fn key(&self) -> &str {
        &self.key
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LogicalFileIdentity {
    pub repository_key: String,
    pub repository_relative_path: String,
    pub existed_at_assessment: bool,
}

impl LogicalFileIdentity {
    /// Length-prefixing prevents different repository/path pairs from producing the same key.
    pub fn key(&self) -> String {
        format!(
            "logical-file:v1:{}:{}{}",
            self.repository_key.len(),
            self.repository_key,
            self.repository_relative_path
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceIdentity {
    pub namespace: String,
    pub canonical_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeClaimPathKind {
    ExactFile,
    DirectoryTree,
}

pub(crate) struct NativeClaimPath {
    pub(crate) repository_relative_path: String,
    pub(crate) kind: NativeClaimPathKind,
    pub(crate) physical_alias: Option<PhysicalPathIdentity>,
    pub(crate) authority: NativeClaimAuthority,
}

pub(crate) struct NativeClaimAuthority {
    guard: RestrictedAuthorityHandle,
    expected_absent: Vec<PathBuf>,
}

pub(crate) struct NativeClaimAuthorityBundle {
    pub(crate) git: NativeGitAuthority,
    pub(crate) claims: Vec<NativeClaimAuthority>,
}

impl fmt::Debug for NativeClaimAuthorityBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeClaimAuthorityBundle")
            .field("claim_count", &self.claims.len())
            .finish_non_exhaustive()
    }
}

impl NativeClaimAuthorityBundle {
    pub(crate) fn revalidate(&self) -> Result<(), IdentityError> {
        self.git.revalidate()?;
        for claim in &self.claims {
            claim.revalidate()?;
        }
        Ok(())
    }
}

impl NativeClaimAuthority {
    pub(crate) fn revalidate(&self) -> Result<(), IdentityError> {
        self.guard.revalidate()?;
        for path in &self.expected_absent {
            match fs::symlink_metadata(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => {
                    return Err(IdentityError::AmbiguousPhysicalIdentity(
                        "a previously missing claim segment appeared during assessment".into(),
                    ))
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhysicalPathKind {
    Directory,
    RegularFile,
}

/// An operating-system-issued identity for an existing local path. Display paths are retained
/// only for native collection; equality authority is the volume/file tuple.
#[derive(Debug, Clone)]
pub(crate) struct PhysicalPathIdentity {
    pub(crate) canonical_path: PathBuf,
    pub(crate) volume_id: u64,
    pub(crate) file_id: [u8; 16],
}

impl PartialEq for PhysicalPathIdentity {
    fn eq(&self, other: &Self) -> bool {
        (self.volume_id, self.file_id) == (other.volume_id, other.file_id)
    }
}

impl Eq for PhysicalPathIdentity {}

impl PartialOrd for PhysicalPathIdentity {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PhysicalPathIdentity {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.volume_id, self.file_id).cmp(&(other.volume_id, other.file_id))
    }
}

/// Bind an existing local directory or regular file to its physical filesystem object.
///
/// Reparse points, remote or substituted drive mappings, and unstable/zero identifiers are
/// denied. Stable hardlinks deliberately share an identity and therefore collide. Callers must
/// re-run this function before consuming a snapshot; path text is never an identity fallback.
pub(crate) fn physical_path_identity(
    path: &Path,
    expected: PhysicalPathKind,
) -> Result<PhysicalPathIdentity, IdentityError> {
    if !path.is_absolute() {
        return Err(IdentityError::AmbiguousPhysicalIdentity(
            "path is not absolute".to_owned(),
        ));
    }
    reject_reparse_ancestors(path)?;
    platform_physical_path_identity(path, expected)
}

/// A share-restricted authority handle. Identity is derived from this exact open handle before
/// any bytes are read and can be rechecked from the same handle after the read. On Windows the
/// handle denies write/delete sharing for its full lifetime, closing swap-and-restore races.
pub(crate) struct RestrictedAuthorityHandle {
    file: File,
    expected: PhysicalPathIdentity,
    kind: PhysicalPathKind,
}

/// Native Git equality authority held open for the lifetime of claim derivation. The caller may
/// retain the opaque physical tuple, but path text is never an equality fallback.
pub(crate) struct NativeGitAuthority {
    pub(crate) common_dir: PhysicalPathIdentity,
    guards: Vec<RestrictedAuthorityHandle>,
    commondir_marker: PathBuf,
    commondir_was_present: bool,
}

impl NativeGitAuthority {
    pub(crate) fn revalidate(&self) -> Result<(), IdentityError> {
        for guard in &self.guards {
            guard.revalidate()?;
        }
        let present = match fs::symlink_metadata(&self.commondir_marker) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => {
                return Err(IdentityError::AmbiguousPhysicalIdentity(
                    "linked-worktree commondir cannot be inspected".into(),
                ))
            }
        };
        if present != self.commondir_was_present {
            return Err(IdentityError::AmbiguousPhysicalIdentity(
                "linked-worktree commondir presence changed".into(),
            ));
        }
        Ok(())
    }
}

impl RestrictedAuthorityHandle {
    pub(crate) fn open(
        path: &Path,
        kind: PhysicalPathKind,
        expected: &PhysicalPathIdentity,
    ) -> Result<Self, IdentityError> {
        Self::open_with_before_open(path, kind, expected, || {})
    }

    pub(crate) fn open_with_before_open<F>(
        path: &Path,
        kind: PhysicalPathKind,
        expected: &PhysicalPathIdentity,
        before_open: F,
    ) -> Result<Self, IdentityError>
    where
        F: FnOnce(),
    {
        let current = physical_path_identity(path, kind)?;
        if current != *expected {
            return Err(IdentityError::AmbiguousPhysicalIdentity(
                "authority identity changed before restricted open".into(),
            ));
        }
        before_open();
        let file = open_share_restricted(path, kind)?;
        let opened = identity_from_open_handle(&file, kind, &current.canonical_path)?;
        if opened != *expected {
            return Err(IdentityError::AmbiguousPhysicalIdentity(
                "restricted handle opened a different authority identity".into(),
            ));
        }
        Ok(Self {
            file,
            expected: expected.clone(),
            kind,
        })
    }

    pub(crate) fn read_bounded(&mut self, maximum: u64) -> Result<Vec<u8>, IdentityError> {
        if self.kind != PhysicalPathKind::RegularFile {
            return Err(IdentityError::UnexpectedPhysicalType);
        }
        self.revalidate()?;
        self.file.seek(SeekFrom::Start(0)).map_err(|_| {
            IdentityError::AmbiguousPhysicalIdentity("authority handle cannot seek".into())
        })?;
        let mut bytes = Vec::new();
        self.file
            .by_ref()
            .take(maximum.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| {
                IdentityError::AmbiguousPhysicalIdentity("authority handle cannot read".into())
            })?;
        if bytes.len() as u64 > maximum {
            return Err(IdentityError::AmbiguousPhysicalIdentity(
                "authority metadata exceeds the bounded read limit".into(),
            ));
        }
        self.revalidate()?;
        Ok(bytes)
    }

    pub(crate) fn revalidate(&self) -> Result<(), IdentityError> {
        let current =
            identity_from_open_handle(&self.file, self.kind, &self.expected.canonical_path)?;
        if current == self.expected {
            Ok(())
        } else {
            Err(IdentityError::AmbiguousPhysicalIdentity(
                "open authority handle identity changed".into(),
            ))
        }
    }
}

/// Resolve a worktree's real `.git` authority, including linked-worktree `gitdir` and
/// `commondir` indirection, while holding every metadata object share-restricted. The common
/// directory's operating-system file identity is the repository equality authority.
pub(crate) fn native_git_authority(
    worktree_root: &Path,
) -> Result<NativeGitAuthority, IdentityError> {
    let root_identity = physical_path_identity(worktree_root, PhysicalPathKind::Directory)?;
    let root_guard = RestrictedAuthorityHandle::open(
        worktree_root,
        PhysicalPathKind::Directory,
        &root_identity,
    )?;
    let dot_git = worktree_root.join(".git");
    let mut guards = vec![root_guard];

    let mut linked_dot_git_identity = None;
    let git_dir = if dot_git.is_dir() {
        let identity = physical_path_identity(&dot_git, PhysicalPathKind::Directory)?;
        guards.push(RestrictedAuthorityHandle::open(
            &dot_git,
            PhysicalPathKind::Directory,
            &identity,
        )?);
        identity.canonical_path
    } else if dot_git.is_file() {
        let identity = physical_path_identity(&dot_git, PhysicalPathKind::RegularFile)?;
        let mut guard =
            RestrictedAuthorityHandle::open(&dot_git, PhysicalPathKind::RegularFile, &identity)?;
        let text = bounded_utf8_authority(&mut guard, 16 * 1024)?;
        let declared = text
            .strip_prefix("gitdir: ")
            .ok_or_else(|| IdentityError::InvalidGitIdentity("invalid .git file".into()))?;
        let declared = declared.trim();
        if declared.is_empty() || declared.contains(['\0', '\r', '\n']) {
            return Err(IdentityError::InvalidGitIdentity(
                "invalid linked-worktree gitdir".into(),
            ));
        }
        let path = PathBuf::from(declared);
        let path = if path.is_absolute() {
            path
        } else {
            worktree_root.join(path)
        };
        linked_dot_git_identity = Some(identity.clone());
        guards.push(guard);
        let git_dir_identity = physical_path_identity(&path, PhysicalPathKind::Directory)?;
        guards.push(RestrictedAuthorityHandle::open(
            &path,
            PhysicalPathKind::Directory,
            &git_dir_identity,
        )?);
        git_dir_identity.canonical_path
    } else {
        return Err(IdentityError::MissingGitIdentity);
    };

    let common_marker = git_dir.join("commondir");
    let commondir_was_present = match fs::symlink_metadata(&common_marker) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => {
            return Err(IdentityError::AmbiguousPhysicalIdentity(
                "linked-worktree commondir cannot be inspected".into(),
            ))
        }
    };
    let common_dir_path = if commondir_was_present {
        let marker_identity =
            physical_path_identity(&common_marker, PhysicalPathKind::RegularFile)?;
        let mut marker_guard = RestrictedAuthorityHandle::open(
            &common_marker,
            PhysicalPathKind::RegularFile,
            &marker_identity,
        )?;
        let declared = bounded_utf8_authority(&mut marker_guard, 16 * 1024)?;
        let declared = declared.trim();
        if declared.is_empty() || declared.contains(['\0', '\r', '\n']) {
            return Err(IdentityError::InvalidGitIdentity(
                "invalid linked-worktree commondir".into(),
            ));
        }
        let path = PathBuf::from(declared);
        guards.push(marker_guard);
        if path.is_absolute() {
            path
        } else {
            git_dir.join(path)
        }
    } else {
        git_dir.clone()
    };

    let common_dir = physical_path_identity(&common_dir_path, PhysicalPathKind::Directory)?;
    guards.push(RestrictedAuthorityHandle::open(
        &common_dir_path,
        PhysicalPathKind::Directory,
        &common_dir,
    )?);
    for (path, kind) in [
        (
            common_dir.canonical_path.join("HEAD"),
            PhysicalPathKind::RegularFile,
        ),
        (
            common_dir.canonical_path.join("objects"),
            PhysicalPathKind::Directory,
        ),
    ] {
        let identity = physical_path_identity(&path, kind).map_err(|_| {
            IdentityError::InvalidGitIdentity("Git common directory is incomplete".into())
        })?;
        guards.push(RestrictedAuthorityHandle::open(&path, kind, &identity)?);
    }
    if let Some(dot_git_identity) = linked_dot_git_identity {
        let backlink = git_dir.join("gitdir");
        let backlink_identity = physical_path_identity(&backlink, PhysicalPathKind::RegularFile)
            .map_err(|_| {
                IdentityError::InvalidGitIdentity("linked worktree backlink is missing".into())
            })?;
        let mut backlink_guard = RestrictedAuthorityHandle::open(
            &backlink,
            PhysicalPathKind::RegularFile,
            &backlink_identity,
        )?;
        let declared = bounded_utf8_authority(&mut backlink_guard, 16 * 1024)?;
        let declared_path = PathBuf::from(declared.trim());
        let declared_path = if declared_path.is_absolute() {
            declared_path
        } else {
            git_dir.join(declared_path)
        };
        let declared_identity =
            physical_path_identity(&declared_path, PhysicalPathKind::RegularFile)?;
        if declared_identity != dot_git_identity {
            return Err(IdentityError::InvalidGitIdentity(
                "linked worktree backlink targets a different worktree".into(),
            ));
        }
        let worktrees_directory = git_dir
            .parent()
            .filter(|path| path.file_name().is_some_and(|name| name == "worktrees"))
            .ok_or_else(|| {
                IdentityError::InvalidGitIdentity("linked worktree admin path is invalid".into())
            })?;
        let membership = worktrees_directory.parent().ok_or_else(|| {
            IdentityError::InvalidGitIdentity("linked worktree admin path is invalid".into())
        })?;
        let membership_identity = physical_path_identity(membership, PhysicalPathKind::Directory)?;
        if membership_identity != common_dir {
            return Err(IdentityError::InvalidGitIdentity(
                "linked worktree admin directory is outside the Git common directory".into(),
            ));
        }
        guards.push(backlink_guard);
    }
    let authority = NativeGitAuthority {
        common_dir,
        guards,
        commondir_marker: common_marker,
        commondir_was_present,
    };
    authority.revalidate()?;
    Ok(authority)
}

fn bounded_utf8_authority(
    guard: &mut RestrictedAuthorityHandle,
    maximum: u64,
) -> Result<String, IdentityError> {
    let bytes = guard.read_bounded(maximum)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| IdentityError::InvalidGitIdentity("Git metadata is not UTF-8".into()))?;
    Ok(text.to_owned())
}

#[cfg(test)]
pub(crate) fn open_restricted_with_before_open<F>(
    path: &Path,
    kind: PhysicalPathKind,
    expected: &PhysicalPathIdentity,
    before_open: F,
) -> Result<RestrictedAuthorityHandle, IdentityError>
where
    F: FnOnce(),
{
    RestrictedAuthorityHandle::open_with_before_open(path, kind, expected, before_open)
}

#[cfg(windows)]
fn open_share_restricted(path: &Path, kind: PhysicalPathKind) -> Result<File, IdentityError> {
    use std::os::windows::fs::OpenOptionsExt;
    const GENERIC_READ: u32 = 0x8000_0000;
    const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    OpenOptions::new()
        .access_mode(if kind == PhysicalPathKind::RegularFile {
            GENERIC_READ
        } else {
            FILE_READ_ATTRIBUTES
        })
        .share_mode(FILE_SHARE_READ)
        .custom_flags(if kind == PhysicalPathKind::Directory {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            0
        })
        .open(path)
        .map_err(|_| {
            IdentityError::AmbiguousPhysicalIdentity(
                "authority cannot be opened with write/delete sharing denied".into(),
            )
        })
}

#[cfg(not(windows))]
fn open_share_restricted(_path: &Path, _kind: PhysicalPathKind) -> Result<File, IdentityError> {
    Err(IdentityError::AmbiguousPhysicalIdentity(
        "share-restricted authority handles are unavailable on this platform".into(),
    ))
}

#[cfg(windows)]
fn identity_from_open_handle(
    file: &File,
    expected: PhysicalPathKind,
    canonical_path: &Path,
) -> Result<PhysicalPathIdentity, IdentityError> {
    use std::os::windows::io::AsRawHandle;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    const FILE_ID_INFO_CLASS: i32 = 18;
    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    struct ByHandleFileInformation {
        attributes: u32,
        creation_time: FileTime,
        last_access_time: FileTime,
        last_write_time: FileTime,
        volume_serial_number: u32,
        file_size_high: u32,
        file_size_low: u32,
        number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }
    #[repr(C)]
    struct FileIdInfo {
        volume_serial_number: u64,
        file_id: [u8; 16],
    }
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(
            handle: *mut std::ffi::c_void,
            information: *mut ByHandleFileInformation,
        ) -> i32;
        fn GetFileInformationByHandleEx(
            handle: *mut std::ffi::c_void,
            information_class: i32,
            information: *mut std::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    let handle = file.as_raw_handle() as *mut std::ffi::c_void;
    let mut basic = std::mem::MaybeUninit::<ByHandleFileInformation>::zeroed();
    if unsafe { GetFileInformationByHandle(handle, basic.as_mut_ptr()) } == 0 {
        return Err(IdentityError::AmbiguousPhysicalIdentity(
            "open handle metadata identity is unavailable".into(),
        ));
    }
    let basic = unsafe { basic.assume_init() };
    if basic.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (basic.attributes & FILE_ATTRIBUTE_DIRECTORY != 0)
            != (expected == PhysicalPathKind::Directory)
    {
        return Err(IdentityError::UnexpectedPhysicalType);
    }
    let mut id = std::mem::MaybeUninit::<FileIdInfo>::zeroed();
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FILE_ID_INFO_CLASS,
            id.as_mut_ptr().cast(),
            std::mem::size_of::<FileIdInfo>() as u32,
        )
    } == 0
    {
        return Err(IdentityError::AmbiguousPhysicalIdentity(
            "open handle 128-bit file identity is unavailable".into(),
        ));
    }
    let id = unsafe { id.assume_init() };
    if id.volume_serial_number == 0 || id.file_id == [0; 16] {
        return Err(IdentityError::AmbiguousPhysicalIdentity(
            "open handle returned a zero identity".into(),
        ));
    }
    Ok(PhysicalPathIdentity {
        canonical_path: canonical_path.to_path_buf(),
        volume_id: id.volume_serial_number,
        file_id: id.file_id,
    })
}

#[cfg(not(windows))]
fn identity_from_open_handle(
    _file: &File,
    _expected: PhysicalPathKind,
    _canonical_path: &Path,
) -> Result<PhysicalPathIdentity, IdentityError> {
    Err(IdentityError::AmbiguousPhysicalIdentity(
        "open-handle identity is unavailable on this platform".into(),
    ))
}

/// Turn a read-only Git `--git-common-dir` result into the machine-local repository identity.
/// The full canonical path remains part of the equality key; no lossy hash is trusted for
/// collision decisions.
pub fn canonical_repository_identity(
    git_common_dir: Option<&Path>,
) -> Result<RepositoryIdentity, IdentityError> {
    let supplied = git_common_dir.ok_or(IdentityError::MissingGitIdentity)?;
    if !supplied.is_absolute() {
        return Err(IdentityError::InvalidGitIdentity(
            "common directory must be absolute".to_owned(),
        ));
    }
    let common_git_dir = supplied.canonicalize().map_err(|error| {
        IdentityError::InvalidGitIdentity(format!("common directory cannot be resolved: {error}"))
    })?;
    if !common_git_dir.is_dir()
        || !common_git_dir.join("HEAD").is_file()
        || !common_git_dir.join("objects").is_dir()
    {
        return Err(IdentityError::InvalidGitIdentity(
            "common directory is not a complete Git identity".to_owned(),
        ));
    }
    let key = format!(
        "git-common-dir:v1:{}",
        normalized_native_path(&common_git_dir)
    );
    Ok(RepositoryIdentity {
        common_git_dir,
        key,
    })
}

/// Resolve a declared repository-relative path to a stable logical identity.
///
/// Existing symlinks and junctions are followed. Internal aliases collapse to the target key;
/// aliases outside the worktree and dangling aliases are denied. A missing tail is allowed only
/// after its deepest existing parent has been resolved. On Windows a missing non-ASCII tail is
/// denied because portable Unicode case folding cannot prove NTFS name equivalence.
pub fn canonical_logical_file_identity(
    repository: &RepositoryIdentity,
    worktree_root: &Path,
    worktree_git_common_dir: Option<&Path>,
    declared_path: &str,
) -> Result<LogicalFileIdentity, IdentityError> {
    let observed_repository = canonical_repository_identity(worktree_git_common_dir)?;
    if observed_repository != *repository {
        return Err(IdentityError::RepositoryMismatch);
    }
    if !worktree_root.is_absolute() {
        return Err(IdentityError::InvalidWorktree(
            "root must be absolute".to_owned(),
        ));
    }
    let canonical_root = worktree_root.canonicalize().map_err(|error| {
        IdentityError::InvalidWorktree(format!("root cannot be resolved: {error}"))
    })?;
    if !canonical_root.is_dir() || !canonical_root.join(".git").exists() {
        return Err(IdentityError::InvalidWorktree(
            "root is not an existing Git worktree".to_owned(),
        ));
    }

    let lexical = normalize_declared_relative_path(declared_path)?;
    let candidate = canonical_root.join(&lexical);
    let (resolved, existed_at_assessment, missing_tail) = resolve_with_missing_tail(&candidate)?;

    #[cfg(windows)]
    if missing_tail
        .iter()
        .any(|component| !component.to_string_lossy().is_ascii())
    {
        return Err(IdentityError::AmbiguousNewPathCase(
            declared_path.to_owned(),
        ));
    }

    if !resolved.starts_with(&canonical_root) || resolved == canonical_root {
        return Err(IdentityError::OutsideRepository);
    }
    let relative = resolved
        .strip_prefix(&canonical_root)
        .map_err(|_| IdentityError::OutsideRepository)?;
    let repository_relative_path = normalized_repository_relative_path(relative)?;
    Ok(LogicalFileIdentity {
        repository_key: repository.key.clone(),
        repository_relative_path,
        existed_at_assessment,
    })
}

/// Native typed claim projection. Existing directories own a segment-bounded tree; existing
/// regular files and all missing paths are exact claims. A missing declaration with directory
/// syntax is intentionally unknowable, and globs are rejected by normalization.
pub(crate) fn canonical_native_claim_path(
    worktree_root: &Path,
    declared_path: &str,
) -> Result<NativeClaimPath, IdentityError> {
    if !worktree_root.is_absolute() {
        return Err(IdentityError::InvalidWorktree(
            "root must be absolute".into(),
        ));
    }
    let canonical_root = worktree_root.canonicalize().map_err(|error| {
        IdentityError::InvalidWorktree(format!("root cannot be resolved: {error}"))
    })?;
    let lexical = normalize_declared_relative_path(declared_path)?;
    let candidate = canonical_root.join(lexical);
    reject_existing_reparse_prefixes(&candidate)?;
    let (resolved, existed, missing_tail) = resolve_with_missing_tail(&candidate)?;
    #[cfg(windows)]
    if missing_tail
        .iter()
        .any(|component| !component.to_string_lossy().is_ascii())
    {
        return Err(IdentityError::AmbiguousNewPathCase(
            declared_path.to_owned(),
        ));
    }
    if !resolved.starts_with(&canonical_root) || resolved == canonical_root {
        return Err(IdentityError::OutsideRepository);
    }
    let relative = resolved
        .strip_prefix(&canonical_root)
        .map_err(|_| IdentityError::OutsideRepository)?;
    let repository_relative_path = normalized_repository_relative_path(relative)?;
    if !existed {
        if declared_path.trim_end().ends_with(['/', '\\']) {
            return Err(IdentityError::InvalidWorktree(
                "missing directory claims require native type evidence".into(),
            ));
        }
        let mut deepest_parent = resolved.clone();
        for _ in &missing_tail {
            deepest_parent.pop();
        }
        let parent_identity = physical_path_identity(&deepest_parent, PhysicalPathKind::Directory)?;
        let parent_guard = RestrictedAuthorityHandle::open(
            &deepest_parent,
            PhysicalPathKind::Directory,
            &parent_identity,
        )?;
        let mut expected_absent = Vec::with_capacity(missing_tail.len());
        let mut absence_cursor = deepest_parent.clone();
        for component in missing_tail.iter().rev() {
            absence_cursor.push(component);
            expected_absent.push(absence_cursor.clone());
        }
        return Ok(NativeClaimPath {
            repository_relative_path,
            kind: NativeClaimPathKind::ExactFile,
            physical_alias: None,
            authority: NativeClaimAuthority {
                guard: parent_guard,
                expected_absent,
            },
        });
    }
    let metadata = fs::metadata(&resolved)
        .map_err(|_| IdentityError::UnresolvedPath(declared_path.to_owned()))?;
    if metadata.is_dir() {
        let identity = physical_path_identity(&resolved, PhysicalPathKind::Directory)?;
        let guard =
            RestrictedAuthorityHandle::open(&resolved, PhysicalPathKind::Directory, &identity)?;
        guard.revalidate()?;
        Ok(NativeClaimPath {
            repository_relative_path,
            kind: NativeClaimPathKind::DirectoryTree,
            physical_alias: None,
            authority: NativeClaimAuthority {
                guard,
                expected_absent: Vec::new(),
            },
        })
    } else if metadata.is_file() {
        let identity = physical_path_identity(&resolved, PhysicalPathKind::RegularFile)?;
        let guard =
            RestrictedAuthorityHandle::open(&resolved, PhysicalPathKind::RegularFile, &identity)?;
        guard.revalidate()?;
        Ok(NativeClaimPath {
            repository_relative_path,
            kind: NativeClaimPathKind::ExactFile,
            physical_alias: Some(identity),
            authority: NativeClaimAuthority {
                guard,
                expected_absent: Vec::new(),
            },
        })
    } else {
        Err(IdentityError::UnexpectedPhysicalType)
    }
}

/// Normalize a declared shared-resource lock. Producers use symbolic, secret-free resource
/// names, never raw connection strings. Unknown namespaces and ambiguous syntax are denied.
pub fn canonical_resource_identity(raw: &str) -> Result<ResourceIdentity, IdentityError> {
    let trimmed = raw.trim();
    let (raw_namespace, raw_payload) = trimmed
        .split_once(':')
        .ok_or(IdentityError::MissingResourceNamespace)?;
    let mut namespace = raw_namespace.trim().to_ascii_lowercase();
    if namespace == "db" {
        namespace = "database".to_owned();
    }
    if !ACCEPTED_RESOURCE_NAMESPACES.contains(&namespace.as_str()) {
        return Err(IdentityError::UnsupportedResourceNamespace(namespace));
    }
    if raw_payload.trim().is_empty() {
        return Err(IdentityError::InvalidResource(
            "payload is empty".to_owned(),
        ));
    }
    if !raw_payload.is_ascii()
        || raw_payload.chars().any(char::is_control)
        || raw_payload.contains(['*', '?', '[', ']', '{', '}', '#'])
    {
        return Err(IdentityError::InvalidResource(
            "payload is non-ASCII, contains control text, or is ambiguous".to_owned(),
        ));
    }

    let mut components = Vec::new();
    for colon_component in raw_payload.split(':') {
        let component = normalize_resource_component(colon_component)?;
        components.push(component);
    }

    match namespace.as_str() {
        "port" => {
            if components.len() != 2 || !matches!(components[0].as_str(), "tcp" | "udp") {
                return Err(IdentityError::InvalidResource(
                    "port keys must be port:<tcp|udp>:<1-65535>".to_owned(),
                ));
            }
            let port = components[1].parse::<u16>().map_err(|_| {
                IdentityError::InvalidResource("port number is outside 1-65535".to_owned())
            })?;
            if port == 0 {
                return Err(IdentityError::InvalidResource(
                    "port zero cannot be reserved".to_owned(),
                ));
            }
            components[1] = port.to_string();
        }
        "schema" if components.len() < 2 => {
            return Err(IdentityError::InvalidResource(
                "schema keys require a database scope and schema name".to_owned(),
            ));
        }
        "deployment" if components.len() < 2 => {
            return Err(IdentityError::InvalidResource(
                "deployment keys require a provider scope and target".to_owned(),
            ));
        }
        _ => {}
    }

    if namespace == "remote" {
        if let Some(last) = components.last_mut() {
            if last.ends_with(".git") {
                last.truncate(last.len() - 4);
                if last.is_empty() {
                    return Err(IdentityError::InvalidResource(
                        "remote repository name is empty".to_owned(),
                    ));
                }
            }
        }
    }

    Ok(ResourceIdentity {
        namespace: namespace.clone(),
        canonical_key: format!("resource:v1:{namespace}:{}", components.join(":")),
    })
}

pub(crate) fn canonical_declared_path(raw: &str) -> Result<String, IdentityError> {
    let normalized = normalize_declared_relative_path(raw)?;
    normalized_repository_relative_path(&normalized)
}

/// Canonical text binding for a declared manifest entry. Unsupported glob syntax is retained in
/// the signed/census input so the participant cannot disappear, while native claim projection
/// still calls `canonical_declared_path` and therefore fails closed with `AmbiguousGlob`.
pub(crate) fn canonical_manifest_declaration(raw: &str) -> Result<String, IdentityError> {
    match canonical_declared_path(raw) {
        Ok(value) => Ok(value),
        Err(IdentityError::AmbiguousGlob) => {
            let trimmed = raw.trim();
            if trimmed.is_empty()
                || trimmed.starts_with(['/', '\\'])
                || trimmed.as_bytes().get(1).is_some_and(|byte| *byte == b':')
            {
                return Err(IdentityError::AbsolutePath);
            }
            let portable = trimmed.replace('\\', "/");
            let mut normalized = PathBuf::new();
            for component in portable.split('/') {
                if component.is_empty() || component == "." {
                    continue;
                }
                if component == ".." {
                    return Err(IdentityError::PathEscape);
                }
                validate_windows_component(component)?;
                normalized.push(component);
            }
            if normalized.as_os_str().is_empty() {
                return Err(IdentityError::EmptyPath);
            }
            normalized_repository_relative_path(&normalized)
        }
        Err(error) => Err(error),
    }
}

fn normalize_declared_relative_path(raw: &str) -> Result<PathBuf, IdentityError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(IdentityError::EmptyPath);
    }
    if trimmed.starts_with(['/', '\\'])
        || trimmed.as_bytes().get(1).is_some_and(|byte| *byte == b':')
    {
        return Err(IdentityError::AbsolutePath);
    }
    if trimmed.contains(['*', '?', '[', ']', '{', '}']) {
        return Err(IdentityError::AmbiguousGlob);
    }

    let portable = trimmed.replace('\\', "/");
    let mut normalized = PathBuf::new();
    for component in portable.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return Err(IdentityError::PathEscape);
        }
        validate_windows_component(component)?;
        normalized.push(component);
    }
    if normalized.as_os_str().is_empty() {
        return Err(IdentityError::EmptyPath);
    }
    Ok(normalized)
}

fn validate_windows_component(component: &str) -> Result<(), IdentityError> {
    if component.chars().any(char::is_control)
        || component.contains(['<', '>', ':', '"', '|'])
        || component.ends_with(['.', ' '])
    {
        return Err(IdentityError::InvalidWindowsName(component.to_owned()));
    }
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'));
    if reserved {
        return Err(IdentityError::InvalidWindowsName(component.to_owned()));
    }
    Ok(())
}

fn resolve_with_missing_tail(
    candidate: &Path,
) -> Result<(PathBuf, bool, Vec<std::ffi::OsString>), IdentityError> {
    let mut cursor = candidate.to_path_buf();
    let mut missing_tail = Vec::new();
    loop {
        match fs::symlink_metadata(&cursor) {
            Ok(_) => {
                let mut resolved = cursor
                    .canonicalize()
                    .map_err(|_| IdentityError::UnresolvedPath(candidate.display().to_string()))?;
                for component in missing_tail.iter().rev() {
                    resolved.push(component);
                }
                return Ok((resolved, missing_tail.is_empty(), missing_tail));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = cursor.file_name().ok_or_else(|| {
                    IdentityError::UnresolvedPath(candidate.display().to_string())
                })?;
                missing_tail.push(name.to_os_string());
                cursor = cursor
                    .parent()
                    .ok_or_else(|| IdentityError::UnresolvedPath(candidate.display().to_string()))?
                    .to_path_buf();
            }
            Err(_) => {
                return Err(IdentityError::UnresolvedPath(
                    candidate.display().to_string(),
                ));
            }
        }
    }
}

fn normalized_repository_relative_path(path: &Path) -> Result<String, IdentityError> {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.is_empty() || text.starts_with('/') {
        return Err(IdentityError::EmptyPath);
    }
    #[cfg(windows)]
    let text = text.to_lowercase();
    Ok(text)
}

fn normalize_resource_component(raw: &str) -> Result<String, IdentityError> {
    let portable = raw.trim().replace('\\', "/");
    if portable.is_empty() || portable.starts_with('/') || portable.ends_with('/') {
        return Err(IdentityError::InvalidResource(
            "resource component is empty or absolute".to_owned(),
        ));
    }
    let mut pieces = Vec::new();
    for piece in portable.split('/') {
        if piece.is_empty() || piece == "." {
            continue;
        }
        if piece == ".."
            || !piece.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '@')
            })
        {
            return Err(IdentityError::InvalidResource(format!(
                "unsupported resource component: {raw}"
            )));
        }
        pieces.push(piece.to_ascii_lowercase());
    }
    if pieces.is_empty() {
        return Err(IdentityError::InvalidResource(
            "resource component normalizes to empty".to_owned(),
        ));
    }
    Ok(pieces.join("/"))
}

fn reject_reparse_ancestors(path: &Path) -> Result<(), IdentityError> {
    let mut cursor = Some(path);
    while let Some(candidate) = cursor {
        let metadata = fs::symlink_metadata(candidate).map_err(|_| {
            IdentityError::AmbiguousPhysicalIdentity(
                "an existing authority path cannot be inspected".to_owned(),
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(IdentityError::AmbiguousPhysicalIdentity(
                "a symbolic-link authority path is not accepted".to_owned(),
            ));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(IdentityError::AmbiguousPhysicalIdentity(
                    "a reparse-point authority path is not accepted".to_owned(),
                ));
            }
        }
        cursor = candidate.parent();
    }
    Ok(())
}

fn reject_existing_reparse_prefixes(path: &Path) -> Result<(), IdentityError> {
    let mut cursor = path;
    loop {
        match fs::symlink_metadata(cursor) {
            Ok(_) => return reject_reparse_ancestors(cursor),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                cursor = cursor.parent().ok_or_else(|| {
                    IdentityError::AmbiguousPhysicalIdentity(
                        "declared claim has no inspectable parent authority".into(),
                    )
                })?;
            }
            Err(_) => {
                return Err(IdentityError::AmbiguousPhysicalIdentity(
                    "declared claim authority cannot be inspected".into(),
                ))
            }
        }
    }
}

#[cfg(windows)]
fn platform_physical_path_identity(
    path: &Path,
    expected: PhysicalPathKind,
) -> Result<PhysicalPathIdentity, IdentityError> {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Prefix;

    const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    const FILE_ID_INFO_CLASS: i32 = 18;
    const DRIVE_FIXED: u32 = 3;
    const INVALID_HANDLE_VALUE: *mut std::ffi::c_void = -1_isize as *mut std::ffi::c_void;

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    struct ByHandleFileInformation {
        attributes: u32,
        creation_time: FileTime,
        last_access_time: FileTime,
        last_write_time: FileTime,
        volume_serial_number: u32,
        file_size_high: u32,
        file_size_low: u32,
        number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }

    #[repr(C)]
    struct FileIdInfo {
        volume_serial_number: u64,
        file_id: [u8; 16],
    }

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *mut std::ffi::c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: *mut std::ffi::c_void,
        ) -> *mut std::ffi::c_void;
        fn GetFileInformationByHandle(
            handle: *mut std::ffi::c_void,
            information: *mut ByHandleFileInformation,
        ) -> i32;
        fn GetFileInformationByHandleEx(
            handle: *mut std::ffi::c_void,
            information_class: i32,
            information: *mut std::ffi::c_void,
            size: u32,
        ) -> i32;
        fn GetDriveTypeW(root_path_name: *const u16) -> u32;
        fn QueryDosDeviceW(device_name: *const u16, target_path: *mut u16, max_chars: u32) -> u32;
    }

    let drive = match path.components().next() {
        Some(std::path::Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => letter,
            _ => {
                return Err(IdentityError::AmbiguousPhysicalIdentity(
                    "UNC, device and volume aliases are not accepted as authority paths".to_owned(),
                ))
            }
        },
        _ => {
            return Err(IdentityError::AmbiguousPhysicalIdentity(
                "path has no local drive identity".to_owned(),
            ))
        }
    };
    let drive_root = format!("{}:\\", drive as char);
    let encoded_root = std::ffi::OsStr::new(&drive_root)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: `encoded_root` is a live, NUL-terminated UTF-16 buffer.
    if unsafe { GetDriveTypeW(encoded_root.as_ptr()) } != DRIVE_FIXED {
        return Err(IdentityError::AmbiguousPhysicalIdentity(
            "authority path is not on a fixed local drive".to_owned(),
        ));
    }
    let device = format!("{}:", drive as char);
    let encoded_device = std::ffi::OsStr::new(&device)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut target = vec![0_u16; 4096];
    // SAFETY: input and output buffers are valid for the duration of this call.
    let target_len = unsafe {
        QueryDosDeviceW(
            encoded_device.as_ptr(),
            target.as_mut_ptr(),
            target.len() as u32,
        )
    };
    if target_len == 0 {
        return Err(IdentityError::AmbiguousPhysicalIdentity(
            "drive mapping cannot be resolved".to_owned(),
        ));
    }
    let mapped = String::from_utf16_lossy(&target[..target_len as usize])
        .trim_matches('\0')
        .to_owned();
    if mapped.starts_with(r"\??\") || !mapped.starts_with(r"\Device\HarddiskVolume") {
        return Err(IdentityError::AmbiguousPhysicalIdentity(
            "SUBST or mapped-drive authority paths are not accepted".to_owned(),
        ));
    }

    let canonical_path = path.canonicalize().map_err(|_| {
        IdentityError::AmbiguousPhysicalIdentity(
            "authority path cannot be canonically resolved".to_owned(),
        )
    })?;
    let encoded = canonical_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: the path buffer is valid and the returned handle is closed by the local guard.
    let handle = unsafe {
        CreateFileW(
            encoded.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(IdentityError::AmbiguousPhysicalIdentity(
            "authority path cannot be opened for identity".to_owned(),
        ));
    }
    struct Handle(*mut std::ffi::c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            #[link(name = "Kernel32")]
            unsafe extern "system" {
                fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
            }
            // SAFETY: this guard uniquely owns the handle.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
    let handle = Handle(handle);
    let mut basic = std::mem::MaybeUninit::<ByHandleFileInformation>::zeroed();
    // SAFETY: `basic` points to correctly sized writable storage.
    if unsafe { GetFileInformationByHandle(handle.0, basic.as_mut_ptr()) } == 0 {
        return Err(IdentityError::AmbiguousPhysicalIdentity(
            "authority path metadata identity is unavailable".to_owned(),
        ));
    }
    // SAFETY: the call above initialized `basic` on success.
    let basic = unsafe { basic.assume_init() };
    if basic.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(IdentityError::AmbiguousPhysicalIdentity(
            "authority path resolved to a reparse point".to_owned(),
        ));
    }
    let is_directory = basic.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    if is_directory != matches!(expected, PhysicalPathKind::Directory) {
        return Err(IdentityError::UnexpectedPhysicalType);
    }
    let mut file_id = std::mem::MaybeUninit::<FileIdInfo>::zeroed();
    // SAFETY: `file_id` is correctly sized writable storage for FILE_ID_INFO.
    if unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            FILE_ID_INFO_CLASS,
            file_id.as_mut_ptr().cast(),
            std::mem::size_of::<FileIdInfo>() as u32,
        )
    } == 0
    {
        return Err(IdentityError::AmbiguousPhysicalIdentity(
            "128-bit file identity is unavailable".to_owned(),
        ));
    }
    // SAFETY: the call above initialized `file_id` on success.
    let file_id = unsafe { file_id.assume_init() };
    if file_id.volume_serial_number == 0 || file_id.file_id == [0; 16] {
        return Err(IdentityError::AmbiguousPhysicalIdentity(
            "filesystem returned a zero authority identity".to_owned(),
        ));
    }
    Ok(PhysicalPathIdentity {
        canonical_path,
        volume_id: file_id.volume_serial_number,
        file_id: file_id.file_id,
    })
}

#[cfg(not(windows))]
fn platform_physical_path_identity(
    path: &Path,
    expected: PhysicalPathKind,
) -> Result<PhysicalPathIdentity, IdentityError> {
    use std::os::unix::fs::MetadataExt;

    let canonical_path = path.canonicalize().map_err(|_| {
        IdentityError::AmbiguousPhysicalIdentity(
            "authority path cannot be canonically resolved".to_owned(),
        )
    })?;
    let metadata = fs::metadata(&canonical_path).map_err(|_| {
        IdentityError::AmbiguousPhysicalIdentity(
            "authority path metadata is unavailable".to_owned(),
        )
    })?;
    if metadata.is_dir() != matches!(expected, PhysicalPathKind::Directory)
        || (!metadata.is_dir() && !metadata.is_file())
    {
        return Err(IdentityError::UnexpectedPhysicalType);
    }
    if metadata.dev() == 0 || metadata.ino() == 0 {
        return Err(IdentityError::AmbiguousPhysicalIdentity(
            "filesystem returned a zero authority identity".to_owned(),
        ));
    }
    let mut file_id = [0_u8; 16];
    file_id[..8].copy_from_slice(&metadata.ino().to_le_bytes());
    Ok(PhysicalPathIdentity {
        canonical_path,
        volume_id: metadata.dev(),
        file_id,
    })
}

#[cfg(windows)]
fn normalized_native_path(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let without_verbatim = if let Some(rest) = raw.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else {
        raw.strip_prefix("//?/").unwrap_or(&raw).to_owned()
    };
    without_verbatim.trim_end_matches('/').to_lowercase()
}

#[cfg(not(windows))]
fn normalized_native_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    struct Fixture {
        root: PathBuf,
        common: PathBuf,
        worktree_a: PathBuf,
        worktree_b: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "pp-collision-identity-{}-{}",
                std::process::id(),
                TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let common = root.join("repository.git");
            let worktree_a = root.join("worktree-a");
            let worktree_b = root.join("worktree-b");
            fs::create_dir_all(common.join("objects")).unwrap();
            fs::write(common.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
            for worktree in [&worktree_a, &worktree_b] {
                fs::create_dir_all(worktree.join(".git")).unwrap();
                fs::create_dir_all(worktree.join("src")).unwrap();
            }
            Self {
                root,
                common,
                worktree_a,
                worktree_b,
            }
        }

        fn repository(&self) -> RepositoryIdentity {
            canonical_repository_identity(Some(&self.common)).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn sibling_worktrees_share_repository_and_logical_file_keys() {
        let fixture = Fixture::new();
        fs::write(fixture.worktree_a.join("src/Module.rs"), b"a").unwrap();
        fs::write(fixture.worktree_b.join("src/Module.rs"), b"b").unwrap();
        let repository = fixture.repository();

        let left = canonical_logical_file_identity(
            &repository,
            &fixture.worktree_a,
            Some(&fixture.common),
            r".\src\Module.rs",
        )
        .unwrap();
        let right = canonical_logical_file_identity(
            &repository,
            &fixture.worktree_b,
            Some(&fixture.common),
            "src/Module.rs",
        )
        .unwrap();

        assert_eq!(left.repository_key, right.repository_key);
        assert_eq!(
            left.repository_relative_path,
            right.repository_relative_path
        );
        assert_eq!(left.key(), right.key());
        assert!(left.existed_at_assessment && right.existed_at_assessment);
    }

    #[test]
    fn missing_git_identity_and_mismatched_repository_fail_closed() {
        let fixture = Fixture::new();
        assert_eq!(
            canonical_repository_identity(None),
            Err(IdentityError::MissingGitIdentity)
        );

        let other_common = fixture.root.join("other.git");
        fs::create_dir_all(other_common.join("objects")).unwrap();
        fs::write(other_common.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        let error = canonical_logical_file_identity(
            &fixture.repository(),
            &fixture.worktree_a,
            Some(&other_common),
            "src/new.rs",
        )
        .unwrap_err();
        assert_eq!(error, IdentityError::RepositoryMismatch);
    }

    #[test]
    fn separators_dot_segments_and_missing_ascii_tails_are_deterministic() {
        let fixture = Fixture::new();
        let repository = fixture.repository();
        let left = canonical_logical_file_identity(
            &repository,
            &fixture.worktree_a,
            Some(&fixture.common),
            r".\src\\generated\.\new.rs",
        )
        .unwrap();
        let right = canonical_logical_file_identity(
            &repository,
            &fixture.worktree_a,
            Some(&fixture.common),
            "src/generated/new.rs",
        )
        .unwrap();
        assert_eq!(left.key(), right.key());
        assert!(!left.existed_at_assessment);
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_case_verbatim_prefixes_and_unc_paths_share_one_native_form() {
        assert_eq!(
            normalized_native_path(Path::new(r"C:\Repos\Planner\.git")),
            "c:/repos/planner/.git"
        );
        assert_eq!(
            normalized_native_path(Path::new(r"\\?\C:\REPOS\Planner\.git\")),
            "c:/repos/planner/.git"
        );
        assert_eq!(
            normalized_native_path(Path::new(r"\\?\UNC\Server\Share\Planner.git")),
            "//server/share/planner.git"
        );
        assert_eq!(
            normalized_native_path(Path::new(r"\\server\SHARE\Planner.git\")),
            "//server/share/planner.git"
        );
    }

    #[cfg(windows)]
    #[test]
    fn existing_windows_paths_collapse_declared_case_aliases() {
        let fixture = Fixture::new();
        fs::write(fixture.worktree_a.join("src/CaseSensitiveLooking.rs"), b"x").unwrap();
        let repository = fixture.repository();
        let stored = canonical_logical_file_identity(
            &repository,
            &fixture.worktree_a,
            Some(&fixture.common),
            "src/CaseSensitiveLooking.rs",
        )
        .unwrap();
        let alias = canonical_logical_file_identity(
            &repository,
            &fixture.worktree_a,
            Some(&fixture.common),
            "SRC/casesensitivelooking.RS",
        )
        .unwrap();
        assert_eq!(stored.key(), alias.key());
    }

    #[cfg(windows)]
    #[test]
    fn missing_non_ascii_windows_path_fails_closed_until_the_os_can_resolve_it() {
        let fixture = Fixture::new();
        assert!(matches!(
            canonical_logical_file_identity(
                &fixture.repository(),
                &fixture.worktree_a,
                Some(&fixture.common),
                "src/naïve/new.rs",
            ),
            Err(IdentityError::AmbiguousNewPathCase(_))
        ));
    }

    #[test]
    fn absolute_escape_glob_ads_reserved_and_trailing_dot_paths_are_denied() {
        let fixture = Fixture::new();
        let repository = fixture.repository();
        for path in [
            "C:/outside.rs",
            "../outside.rs",
            "src/*.rs",
            "src/file.rs:secret",
            "src/CON.txt",
            "src/name.",
        ] {
            assert!(
                canonical_logical_file_identity(
                    &repository,
                    &fixture.worktree_a,
                    Some(&fixture.common),
                    path,
                )
                .is_err(),
                "{path} must be denied"
            );
        }
    }

    #[test]
    fn internal_alias_collapses_and_external_or_dangling_alias_is_denied() {
        let fixture = Fixture::new();
        let internal = fixture.worktree_a.join("real");
        fs::create_dir_all(internal.join("nested")).unwrap();
        fs::write(internal.join("nested/file.rs"), b"internal").unwrap();
        create_directory_alias(&fixture.worktree_a.join("alias"), &internal);
        let repository = fixture.repository();
        let aliased = canonical_logical_file_identity(
            &repository,
            &fixture.worktree_a,
            Some(&fixture.common),
            "alias/nested/file.rs",
        )
        .unwrap();
        let direct = canonical_logical_file_identity(
            &repository,
            &fixture.worktree_a,
            Some(&fixture.common),
            "real/nested/file.rs",
        )
        .unwrap();
        assert_eq!(aliased.key(), direct.key());

        let outside = fixture.root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("outside.rs"), b"outside").unwrap();
        create_directory_alias(&fixture.worktree_a.join("outside-alias"), &outside);
        assert_eq!(
            canonical_logical_file_identity(
                &repository,
                &fixture.worktree_a,
                Some(&fixture.common),
                "outside-alias/outside.rs",
            ),
            Err(IdentityError::OutsideRepository)
        );

        let disappearing = fixture.worktree_a.join("disappearing");
        fs::create_dir_all(&disappearing).unwrap();
        create_directory_alias(&fixture.worktree_a.join("dangling"), &disappearing);
        fs::remove_dir_all(&disappearing).unwrap();
        assert!(matches!(
            canonical_logical_file_identity(
                &repository,
                &fixture.worktree_a,
                Some(&fixture.common),
                "dangling/file.rs",
            ),
            Err(IdentityError::UnresolvedPath(_))
        ));
    }

    #[test]
    fn resource_namespaces_are_canonical_secret_free_locks() {
        assert_eq!(
            canonical_resource_identity(" DB:Table:Public.Invoices ")
                .unwrap()
                .canonical_key,
            "resource:v1:database:table:public.invoices"
        );
        assert_eq!(
            canonical_resource_identity("remote:GitHub.com/Looplet/Planner.git")
                .unwrap()
                .canonical_key,
            "resource:v1:remote:github.com/looplet/planner"
        );
        assert_eq!(
            canonical_resource_identity("port:TCP:05230")
                .unwrap()
                .canonical_key,
            "resource:v1:port:tcp:5230"
        );
        assert_eq!(
            canonical_resource_identity("protocol:collision-assessor:v1")
                .unwrap()
                .canonical_key,
            "resource:v1:protocol:collision-assessor:v1"
        );
        for resource in [
            "database:postgres:accounts",
            "schema:accounts:public",
            "api:api.example.test/v1",
            "service:windows:postgresql",
            "deployment:azure:production",
            "supabase:function:invoice-pdf",
            "app-data:control-plane-ledger",
        ] {
            assert!(canonical_resource_identity(resource).is_ok(), "{resource}");
        }
    }

    #[test]
    fn unknown_ambiguous_and_under_scoped_resources_fail_closed() {
        for resource in [
            "mystery:anything",
            "port:tcp:*",
            "port:tcp:0",
            "schema:public",
            "deployment:production",
            "api:https://user:secret@example.test/v1",
            "service:../outside",
        ] {
            assert!(
                canonical_resource_identity(resource).is_err(),
                "{resource} must be denied"
            );
        }
    }

    #[test]
    fn physical_identity_distinguishes_objects_and_collapses_hardlinks() {
        let fixture = Fixture::new();
        let first = fixture.root.join("first.json");
        let second = fixture.root.join("second.json");
        fs::write(&first, b"same bytes").unwrap();
        fs::write(&second, b"same bytes").unwrap();
        let first_identity = physical_path_identity(&first, PhysicalPathKind::RegularFile).unwrap();
        let second_identity =
            physical_path_identity(&second, PhysicalPathKind::RegularFile).unwrap();
        assert_ne!(first_identity, second_identity);

        let alias = fixture.root.join("first-hardlink.json");
        fs::hard_link(&first, &alias).unwrap();
        let original = physical_path_identity(&first, PhysicalPathKind::RegularFile).unwrap();
        let hardlink = physical_path_identity(&alias, PhysicalPathKind::RegularFile).unwrap();
        assert_eq!(original, hardlink);
    }

    #[test]
    fn physical_identity_changes_when_a_path_is_recreated() {
        let fixture = Fixture::new();
        let path = fixture.root.join("replaceable.json");
        fs::write(&path, b"first").unwrap();
        let before = physical_path_identity(&path, PhysicalPathKind::RegularFile).unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"second").unwrap();
        let after = physical_path_identity(&path, PhysicalPathKind::RegularFile).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn physical_identity_rejects_directory_aliases() {
        let fixture = Fixture::new();
        let target = fixture.root.join("physical-target");
        let alias = fixture.root.join("physical-alias");
        fs::create_dir_all(&target).unwrap();
        create_directory_alias(&alias, &target);
        assert!(matches!(
            physical_path_identity(&alias, PhysicalPathKind::Directory),
            Err(IdentityError::AmbiguousPhysicalIdentity(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn restricted_registered_plan_handle_rejects_swap_before_attacker_bytes_are_read() {
        let fixture = Fixture::new();
        let authority = fixture.root.join("registered-plan.json");
        let attacker = fixture.root.join("attacker-plan.json");
        let backup = fixture.root.join("registered-plan.backup");
        let sentinel = "B04-SENTINEL-MUST-NEVER-BE-READ";
        fs::write(&authority, b"trusted").unwrap();
        fs::write(&attacker, sentinel.as_bytes()).unwrap();
        let expected = physical_path_identity(&authority, PhysicalPathKind::RegularFile).unwrap();
        let read_started = AtomicBool::new(false);

        let result = open_restricted_with_before_open(
            &authority,
            PhysicalPathKind::RegularFile,
            &expected,
            || {
                fs::rename(&authority, &backup).unwrap();
                fs::rename(&attacker, &authority).unwrap();
            },
        )
        .and_then(|mut handle| {
            read_started.store(true, Ordering::Release);
            handle.read_bounded(1024)
        });

        fs::rename(&authority, &attacker).unwrap();
        fs::rename(&backup, &authority).unwrap();
        assert!(matches!(
            &result,
            Err(IdentityError::AmbiguousPhysicalIdentity(_))
        ));
        assert!(!read_started.load(Ordering::Acquire));
        assert!(!format!("{result:?}").contains(sentinel));
    }

    #[cfg(windows)]
    #[test]
    fn restricted_inventory_entry_handle_rejects_swap_before_attacker_bytes_are_read() {
        let fixture = Fixture::new();
        let authority = fixture.root.join("inventory-entry.json");
        let attacker = fixture.root.join("inventory-attacker.json");
        let backup = fixture.root.join("inventory-entry.backup");
        let sentinel = "B04-INVENTORY-SENTINEL-MUST-NEVER-BE-READ";
        fs::write(&authority, b"trusted inventory").unwrap();
        fs::write(&attacker, sentinel.as_bytes()).unwrap();
        let expected = physical_path_identity(&authority, PhysicalPathKind::RegularFile).unwrap();
        let read_started = AtomicBool::new(false);

        let result = open_restricted_with_before_open(
            &authority,
            PhysicalPathKind::RegularFile,
            &expected,
            || {
                fs::rename(&authority, &backup).unwrap();
                fs::rename(&attacker, &authority).unwrap();
            },
        )
        .and_then(|mut handle| {
            read_started.store(true, Ordering::Release);
            handle.read_bounded(1024)
        });

        fs::rename(&authority, &attacker).unwrap();
        fs::rename(&backup, &authority).unwrap();
        assert!(matches!(
            &result,
            Err(IdentityError::AmbiguousPhysicalIdentity(_))
        ));
        assert!(!read_started.load(Ordering::Acquire));
        assert!(!format!("{result:?}").contains(sentinel));
    }

    #[cfg(windows)]
    #[test]
    fn restricted_directory_handle_rejects_junction_swap_and_restore() {
        let fixture = Fixture::new();
        let authority = fixture.root.join("inventory-directory");
        let backup = fixture.root.join("inventory-directory.backup");
        let outside = fixture.root.join("outside-directory");
        fs::create_dir_all(&authority).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let expected = physical_path_identity(&authority, PhysicalPathKind::Directory).unwrap();

        let result = open_restricted_with_before_open(
            &authority,
            PhysicalPathKind::Directory,
            &expected,
            || {
                fs::rename(&authority, &backup).unwrap();
                create_directory_alias(&authority, &outside);
            },
        );

        fs::remove_dir(&authority).unwrap();
        fs::rename(&backup, &authority).unwrap();
        assert!(matches!(
            result,
            Err(IdentityError::AmbiguousPhysicalIdentity(_))
                | Err(IdentityError::UnexpectedPhysicalType)
        ));
    }

    #[test]
    fn missing_claim_prefix_appearance_invalidates_retained_authority() {
        let fixture = Fixture::new();
        let claim = canonical_native_claim_path(&fixture.worktree_a, "src/generated/deep/new.rs")
            .expect("missing exact claim");
        fs::create_dir_all(fixture.worktree_a.join("src/generated")).unwrap();
        assert!(matches!(
            claim.authority.revalidate(),
            Err(IdentityError::AmbiguousPhysicalIdentity(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn declared_junction_component_is_denied_before_claim_projection() {
        let fixture = Fixture::new();
        let target = fixture.worktree_a.join("src/real");
        let alias = fixture.worktree_a.join("src/alias");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("file.rs"), b"x").unwrap();
        create_directory_alias(&alias, &target);
        assert!(matches!(
            canonical_native_claim_path(&fixture.worktree_a, "src/alias/file.rs"),
            Err(IdentityError::AmbiguousPhysicalIdentity(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn real_linked_worktrees_share_native_common_identity() {
        let root = std::env::temp_dir().join(format!(
            "pp-real-git-authority-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let repository = root.join("repository");
        let sibling = root.join("sibling");
        let unrelated = root.join("unrelated");
        let forged = root.join("forged");
        fs::create_dir_all(&repository).unwrap();
        fs::create_dir_all(&unrelated).unwrap();
        fs::create_dir_all(&forged).unwrap();
        let run = |cwd: &Path, args: &[&str]| {
            let output = std::process::Command::new("git")
                .current_dir(cwd)
                .args(args)
                .output()
                .expect("start git");
            assert!(
                output.status.success(),
                "git {:?}: {}{}",
                args,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&repository, &["init"]);
        run(
            &repository,
            &["config", "user.email", "collision@example.invalid"],
        );
        run(&repository, &["config", "user.name", "Collision Test"]);
        fs::write(repository.join("seed.txt"), b"seed").unwrap();
        run(&repository, &["add", "seed.txt"]);
        run(&repository, &["commit", "-m", "seed"]);
        run(
            &repository,
            &[
                "worktree",
                "add",
                "-b",
                "collision-sibling",
                sibling.to_str().unwrap(),
            ],
        );
        run(&unrelated, &["init"]);
        let primary = native_git_authority(&repository).expect("primary authority");
        let linked = native_git_authority(&sibling).expect("linked authority");
        let independent = native_git_authority(&unrelated).expect("independent authority");
        assert_eq!(
            (primary.common_dir.volume_id, primary.common_dir.file_id),
            (linked.common_dir.volume_id, linked.common_dir.file_id)
        );
        assert_ne!(
            (primary.common_dir.volume_id, primary.common_dir.file_id),
            (
                independent.common_dir.volume_id,
                independent.common_dir.file_id
            )
        );
        fs::copy(sibling.join(".git"), forged.join(".git")).unwrap();
        assert!(matches!(
            native_git_authority(&forged),
            Err(IdentityError::InvalidGitIdentity(_))
                | Err(IdentityError::AmbiguousPhysicalIdentity(_))
        ));
        primary.revalidate().unwrap();
        linked.revalidate().unwrap();
        independent.revalidate().unwrap();
        drop(primary);
        drop(linked);
        drop(independent);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn subst_drive_is_never_accepted_as_physical_authority() {
        struct SubstGuard(char);
        impl Drop for SubstGuard {
            fn drop(&mut self) {
                let _ = std::process::Command::new("subst")
                    .args([format!("{}:", self.0), "/D".into()])
                    .status();
            }
        }

        let root = std::env::temp_dir().join(format!(
            "pp-subst-authority-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("probe.txt"), b"physical authority probe").unwrap();
        let mapped = (b'D'..=b'Z').rev().find_map(|letter| {
            let letter = letter as char;
            if PathBuf::from(format!("{letter}:\\")).exists() {
                return None;
            }
            std::process::Command::new("subst")
                .arg(format!("{letter}:"))
                .arg(&root)
                .status()
                .ok()
                .filter(|status| status.success())
                .map(|_| letter)
        });
        let letter = mapped.expect(
            "SUBST proof fixture unavailable: no free drive letter or subst.exe was denied",
        );
        let guard = SubstGuard(letter);
        let alias = PathBuf::from(format!("{letter}:\\probe.txt"));
        assert!(matches!(
            physical_path_identity(&alias, PhysicalPathKind::RegularFile),
            Err(IdentityError::AmbiguousPhysicalIdentity(_))
        ));
        drop(guard);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    fn create_directory_alias(link: &Path, target: &Path) {
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "& { param($Link,$Target) New-Item -ItemType Junction -Path $Link -Target $Target | Out-Null }",
            ])
            .arg(link)
            .arg(target)
            .output()
            .expect("start mklink");
        assert!(
            output.status.success(),
            "mklink /J failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    fn create_directory_alias(link: &Path, target: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }
}
