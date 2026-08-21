use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const SCHEMA_VERSION: u32 = 1;
const MANIFEST_FILE: &str = "manifest.json";
const AUDIT_FILE: &str = "audit.jsonl";
const EVENTS_FILE: &str = "events.jsonl";
const HOT_RESUME_FILE: &str = "hot-resume.json";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct CreateRunScope {
    pub repository_root: PathBuf,
    pub run_id: String,
    pub branch: String,
    pub allowed_files: Vec<PathBuf>,
    pub next_actions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowedFileManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub repository_root: PathBuf,
    pub branch: String,
    pub allowed_files: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotResumeState {
    pub schema_version: u32,
    pub run_id: String,
    pub repository_root: PathBuf,
    pub branch: String,
    pub status: String,
    pub last_completed_step: Option<String>,
    pub locked_files: Vec<PathBuf>,
    pub next_actions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunScope {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub audit_path: PathBuf,
    pub events_path: PathBuf,
    pub hot_resume_path: PathBuf,
    pub manifest: AllowedFileManifest,
}

impl RunScope {
    pub fn create(request: CreateRunScope) -> Result<Self, String> {
        validate_repository(&request.repository_root)?;
        validate_id("run id", &request.run_id)?;
        validate_nonempty("branch", &request.branch)?;
        validate_actions(&request.next_actions)?;

        let allowed_files = normalize_allowed_files(&request.allowed_files)?;
        if allowed_files.is_empty() {
            return Err("allowed-file manifest must not be empty".to_string());
        }

        let repository_root = request
            .repository_root
            .canonicalize()
            .map_err(|error| format!("failed to canonicalize repository root: {error}"))?;
        let parent = repository_root
            .join(".claude")
            .join("scratch")
            .join("orchestrator");
        fs::create_dir_all(&parent)
            .map_err(|error| format!("failed to create orchestrator state directory: {error}"))?;

        let root = parent.join(&request.run_id);
        fs::create_dir(&root).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                format!(
                    "run scope {} already exists; refusing to overwrite it",
                    root.display()
                )
            } else {
                format!("failed to create run scope {}: {error}", root.display())
            }
        })?;

        let manifest = AllowedFileManifest {
            schema_version: SCHEMA_VERSION,
            run_id: request.run_id.clone(),
            repository_root: repository_root.clone(),
            branch: request.branch.clone(),
            allowed_files,
        };
        let hot_resume = HotResumeState {
            schema_version: SCHEMA_VERSION,
            run_id: request.run_id,
            repository_root,
            branch: request.branch,
            status: "ready".to_string(),
            last_completed_step: None,
            locked_files: Vec::new(),
            next_actions: request.next_actions,
        };

        let scope = Self {
            manifest_path: root.join(MANIFEST_FILE),
            audit_path: root.join(AUDIT_FILE),
            events_path: root.join(EVENTS_FILE),
            hot_resume_path: root.join(HOT_RESUME_FILE),
            root,
            manifest,
        };

        let initialization = (|| -> Result<(), String> {
            atomic_write_json(&scope.manifest_path, &scope.manifest)?;
            atomic_write(&scope.audit_path, b"")?;
            atomic_write(&scope.events_path, b"")?;
            atomic_write_json(&scope.hot_resume_path, &hot_resume)?;
            Ok(())
        })();

        if let Err(error) = initialization {
            // The directory was exclusively created above, so cleanup cannot touch a prior run.
            let _ = fs::remove_dir_all(&scope.root);
            return Err(error);
        }

        Ok(scope)
    }

    pub fn update_hot_resume(&self, state: &HotResumeState) -> Result<(), String> {
        self.validate_hot_resume(state)?;
        atomic_write_json(&self.hot_resume_path, state)
    }

    pub fn read_hot_resume(&self) -> Result<HotResumeState, String> {
        let bytes = fs::read(&self.hot_resume_path)
            .map_err(|error| format!("failed to read hot-resume state: {error}"))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("failed to parse hot-resume state: {error}"))
    }

    fn validate_hot_resume(&self, state: &HotResumeState) -> Result<(), String> {
        if state.schema_version != SCHEMA_VERSION
            || state.run_id != self.manifest.run_id
            || state.repository_root != self.manifest.repository_root
            || state.branch != self.manifest.branch
        {
            return Err(
                "hot-resume identity does not match its immutable run manifest".to_string(),
            );
        }
        validate_nonempty("status", &state.status)?;
        validate_actions(&state.next_actions)?;

        let locked_files = normalize_allowed_files(&state.locked_files)?;
        let allowed: BTreeSet<_> = self.manifest.allowed_files.iter().cloned().collect();
        if let Some(outside) = locked_files.iter().find(|path| !allowed.contains(*path)) {
            return Err(format!(
                "locked file {} is outside the allowed-file manifest",
                outside.display()
            ));
        }
        if locked_files != state.locked_files {
            return Err("locked files must be normalized, sorted and unique".to_string());
        }
        Ok(())
    }
}

fn validate_repository(repository_root: &Path) -> Result<(), String> {
    if !repository_root.is_absolute() || !repository_root.is_dir() {
        return Err("repository root must be an existing absolute directory".to_string());
    }
    if !repository_root.join(".git").exists() {
        return Err("repository root must contain a .git file or directory".to_string());
    }
    Ok(())
}

fn validate_id(label: &str, value: &str) -> Result<(), String> {
    validate_nonempty(label, value)?;
    if value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!(
            "{label} may contain only ASCII letters, digits, dash, underscore and dot"
        ));
    }
    Ok(())
}

fn validate_nonempty(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if value.contains(['\r', '\n', '\0']) {
        return Err(format!("{label} contains forbidden control characters"));
    }
    Ok(())
}

fn validate_actions(actions: &[String]) -> Result<(), String> {
    if actions.len() > 3 {
        return Err("hot-resume state may contain at most three next actions".to_string());
    }
    for action in actions {
        validate_nonempty("next action", action)?;
    }
    Ok(())
}

fn normalize_allowed_files(files: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut normalized = BTreeSet::new();
    for path in files {
        if path.as_os_str().is_empty() || path.is_absolute() {
            return Err(format!(
                "allowed file {} must be a non-empty repository-relative path",
                path.display()
            ));
        }

        let mut clean = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Normal(part) => clean.push(part),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(format!(
                        "allowed file {} escapes the repository root",
                        path.display()
                    ));
                }
            }
        }
        if clean.as_os_str().is_empty() {
            return Err("allowed file must not normalize to an empty path".to_string());
        }
        normalized.insert(clean);
    }
    Ok(normalized.into_iter().collect())
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("failed to serialize {}: {error}", path.display()))?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has an invalid file name", path.display()))?;
    let temporary = parent.join(format!(
        ".{file_name}.tmp-{}-{sequence}",
        std::process::id()
    ));

    let write_result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("failed to create {}: {error}", temporary.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("failed to write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("failed to flush {}: {error}", temporary.display()))?;
        replace_file(&temporary, path)
            .map_err(|error| format!("failed to publish {}: {error}", path.display()))?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: both buffers are valid, NUL-terminated UTF-16 paths and remain alive for the call.
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_ID: AtomicU64 = AtomicU64::new(1);

    struct TempRepo(PathBuf);

    impl TempRepo {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "perfect-planner-run-scope-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join(".git")).unwrap();
            Self(path)
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn request(repository_root: PathBuf) -> CreateRunScope {
        CreateRunScope {
            repository_root,
            run_id: "run-TO-02".to_string(),
            branch: "feature/isolated".to_string(),
            allowed_files: vec![
                PathBuf::from("src-tauri/src/orchestrator/run_scope.rs"),
                PathBuf::from("src-tauri/src/orchestrator/preflight.rs"),
            ],
            next_actions: vec!["run preflight".to_string(), "claim first node".to_string()],
        }
    }

    #[test]
    fn creates_repository_scoped_manifest_logs_and_hot_resume() {
        let repository = TempRepo::new();
        let scope = RunScope::create(request(repository.0.clone())).unwrap();

        assert!(scope.root.starts_with(repository.0.canonicalize().unwrap()));
        assert!(scope
            .root
            .ends_with(".claude/scratch/orchestrator/run-TO-02"));
        assert_eq!(fs::read(&scope.audit_path).unwrap(), b"");
        assert_eq!(fs::read(&scope.events_path).unwrap(), b"");

        let manifest: AllowedFileManifest =
            serde_json::from_slice(&fs::read(&scope.manifest_path).unwrap()).unwrap();
        assert_eq!(manifest, scope.manifest);
        let resume = scope.read_hot_resume().unwrap();
        assert_eq!(resume.status, "ready");
        assert_eq!(resume.next_actions.len(), 2);
    }

    #[test]
    fn refuses_path_escape_and_existing_run_overwrite() {
        let repository = TempRepo::new();
        let mut unsafe_request = request(repository.0.clone());
        unsafe_request.allowed_files = vec![PathBuf::from("../outside.txt")];
        assert!(RunScope::create(unsafe_request)
            .unwrap_err()
            .contains("escapes the repository root"));

        let scope = RunScope::create(request(repository.0.clone())).unwrap();
        fs::write(&scope.audit_path, "preserve me").unwrap();
        let error = RunScope::create(request(repository.0.clone())).unwrap_err();
        assert!(error.contains("refusing to overwrite"));
        assert_eq!(
            fs::read_to_string(&scope.audit_path).unwrap(),
            "preserve me"
        );
    }

    #[test]
    fn hot_resume_update_is_atomic_and_manifest_bounded() {
        let repository = TempRepo::new();
        let scope = RunScope::create(request(repository.0.clone())).unwrap();
        let mut state = scope.read_hot_resume().unwrap();
        state.status = "running".to_string();
        state.last_completed_step = Some("TO-01".to_string());
        state.locked_files = vec![PathBuf::from("src-tauri/src/orchestrator/preflight.rs")];
        state.next_actions = vec!["complete TO-02".to_string()];

        scope.update_hot_resume(&state).unwrap();
        assert_eq!(scope.read_hot_resume().unwrap(), state);
        assert!(fs::read_dir(&scope.root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")));

        let mut outside = state;
        outside.locked_files = vec![PathBuf::from("src-tauri/src/lib.rs")];
        let error = scope.update_hot_resume(&outside).unwrap_err();
        assert!(error.contains("outside the allowed-file manifest"));
    }

    #[test]
    fn invalid_run_id_cannot_escape_scope_directory() {
        let repository = TempRepo::new();
        let mut bad = request(repository.0.clone());
        bad.run_id = "../../elsewhere".to_string();

        assert!(RunScope::create(bad)
            .unwrap_err()
            .contains("may contain only ASCII"));
    }
}
