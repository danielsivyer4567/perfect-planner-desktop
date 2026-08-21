use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const TOKEN_BYTES: usize = 32;
const MAX_TTL_MS: u64 = 60_000;
const MAX_RECORDS: usize = 4_096;
const ENTROPY_ATTEMPTS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryScope {
    pub run_id: String,
    pub registry_generation: u64,
    pub repository_census_hash: String,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedDiscoveryCapability {
    pub token: String,
    pub run_id: String,
    pub registry_generation: u64,
    pub repository_census_hash: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone)]
pub struct DiscoveryPermit {
    pub run_id: String,
    pub registry_generation: u64,
    pub repository_census_hash: String,
    pub expires_at_ms: u64,
    cancellation: Arc<AtomicBool>,
}

impl fmt::Debug for DiscoveryPermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiscoveryPermit")
            .field("run_id", &self.run_id)
            .field("registry_generation", &self.registry_generation)
            .field("repository_census_hash", &self.repository_census_hash)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("cancellation", &"<redacted-state>")
            .finish()
    }
}

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct DiscoveryCancellation {
    flag: Arc<AtomicBool>,
}

#[allow(dead_code)]
impl DiscoveryCancellation {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

impl DiscoveryPermit {
    pub(crate) fn cancellation(&self) -> DiscoveryCancellation {
        DiscoveryCancellation {
            flag: Arc::clone(&self.cancellation),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifecycle {
    Issued,
    DiscoveryStarted,
    Consumed,
    Revoked,
}

#[derive(Clone, Debug)]
struct CapabilityRecord {
    scope: DiscoveryScope,
    issued_at_ms: u64,
    expires_at_ms: u64,
    lifecycle: Lifecycle,
    cancellation: Arc<AtomicBool>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CapabilityError {
    InvalidRunId,
    InvalidRegistryGeneration,
    InvalidCensusHash,
    InvalidTtl,
    ExpiryOverflow,
    EntropyUnavailable,
    EntropyCollision,
    CapacityExceeded,
    StoreUnavailable,
    InvalidToken,
    UnknownToken,
    Expired,
    ClockRollback,
    BindingMismatch,
    AlreadyUsed,
    DiscoveryNotStarted,
    SnapshotCreationFailed,
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidRunId => "discovery capability requires a bounded non-path run ID",
            Self::InvalidRegistryGeneration => {
                "discovery capability requires a non-zero registry generation"
            }
            Self::InvalidCensusHash => {
                "repository census hash must be a 64-character hexadecimal SHA-256"
            }
            Self::InvalidTtl => "discovery capability TTL must be between 1ms and 60000ms",
            Self::ExpiryOverflow => "discovery capability expiry overflowed",
            Self::EntropyUnavailable => "operating-system entropy is unavailable",
            Self::EntropyCollision => "operating-system entropy repeated a capability token",
            Self::CapacityExceeded => "discovery capability store is at its bounded capacity",
            Self::StoreUnavailable => "discovery capability store lock is unavailable",
            Self::InvalidToken => "discovery capability token has an invalid shape",
            Self::UnknownToken => "discovery capability is unknown or from a prior process",
            Self::Expired => "discovery capability expired",
            Self::ClockRollback => "system time moved behind capability issuance",
            Self::BindingMismatch => {
                "discovery capability does not match the current run, registry or census"
            }
            Self::AlreadyUsed => "discovery capability has already been used or revoked",
            Self::DiscoveryNotStarted => {
                "discovery capability cannot create a snapshot before discovery starts"
            }
            Self::SnapshotCreationFailed => {
                "assessment snapshot creation failed and the capability was revoked"
            }
        };
        formatter.write_str(message)
    }
}

#[derive(Default)]
pub struct CapabilityStore {
    records: Mutex<BTreeMap<String, CapabilityRecord>>,
}

impl CapabilityStore {
    pub fn issue(
        &self,
        scope: DiscoveryScope,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<IssuedDiscoveryCapability, CapabilityError> {
        self.issue_with_entropy(scope, now_ms, ttl_ms, fill_os_random)
    }

    fn issue_with_entropy<F>(
        &self,
        scope: DiscoveryScope,
        now_ms: u64,
        ttl_ms: u64,
        mut fill: F,
    ) -> Result<IssuedDiscoveryCapability, CapabilityError>
    where
        F: FnMut(&mut [u8]) -> Result<(), CapabilityError>,
    {
        validate_scope(&scope)?;
        if ttl_ms == 0 || ttl_ms > MAX_TTL_MS {
            return Err(CapabilityError::InvalidTtl);
        }
        let expires_at_ms = now_ms
            .checked_add(ttl_ms)
            .ok_or(CapabilityError::ExpiryOverflow)?;
        let mut records = self
            .records
            .lock()
            .map_err(|_| CapabilityError::StoreUnavailable)?;
        records.retain(|_, record| {
            let keep = record.expires_at_ms > now_ms;
            if !keep {
                record.cancellation.store(true, Ordering::Release);
            }
            keep
        });
        if records.len() >= MAX_RECORDS {
            return Err(CapabilityError::CapacityExceeded);
        }

        for _ in 0..ENTROPY_ATTEMPTS {
            let mut bytes = [0_u8; TOKEN_BYTES];
            fill(&mut bytes)?;
            let token = hex_encode(&bytes);
            bytes.fill(0);
            let token_hash = hash_token(&token);
            if records.contains_key(&token_hash) {
                continue;
            }
            records.insert(
                token_hash,
                CapabilityRecord {
                    scope: scope.clone(),
                    issued_at_ms: now_ms,
                    expires_at_ms,
                    lifecycle: Lifecycle::Issued,
                    cancellation: Arc::new(AtomicBool::new(false)),
                },
            );
            return Ok(IssuedDiscoveryCapability {
                token,
                run_id: scope.run_id,
                registry_generation: scope.registry_generation,
                repository_census_hash: scope.repository_census_hash,
                issued_at_ms: now_ms,
                expires_at_ms,
            });
        }
        Err(CapabilityError::EntropyCollision)
    }

    /// Begin the single permitted census collection. Calling this twice is a replay.
    pub fn begin_discovery(
        &self,
        token: &str,
        current_scope: &DiscoveryScope,
        now_ms: u64,
    ) -> Result<DiscoveryPermit, CapabilityError> {
        let token_hash = checked_token_hash(token)?;
        let mut records = self
            .records
            .lock()
            .map_err(|_| CapabilityError::StoreUnavailable)?;
        let record = records
            .get_mut(&token_hash)
            .ok_or(CapabilityError::UnknownToken)?;
        validate_live_binding(record, current_scope, now_ms)?;
        if record.lifecycle != Lifecycle::Issued {
            return Err(CapabilityError::AlreadyUsed);
        }
        record.lifecycle = Lifecycle::DiscoveryStarted;
        Ok(DiscoveryPermit {
            run_id: record.scope.run_id.clone(),
            registry_generation: record.scope.registry_generation,
            repository_census_hash: record.scope.repository_census_hash.clone(),
            expires_at_ms: record.expires_at_ms,
            cancellation: Arc::clone(&record.cancellation),
        })
    }

    /// Fence the capability before any native registry or filesystem read. Only the run ID is
    /// accepted from IPC; the returned generation/digest are the native binding issued earlier.
    pub fn begin_discovery_for_run(
        &self,
        token: &str,
        run_id: &str,
        now_ms: u64,
    ) -> Result<DiscoveryPermit, CapabilityError> {
        validate_run_id(run_id)?;
        let token_hash = checked_token_hash(token)?;
        let mut records = self
            .records
            .lock()
            .map_err(|_| CapabilityError::StoreUnavailable)?;
        let record = records
            .get_mut(&token_hash)
            .ok_or(CapabilityError::UnknownToken)?;
        validate_live_record(record, now_ms)?;
        if record.scope.run_id != run_id {
            revoke_record(record);
            return Err(CapabilityError::BindingMismatch);
        }
        if record.lifecycle != Lifecycle::Issued {
            return Err(CapabilityError::AlreadyUsed);
        }
        record.lifecycle = Lifecycle::DiscoveryStarted;
        Ok(DiscoveryPermit {
            run_id: record.scope.run_id.clone(),
            registry_generation: record.scope.registry_generation,
            repository_census_hash: record.scope.repository_census_hash.clone(),
            expires_at_ms: record.expires_at_ms,
            cancellation: Arc::clone(&record.cancellation),
        })
    }

    /// Hold the store fence while the immutable snapshot is created, then consume the token.
    /// If persistence fails, the token is revoked rather than restored for a risky retry.
    pub fn consume_with_snapshot<T, F>(
        &self,
        token: &str,
        current_scope: &DiscoveryScope,
        now_ms: u64,
        create_snapshot: F,
    ) -> Result<T, CapabilityError>
    where
        F: FnOnce(&DiscoveryPermit) -> Result<T, String>,
    {
        self.consume_with_snapshot_at(
            token,
            current_scope,
            || now_ms,
            |permit, _accepted_at_ms| create_snapshot(permit),
        )
    }

    /// Sample trusted time only after the capability mutex is held, immediately before the
    /// acceptance/persistence closure. This prevents lock wait from consuming the remaining TTL.
    pub fn consume_with_snapshot_at<T, N, F>(
        &self,
        token: &str,
        current_scope: &DiscoveryScope,
        now: N,
        create_snapshot: F,
    ) -> Result<T, CapabilityError>
    where
        N: FnOnce() -> u64,
        F: FnOnce(&DiscoveryPermit, u64) -> Result<T, String>,
    {
        let token_hash = checked_token_hash(token)?;
        let mut records = self
            .records
            .lock()
            .map_err(|_| CapabilityError::StoreUnavailable)?;
        let record = records
            .get_mut(&token_hash)
            .ok_or(CapabilityError::UnknownToken)?;
        let accepted_at_ms = now();
        validate_live_binding(record, current_scope, accepted_at_ms)?;
        if record.lifecycle == Lifecycle::Issued {
            revoke_record(record);
            return Err(CapabilityError::DiscoveryNotStarted);
        }
        if record.lifecycle != Lifecycle::DiscoveryStarted {
            return Err(CapabilityError::AlreadyUsed);
        }
        let permit = DiscoveryPermit {
            run_id: record.scope.run_id.clone(),
            registry_generation: record.scope.registry_generation,
            repository_census_hash: record.scope.repository_census_hash.clone(),
            expires_at_ms: record.expires_at_ms,
            cancellation: Arc::clone(&record.cancellation),
        };
        match create_snapshot(&permit, accepted_at_ms) {
            Ok(snapshot) => {
                record.lifecycle = Lifecycle::Consumed;
                record.cancellation.store(true, Ordering::Release);
                Ok(snapshot)
            }
            Err(_) => {
                revoke_record(record);
                Err(CapabilityError::SnapshotCreationFailed)
            }
        }
    }

    pub fn revoke(&self, token: &str) -> Result<(), CapabilityError> {
        let token_hash = checked_token_hash(token)?;
        let mut records = self
            .records
            .lock()
            .map_err(|_| CapabilityError::StoreUnavailable)?;
        let record = records
            .get_mut(&token_hash)
            .ok_or(CapabilityError::UnknownToken)?;
        if matches!(record.lifecycle, Lifecycle::Consumed | Lifecycle::Revoked) {
            return Err(CapabilityError::AlreadyUsed);
        }
        revoke_record(record);
        Ok(())
    }
}

fn validate_scope(scope: &DiscoveryScope) -> Result<(), CapabilityError> {
    validate_run_id(&scope.run_id)?;
    if scope.registry_generation == 0 {
        return Err(CapabilityError::InvalidRegistryGeneration);
    }
    if !is_sha256(&scope.repository_census_hash) {
        return Err(CapabilityError::InvalidCensusHash);
    }
    Ok(())
}

pub(crate) fn validate_run_id(run_id: &str) -> Result<(), CapabilityError> {
    let run_id = run_id.as_bytes();
    if run_id.is_empty()
        || run_id.len() > 128
        || run_id.iter().any(|byte| {
            !byte.is_ascii_alphanumeric() && !matches!(*byte, b'-' | b'_' | b'.' | b':')
        })
    {
        return Err(CapabilityError::InvalidRunId);
    }
    Ok(())
}

fn validate_live_record(record: &mut CapabilityRecord, now_ms: u64) -> Result<(), CapabilityError> {
    if matches!(record.lifecycle, Lifecycle::Consumed | Lifecycle::Revoked) {
        return Err(CapabilityError::AlreadyUsed);
    }
    if now_ms < record.issued_at_ms {
        revoke_record(record);
        return Err(CapabilityError::ClockRollback);
    }
    if now_ms >= record.expires_at_ms {
        revoke_record(record);
        return Err(CapabilityError::Expired);
    }
    Ok(())
}

fn validate_live_binding(
    record: &mut CapabilityRecord,
    current_scope: &DiscoveryScope,
    now_ms: u64,
) -> Result<(), CapabilityError> {
    validate_live_record(record, now_ms)?;
    if &record.scope != current_scope {
        revoke_record(record);
        return Err(CapabilityError::BindingMismatch);
    }
    Ok(())
}

fn revoke_record(record: &mut CapabilityRecord) {
    record.lifecycle = Lifecycle::Revoked;
    record.cancellation.store(true, Ordering::Release);
}

fn checked_token_hash(token: &str) -> Result<String, CapabilityError> {
    if token.len() != TOKEN_BYTES * 2 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CapabilityError::InvalidToken);
    }
    Ok(hash_token(token))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn hash_token(token: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(token.as_bytes());
    format!("{:x}", digest.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(windows)]
fn fill_os_random(bytes: &mut [u8]) -> Result<(), CapabilityError> {
    use std::ffi::c_void;
    use std::ptr;

    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut c_void,
            buffer: *mut u8,
            buffer_len: u32,
            flags: u32,
        ) -> i32;
    }

    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    let length = u32::try_from(bytes.len()).map_err(|_| CapabilityError::EntropyUnavailable)?;
    let status = unsafe {
        BCryptGenRandom(
            ptr::null_mut(),
            bytes.as_mut_ptr(),
            length,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(CapabilityError::EntropyUnavailable)
    }
}

#[cfg(unix)]
fn fill_os_random(bytes: &mut [u8]) -> Result<(), CapabilityError> {
    use std::fs::File;
    use std::io::Read;

    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(bytes))
        .map_err(|_| CapabilityError::EntropyUnavailable)
}

#[cfg(not(any(windows, unix)))]
fn fill_os_random(_bytes: &mut [u8]) -> Result<(), CapabilityError> {
    Err(CapabilityError::EntropyUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::thread;

    fn scope() -> DiscoveryScope {
        DiscoveryScope {
            run_id: "run-PP-002-B03".to_string(),
            registry_generation: 7,
            repository_census_hash: "a".repeat(64),
        }
    }

    fn issue_error(result: Result<IssuedDiscoveryCapability, CapabilityError>) -> CapabilityError {
        match result {
            Ok(_) => panic!("capability issuance unexpectedly succeeded"),
            Err(error) => error,
        }
    }

    #[test]
    fn operating_system_tokens_are_256_bit_hex_and_do_not_repeat() {
        let store = CapabilityStore::default();
        let mut tokens = BTreeSet::new();
        for index in 0..128 {
            let issued = store.issue(scope(), index, 60_000).unwrap();
            assert_eq!(issued.token.len(), 64);
            assert!(issued.token.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert!(tokens.insert(issued.token));
        }
    }

    #[test]
    fn issue_rejects_unbounded_bindings_and_expiry() {
        let store = CapabilityStore::default();
        let mut bad_run = scope();
        bad_run.run_id = "../another/repository".to_string();
        assert_eq!(
            issue_error(store.issue(bad_run, 1, 1)),
            CapabilityError::InvalidRunId
        );
        let mut no_generation = scope();
        no_generation.registry_generation = 0;
        assert_eq!(
            issue_error(store.issue(no_generation, 1, 1)),
            CapabilityError::InvalidRegistryGeneration
        );
        let mut bad_hash = scope();
        bad_hash.repository_census_hash = "not-a-hash".to_string();
        assert_eq!(
            issue_error(store.issue(bad_hash, 1, 1)),
            CapabilityError::InvalidCensusHash
        );
        assert_eq!(
            issue_error(store.issue(scope(), 1, 0)),
            CapabilityError::InvalidTtl
        );
        assert_eq!(
            issue_error(store.issue(scope(), 1, MAX_TTL_MS + 1)),
            CapabilityError::InvalidTtl
        );
    }

    #[test]
    fn discovery_is_single_start_and_single_snapshot_consumption() {
        let store = CapabilityStore::default();
        let issued = store.issue(scope(), 1_000, 10_000).unwrap();
        let permit = store
            .begin_discovery(&issued.token, &scope(), 1_001)
            .unwrap();
        assert_eq!(permit.registry_generation, 7);
        assert_eq!(
            store
                .begin_discovery(&issued.token, &scope(), 1_002)
                .unwrap_err(),
            CapabilityError::AlreadyUsed
        );
        let snapshot = store
            .consume_with_snapshot(&issued.token, &scope(), 1_003, |permit| {
                Ok(format!("snapshot:{}", permit.repository_census_hash))
            })
            .unwrap();
        assert!(snapshot.starts_with("snapshot:"));
        assert_eq!(
            store
                .consume_with_snapshot(&issued.token, &scope(), 1_004, |_| Ok(()))
                .unwrap_err(),
            CapabilityError::AlreadyUsed
        );
    }

    #[test]
    fn expiry_generation_census_and_clock_drift_revoke_access() {
        let store = CapabilityStore::default();
        let expired = store.issue(scope(), 1_000, 10).unwrap();
        assert_eq!(
            store
                .begin_discovery(&expired.token, &scope(), 1_010)
                .unwrap_err(),
            CapabilityError::Expired
        );

        let generation = store.issue(scope(), 2_000, 100).unwrap();
        let mut newer = scope();
        newer.registry_generation += 1;
        assert_eq!(
            store
                .begin_discovery(&generation.token, &newer, 2_001)
                .unwrap_err(),
            CapabilityError::BindingMismatch
        );
        assert_eq!(
            store
                .begin_discovery(&generation.token, &scope(), 2_002)
                .unwrap_err(),
            CapabilityError::AlreadyUsed
        );

        let census = store.issue(scope(), 3_000, 100).unwrap();
        let mut changed = scope();
        changed.repository_census_hash = "b".repeat(64);
        assert_eq!(
            store
                .begin_discovery(&census.token, &changed, 3_001)
                .unwrap_err(),
            CapabilityError::BindingMismatch
        );

        let rollback = store.issue(scope(), 4_000, 100).unwrap();
        assert_eq!(
            store
                .begin_discovery(&rollback.token, &scope(), 3_999)
                .unwrap_err(),
            CapabilityError::ClockRollback
        );
    }

    #[test]
    fn snapshot_failure_and_explicit_revoke_never_restore_access() {
        let store = CapabilityStore::default();
        let failed = store.issue(scope(), 1_000, 100).unwrap();
        store
            .begin_discovery(&failed.token, &scope(), 1_001)
            .unwrap();
        assert_eq!(
            store
                .consume_with_snapshot(&failed.token, &scope(), 1_002, |_| {
                    Err::<(), _>("disk full".to_string())
                })
                .unwrap_err(),
            CapabilityError::SnapshotCreationFailed
        );
        assert_eq!(
            store
                .begin_discovery(&failed.token, &scope(), 1_003)
                .unwrap_err(),
            CapabilityError::AlreadyUsed
        );

        let revoked = store.issue(scope(), 2_000, 100).unwrap();
        store.revoke(&revoked.token).unwrap();
        assert_eq!(
            store
                .begin_discovery(&revoked.token, &scope(), 2_001)
                .unwrap_err(),
            CapabilityError::AlreadyUsed
        );
    }

    #[test]
    fn concurrent_snapshot_consumers_have_exactly_one_winner() {
        let store = Arc::new(CapabilityStore::default());
        let issued = store.issue(scope(), 1_000, 10_000).unwrap();
        store
            .begin_discovery(&issued.token, &scope(), 1_001)
            .unwrap();
        let handles = (0..16)
            .map(|index| {
                let store = Arc::clone(&store);
                let token = issued.token.clone();
                thread::spawn(move || {
                    store.consume_with_snapshot(&token, &scope(), 1_002, |_| Ok(index))
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(CapabilityError::AlreadyUsed)))
                .count(),
            15
        );
    }

    #[test]
    fn raw_tokens_are_not_retained_and_entropy_collision_fails_closed() {
        let store = CapabilityStore::default();
        let issued = store
            .issue_with_entropy(scope(), 1_000, 10_000, |bytes| {
                bytes.fill(7);
                Ok(())
            })
            .unwrap();
        let records = store.records.lock().unwrap();
        assert!(!records.contains_key(&issued.token));
        assert!(records.contains_key(&hash_token(&issued.token)));
        drop(records);
        assert_eq!(
            issue_error(store.issue_with_entropy(scope(), 1_001, 10_000, |bytes| {
                bytes.fill(7);
                Ok(())
            })),
            CapabilityError::EntropyCollision
        );
    }
}
