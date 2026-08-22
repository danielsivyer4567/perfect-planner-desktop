//! Durable, native-only admission registry for Perfect Planner managed runs.
//!
//! A worker never receives the registry path, another plan's paths, or peer-enumeration
//! authority. The head orchestrator registers one immutable run binding and this store assesses
//! it while an operating-system lock is held. Stale overlapping records fail UNKNOWN; they are
//! never silently deleted.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const SCHEMA_VERSION: u32 = 1;
const MAX_RECORDS: usize = 4_096;
const MAX_CLAIMS: usize = 131_072;
const MAX_TTL_MS: u64 = 300_000;
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ManagedRunState {
    Active,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManagedRunBinding {
    pub run_id: String,
    pub plan_id: String,
    pub repository_root: PathBuf,
    pub git_common_dir: PathBuf,
    pub worktree_id: String,
    pub branch: String,
    pub plan_contract_digest: String,
    pub approval_receipt_digest: String,
    pub manifest_digest: String,
    pub allowed_files: Vec<PathBuf>,
    #[serde(default)]
    pub allowed_resources: Vec<String>,
    pub registered_at_ms: u64,
    pub heartbeat_at_ms: u64,
    pub expires_at_ms: u64,
    pub state: ManagedRunState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistryDocument {
    schema_version: u32,
    generation: u64,
    head_digest: String,
    records: BTreeMap<String, ManagedRunBinding>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum AdmissionVerdict {
    Clear,
    Wait,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdmissionConflict {
    pub other_run_id: String,
    pub other_plan_id: String,
    pub other_branch: String,
    pub other_worktree_id: String,
    pub files: Vec<String>,
    pub resources: Vec<String>,
    pub other_stale: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdmissionAssessment {
    pub assessment_id: String,
    pub registry_generation: u64,
    pub run_id: String,
    pub plan_id: String,
    pub manifest_digest: String,
    pub verdict: AdmissionVerdict,
    pub conflicts: Vec<AdmissionConflict>,
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
    pub registry_digest: String,
}

#[derive(Clone, Debug)]
pub(crate) struct AdmissionRegistryStore {
    state_path: PathBuf,
    lock_path: PathBuf,
}

impl AdmissionRegistryStore {
    pub(crate) fn open(state_path: PathBuf) -> Result<Self, String> {
        if !state_path.is_absolute() || state_path.file_name().is_none() {
            return Err("admission registry requires an absolute file path".to_string());
        }
        let parent = state_path
            .parent()
            .ok_or_else(|| "admission registry has no parent directory".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create admission registry directory: {error}"))?;
        let store = Self {
            lock_path: state_path.with_extension("lock"),
            state_path,
        };
        let _lock = StoreLock::acquire(&store.lock_path, LOCK_TIMEOUT)?;
        if !store.state_path.exists() {
            persist(&store.state_path, &empty_document())?;
        }
        store.read().map(|_| store)
    }

    pub(crate) fn register(
        &self,
        binding: ManagedRunBinding,
        now_ms: u64,
    ) -> Result<AdmissionAssessment, String> {
        let run_id = binding.run_id.clone();
        validate_binding(&binding, now_ms)?;
        let _lock = StoreLock::acquire(&self.lock_path, LOCK_TIMEOUT)?;
        let mut document = self.read()?;
        if let Some(existing) = document.records.get(&run_id) {
            if immutable_binding(existing) != immutable_binding(&binding) {
                return Err("run id is already bound to a different immutable scope".to_string());
            }
        } else if document.records.len() >= MAX_RECORDS {
            return Err("admission registry capacity exceeded".to_string());
        }
        document.records.insert(run_id.clone(), binding);
        publish_next(&self.state_path, &mut document)?;
        assess_document(&document, &run_id, now_ms)
    }

    pub(crate) fn assess(
        &self,
        run_id: &str,
        manifest_digest: &str,
        now_ms: u64,
    ) -> Result<AdmissionAssessment, String> {
        let _lock = StoreLock::acquire(&self.lock_path, LOCK_TIMEOUT)?;
        let document = self.read()?;
        let binding = document
            .records
            .get(run_id)
            .ok_or_else(|| "managed run is not registered for admission".to_string())?;
        if binding.manifest_digest != manifest_digest {
            return Err("managed run manifest changed after registration".to_string());
        }
        assess_document(&document, run_id, now_ms)
    }

    #[allow(dead_code)]
    pub(crate) fn heartbeat(
        &self,
        run_id: &str,
        manifest_digest: &str,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<AdmissionAssessment, String> {
        if ttl_ms == 0 || ttl_ms > MAX_TTL_MS {
            return Err("admission heartbeat TTL is invalid".to_string());
        }
        let _lock = StoreLock::acquire(&self.lock_path, LOCK_TIMEOUT)?;
        let mut document = self.read()?;
        let binding = document
            .records
            .get_mut(run_id)
            .ok_or_else(|| "managed run is not registered for admission".to_string())?;
        if binding.manifest_digest != manifest_digest || binding.state != ManagedRunState::Active {
            return Err("managed run is not the active immutable registration".to_string());
        }
        if now_ms < binding.heartbeat_at_ms {
            return Err("trusted admission clock moved backwards".to_string());
        }
        binding.heartbeat_at_ms = now_ms;
        binding.expires_at_ms = now_ms
            .checked_add(ttl_ms)
            .ok_or_else(|| "admission heartbeat expiry overflow".to_string())?;
        publish_next(&self.state_path, &mut document)?;
        assess_document(&document, run_id, now_ms)
    }

    pub(crate) fn terminal(
        &self,
        run_id: &str,
        manifest_digest: &str,
        state: ManagedRunState,
        now_ms: u64,
    ) -> Result<(), String> {
        if state == ManagedRunState::Active {
            return Err("terminal transition cannot restore active state".to_string());
        }
        let _lock = StoreLock::acquire(&self.lock_path, LOCK_TIMEOUT)?;
        let mut document = self.read()?;
        let binding = document
            .records
            .get_mut(run_id)
            .ok_or_else(|| "managed run is not registered for admission".to_string())?;
        if binding.manifest_digest != manifest_digest {
            return Err("managed run manifest changed before terminal transition".to_string());
        }
        if binding.state == state {
            return Ok(());
        }
        if binding.state != ManagedRunState::Active {
            return Err("managed run already has a different terminal state".to_string());
        }
        binding.state = state;
        binding.heartbeat_at_ms = now_ms;
        binding.expires_at_ms = now_ms;
        publish_next(&self.state_path, &mut document)
    }

    fn read(&self) -> Result<RegistryDocument, String> {
        let mut bytes = Vec::new();
        File::open(&self.state_path)
            .and_then(|file| file.take(16 * 1024 * 1024).read_to_end(&mut bytes))
            .map_err(|error| format!("cannot read admission registry: {error}"))?;
        let document: RegistryDocument = serde_json::from_slice(&bytes)
            .map_err(|error| format!("cannot parse admission registry: {error}"))?;
        validate_document(&document)?;
        Ok(document)
    }
}

fn empty_document() -> RegistryDocument {
    let mut document = RegistryDocument {
        schema_version: SCHEMA_VERSION,
        generation: 0,
        head_digest: String::new(),
        records: BTreeMap::new(),
    };
    document.head_digest = document_digest(&document);
    document
}

fn publish_next(path: &Path, document: &mut RegistryDocument) -> Result<(), String> {
    document.generation = document
        .generation
        .checked_add(1)
        .ok_or_else(|| "admission registry generation exhausted".to_string())?;
    document.head_digest = document_digest(document);
    validate_document(document)?;
    persist(path, document)
}

fn assess_document(
    document: &RegistryDocument,
    run_id: &str,
    now_ms: u64,
) -> Result<AdmissionAssessment, String> {
    let current = document
        .records
        .get(run_id)
        .ok_or_else(|| "managed run is absent during assessment".to_string())?;
    if current.state != ManagedRunState::Active || current.expires_at_ms <= now_ms {
        return Err("managed run registration is stale or terminal".to_string());
    }
    let mut conflicts = document
        .records
        .values()
        .filter(|other| other.run_id != current.run_id && other.state == ManagedRunState::Active)
        .filter_map(|other| collision(current, other, now_ms))
        .collect::<Vec<_>>();
    conflicts.sort_by(|left, right| left.other_run_id.cmp(&right.other_run_id));
    let verdict = if conflicts.iter().any(|conflict| conflict.other_stale) {
        AdmissionVerdict::Unknown
    } else if conflicts.is_empty() {
        AdmissionVerdict::Clear
    } else {
        AdmissionVerdict::Wait
    };
    let expires_at_ms = conflicts
        .iter()
        .filter_map(|conflict| document.records.get(&conflict.other_run_id))
        .map(|record| record.expires_at_ms)
        .chain(std::iter::once(current.expires_at_ms))
        .min()
        .unwrap_or(current.expires_at_ms);
    let assessment_id = assessment_digest(
        document.generation,
        current,
        &verdict,
        &conflicts,
        now_ms,
        expires_at_ms,
        &document.head_digest,
    );
    Ok(AdmissionAssessment {
        assessment_id,
        registry_generation: document.generation,
        run_id: current.run_id.clone(),
        plan_id: current.plan_id.clone(),
        manifest_digest: current.manifest_digest.clone(),
        verdict,
        conflicts,
        observed_at_ms: now_ms,
        expires_at_ms,
        registry_digest: document.head_digest.clone(),
    })
}

fn collision(
    current: &ManagedRunBinding,
    other: &ManagedRunBinding,
    now_ms: u64,
) -> Option<AdmissionConflict> {
    let same_repository = path_key(&current.git_common_dir) == path_key(&other.git_common_dir);
    let mut files = if same_repository {
        let current_relative = current
            .allowed_files
            .iter()
            .map(|path| relative_claim_key(path))
            .collect::<BTreeSet<_>>();
        other
            .allowed_files
            .iter()
            .map(|path| relative_claim_key(path))
            .filter(|key| current_relative.contains(key))
            .collect::<Vec<_>>()
    } else {
        let current_files = current
            .allowed_files
            .iter()
            .map(|path| absolute_claim_key(&current.repository_root, path))
            .collect::<BTreeSet<_>>();
        let other_files = other
            .allowed_files
            .iter()
            .map(|path| absolute_claim_key(&other.repository_root, path))
            .collect::<BTreeSet<_>>();
        current_files
            .intersection(&other_files)
            .cloned()
            .collect::<Vec<_>>()
    };
    files.sort();
    files.dedup();
    let current_resources = current
        .allowed_resources
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut resources = other
        .allowed_resources
        .iter()
        .filter(|resource| current_resources.contains(*resource))
        .cloned()
        .collect::<Vec<_>>();
    resources.sort();
    resources.dedup();
    if files.is_empty() && resources.is_empty() {
        return None;
    }
    Some(AdmissionConflict {
        other_run_id: other.run_id.clone(),
        other_plan_id: other.plan_id.clone(),
        other_branch: other.branch.clone(),
        other_worktree_id: other.worktree_id.clone(),
        files,
        resources,
        other_stale: other.expires_at_ms <= now_ms,
    })
}

fn validate_document(document: &RegistryDocument) -> Result<(), String> {
    if document.schema_version != SCHEMA_VERSION
        || document.records.len() > MAX_RECORDS
        || document
            .records
            .iter()
            .any(|(run_id, record)| run_id != &record.run_id)
        || document.head_digest != document_digest(document)
    {
        return Err("admission registry is corrupt or unsupported".to_string());
    }
    let claims = document
        .records
        .values()
        .try_fold(0usize, |total, record| {
            validate_binding_shape(record)?;
            total
                .checked_add(record.allowed_files.len() + record.allowed_resources.len())
                .ok_or_else(|| "admission registry claim count overflow".to_string())
        })?;
    if claims > MAX_CLAIMS {
        return Err("admission registry claim capacity exceeded".to_string());
    }
    Ok(())
}

fn validate_binding(binding: &ManagedRunBinding, now_ms: u64) -> Result<(), String> {
    validate_binding_shape(binding)?;
    if binding.registered_at_ms == 0
        || binding.heartbeat_at_ms < binding.registered_at_ms
        || binding.heartbeat_at_ms > now_ms
        || binding.expires_at_ms <= now_ms
        || binding.expires_at_ms.saturating_sub(now_ms) > MAX_TTL_MS
        || binding.state != ManagedRunState::Active
    {
        return Err("managed run registration has an invalid active timeline".to_string());
    }
    Ok(())
}

fn validate_binding_shape(binding: &ManagedRunBinding) -> Result<(), String> {
    for (label, value) in [
        ("run id", binding.run_id.as_str()),
        ("plan id", binding.plan_id.as_str()),
        ("worktree id", binding.worktree_id.as_str()),
        ("branch", binding.branch.as_str()),
        ("plan digest", binding.plan_contract_digest.as_str()),
        ("approval digest", binding.approval_receipt_digest.as_str()),
        ("manifest digest", binding.manifest_digest.as_str()),
    ] {
        if value.trim().is_empty() || value.len() > 512 {
            return Err(format!("managed run {label} is empty or oversized"));
        }
    }
    if !binding.repository_root.is_absolute()
        || !binding.git_common_dir.is_absolute()
        || binding.allowed_files.is_empty()
        || binding
            .allowed_files
            .iter()
            .any(|path| !valid_relative_path(path))
        || !is_sha256(&binding.worktree_id)
        || !is_sha256(&binding.plan_contract_digest)
        || !is_sha256(&binding.approval_receipt_digest)
        || !is_sha256(&binding.manifest_digest)
        || binding.allowed_resources.iter().any(|resource| {
            resource.trim().is_empty() || resource.len() > 512 || resource.contains('\0')
        })
    {
        return Err("managed run binding contains an invalid path, digest or resource".to_string());
    }
    Ok(())
}

fn immutable_binding(binding: &ManagedRunBinding) -> String {
    let bytes = serde_json::to_vec(&(
        &binding.run_id,
        &binding.plan_id,
        &binding.repository_root,
        &binding.git_common_dir,
        &binding.worktree_id,
        &binding.branch,
        &binding.plan_contract_digest,
        &binding.approval_receipt_digest,
        &binding.manifest_digest,
        &binding.allowed_files,
        &binding.allowed_resources,
    ))
    .expect("managed run immutable binding is serializable");
    sha256(b"perfect-planner:managed-run-binding:v1\0", &bytes)
}

fn document_digest(document: &RegistryDocument) -> String {
    let bytes = serde_json::to_vec(&(
        document.schema_version,
        document.generation,
        &document.records,
    ))
    .expect("admission registry is serializable");
    sha256(b"perfect-planner:admission-registry:v1\0", &bytes)
}

#[allow(clippy::too_many_arguments)]
fn assessment_digest(
    generation: u64,
    current: &ManagedRunBinding,
    verdict: &AdmissionVerdict,
    conflicts: &[AdmissionConflict],
    observed_at_ms: u64,
    expires_at_ms: u64,
    registry_digest: &str,
) -> String {
    let bytes = serde_json::to_vec(&(
        generation,
        immutable_binding(current),
        verdict,
        conflicts,
        observed_at_ms,
        expires_at_ms,
        registry_digest,
    ))
    .expect("admission assessment is serializable");
    sha256(b"perfect-planner:admission-assessment:v1\0", &bytes)
}

fn sha256(domain: &[u8], bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path.components().all(|component| {
            matches!(component, Component::Normal(_))
                && !component.as_os_str().to_string_lossy().contains(':')
        })
}

fn relative_claim_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn absolute_claim_key(root: &Path, path: &Path) -> String {
    root.join(path)
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn persist(path: &Path, document: &RegistryDocument) -> Result<(), String> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "admission registry file name is invalid".to_string())?;
    let temporary = path.with_file_name(format!(
        ".{file_name}.tmp-{}-{sequence}",
        std::process::id()
    ));
    let bytes = serde_json::to_vec_pretty(document)
        .map_err(|error| format!("cannot encode admission registry: {error}"))?;
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("cannot create admission registry temporary: {error}"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("cannot flush admission registry: {error}"))?;
        replace_file(&temporary, path)
            .map_err(|error| format!("cannot publish admission registry: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

struct StoreLock {
    file: Option<File>,
    #[cfg(not(windows))]
    path: PathBuf,
}

impl StoreLock {
    fn acquire(path: &Path, timeout: Duration) -> Result<Self, String> {
        let started = Instant::now();
        loop {
            match open_lock(path) {
                Ok(file) => {
                    return Ok(Self {
                        file: Some(file),
                        #[cfg(not(windows))]
                        path: path.to_path_buf(),
                    })
                }
                Err(error) if is_lock_contention(&error) && started.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(format!("cannot acquire admission registry lock: {error}"))
                }
            }
        }
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = self.file.take();
        #[cfg(not(windows))]
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(windows)]
fn open_lock(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
}

#[cfg(not(windows))]
fn open_lock(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create_new(true).write(true).open(path)
}

#[cfg(windows)]
fn is_lock_contention(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(not(windows))]
fn is_lock_contention(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::AlreadyExists
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pp-admission-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("temp root");
        root.join("registry.json")
    }

    fn binding(run_id: &str, repo: &str, files: &[&str], resources: &[&str]) -> ManagedRunBinding {
        ManagedRunBinding {
            run_id: run_id.to_string(),
            plan_id: format!("PP-{run_id}"),
            repository_root: PathBuf::from(format!(r"C:\repos\{repo}")),
            git_common_dir: PathBuf::from(format!(r"C:\repos\{repo}\.git")),
            worktree_id: sha256(b"w", run_id.as_bytes()),
            branch: format!("feature/{run_id}"),
            plan_contract_digest: sha256(b"p", run_id.as_bytes()),
            approval_receipt_digest: sha256(b"a", run_id.as_bytes()),
            manifest_digest: sha256(b"m", run_id.as_bytes()),
            allowed_files: files.iter().map(PathBuf::from).collect(),
            allowed_resources: resources.iter().map(|value| value.to_string()).collect(),
            registered_at_ms: 100,
            heartbeat_at_ms: 100,
            expires_at_ms: 10_000,
            state: ManagedRunState::Active,
        }
    }

    #[test]
    fn disjoint_runs_are_clear_and_exact_file_or_resource_overlap_waits() {
        let path = temp_file("overlap");
        let store = AdmissionRegistryStore::open(path.clone()).expect("store");
        let first = binding("one", "a", &["src/a.rs"], &["db:table:one"]);
        assert_eq!(
            store.register(first, 100).expect("first").verdict,
            AdmissionVerdict::Clear
        );
        let disjoint = binding("two", "a", &["src/b.rs"], &["db:table:two"]);
        assert_eq!(
            store.register(disjoint, 100).expect("disjoint").verdict,
            AdmissionVerdict::Clear
        );
        let file_overlap = binding("three", "a", &["src/a.rs"], &[]);
        let assessment = store.register(file_overlap, 100).expect("file conflict");
        assert_eq!(assessment.verdict, AdmissionVerdict::Wait);
        assert_eq!(assessment.conflicts[0].other_run_id, "one");
        assert_eq!(assessment.conflicts[0].files, vec!["src/a.rs"]);
        let resource_overlap = binding("four", "b", &["src/z.rs"], &["db:table:one"]);
        let assessment = store
            .register(resource_overlap, 100)
            .expect("resource conflict");
        assert_eq!(assessment.verdict, AdmissionVerdict::Wait);
        assert_eq!(assessment.conflicts[0].resources, vec!["db:table:one"]);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn stale_overlap_is_unknown_and_terminal_records_release_ownership() {
        let path = temp_file("stale");
        let store = AdmissionRegistryStore::open(path.clone()).expect("store");
        let mut first = binding("one", "a", &["src/a.rs"], &[]);
        first.expires_at_ms = 200;
        store.register(first.clone(), 100).expect("first");
        let second = binding("two", "a", &["src/a.rs"], &[]);
        let assessment = store.register(second.clone(), 300).expect("second");
        assert_eq!(assessment.verdict, AdmissionVerdict::Unknown);
        assert!(assessment.conflicts[0].other_stale);
        store
            .terminal(
                "one",
                &first.manifest_digest,
                ManagedRunState::Completed,
                301,
            )
            .expect("terminal");
        assert_eq!(
            store
                .assess("two", &second.manifest_digest, 302)
                .expect("released")
                .verdict,
            AdmissionVerdict::Clear
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn duplicate_run_id_cannot_rebind_and_tamper_fails_closed() {
        let path = temp_file("rebind");
        let store = AdmissionRegistryStore::open(path.clone()).expect("store");
        let first = binding("one", "a", &["src/a.rs"], &[]);
        store.register(first.clone(), 100).expect("first");
        let rebound = binding("one", "b", &["src/b.rs"], &[]);
        assert!(store.register(rebound, 100).is_err());
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["generation"] = serde_json::json!(999);
        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        assert!(store.assess("one", &first.manifest_digest, 101).is_err());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
