use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const SCHEMA_VERSION: u32 = 2;
const MANIFEST_FILE: &str = "manifest.json";
const AUDIT_FILE: &str = "audit.jsonl";
const EVENTS_FILE: &str = "events.jsonl";
const HOT_RESUME_FILE: &str = "hot-resume.json";
const MAX_PLAN_BYTES: u64 = 16 * 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct CreateRunScope {
    pub repository_root: PathBuf,
    pub run_id: String,
    pub plan_path: PathBuf,
    pub next_actions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowedFileManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub repository_root: PathBuf,
    pub worktree_git_dir: PathBuf,
    pub git_common_dir: PathBuf,
    pub worktree_id: String,
    pub branch: String,
    pub baseline_commit: String,
    pub plan_id: String,
    pub plan_path: PathBuf,
    pub plan_contract_digest: String,
    pub approval_receipt_digest: String,
    pub allowed_files: Vec<PathBuf>,
    pub manifest_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotResumeState {
    pub schema_version: u32,
    pub run_id: String,
    pub repository_root: PathBuf,
    pub worktree_id: String,
    pub branch: String,
    pub plan_id: String,
    pub manifest_digest: String,
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

#[derive(Clone, Debug)]
struct LiveRunBinding {
    repository_root: PathBuf,
    worktree_git_dir: PathBuf,
    git_common_dir: PathBuf,
    worktree_id: String,
    branch: String,
    head_commit: String,
    plan_id: String,
    plan_path: PathBuf,
    plan_contract_digest: String,
    approval_receipt_digest: String,
    allowed_files: Vec<PathBuf>,
}

impl RunScope {
    pub fn create(request: CreateRunScope) -> Result<Self, String> {
        validate_repository(&request.repository_root)?;
        validate_id("run id", &request.run_id)?;
        validate_actions(&request.next_actions)?;

        let repository_root = request
            .repository_root
            .canonicalize()
            .map_err(|error| format!("failed to canonicalize repository root: {error}"))?;
        let binding = derive_live_binding(&repository_root, &request.plan_path)?;
        let parent = repository_root
            .join(".claude")
            .join("scratch")
            .join("orchestrator");
        fs::create_dir_all(&parent)
            .map_err(|error| format!("failed to create orchestrator state directory: {error}"))?;

        let root = parent.join(&request.run_id);
        match fs::create_dir(&root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Self::open_existing(root, &request.run_id, &binding);
            }
            Err(error) => {
                return Err(format!(
                    "failed to create run scope {}: {error}",
                    root.display()
                ));
            }
        }

        let manifest = build_manifest(&request.run_id, &binding, &binding.head_commit)?;
        let hot_resume = HotResumeState {
            schema_version: SCHEMA_VERSION,
            run_id: request.run_id,
            repository_root,
            worktree_id: manifest.worktree_id.clone(),
            branch: manifest.branch.clone(),
            plan_id: manifest.plan_id.clone(),
            manifest_digest: manifest.manifest_digest.clone(),
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

    fn open_existing(
        root: PathBuf,
        run_id: &str,
        binding: &LiveRunBinding,
    ) -> Result<Self, String> {
        let manifest_path = root.join(MANIFEST_FILE);
        let bytes = fs::read(&manifest_path).map_err(|error| {
            format!("existing run is incomplete; failed to read immutable run manifest: {error}")
        })?;
        let stored: AllowedFileManifest = serde_json::from_slice(&bytes)
            .map_err(|error| format!("existing immutable run manifest is invalid: {error}"))?;
        validate_manifest_integrity(&stored)?;
        let expected = build_manifest(run_id, binding, &stored.baseline_commit)?;
        if stored != expected {
            return Err(
                "existing immutable run manifest does not match the requested run scope"
                    .to_string(),
            );
        }
        let scope = Self {
            manifest_path,
            audit_path: root.join(AUDIT_FILE),
            events_path: root.join(EVENTS_FILE),
            hot_resume_path: root.join(HOT_RESUME_FILE),
            root,
            manifest: stored,
        };
        if !scope.audit_path.is_file() || !scope.events_path.is_file() {
            return Err("existing run is incomplete; audit or event ledger is missing".to_string());
        }
        let state = scope.read_hot_resume()?;
        scope.validate_hot_resume(&state)?;
        Ok(scope)
    }

    pub fn open(repository_root: &Path, run_id: &str) -> Result<Self, String> {
        validate_repository(repository_root)?;
        validate_id("run id", run_id)?;
        let repository_root = repository_root
            .canonicalize()
            .map_err(|error| format!("failed to canonicalize repository root: {error}"))?;
        let parent = repository_root
            .join(".claude")
            .join("scratch")
            .join("orchestrator")
            .canonicalize()
            .map_err(|error| format!("failed to resolve orchestrator state directory: {error}"))?;
        if !parent.starts_with(&repository_root) {
            return Err("orchestrator state directory escapes the repository".to_string());
        }
        let root = parent
            .join(run_id)
            .canonicalize()
            .map_err(|error| format!("failed to resolve run scope: {error}"))?;
        if root.parent() != Some(parent.as_path()) || !root.is_dir() {
            return Err("run scope is not a direct repository child".to_string());
        }
        let manifest_path = root.join(MANIFEST_FILE);
        let stored: AllowedFileManifest = serde_json::from_slice(
            &fs::read(&manifest_path)
                .map_err(|error| format!("failed to read immutable run manifest: {error}"))?,
        )
        .map_err(|error| format!("failed to parse immutable run manifest: {error}"))?;
        validate_manifest_integrity(&stored)?;
        if stored.run_id != run_id || stored.repository_root != repository_root {
            return Err("run manifest identity does not match its repository scope".to_string());
        }
        let binding = derive_live_binding(&repository_root, &stored.plan_path)?;
        Self::open_existing(root, run_id, &binding)
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
            || state.worktree_id != self.manifest.worktree_id
            || state.branch != self.manifest.branch
            || state.plan_id != self.manifest.plan_id
            || state.manifest_digest != self.manifest.manifest_digest
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

fn derive_live_binding(
    repository_root: &Path,
    requested_plan_path: &Path,
) -> Result<LiveRunBinding, String> {
    let top_level = canonicalize_git_path(
        repository_root,
        &git_text(repository_root, &["rev-parse", "--show-toplevel"])?,
    )?;
    if top_level != repository_root {
        return Err("repository root does not match Git's physical worktree root".to_string());
    }

    let worktree_git_dir = canonicalize_git_path(
        repository_root,
        &git_text(repository_root, &["rev-parse", "--absolute-git-dir"])?,
    )?;
    let git_common_dir = canonicalize_git_path(
        repository_root,
        &git_text(repository_root, &["rev-parse", "--git-common-dir"])?,
    )?;
    let branch = git_text(
        repository_root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .map_err(|_| "detached HEAD is not an admissible run branch".to_string())?;
    validate_nonempty("branch", &branch)?;
    let head_commit = git_text(repository_root, &["rev-parse", "--verify", "HEAD"])?;
    if !is_git_oid(&head_commit) {
        return Err("Git HEAD is not a full SHA-1 commit identity".to_string());
    }

    let mut worktree_hasher = Sha256::new();
    worktree_hasher.update(b"perfect-planner-worktree-v1\0");
    hash_path(&mut worktree_hasher, repository_root);
    hash_path(&mut worktree_hasher, &worktree_git_dir);
    hash_path(&mut worktree_hasher, &git_common_dir);
    let worktree_id = format!("{:x}", worktree_hasher.finalize());

    let plan_path = if requested_plan_path.is_absolute() {
        requested_plan_path.to_path_buf()
    } else {
        repository_root.join(requested_plan_path)
    }
    .canonicalize()
    .map_err(|error| format!("failed to canonicalize plan path: {error}"))?;
    if !plan_path.starts_with(repository_root) || !plan_path.is_file() {
        return Err("plan path must be a file inside the exact repository worktree".to_string());
    }
    let metadata = fs::metadata(&plan_path)
        .map_err(|error| format!("failed to inspect plan file: {error}"))?;
    if metadata.len() == 0 || metadata.len() > MAX_PLAN_BYTES {
        return Err("plan file must be non-empty and no larger than 16 MiB".to_string());
    }
    let bytes =
        fs::read(&plan_path).map_err(|error| format!("failed to read plan file: {error}"))?;
    let plan: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("failed to parse plan file: {error}"))?;
    let object = plan
        .as_object()
        .ok_or_else(|| "plan file must contain a JSON object".to_string())?;
    let approval = object
        .get("approved")
        .and_then(Value::as_str)
        .ok_or_else(|| "plan has no explicit approval receipt".to_string())?;
    let approval_word = approval.split_whitespace().next().unwrap_or_default();
    if !approval_word.eq_ignore_ascii_case("yes") {
        return Err("plan is not explicitly approved".to_string());
    }
    let meta = object
        .get("meta")
        .and_then(Value::as_object)
        .ok_or_else(|| "plan metadata is missing".to_string())?;
    let plan_id = required_json_text(meta, "number", "plan metadata.number")?;
    validate_id("plan id", &plan_id)?;
    let plan_branch = required_json_text(meta, "branch", "plan metadata.branch")?;
    if plan_branch != branch {
        return Err(format!(
            "plan branch {plan_branch} does not match the live Git branch {branch}"
        ));
    }
    let vertebrae = object
        .get("vertebrae")
        .and_then(Value::as_array)
        .ok_or_else(|| "plan has no vertebrae array".to_string())?;
    if vertebrae.is_empty() {
        return Err("plan must contain at least one vertebra".to_string());
    }
    let mut allowed_files = Vec::new();
    for vertebra in vertebrae {
        let item = vertebra
            .as_object()
            .ok_or_else(|| "plan vertebra must be an object".to_string())?;
        let id = required_json_text(item, "id", "plan vertebra.id")?;
        validate_id("vertebra id", &id)?;
        let files = item
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("plan vertebra {id} has no files array"))?;
        for file in files {
            let file = file
                .as_str()
                .ok_or_else(|| format!("plan vertebra {id} contains a non-text file claim"))?;
            allowed_files.push(PathBuf::from(file));
        }
    }
    let allowed_files = normalize_allowed_files(&allowed_files)?;
    if allowed_files.is_empty() {
        return Err("plan allowed-file manifest must not be empty".to_string());
    }

    let mut contract = plan.clone();
    strip_volatile_plan_fields(&mut contract);
    let contract_bytes = serde_json::to_vec(&contract)
        .map_err(|error| format!("failed to serialize plan contract: {error}"))?;
    let plan_contract_digest =
        sha256_domain(b"perfect-planner-plan-contract-v1\0", &contract_bytes);

    let mut approval_bytes = Vec::new();
    approval_bytes.extend_from_slice(plan_id.as_bytes());
    approval_bytes.push(0);
    approval_bytes.extend_from_slice(plan_path.to_string_lossy().as_bytes());
    approval_bytes.push(0);
    approval_bytes.extend_from_slice(approval.as_bytes());
    approval_bytes.push(0);
    approval_bytes.extend_from_slice(plan_contract_digest.as_bytes());
    let approval_receipt_digest =
        sha256_domain(b"perfect-planner-approval-receipt-v1\0", &approval_bytes);

    Ok(LiveRunBinding {
        repository_root: repository_root.to_path_buf(),
        worktree_git_dir,
        git_common_dir,
        worktree_id,
        branch,
        head_commit,
        plan_id,
        plan_path,
        plan_contract_digest,
        approval_receipt_digest,
        allowed_files,
    })
}

fn build_manifest(
    run_id: &str,
    binding: &LiveRunBinding,
    baseline_commit: &str,
) -> Result<AllowedFileManifest, String> {
    if !is_git_oid(baseline_commit) {
        return Err("stored baseline commit is not a full SHA-1 identity".to_string());
    }
    let mut manifest = AllowedFileManifest {
        schema_version: SCHEMA_VERSION,
        run_id: run_id.to_string(),
        repository_root: binding.repository_root.clone(),
        worktree_git_dir: binding.worktree_git_dir.clone(),
        git_common_dir: binding.git_common_dir.clone(),
        worktree_id: binding.worktree_id.clone(),
        branch: binding.branch.clone(),
        baseline_commit: baseline_commit.to_string(),
        plan_id: binding.plan_id.clone(),
        plan_path: binding.plan_path.clone(),
        plan_contract_digest: binding.plan_contract_digest.clone(),
        approval_receipt_digest: binding.approval_receipt_digest.clone(),
        allowed_files: binding.allowed_files.clone(),
        manifest_digest: String::new(),
    };
    manifest.manifest_digest = manifest_digest(&manifest)?;
    Ok(manifest)
}

fn validate_manifest_integrity(manifest: &AllowedFileManifest) -> Result<(), String> {
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported immutable run manifest schema {}",
            manifest.schema_version
        ));
    }
    validate_id("run id", &manifest.run_id)?;
    validate_id("plan id", &manifest.plan_id)?;
    validate_nonempty("branch", &manifest.branch)?;
    if !is_digest(&manifest.worktree_id)
        || !is_digest(&manifest.plan_contract_digest)
        || !is_digest(&manifest.approval_receipt_digest)
        || !is_digest(&manifest.manifest_digest)
        || !is_git_oid(&manifest.baseline_commit)
    {
        return Err("immutable run manifest contains an invalid digest or commit".to_string());
    }
    let normalized = normalize_allowed_files(&manifest.allowed_files)?;
    if normalized.is_empty() || normalized != manifest.allowed_files {
        return Err("immutable run manifest files are empty or non-canonical".to_string());
    }
    if manifest_digest(manifest)? != manifest.manifest_digest {
        return Err("immutable run manifest digest does not verify".to_string());
    }
    Ok(())
}

fn manifest_digest(manifest: &AllowedFileManifest) -> Result<String, String> {
    let mut unsigned = manifest.clone();
    unsigned.manifest_digest.clear();
    let bytes = serde_json::to_vec(&unsigned)
        .map_err(|error| format!("failed to serialize immutable run manifest: {error}"))?;
    Ok(sha256_domain(b"perfect-planner-run-manifest-v2\0", &bytes))
}

fn git_text(repository_root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository_root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|error| format!("failed to launch Git: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "Git {} failed: {}",
            args.join(" "),
            if stderr.is_empty() {
                "unknown Git error"
            } else {
                &stderr
            }
        ));
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|_| "Git returned non-UTF-8 identity output".to_string())?
        .trim()
        .to_string();
    validate_nonempty("Git identity output", &value)?;
    Ok(value)
}

fn canonicalize_git_path(repository_root: &Path, value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    let path = if path.is_absolute() {
        path
    } else {
        repository_root.join(path)
    };
    path.canonicalize()
        .map_err(|error| format!("failed to canonicalize Git identity path: {error}"))
}

fn required_json_text(
    object: &serde_json::Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<String, String> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{label} is missing"))?
        .to_string();
    validate_nonempty(label, &value)?;
    Ok(value)
}

fn strip_volatile_plan_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for key in [
                "status",
                "built",
                "tested",
                "builtBy",
                "proof",
                "startSha",
                "startedAt",
                "startedBy",
                "completedAt",
                "completedBy",
                "blocked",
            ] {
                object.remove(key);
            }
            for child in object.values_mut() {
                strip_volatile_plan_fields(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                strip_volatile_plan_fields(child);
            }
        }
        _ => {}
    }
}

fn sha256_domain(domain: &[u8], bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn hash_path(hasher: &mut Sha256, path: &Path) {
    let bytes = path.to_string_lossy();
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes.as_bytes());
}

fn is_git_oid(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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
            fs::create_dir_all(&path).unwrap();
            run_git(&path, &["init", "-q", "-b", "feature/isolated"]);
            run_git(
                &path,
                &["config", "user.email", "perfect-planner@example.invalid"],
            );
            run_git(&path, &["config", "user.name", "Perfect Planner Test"]);
            fs::write(path.join("seed.txt"), "seed\n").unwrap();
            run_git(&path, &["add", "seed.txt"]);
            run_git(&path, &["commit", "-q", "-m", "seed"]);
            write_plan(
                &path,
                "yes @ test",
                "feature/isolated",
                "src-tauri/src/orchestrator/run_scope.rs",
            );
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
            plan_path: PathBuf::from(".claude/scratch/perfect-plan/test-plan.json"),
            next_actions: vec!["run preflight".to_string(), "claim first node".to_string()],
        }
    }

    fn run_git(repository: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write_plan(repository: &Path, approved: &str, branch: &str, file: &str) {
        let path = repository.join(".claude/scratch/perfect-plan/test-plan.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let plan = serde_json::json!({
            "title": "Bound test plan",
            "goal": "Prove exact scope binding",
            "approved": approved,
            "meta": {
                "number": "PP-TEST",
                "branch": branch
            },
            "spine": [{ "id": "P1", "title": "Safety" }],
            "vertebrae": [{
                "id": "B01",
                "spineId": "P1",
                "title": "Bind scope",
                "status": "pending",
                "dependsOn": [],
                "files": [file],
                "resources": [],
                "checklist": [{
                    "text": "Exact binding holds",
                    "built": false,
                    "tested": false,
                    "verify": "cargo test"
                }]
            }]
        });
        fs::write(path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
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
        assert_eq!(manifest.schema_version, 2);
        assert_eq!(manifest.plan_id, "PP-TEST");
        assert!(is_digest(&manifest.worktree_id));
        assert!(is_digest(&manifest.plan_contract_digest));
        assert!(is_digest(&manifest.approval_receipt_digest));
        assert!(is_digest(&manifest.manifest_digest));
        assert_eq!(manifest.allowed_files.len(), 1);
        let resume = scope.read_hot_resume().unwrap();
        assert_eq!(resume.status, "ready");
        assert_eq!(resume.next_actions.len(), 2);
        assert_eq!(resume.manifest_digest, manifest.manifest_digest);
    }

    #[test]
    fn refuses_path_escape_and_idempotently_reopens_exact_run() {
        let repository = TempRepo::new();
        write_plan(
            &repository.0,
            "yes @ test",
            "feature/isolated",
            "../outside.txt",
        );
        assert!(RunScope::create(request(repository.0.clone()))
            .unwrap_err()
            .contains("escapes the repository root"));

        write_plan(
            &repository.0,
            "yes @ test",
            "feature/isolated",
            "src-tauri/src/orchestrator/run_scope.rs",
        );
        let scope = RunScope::create(request(repository.0.clone())).unwrap();
        fs::write(&scope.audit_path, "preserve me").unwrap();
        let reopened = RunScope::create(request(repository.0.clone())).unwrap();
        assert_eq!(reopened.root, scope.root);
        assert_eq!(reopened.manifest, scope.manifest);
        assert_eq!(
            fs::read_to_string(&scope.audit_path).unwrap(),
            "preserve me"
        );

        let plan_path = repository
            .0
            .join(".claude/scratch/perfect-plan/test-plan.json");
        let mut plan: Value = serde_json::from_slice(&fs::read(&plan_path).unwrap()).unwrap();
        plan["vertebrae"][0]["status"] = Value::String("completed".to_string());
        plan["vertebrae"][0]["checklist"][0]["built"] = Value::Bool(true);
        plan["vertebrae"][0]["checklist"][0]["tested"] = Value::Bool(true);
        plan["vertebrae"][0]["checklist"][0]["proof"] =
            serde_json::json!({ "at": "test", "exit": 0 });
        fs::write(&plan_path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
        let opened_after_progress = RunScope::open(&repository.0, "run-TO-02").unwrap();
        assert_eq!(opened_after_progress.manifest, scope.manifest);

        fs::write(repository.0.join("seed.txt"), "seed\nprogress\n").unwrap();
        run_git(&repository.0, &["add", "seed.txt"]);
        run_git(&repository.0, &["commit", "-q", "-m", "progress"]);
        let opened_after_commit = RunScope::open(&repository.0, "run-TO-02").unwrap();
        assert_eq!(
            opened_after_commit.manifest.baseline_commit,
            scope.manifest.baseline_commit
        );

        write_plan(
            &repository.0,
            "yes @ test",
            "feature/isolated",
            "src-tauri/src/lib.rs",
        );
        let error = RunScope::create(request(repository.0.clone())).unwrap_err();
        assert!(error.contains("immutable run manifest"));
        assert_eq!(
            fs::read_to_string(&scope.audit_path).unwrap(),
            "preserve me"
        );
    }

    #[test]
    fn rejects_unapproved_cross_branch_and_tampered_scope() {
        let repository = TempRepo::new();
        write_plan(
            &repository.0,
            "no",
            "feature/isolated",
            "src-tauri/src/orchestrator/run_scope.rs",
        );
        assert!(RunScope::create(request(repository.0.clone()))
            .unwrap_err()
            .contains("not explicitly approved"));

        write_plan(
            &repository.0,
            "yes @ test",
            "feature/other",
            "src-tauri/src/orchestrator/run_scope.rs",
        );
        assert!(RunScope::create(request(repository.0.clone()))
            .unwrap_err()
            .contains("does not match the live Git branch"));

        write_plan(
            &repository.0,
            "yes @ test",
            "feature/isolated",
            "src-tauri/src/orchestrator/run_scope.rs",
        );
        let scope = RunScope::create(request(repository.0.clone())).unwrap();
        let mut tampered: AllowedFileManifest =
            serde_json::from_slice(&fs::read(&scope.manifest_path).unwrap()).unwrap();
        tampered.worktree_id = "0".repeat(64);
        fs::write(
            &scope.manifest_path,
            serde_json::to_vec_pretty(&tampered).unwrap(),
        )
        .unwrap();
        assert!(RunScope::create(request(repository.0.clone()))
            .unwrap_err()
            .contains("digest does not verify"));
    }

    #[test]
    fn hot_resume_update_is_atomic_and_manifest_bounded() {
        let repository = TempRepo::new();
        let scope = RunScope::create(request(repository.0.clone())).unwrap();
        let mut state = scope.read_hot_resume().unwrap();
        state.status = "running".to_string();
        state.last_completed_step = Some("TO-01".to_string());
        state.locked_files = vec![PathBuf::from("src-tauri/src/orchestrator/run_scope.rs")];
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
