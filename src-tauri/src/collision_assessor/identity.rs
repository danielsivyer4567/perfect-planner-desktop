//! Canonical, fail-closed identities for collision-assessor files and shared resources.
//!
//! Physical worktree paths are never used as logical file identities. Callers must supply the
//! Git common directory discovered by the read-only census; linked worktrees therefore share one
//! repository identity while independent repositories remain separate. Filesystem aliases are
//! resolved through the operating system before a repository-relative key is issued.

use std::fmt;
use std::fs;
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
    use std::sync::atomic::{AtomicU64, Ordering};

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

    #[cfg(windows)]
    fn create_directory_alias(link: &Path, target: &Path) {
        let output = std::process::Command::new("cmd")
            .args(["/D", "/S", "/C", "mklink", "/J"])
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
