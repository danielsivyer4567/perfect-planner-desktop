//! Durable, non-writing reservations made before a worker receives edit authority.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const DOCUMENT_VERSION: u32 = 1;
const MAX_ACTIVE_RESERVATIONS: usize = 4_096;
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PreclaimRecord {
    pub reservation_id: String,
    pub scope_id: String,
    pub run_id: String,
    pub node_id: String,
    pub worker_id: String,
    pub lease_generation: u64,
    pub fence: u64,
    pub manifest_digest: String,
    pub policy_digest: String,
    pub delivered_approval_digest: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PreclaimDocument {
    pub version: u32,
    pub generation: u64,
    pub head_digest: String,
    pub reservations: BTreeMap<String, PreclaimRecord>,
    pub consumed_reservation_ids: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CasExpectation {
    pub generation: u64,
    pub head_digest: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PreclaimStore {
    state_path: PathBuf,
    lock_path: PathBuf,
}

impl PreclaimStore {
    pub(crate) fn open(state_path: PathBuf) -> Result<Self, String> {
        let parent = state_path
            .parent()
            .ok_or_else(|| "preclaim state requires a parent directory".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create preclaim directory: {error}"))?;
        let store = Self {
            lock_path: state_path.with_extension("lock"),
            state_path,
        };
        if !store.state_path.exists() {
            let _lock = StoreLock::acquire(&store.lock_path, LOCK_TIMEOUT)?;
            if !store.state_path.exists() {
                persist(&store.state_path, &empty_document())?;
            }
        }
        store.read().map(|_| store)
    }

    pub(crate) fn read(&self) -> Result<PreclaimDocument, String> {
        let mut bytes = Vec::new();
        File::open(&self.state_path)
            .and_then(|file| file.take(2 * 1024 * 1024).read_to_end(&mut bytes))
            .map_err(|error| format!("cannot read preclaim state: {error}"))?;
        let document: PreclaimDocument = serde_json::from_slice(&bytes)
            .map_err(|error| format!("cannot parse preclaim state: {error}"))?;
        validate_document(&document)?;
        Ok(document)
    }

    pub(crate) fn reserve(
        &self,
        expected: &CasExpectation,
        record: PreclaimRecord,
    ) -> Result<CasExpectation, String> {
        validate_record(&record)?;
        let _lock = StoreLock::acquire(&self.lock_path, LOCK_TIMEOUT)?;
        let mut document = self.read()?;
        require_cas(&document, expected)?;
        if document.reservations.len() >= MAX_ACTIVE_RESERVATIONS {
            return Err("preclaim reservation capacity exceeded".to_string());
        }
        if document.reservations.contains_key(&record.scope_id)
            || document
                .reservations
                .values()
                .any(|existing| existing.reservation_id == record.reservation_id)
            || document
                .consumed_reservation_ids
                .contains(&record.reservation_id)
        {
            return Err("preclaim reservation is duplicate or replayed".to_string());
        }
        document
            .reservations
            .insert(record.scope_id.clone(), record);
        publish_next(&self.state_path, &mut document)
    }

    pub(crate) fn consume(
        &self,
        expected: &CasExpectation,
        scope_id: &str,
        reservation_id: &str,
        now_ms: u64,
    ) -> Result<CasExpectation, String> {
        let _lock = StoreLock::acquire(&self.lock_path, LOCK_TIMEOUT)?;
        let mut document = self.read()?;
        require_cas(&document, expected)?;
        let record = document
            .reservations
            .get(scope_id)
            .ok_or_else(|| "preclaim reservation is absent".to_string())?;
        if record.reservation_id != reservation_id || record.expires_at_ms <= now_ms {
            return Err("preclaim reservation is mismatched or expired".to_string());
        }
        let removed = document
            .reservations
            .remove(scope_id)
            .ok_or_else(|| "preclaim reservation changed before consume".to_string())?;
        if !document
            .consumed_reservation_ids
            .insert(removed.reservation_id)
        {
            return Err("preclaim reservation replay detected".to_string());
        }
        publish_next(&self.state_path, &mut document)
    }
}

fn empty_document() -> PreclaimDocument {
    let mut document = PreclaimDocument {
        version: DOCUMENT_VERSION,
        generation: 0,
        head_digest: String::new(),
        reservations: BTreeMap::new(),
        consumed_reservation_ids: BTreeSet::new(),
    };
    document.head_digest = document_digest(&document);
    document
}

fn publish_next(path: &Path, document: &mut PreclaimDocument) -> Result<CasExpectation, String> {
    document.generation = document
        .generation
        .checked_add(1)
        .ok_or_else(|| "preclaim generation exhausted".to_string())?;
    document.head_digest = document_digest(document);
    validate_document(document)?;
    persist(path, document)?;
    Ok(CasExpectation {
        generation: document.generation,
        head_digest: document.head_digest.clone(),
    })
}

fn require_cas(document: &PreclaimDocument, expected: &CasExpectation) -> Result<(), String> {
    if document.generation != expected.generation || document.head_digest != expected.head_digest {
        return Err("preclaim generation-and-digest CAS rejected stale state".to_string());
    }
    Ok(())
}

fn validate_document(document: &PreclaimDocument) -> Result<(), String> {
    if document.version != DOCUMENT_VERSION
        || document.reservations.len() > MAX_ACTIVE_RESERVATIONS
        || document.head_digest != document_digest(document)
    {
        return Err("preclaim state is malformed or digest-invalid".to_string());
    }
    for (scope, record) in &document.reservations {
        if scope != &record.scope_id
            || document
                .consumed_reservation_ids
                .contains(&record.reservation_id)
        {
            return Err("preclaim state contains contradictory ownership".to_string());
        }
        validate_record(record)?;
    }
    Ok(())
}

fn validate_record(record: &PreclaimRecord) -> Result<(), String> {
    for value in [
        &record.reservation_id,
        &record.scope_id,
        &record.run_id,
        &record.node_id,
        &record.worker_id,
    ] {
        if value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
            return Err("preclaim identity is empty, oversized or unsafe".to_string());
        }
    }
    for digest in [
        &record.manifest_digest,
        &record.policy_digest,
        &record.delivered_approval_digest,
    ] {
        if !is_sha256(digest) {
            return Err("preclaim digest is malformed".to_string());
        }
    }
    if record.lease_generation == 0
        || record.fence == 0
        || record.created_at_ms >= record.expires_at_ms
    {
        return Err("preclaim generation, fence or lifetime is invalid".to_string());
    }
    Ok(())
}

fn document_digest(document: &PreclaimDocument) -> String {
    let mut canonical = document.clone();
    canonical.head_digest.clear();
    let bytes = serde_json::to_vec(&canonical).expect("preclaim canonical serialization");
    format!("{:x}", Sha256::digest(bytes))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn persist(path: &Path, document: &PreclaimDocument) -> Result<(), String> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("tmp-{}-{sequence}", std::process::id()));
    let bytes = serde_json::to_vec_pretty(document)
        .map_err(|error| format!("cannot encode preclaim state: {error}"))?;
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("cannot create preclaim temporary state: {error}"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("cannot flush preclaim state: {error}"))?;
        replace_file(&temporary, path)
            .map_err(|error| format!("cannot publish preclaim state: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

struct StoreLock {
    _file: File,
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
                        _file: file,
                        #[cfg(not(windows))]
                        path: path.to_path_buf(),
                    })
                }
                Err(error) if started.elapsed() < timeout => {
                    if !is_lock_contention(&error) {
                        return Err(format!("cannot acquire preclaim lock: {error}"));
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(format!("preclaim lock timed out: {error}")),
            }
        }
    }
}

#[cfg(not(windows))]
impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(windows)]
fn open_lock(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
}

#[cfg(not(windows))]
fn open_lock(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create_new(true).write(true).open(path)
}

fn is_lock_contention(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(5 | 32 | 33 | 80 | 183))
        || error.kind() == std::io::ErrorKind::AlreadyExists
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_store() -> (PathBuf, PreclaimStore) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("pp-preclaim-store-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&root).expect("root");
        let store = PreclaimStore::open(root.join("preclaims.json")).expect("store");
        (root, store)
    }

    fn record(scope: &str, reservation: &str, worker: &str) -> PreclaimRecord {
        PreclaimRecord {
            reservation_id: reservation.to_string(),
            scope_id: scope.to_string(),
            run_id: "run-1".to_string(),
            node_id: "B20".to_string(),
            worker_id: worker.to_string(),
            lease_generation: 1,
            fence: 1,
            manifest_digest: "a".repeat(64),
            policy_digest: "b".repeat(64),
            delivered_approval_digest: "c".repeat(64),
            created_at_ms: 100,
            expires_at_ms: 10_000,
        }
    }

    fn cas(document: &PreclaimDocument) -> CasExpectation {
        CasExpectation {
            generation: document.generation,
            head_digest: document.head_digest.clone(),
        }
    }

    #[test]
    fn stale_cas_and_replay_never_create_a_second_reservation() {
        let (root, store) = temp_store();
        let initial = cas(&store.read().expect("initial"));
        let after_reserve = store
            .reserve(&initial, record("scope-a", "reservation-a", "worker-a"))
            .expect("reserve");
        assert!(store
            .reserve(&initial, record("scope-b", "reservation-b", "worker-b"))
            .unwrap_err()
            .contains("CAS"));
        let after_consume = store
            .consume(&after_reserve, "scope-a", "reservation-a", 200)
            .expect("consume");
        assert!(store
            .reserve(
                &after_consume,
                record("scope-a", "reservation-a", "worker-a")
            )
            .unwrap_err()
            .contains("replayed"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cross_thread_claim_race_has_exactly_one_cas_winner() {
        let (root, store) = temp_store();
        let expected = cas(&store.read().expect("initial"));
        let barrier = Arc::new(Barrier::new(16));
        let handles = (0..16)
            .map(|index| {
                let store = store.clone();
                let expected = expected.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    store.reserve(
                        &expected,
                        record(
                            &format!("scope-{index}"),
                            &format!("reservation-{index}"),
                            &format!("worker-{index}"),
                        ),
                    )
                })
            })
            .collect::<Vec<_>>();
        let winners = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread"))
            .filter(Result::is_ok)
            .count();
        assert_eq!(winners, 1);
        assert_eq!(store.read().expect("final").reservations.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tamper_is_unknown_and_never_repaired_implicitly() {
        let (root, store) = temp_store();
        let mut document = store.read().expect("initial");
        document.generation = 99;
        fs::write(
            &store.state_path,
            serde_json::to_vec_pretty(&document).expect("encode"),
        )
        .expect("tamper");
        assert!(store.read().unwrap_err().contains("digest-invalid"));
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    const MOVEFILE_REPLACE_EXISTING: u32 = 1;
    const MOVEFILE_WRITE_THROUGH: u32 = 8;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // SAFETY: the buffers are NUL-terminated UTF-16 paths valid for this call.
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
