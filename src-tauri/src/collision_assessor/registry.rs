//! Machine-wide registry for Perfect Planner collision assessment.
//!
//! The registry is deliberately independent from Tauri commands. `for_app_data` fixes its
//! location beneath Tauri's per-user app-data directory, while every read-modify-write operation
//! is serialized by an operating-system-backed lock and published with atomic replacement.
//! Invalid state is reported as `UNKNOWN`; it is never interpreted as an empty registry.

use super::identity::{
    canonical_declared_path, canonical_resource_identity, physical_path_identity,
    PhysicalPathIdentity, PhysicalPathKind,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub const REGISTRY_SCHEMA_VERSION: u32 = 1;
pub const REGISTRY_RELATIVE_PATH: &str = "collision-assessor/registry-v1.json";
const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REGISTRY_BYTES: u64 = 8 * 1024 * 1024;
const MIN_LEASE_MS: u64 = 1_000;
const MAX_LEASE_MS: u64 = 24 * 60 * 60 * 1_000;
const MAX_CENSUS_TTL_MS: u64 = 10 * 60 * 1_000;
const MAX_TEXT_BYTES: usize = 4_096;
const MAX_CONFIGURED_ROOTS: usize = 128;
const MAX_REGISTRATIONS: usize = 512;
const MAX_NODES_PER_REGISTRATION: usize = 2_048;
const MAX_FILES_PER_MANIFEST: usize = 8_192;
const MAX_RESOURCES_PER_MANIFEST: usize = 2_048;
const MAX_CENSUS_PLANNERS_PER_ROOT: usize = MAX_REGISTRATIONS;
const MAX_TOTAL_MANIFEST_ENTRIES: usize = 65_536;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannerNodeManifest {
    pub node_id: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub resources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannerRegistrationSeed {
    pub planner_id: String,
    pub repository_id: String,
    pub repository_root: String,
    pub worktree_root: String,
    pub branch: String,
    pub plan_id: String,
    pub plan_path: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub nodes: Vec<PlannerNodeManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannerRegistration {
    pub identity: PlannerRegistrationSeed,
    pub lease_generation: u64,
    pub registered_at_ms: u64,
    pub updated_at_ms: u64,
    pub heartbeat_at_ms: u64,
    pub lease_expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfiguredDiscoveryRoot {
    pub root_id: String,
    pub canonical_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiscoveryFailureCode {
    AccessDenied,
    Missing,
    Unreadable,
    Malformed,
    Unsupported,
    IdentityAmbiguous,
    LimitExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoveryRootCensus {
    pub root_id: String,
    pub reachable: bool,
    #[serde(default)]
    pub planner_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<DiscoveryFailureCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoveryCensus {
    pub registry_generation: u64,
    /// Exact native authority input covered by this census. A generation alone cannot detect a
    /// same-generation filesystem identity replacement after restart.
    pub input_digest: String,
    pub captured_at_ms: u64,
    pub expires_at_ms: u64,
    #[serde(default)]
    pub roots: Vec<DiscoveryRootCensus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistryDocument {
    pub schema_version: u32,
    pub generation: u64,
    pub updated_at_ms: u64,
    #[serde(default)]
    pub configured_roots: Vec<ConfiguredDiscoveryRoot>,
    #[serde(default)]
    pub registrations: Vec<PlannerRegistration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub census: Option<DiscoveryCensus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CensusInputAttestation {
    pub(crate) registry_generation: u64,
    pub(crate) input_digest: [u8; 32],
}

impl CensusInputAttestation {
    pub(crate) fn digest_hex(&self) -> String {
        hex_digest(&self.input_digest)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedDiscoveryRoot {
    pub(crate) root_id: String,
    pub(crate) path: PathBuf,
    pub(crate) identity: PhysicalPathIdentity,
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedPlannerRegistration {
    pub(crate) registration: PlannerRegistration,
    pub(crate) repository_root_identity: PhysicalPathIdentity,
    pub(crate) worktree_root_identity: PhysicalPathIdentity,
    pub(crate) plan_identity: PhysicalPathIdentity,
}

#[derive(Debug, Clone)]
pub(crate) struct CensusInputSnapshot {
    pub(crate) attestation: CensusInputAttestation,
    pub(crate) configured_roots: Vec<ValidatedDiscoveryRoot>,
    pub(crate) registrations: Vec<ValidatedPlannerRegistration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryIssue {
    MissingRegistry,
    Unreadable(String),
    Malformed(String),
    UnsupportedSchema(u64),
    InvalidDocument(String),
    DuplicatePlanner(String),
    DuplicateConfiguredRoot(String),
    DuplicateConfiguredPath(String),
    NoConfiguredRoots,
    StaleRegistration {
        planner_id: String,
        expired_at_ms: u64,
    },
    FutureHeartbeat {
        planner_id: String,
        heartbeat_at_ms: u64,
    },
    FutureDocumentUpdate(u64),
    FutureRegistrationTime {
        planner_id: String,
        timestamp_ms: u64,
    },
    MissingCensus,
    CensusGenerationMismatch {
        registry_generation: u64,
        census_generation: u64,
    },
    CensusInputDigestMismatch,
    StaleCensus(u64),
    FutureCensus(u64),
    MissingRootCensus(String),
    UnexpectedRootCensus(String),
    DuplicateRootCensus(String),
    UnreachableRoot {
        root_id: String,
        failure: Option<DiscoveryFailureCode>,
    },
    DuplicateObservedPlanner(String),
    UnaccountedPlanner(String),
    UnregisteredObservedPlanner(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownRegistry {
    pub generation: Option<u64>,
    pub registration_count: usize,
    pub issues: Vec<RegistryIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryRead {
    Complete(RegistryDocument),
    Unknown(UnknownRegistry),
}

impl RegistryRead {
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete(_))
    }

    pub fn issues(&self) -> &[RegistryIssue] {
        match self {
            Self::Complete(_) => &[],
            Self::Unknown(unknown) => &unknown.issues,
        }
    }
}

#[derive(Debug)]
pub enum RegistryError {
    InvalidInput(String),
    UnknownState(Vec<RegistryIssue>),
    LockTimeout(PathBuf),
    Io(String),
    Conflict(String),
    CapabilityExpired,
    ClockRollback,
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(formatter, "invalid registry input: {message}"),
            Self::UnknownState(issues) => {
                write!(formatter, "registry is UNKNOWN ({} issue(s))", issues.len())
            }
            Self::LockTimeout(path) => {
                write!(
                    formatter,
                    "timed out acquiring registry lock {}",
                    path.display()
                )
            }
            Self::Io(message) => formatter.write_str(message),
            Self::Conflict(message) => write!(formatter, "registry conflict: {message}"),
            Self::CapabilityExpired => {
                formatter.write_str("discovery capability expired before registry acceptance")
            }
            Self::ClockRollback => {
                formatter.write_str("trusted clock moved backwards during registry acceptance")
            }
        }
    }
}

impl Error for RegistryError {}

#[derive(Clone, Debug)]
pub struct PlannerRegistryStore {
    path: Arc<PathBuf>,
    lock_timeout: Duration,
}

impl PlannerRegistryStore {
    pub fn for_app_data(app_data_dir: impl AsRef<Path>) -> Result<Self, RegistryError> {
        let app_data_dir = app_data_dir.as_ref();
        if !app_data_dir.is_absolute() || has_parent_component(app_data_dir) {
            return Err(RegistryError::InvalidInput(
                "app-data directory must be an absolute path without parent traversal".into(),
            ));
        }
        Self::new(app_data_dir.join(REGISTRY_RELATIVE_PATH))
    }

    pub fn new(path: impl Into<PathBuf>) -> Result<Self, RegistryError> {
        let path = path.into();
        if !path.is_absolute() || has_parent_component(&path) || path.file_name().is_none() {
            return Err(RegistryError::InvalidInput(
                "registry path must be an absolute file path without parent traversal".into(),
            ));
        }
        Ok(Self {
            path: Arc::new(path),
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
        })
    }

    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    /// Create the registry once. Existing or malformed state is never overwritten.
    pub fn initialize(
        &self,
        configured_roots: Vec<ConfiguredDiscoveryRoot>,
        now_ms: u64,
    ) -> Result<RegistryDocument, RegistryError> {
        validate_timestamp("initialization time", now_ms)?;
        let configured_roots = validate_and_sort_roots(configured_roots)?;
        let parent = self.path.parent().ok_or_else(|| {
            RegistryError::InvalidInput("registry path has no parent directory".into())
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            RegistryError::Io(format!(
                "cannot create registry directory {}: {error}",
                parent.display()
            ))
        })?;
        let _lock = RegistryLock::acquire(self.path.as_path(), self.lock_timeout)?;

        match fs::symlink_metadata(self.path.as_path()) {
            Ok(_) => {
                let read = inspect_registry_file(self.path.as_path(), now_ms);
                return match read {
                    RegistryRead::Complete(_) | RegistryRead::Unknown(_) => {
                        Err(RegistryError::Conflict(
                            "registry already exists; refusing overwrite".into(),
                        ))
                    }
                };
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(RegistryError::Io(format!(
                    "cannot inspect registry target {}: {error}",
                    self.path.display()
                )))
            }
        }

        let document = RegistryDocument {
            schema_version: REGISTRY_SCHEMA_VERSION,
            generation: 1,
            updated_at_ms: now_ms,
            configured_roots,
            registrations: Vec::new(),
            census: None,
        };
        persist_document(self.path.as_path(), &document)?;
        Ok(document)
    }

    pub fn inspect(&self, now_ms: u64) -> RegistryRead {
        inspect_registry_file(self.path.as_path(), now_ms)
    }

    /// Capture the exact authority input for a first or subsequent census. Prior census output
    /// is intentionally ignored, but malformed state, stale registrations and ambiguous native
    /// filesystem identities remain fatal. The registry mutex keeps this load and validation
    /// atomic with respect to every registry mutation.
    pub(crate) fn census_input_snapshot(
        &self,
        now_ms: u64,
    ) -> Result<CensusInputSnapshot, RegistryError> {
        validate_timestamp("census snapshot time", now_ms)?;
        let _lock = RegistryLock::acquire(self.path.as_path(), self.lock_timeout)?;
        let document = load_document(self.path.as_path()).map_err(RegistryError::UnknownState)?;
        build_census_input_snapshot(&document, now_ms)
    }

    pub fn register(
        &self,
        seed: PlannerRegistrationSeed,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<PlannerRegistration, RegistryError> {
        validate_seed(&seed)?;
        let expires_at_ms = lease_expiry(now_ms, lease_ms)?;
        self.mutate(now_ms, true, move |document| {
            if document
                .registrations
                .iter()
                .any(|entry| entry.identity.planner_id == seed.planner_id)
            {
                return Err(RegistryError::Conflict(format!(
                    "planner {} is already registered",
                    seed.planner_id
                )));
            }
            let registration = PlannerRegistration {
                identity: seed,
                lease_generation: 1,
                registered_at_ms: now_ms,
                updated_at_ms: now_ms,
                heartbeat_at_ms: now_ms,
                lease_expires_at_ms: expires_at_ms,
            };
            document.registrations.push(registration.clone());
            sort_registrations(&mut document.registrations);
            Ok(registration)
        })
    }

    pub fn heartbeat(
        &self,
        planner_id: &str,
        expected_lease_generation: u64,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<PlannerRegistration, RegistryError> {
        validate_id("planner id", planner_id)?;
        if expected_lease_generation == 0 {
            return Err(RegistryError::InvalidInput(
                "expected lease generation must be non-zero".into(),
            ));
        }
        let expires_at_ms = lease_expiry(now_ms, lease_ms)?;
        self.mutate(now_ms, true, |document| {
            let registration = registration_mut(document, planner_id)?;
            require_generation(registration, expected_lease_generation)?;
            registration.lease_generation = registration
                .lease_generation
                .checked_add(1)
                .ok_or_else(|| {
                    RegistryError::Conflict("planner lease generation overflowed".into())
                })?;
            registration.updated_at_ms = now_ms;
            registration.heartbeat_at_ms = now_ms;
            registration.lease_expires_at_ms = expires_at_ms;
            Ok(registration.clone())
        })
    }

    pub fn update(
        &self,
        planner_id: &str,
        expected_lease_generation: u64,
        replacement: PlannerRegistrationSeed,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<PlannerRegistration, RegistryError> {
        validate_id("planner id", planner_id)?;
        validate_seed(&replacement)?;
        if replacement.planner_id != planner_id {
            return Err(RegistryError::InvalidInput(
                "replacement planner identity does not match target".into(),
            ));
        }
        let expires_at_ms = lease_expiry(now_ms, lease_ms)?;
        self.mutate(now_ms, true, move |document| {
            let registration = registration_mut(document, planner_id)?;
            require_generation(registration, expected_lease_generation)?;
            registration.identity = replacement;
            registration.lease_generation = registration
                .lease_generation
                .checked_add(1)
                .ok_or_else(|| {
                    RegistryError::Conflict("planner lease generation overflowed".into())
                })?;
            registration.updated_at_ms = now_ms;
            registration.heartbeat_at_ms = now_ms;
            registration.lease_expires_at_ms = expires_at_ms;
            Ok(registration.clone())
        })
    }

    pub fn unregister(
        &self,
        planner_id: &str,
        expected_lease_generation: u64,
        now_ms: u64,
    ) -> Result<PlannerRegistration, RegistryError> {
        validate_id("planner id", planner_id)?;
        self.mutate(now_ms, true, |document| {
            let index = document
                .registrations
                .iter()
                .position(|entry| entry.identity.planner_id == planner_id)
                .ok_or_else(|| {
                    RegistryError::Conflict(format!("planner {planner_id} is not registered"))
                })?;
            require_generation(&document.registrations[index], expected_lease_generation)?;
            Ok(document.registrations.remove(index))
        })
    }

    pub fn configure_roots(
        &self,
        roots: Vec<ConfiguredDiscoveryRoot>,
        now_ms: u64,
    ) -> Result<Vec<ConfiguredDiscoveryRoot>, RegistryError> {
        let roots = validate_and_sort_roots(roots)?;
        self.mutate(now_ms, true, move |document| {
            document.configured_roots = roots.clone();
            Ok(roots)
        })
    }

    /// Record either a successful or failed census. Completeness is assessed only by `inspect`.
    /// Keeping failed observations makes missing/unreachable roots auditable instead of absent.
    #[cfg(test)]
    pub fn record_census(
        &self,
        mut census: DiscoveryCensus,
        now_ms: u64,
    ) -> Result<DiscoveryCensus, RegistryError> {
        let snapshot = self.census_input_snapshot(now_ms)?;
        census.input_digest = snapshot.attestation.digest_hex();
        self.record_census_if_unchanged(&snapshot.attestation, census, now_ms)
    }

    /// Atomically publish census output only if the authority input still matches the snapshot
    /// that was collected. Rechecking both generation and digest under the write lock closes the
    /// mutation/path-swap window between collection and persistence.
    #[cfg(test)]
    pub(crate) fn record_census_if_unchanged(
        &self,
        expected: &CensusInputAttestation,
        census: DiscoveryCensus,
        now_ms: u64,
    ) -> Result<DiscoveryCensus, RegistryError> {
        self.record_census_if_unchanged_before(expected, census, u64::MAX, || now_ms)
    }

    /// Capability-aware conditional publish. Trusted time is sampled only while the registry
    /// lock is held and again immediately before atomic replacement, so lock wait can never spend
    /// the capability lifetime unnoticed.
    pub(crate) fn record_census_if_unchanged_before<N>(
        &self,
        expected: &CensusInputAttestation,
        mut census: DiscoveryCensus,
        capability_expires_at_ms: u64,
        mut trusted_now: N,
    ) -> Result<DiscoveryCensus, RegistryError>
    where
        N: FnMut() -> u64,
    {
        census.input_digest = expected.digest_hex();
        validate_census_shape(&census)?;
        let _lock = RegistryLock::acquire(self.path.as_path(), self.lock_timeout)?;
        let validation_now_ms = trusted_now();
        validate_timestamp("census record time", validation_now_ms)?;
        if validation_now_ms >= capability_expires_at_ms {
            return Err(RegistryError::CapabilityExpired);
        }
        let mut document =
            load_document(self.path.as_path()).map_err(RegistryError::UnknownState)?;
        let current = build_census_input_snapshot(&document, validation_now_ms)?;
        if &current.attestation != expected {
            return Err(RegistryError::Conflict(
                "registry census authority changed during collection".into(),
            ));
        }
        if census.registry_generation != current.attestation.registry_generation {
            return Err(RegistryError::Conflict(format!(
                "census generation {} does not match registry generation {}",
                census.registry_generation, current.attestation.registry_generation
            )));
        }
        validate_census_roots(&census, &current.configured_roots)?;
        if document
            .census
            .as_ref()
            .is_some_and(|prior| prior.captured_at_ms >= census.captured_at_ms)
        {
            return Err(RegistryError::Conflict(
                "census output is not newer than the recorded census".into(),
            ));
        }
        revalidate_snapshot_identities(&current)?;
        let persistence_now_ms = trusted_now();
        validate_timestamp("census persistence time", persistence_now_ms)?;
        if persistence_now_ms < validation_now_ms {
            return Err(RegistryError::ClockRollback);
        }
        if persistence_now_ms >= capability_expires_at_ms {
            return Err(RegistryError::CapabilityExpired);
        }
        validate_census_time(&census, persistence_now_ms)?;
        let authority_issues = validate_authority_time(&document, persistence_now_ms);
        if !authority_issues.is_empty() {
            return Err(RegistryError::UnknownState(authority_issues));
        }
        document.updated_at_ms = persistence_now_ms;
        document.census = Some(census.clone());
        let issues = validate_document_static(&document);
        if !issues.is_empty() {
            return Err(RegistryError::UnknownState(issues));
        }
        persist_document_before(self.path.as_path(), &document, || {
            let replace_now_ms = trusted_now();
            validate_timestamp("census atomic-replace time", replace_now_ms)?;
            if replace_now_ms < persistence_now_ms {
                return Err(RegistryError::ClockRollback);
            }
            if replace_now_ms >= capability_expires_at_ms {
                return Err(RegistryError::CapabilityExpired);
            }
            Ok(())
        })?;
        Ok(census)
    }

    fn mutate<T>(
        &self,
        now_ms: u64,
        advance_generation: bool,
        operation: impl FnOnce(&mut RegistryDocument) -> Result<T, RegistryError>,
    ) -> Result<T, RegistryError> {
        validate_timestamp("mutation time", now_ms)?;
        let parent = self.path.parent().ok_or_else(|| {
            RegistryError::InvalidInput("registry path has no parent directory".into())
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            RegistryError::Io(format!(
                "cannot create registry directory {}: {error}",
                parent.display()
            ))
        })?;
        let _lock = RegistryLock::acquire(self.path.as_path(), self.lock_timeout)?;
        let mut document = load_document_for_mutation(self.path.as_path(), now_ms)?;
        let result = operation(&mut document)?;
        if advance_generation {
            document.generation = document
                .generation
                .checked_add(1)
                .ok_or_else(|| RegistryError::Conflict("registry generation overflowed".into()))?;
        }
        document.updated_at_ms = now_ms;
        let static_issues = validate_document_static(&document);
        if !static_issues.is_empty() {
            return Err(RegistryError::UnknownState(static_issues));
        }
        persist_document(self.path.as_path(), &document)?;
        Ok(result)
    }
}

fn inspect_registry_file(path: &Path, now_ms: u64) -> RegistryRead {
    let document = match load_document(path) {
        Ok(document) => document,
        Err(issues) => {
            return RegistryRead::Unknown(UnknownRegistry {
                generation: None,
                registration_count: 0,
                issues,
            })
        }
    };
    let mut issues = validate_document_static(&document);
    issues.extend(validate_authority_time(&document, now_ms));
    issues.extend(validate_completeness(&document, now_ms));
    if issues.is_empty() {
        RegistryRead::Complete(document)
    } else {
        RegistryRead::Unknown(UnknownRegistry {
            generation: Some(document.generation),
            registration_count: document.registrations.len(),
            issues,
        })
    }
}

fn load_document_for_mutation(path: &Path, now_ms: u64) -> Result<RegistryDocument, RegistryError> {
    let document = load_document(path).map_err(RegistryError::UnknownState)?;
    let mut issues = validate_document_static(&document);
    issues.extend(validate_authority_time(&document, now_ms));
    if issues.is_empty() {
        Ok(document)
    } else {
        Err(RegistryError::UnknownState(issues))
    }
}

fn load_document(path: &Path) -> Result<RegistryDocument, Vec<RegistryIssue>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(vec![RegistryIssue::MissingRegistry])
        }
        Err(error) => return Err(vec![RegistryIssue::Unreadable(error.to_string())]),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(vec![RegistryIssue::Unreadable(
            "registry target is not a regular file".into(),
        )]);
    }
    if metadata.len() > MAX_REGISTRY_BYTES {
        return Err(vec![RegistryIssue::Unreadable(format!(
            "registry exceeds {MAX_REGISTRY_BYTES} bytes"
        ))]);
    }
    let bytes =
        fs::read(path).map_err(|error| vec![RegistryIssue::Unreadable(error.to_string())])?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| vec![RegistryIssue::Malformed(error.to_string())])?;
    let version = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            vec![RegistryIssue::Malformed(
                "missing numeric schemaVersion".into(),
            )]
        })?;
    if version != REGISTRY_SCHEMA_VERSION as u64 {
        return Err(vec![RegistryIssue::UnsupportedSchema(version)]);
    }
    serde_json::from_value(value).map_err(|error| vec![RegistryIssue::Malformed(error.to_string())])
}

fn validate_document_static(document: &RegistryDocument) -> Vec<RegistryIssue> {
    let mut issues = Vec::new();
    if document.schema_version != REGISTRY_SCHEMA_VERSION {
        issues.push(RegistryIssue::UnsupportedSchema(
            document.schema_version as u64,
        ));
    }
    if document.generation == 0 || document.updated_at_ms == 0 {
        issues.push(RegistryIssue::InvalidDocument(
            "generation and updatedAtMs must be non-zero".into(),
        ));
    }

    if document.registrations.len() > MAX_REGISTRATIONS {
        issues.push(RegistryIssue::InvalidDocument(format!(
            "registration count exceeds {MAX_REGISTRATIONS}"
        )));
    }
    if document.configured_roots.len() > MAX_CONFIGURED_ROOTS {
        issues.push(RegistryIssue::InvalidDocument(format!(
            "configured root count exceeds {MAX_CONFIGURED_ROOTS}"
        )));
    }

    let mut planner_ids = BTreeSet::new();
    let mut last_planner = None::<&str>;
    for registration in &document.registrations {
        if last_planner.is_some_and(|last| last >= registration.identity.planner_id.as_str()) {
            issues.push(RegistryIssue::InvalidDocument(
                "registrations must be sorted and unique by planner id".into(),
            ));
        }
        if !planner_ids.insert(registration.identity.planner_id.clone()) {
            issues.push(RegistryIssue::DuplicatePlanner(
                registration.identity.planner_id.clone(),
            ));
        }
        if let Err(error) = validate_registration(registration) {
            issues.push(RegistryIssue::InvalidDocument(error.to_string()));
        }
        last_planner = Some(registration.identity.planner_id.as_str());
    }

    let mut root_ids = BTreeSet::new();
    let mut root_paths = BTreeSet::new();
    let mut last_root = None::<&str>;
    for root in &document.configured_roots {
        if last_root.is_some_and(|last| last >= root.root_id.as_str()) {
            issues.push(RegistryIssue::InvalidDocument(
                "configured roots must be sorted and unique by root id".into(),
            ));
        }
        if let Err(error) = validate_root(root) {
            issues.push(RegistryIssue::InvalidDocument(error.to_string()));
        }
        if !root_ids.insert(root.root_id.clone()) {
            issues.push(RegistryIssue::DuplicateConfiguredRoot(root.root_id.clone()));
        }
        let comparable = comparable_path(&root.canonical_path);
        if !root_paths.insert(comparable) {
            issues.push(RegistryIssue::DuplicateConfiguredPath(
                root.canonical_path.clone(),
            ));
        }
        last_root = Some(root.root_id.as_str());
    }
    if document.configured_roots.is_empty() {
        issues.push(RegistryIssue::NoConfiguredRoots);
    }
    if let Some(census) = &document.census {
        if let Err(error) = validate_census_shape(census) {
            issues.push(RegistryIssue::InvalidDocument(error.to_string()));
        }
    }
    issues
}

fn validate_completeness(document: &RegistryDocument, now_ms: u64) -> Vec<RegistryIssue> {
    let mut issues = Vec::new();
    let Some(census) = &document.census else {
        issues.push(RegistryIssue::MissingCensus);
        return issues;
    };
    if census.registry_generation != document.generation {
        issues.push(RegistryIssue::CensusGenerationMismatch {
            registry_generation: document.generation,
            census_generation: census.registry_generation,
        });
    }
    match build_census_input_snapshot(document, now_ms) {
        Ok(input) if census.input_digest != input.attestation.digest_hex() => {
            issues.push(RegistryIssue::CensusInputDigestMismatch);
        }
        Ok(_) => {}
        Err(RegistryError::UnknownState(mut authority_issues)) => {
            issues.append(&mut authority_issues);
        }
        Err(_) => issues.push(RegistryIssue::InvalidDocument(
            "census authority input cannot be reconstructed".into(),
        )),
    }
    if census.captured_at_ms > now_ms {
        issues.push(RegistryIssue::FutureCensus(census.captured_at_ms));
    }
    if census.expires_at_ms <= now_ms {
        issues.push(RegistryIssue::StaleCensus(census.expires_at_ms));
    }

    let configured: BTreeSet<_> = document
        .configured_roots
        .iter()
        .map(|root| root.root_id.as_str())
        .collect();
    let mut observed_roots = BTreeSet::new();
    let mut observed_planners = BTreeSet::new();
    for root in &census.roots {
        if !observed_roots.insert(root.root_id.as_str()) {
            issues.push(RegistryIssue::DuplicateRootCensus(root.root_id.clone()));
        }
        if !configured.contains(root.root_id.as_str()) {
            issues.push(RegistryIssue::UnexpectedRootCensus(root.root_id.clone()));
        }
        if !root.reachable {
            issues.push(RegistryIssue::UnreachableRoot {
                root_id: root.root_id.clone(),
                failure: root.failure.clone(),
            });
        }
        for planner_id in &root.planner_ids {
            if !observed_planners.insert(planner_id.as_str()) {
                issues.push(RegistryIssue::DuplicateObservedPlanner(planner_id.clone()));
            }
        }
    }
    for root_id in configured.difference(&observed_roots) {
        issues.push(RegistryIssue::MissingRootCensus((*root_id).to_string()));
    }

    let registered: BTreeSet<_> = document
        .registrations
        .iter()
        .map(|entry| entry.identity.planner_id.as_str())
        .collect();
    for planner_id in registered.difference(&observed_planners) {
        issues.push(RegistryIssue::UnaccountedPlanner((*planner_id).to_string()));
    }
    for planner_id in observed_planners.difference(&registered) {
        issues.push(RegistryIssue::UnregisteredObservedPlanner(
            (*planner_id).to_string(),
        ));
    }
    issues
}

fn validate_authority_time(document: &RegistryDocument, now_ms: u64) -> Vec<RegistryIssue> {
    let mut issues = Vec::new();
    if now_ms == 0 {
        issues.push(RegistryIssue::InvalidDocument(
            "assessment time must be non-zero".into(),
        ));
        return issues;
    }
    if document.updated_at_ms > now_ms {
        issues.push(RegistryIssue::FutureDocumentUpdate(document.updated_at_ms));
    }
    for registration in &document.registrations {
        let latest_authority_time = registration
            .registered_at_ms
            .max(registration.updated_at_ms)
            .max(registration.heartbeat_at_ms);
        if latest_authority_time > now_ms {
            issues.push(RegistryIssue::FutureRegistrationTime {
                planner_id: registration.identity.planner_id.clone(),
                timestamp_ms: latest_authority_time,
            });
        }
        if registration.heartbeat_at_ms > now_ms {
            issues.push(RegistryIssue::FutureHeartbeat {
                planner_id: registration.identity.planner_id.clone(),
                heartbeat_at_ms: registration.heartbeat_at_ms,
            });
        }
        if registration.lease_expires_at_ms <= now_ms {
            issues.push(RegistryIssue::StaleRegistration {
                planner_id: registration.identity.planner_id.clone(),
                expired_at_ms: registration.lease_expires_at_ms,
            });
        }
    }
    issues
}

fn build_census_input_snapshot(
    document: &RegistryDocument,
    now_ms: u64,
) -> Result<CensusInputSnapshot, RegistryError> {
    let mut issues = validate_document_static(document);
    issues.extend(validate_authority_time(document, now_ms));
    if document.registrations.is_empty() {
        issues.push(RegistryIssue::InvalidDocument(
            "census input contains no registered Planner".into(),
        ));
    }
    if !issues.is_empty() {
        return Err(RegistryError::UnknownState(issues));
    }

    let configured_roots = document
        .configured_roots
        .iter()
        .map(|root| {
            let path = PathBuf::from(&root.canonical_path);
            let identity =
                physical_path_identity(&path, PhysicalPathKind::Directory).map_err(|_| {
                    RegistryError::UnknownState(vec![RegistryIssue::InvalidDocument(format!(
                        "configured root {} has ambiguous physical identity",
                        root.root_id
                    ))])
                })?;
            Ok(ValidatedDiscoveryRoot {
                root_id: root.root_id.clone(),
                path,
                identity,
            })
        })
        .collect::<Result<Vec<_>, RegistryError>>()?;
    let root_identities = configured_roots
        .iter()
        .map(|root| (root.identity.volume_id, root.identity.file_id))
        .collect::<BTreeSet<_>>();
    if root_identities.len() != configured_roots.len() {
        return Err(RegistryError::UnknownState(vec![
            RegistryIssue::InvalidDocument(
                "configured roots contain physical filesystem aliases".into(),
            ),
        ]));
    }

    let registrations = document
        .registrations
        .iter()
        .map(|registration| {
            let repository_root_identity = physical_path_identity(
                Path::new(&registration.identity.repository_root),
                PhysicalPathKind::Directory,
            )
            .map_err(|_| {
                authority_identity_error(&registration.identity.planner_id, "repository")
            })?;
            let worktree_root_identity = physical_path_identity(
                Path::new(&registration.identity.worktree_root),
                PhysicalPathKind::Directory,
            )
            .map_err(|_| authority_identity_error(&registration.identity.planner_id, "worktree"))?;
            let plan_identity = physical_path_identity(
                Path::new(&registration.identity.plan_path),
                PhysicalPathKind::RegularFile,
            )
            .map_err(|_| authority_identity_error(&registration.identity.planner_id, "plan"))?;
            if !plan_identity
                .canonical_path
                .starts_with(&worktree_root_identity.canonical_path)
            {
                return Err(RegistryError::UnknownState(vec![
                    RegistryIssue::InvalidDocument(format!(
                        "planner {} plan is outside its worktree",
                        registration.identity.planner_id
                    )),
                ]));
            }
            Ok(ValidatedPlannerRegistration {
                registration: registration.clone(),
                repository_root_identity,
                worktree_root_identity,
                plan_identity,
            })
        })
        .collect::<Result<Vec<_>, RegistryError>>()?;

    let input_digest = census_input_digest(document, &configured_roots, &registrations)?;
    let snapshot = CensusInputSnapshot {
        attestation: CensusInputAttestation {
            registry_generation: document.generation,
            input_digest,
        },
        configured_roots,
        registrations,
    };
    revalidate_snapshot_identities(&snapshot)?;
    Ok(snapshot)
}

fn authority_identity_error(planner_id: &str, subject: &str) -> RegistryError {
    RegistryError::UnknownState(vec![RegistryIssue::InvalidDocument(format!(
        "planner {planner_id} {subject} authority has ambiguous physical identity"
    ))])
}

fn revalidate_snapshot_identities(snapshot: &CensusInputSnapshot) -> Result<(), RegistryError> {
    for root in &snapshot.configured_roots {
        let current = physical_path_identity(&root.path, PhysicalPathKind::Directory)
            .map_err(|_| authority_identity_error(&root.root_id, "configured-root"))?;
        if current != root.identity {
            return Err(RegistryError::Conflict(
                "configured root identity changed during census snapshot".into(),
            ));
        }
    }
    for registration in &snapshot.registrations {
        let identity = &registration.registration.identity;
        let repository = physical_path_identity(
            Path::new(&identity.repository_root),
            PhysicalPathKind::Directory,
        )
        .map_err(|_| authority_identity_error(&identity.planner_id, "repository"))?;
        let worktree = physical_path_identity(
            Path::new(&identity.worktree_root),
            PhysicalPathKind::Directory,
        )
        .map_err(|_| authority_identity_error(&identity.planner_id, "worktree"))?;
        let plan = physical_path_identity(
            Path::new(&identity.plan_path),
            PhysicalPathKind::RegularFile,
        )
        .map_err(|_| authority_identity_error(&identity.planner_id, "plan"))?;
        if repository != registration.repository_root_identity
            || worktree != registration.worktree_root_identity
            || plan != registration.plan_identity
        {
            return Err(RegistryError::Conflict(format!(
                "planner {} authority identity changed during census snapshot",
                identity.planner_id
            )));
        }
    }
    Ok(())
}

fn census_input_digest(
    document: &RegistryDocument,
    roots: &[ValidatedDiscoveryRoot],
    registrations: &[ValidatedPlannerRegistration],
) -> Result<[u8; 32], RegistryError> {
    let mut encoder = DigestEncoder::new(b"perfect-planner:first-census-input:v1");
    encoder.u64(document.schema_version as u64);
    encoder.u64(document.generation);
    encoder.u64(roots.len() as u64);
    for root in roots {
        encoder.text(&root.root_id);
        encoder.physical(&root.identity);
    }
    encoder.u64(registrations.len() as u64);
    for validated in registrations {
        let registration = &validated.registration;
        let seed = &registration.identity;
        encoder.text(&seed.planner_id);
        encoder.text(&seed.repository_id);
        encoder.physical(&validated.repository_root_identity);
        encoder.physical(&validated.worktree_root_identity);
        encoder.text(&seed.branch);
        encoder.text(&seed.plan_id);
        encoder.physical(&validated.plan_identity);
        encoder.u64(registration.lease_generation);
        encode_paths(&mut encoder, &seed.files)?;
        encode_resources(&mut encoder, &seed.resources)?;
        encoder.u64(seed.nodes.len() as u64);
        for node in &seed.nodes {
            encoder.text(&node.node_id);
            encode_paths(&mut encoder, &node.files)?;
            encode_resources(&mut encoder, &node.resources)?;
        }
    }
    Ok(encoder.finish())
}

fn hex_digest(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn encode_paths(encoder: &mut DigestEncoder, paths: &[String]) -> Result<(), RegistryError> {
    encoder.u64(paths.len() as u64);
    for path in paths {
        encoder.text(&canonical_declared_path(path).map_err(|error| {
            RegistryError::InvalidInput(format!("cannot canonicalize manifest path: {error}"))
        })?);
    }
    Ok(())
}

fn encode_resources(
    encoder: &mut DigestEncoder,
    resources: &[String],
) -> Result<(), RegistryError> {
    encoder.u64(resources.len() as u64);
    for resource in resources {
        encoder.text(
            &canonical_resource_identity(resource)
                .map_err(|error| {
                    RegistryError::InvalidInput(format!(
                        "cannot canonicalize manifest resource: {error}"
                    ))
                })?
                .canonical_key,
        );
    }
    Ok(())
}

struct DigestEncoder(Sha256);

impl DigestEncoder {
    fn new(domain: &[u8]) -> Self {
        let mut encoder = Self(Sha256::new());
        encoder.bytes(domain);
        encoder
    }

    fn u64(&mut self, value: u64) {
        self.0.update(value.to_le_bytes());
    }

    fn bytes(&mut self, value: &[u8]) {
        self.u64(value.len() as u64);
        self.0.update(value);
    }

    fn text(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }

    fn physical(&mut self, value: &PhysicalPathIdentity) {
        self.u64(value.volume_id);
        self.bytes(&value.file_id);
    }

    fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

fn validate_census_time(census: &DiscoveryCensus, now_ms: u64) -> Result<(), RegistryError> {
    if census.captured_at_ms > now_ms || census.expires_at_ms <= now_ms {
        return Err(RegistryError::InvalidInput(
            "census must be captured no later than now and remain unexpired".into(),
        ));
    }
    Ok(())
}

fn validate_census_roots(
    census: &DiscoveryCensus,
    configured_roots: &[ValidatedDiscoveryRoot],
) -> Result<(), RegistryError> {
    if census.roots.len() != configured_roots.len()
        || census
            .roots
            .iter()
            .zip(configured_roots)
            .any(|(observed, configured)| observed.root_id != configured.root_id)
    {
        return Err(RegistryError::InvalidInput(
            "census must contain exactly every configured root in canonical order".into(),
        ));
    }
    Ok(())
}

fn validate_registration(registration: &PlannerRegistration) -> Result<(), RegistryError> {
    validate_seed(&registration.identity)?;
    if registration.lease_generation == 0
        || registration.registered_at_ms == 0
        || registration.updated_at_ms < registration.registered_at_ms
        || registration.heartbeat_at_ms < registration.registered_at_ms
        || registration.lease_expires_at_ms <= registration.heartbeat_at_ms
    {
        return Err(RegistryError::InvalidInput(format!(
            "planner {} has an invalid lease timeline",
            registration.identity.planner_id
        )));
    }
    let lease_span = registration
        .lease_expires_at_ms
        .checked_sub(registration.heartbeat_at_ms)
        .ok_or_else(|| RegistryError::InvalidInput("planner lease timeline underflowed".into()))?;
    if !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&lease_span) {
        return Err(RegistryError::InvalidInput(format!(
            "planner {} lease span exceeds the supported bound",
            registration.identity.planner_id
        )));
    }
    Ok(())
}

fn validate_seed(seed: &PlannerRegistrationSeed) -> Result<(), RegistryError> {
    validate_id("planner id", &seed.planner_id)?;
    validate_id("repository id", &seed.repository_id)?;
    validate_absolute_path("repository root", &seed.repository_root)?;
    validate_absolute_path("worktree root", &seed.worktree_root)?;
    validate_text("branch", &seed.branch)?;
    validate_id("plan id", &seed.plan_id)?;
    validate_absolute_path("plan path", &seed.plan_path)?;
    require_bound("plan file count", seed.files.len(), MAX_FILES_PER_MANIFEST)?;
    require_bound(
        "plan resource count",
        seed.resources.len(),
        MAX_RESOURCES_PER_MANIFEST,
    )?;
    validate_sorted_unique_relative_paths("plan files", &seed.files)?;
    validate_sorted_unique_resources("plan resources", &seed.resources)?;
    if seed.nodes.is_empty() {
        return Err(RegistryError::InvalidInput(
            "registration must contain at least one node manifest".into(),
        ));
    }
    require_bound(
        "node manifest count",
        seed.nodes.len(),
        MAX_NODES_PER_REGISTRATION,
    )?;
    let mut total_manifest_entries = seed
        .files
        .len()
        .checked_add(seed.resources.len())
        .ok_or_else(|| RegistryError::InvalidInput("manifest entry count overflowed".into()))?;
    let mut last_node = None::<&str>;
    for node in &seed.nodes {
        validate_id("node id", &node.node_id)?;
        if last_node.is_some_and(|last| last >= node.node_id.as_str()) {
            return Err(RegistryError::InvalidInput(
                "node manifests must be sorted and unique by node id".into(),
            ));
        }
        if node.files.is_empty() && node.resources.is_empty() {
            return Err(RegistryError::InvalidInput(format!(
                "node {} has neither files nor resources",
                node.node_id
            )));
        }
        require_bound("node file count", node.files.len(), MAX_FILES_PER_MANIFEST)?;
        require_bound(
            "node resource count",
            node.resources.len(),
            MAX_RESOURCES_PER_MANIFEST,
        )?;
        total_manifest_entries = total_manifest_entries
            .checked_add(node.files.len())
            .and_then(|count| count.checked_add(node.resources.len()))
            .ok_or_else(|| RegistryError::InvalidInput("manifest entry count overflowed".into()))?;
        validate_sorted_unique_relative_paths("node files", &node.files)?;
        validate_sorted_unique_resources("node resources", &node.resources)?;
        last_node = Some(node.node_id.as_str());
    }
    require_bound(
        "aggregate manifest entry count",
        total_manifest_entries,
        MAX_TOTAL_MANIFEST_ENTRIES,
    )?;
    Ok(())
}

fn validate_and_sort_roots(
    mut roots: Vec<ConfiguredDiscoveryRoot>,
) -> Result<Vec<ConfiguredDiscoveryRoot>, RegistryError> {
    if roots.is_empty() {
        return Err(RegistryError::InvalidInput(
            "at least one discovery root must be configured".into(),
        ));
    }
    require_bound("configured root count", roots.len(), MAX_CONFIGURED_ROOTS)?;
    roots.sort_by(|left, right| left.root_id.cmp(&right.root_id));
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for root in &roots {
        validate_root(root)?;
        if !ids.insert(root.root_id.clone()) {
            return Err(RegistryError::InvalidInput(format!(
                "duplicate discovery root id {}",
                root.root_id
            )));
        }
        if !paths.insert(comparable_path(&root.canonical_path)) {
            return Err(RegistryError::InvalidInput(format!(
                "duplicate discovery root path {}",
                root.canonical_path
            )));
        }
    }
    Ok(roots)
}

fn validate_root(root: &ConfiguredDiscoveryRoot) -> Result<(), RegistryError> {
    validate_id("discovery root id", &root.root_id)?;
    validate_absolute_path("discovery root path", &root.canonical_path)
}

fn validate_census_shape(census: &DiscoveryCensus) -> Result<(), RegistryError> {
    if census.registry_generation == 0
        || census.captured_at_ms == 0
        || census.expires_at_ms <= census.captured_at_ms
    {
        return Err(RegistryError::InvalidInput(
            "census generation and time window must be valid".into(),
        ));
    }
    if census.input_digest.len() != 64
        || !census
            .input_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(RegistryError::InvalidInput(
            "census input digest must be a lowercase SHA-256".into(),
        ));
    }
    let span = census
        .expires_at_ms
        .checked_sub(census.captured_at_ms)
        .ok_or_else(|| RegistryError::InvalidInput("census time window underflowed".into()))?;
    if span > MAX_CENSUS_TTL_MS {
        return Err(RegistryError::InvalidInput(format!(
            "census time window exceeds {MAX_CENSUS_TTL_MS} milliseconds"
        )));
    }
    require_bound(
        "census root count",
        census.roots.len(),
        MAX_CONFIGURED_ROOTS,
    )?;
    let mut last_root = None::<&str>;
    for root in &census.roots {
        validate_id("census root id", &root.root_id)?;
        if last_root.is_some_and(|last| last >= root.root_id.as_str()) {
            return Err(RegistryError::InvalidInput(
                "census roots must be sorted and unique by root id".into(),
            ));
        }
        validate_sorted_unique_ids("census planner ids", &root.planner_ids)?;
        require_bound(
            "census planner count",
            root.planner_ids.len(),
            MAX_CENSUS_PLANNERS_PER_ROOT,
        )?;
        match (root.reachable, root.failure) {
            (true, None) => {}
            (false, Some(_)) => {}
            (true, Some(_)) => {
                return Err(RegistryError::InvalidInput(
                    "reachable census root must not carry a failure".into(),
                ))
            }
            (false, None) => {
                return Err(RegistryError::InvalidInput(
                    "unreachable census root must carry a failure".into(),
                ))
            }
        }
        last_root = Some(root.root_id.as_str());
    }
    Ok(())
}

fn validate_text(label: &str, value: &str) -> Result<(), RegistryError> {
    if value.trim().is_empty()
        || value.len() > MAX_TEXT_BYTES
        || value.chars().any(|character| character.is_control())
    {
        return Err(RegistryError::InvalidInput(format!(
            "{label} must be non-empty, bounded text without control characters"
        )));
    }
    Ok(())
}

fn validate_id(label: &str, value: &str) -> Result<(), RegistryError> {
    validate_text(label, value)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(RegistryError::InvalidInput(format!(
            "{label} contains unsupported characters"
        )));
    }
    Ok(())
}

fn validate_absolute_path(label: &str, value: &str) -> Result<(), RegistryError> {
    validate_text(label, value)?;
    let path = Path::new(value);
    if !path.is_absolute() || has_parent_component(path) {
        return Err(RegistryError::InvalidInput(format!(
            "{label} must be an absolute path without parent traversal"
        )));
    }
    Ok(())
}

fn validate_sorted_unique_relative_paths(
    label: &str,
    values: &[String],
) -> Result<(), RegistryError> {
    require_bound(label, values.len(), MAX_FILES_PER_MANIFEST)?;
    let mut last = None::<String>;
    for value in values {
        validate_text(label, value)?;
        let normalized = canonical_declared_path(value).map_err(|error| {
            RegistryError::InvalidInput(format!("{label} contains an invalid path: {error}"))
        })?;
        if last
            .as_ref()
            .is_some_and(|previous| previous >= &normalized)
        {
            return Err(RegistryError::InvalidInput(format!(
                "{label} must be canonically sorted and unique"
            )));
        }
        last = Some(normalized);
    }
    Ok(())
}

fn validate_sorted_unique_text(label: &str, values: &[String]) -> Result<(), RegistryError> {
    let mut last = None::<&str>;
    for value in values {
        validate_text(label, value)?;
        if last.is_some_and(|previous| previous >= value.as_str()) {
            return Err(RegistryError::InvalidInput(format!(
                "{label} must be sorted and unique"
            )));
        }
        last = Some(value.as_str());
    }
    Ok(())
}

fn validate_sorted_unique_resources(label: &str, values: &[String]) -> Result<(), RegistryError> {
    let mut last = None::<String>;
    for value in values {
        validate_text(label, value)?;
        let canonical = canonical_resource_identity(value)
            .map_err(|error| RegistryError::InvalidInput(format!("{label}: {error}")))?
            .canonical_key;
        if last.as_ref().is_some_and(|previous| previous >= &canonical) {
            return Err(RegistryError::InvalidInput(format!(
                "{label} must be canonically sorted and unique"
            )));
        }
        last = Some(canonical);
    }
    Ok(())
}

fn require_bound(label: &str, actual: usize, maximum: usize) -> Result<(), RegistryError> {
    if actual > maximum {
        Err(RegistryError::InvalidInput(format!(
            "{label} exceeds maximum {maximum}"
        )))
    } else {
        Ok(())
    }
}

fn validate_sorted_unique_ids(label: &str, values: &[String]) -> Result<(), RegistryError> {
    validate_sorted_unique_text(label, values)?;
    for value in values {
        validate_id(label, value)?;
    }
    Ok(())
}

fn validate_timestamp(label: &str, value: u64) -> Result<(), RegistryError> {
    if value == 0 {
        Err(RegistryError::InvalidInput(format!(
            "{label} must be non-zero"
        )))
    } else {
        Ok(())
    }
}

fn lease_expiry(now_ms: u64, lease_ms: u64) -> Result<u64, RegistryError> {
    validate_timestamp("lease time", now_ms)?;
    if !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&lease_ms) {
        return Err(RegistryError::InvalidInput(format!(
            "lease must be between {MIN_LEASE_MS} and {MAX_LEASE_MS} milliseconds"
        )));
    }
    now_ms
        .checked_add(lease_ms)
        .ok_or_else(|| RegistryError::InvalidInput("lease expiry overflowed".into()))
}

fn registration_mut<'a>(
    document: &'a mut RegistryDocument,
    planner_id: &str,
) -> Result<&'a mut PlannerRegistration, RegistryError> {
    document
        .registrations
        .iter_mut()
        .find(|entry| entry.identity.planner_id == planner_id)
        .ok_or_else(|| RegistryError::Conflict(format!("planner {planner_id} is not registered")))
}

fn require_generation(
    registration: &PlannerRegistration,
    expected: u64,
) -> Result<(), RegistryError> {
    if registration.lease_generation != expected {
        Err(RegistryError::Conflict(format!(
            "planner {} lease generation changed from {expected} to {}",
            registration.identity.planner_id, registration.lease_generation
        )))
    } else {
        Ok(())
    }
}

fn sort_registrations(registrations: &mut [PlannerRegistration]) {
    registrations.sort_by(|left, right| left.identity.planner_id.cmp(&right.identity.planner_id));
}

fn comparable_path(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\").to_ascii_lowercase()
    } else {
        path.to_string()
    }
}

fn has_parent_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}

fn persist_document(path: &Path, document: &RegistryDocument) -> Result<(), RegistryError> {
    persist_document_before(path, document, || Ok(()))
}

fn persist_document_before<F>(
    path: &Path,
    document: &RegistryDocument,
    before_replace: F,
) -> Result<(), RegistryError>
where
    F: FnOnce() -> Result<(), RegistryError>,
{
    let mut bytes = serde_json::to_vec_pretty(document).map_err(|error| {
        RegistryError::Io(format!("cannot serialize registry document: {error}"))
    })?;
    bytes.push(b'\n');
    atomic_replace_before(path, &bytes, before_replace)
}

fn atomic_replace_before<F>(
    path: &Path,
    bytes: &[u8],
    before_replace: F,
) -> Result<(), RegistryError>
where
    F: FnOnce() -> Result<(), RegistryError>,
{
    let parent = path
        .parent()
        .ok_or_else(|| RegistryError::InvalidInput("registry target has no parent".into()))?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(RegistryError::InvalidInput(
                "registry target is not a regular file".into(),
            ));
        }
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| RegistryError::InvalidInput("invalid registry target name".into()))?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{file_name}.tmp-{}-{sequence}",
        std::process::id()
    ));
    let result = (|| -> Result<(), RegistryError> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| registry_persist_io(path, error))?;
        file.write_all(bytes)
            .map_err(|error| registry_persist_io(path, error))?;
        file.sync_all()
            .map_err(|error| registry_persist_io(path, error))?;
        before_replace()?;
        replace_file(&temporary, path).map_err(|error| registry_persist_io(path, error))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn registry_persist_io(path: &Path, error: io::Error) -> RegistryError {
    RegistryError::Io(format!(
        "cannot atomically persist registry {}: {error}",
        path.display()
    ))
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
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
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
    let started = Instant::now();
    let mut attempt = 0_usize;
    loop {
        // SAFETY: both buffers are valid NUL-terminated UTF-16 for the duration of this call.
        let moved = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if moved != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(5 | 32))
            || started.elapsed() >= DEFAULT_LOCK_TIMEOUT
        {
            return Err(error);
        }
        thread::sleep(retry_delay(attempt));
        attempt = attempt.saturating_add(1);
    }
}

fn registry_lock_path(registry_path: &Path) -> PathBuf {
    let mut name = registry_path
        .file_name()
        .map_or_else(|| OsString::from("registry-v1.json"), OsString::from);
    name.push(".mutex");
    registry_path.with_file_name(name)
}

#[cfg(windows)]
struct RegistryLock {
    handle: *mut std::ffi::c_void,
}

#[cfg(windows)]
impl RegistryLock {
    fn acquire(registry_path: &Path, timeout: Duration) -> Result<Self, RegistryError> {
        use std::os::windows::ffi::OsStrExt;

        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const OPEN_ALWAYS: u32 = 4;
        const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
        const INVALID_HANDLE_VALUE: *mut std::ffi::c_void = -1_isize as *mut std::ffi::c_void;
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
        }

        let path = registry_lock_path(registry_path);
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(RegistryError::InvalidInput(
                    "registry mutex target is not a regular file".into(),
                ));
            }
        }
        let encoded = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let started = Instant::now();
        let mut attempt = 0_usize;
        loop {
            // SAFETY: the path is a valid NUL-terminated UTF-16 buffer. Share mode zero makes
            // this handle an exclusive cross-process mutex; process death closes the handle.
            let handle = unsafe {
                CreateFileW(
                    encoded.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    std::ptr::null_mut(),
                    OPEN_ALWAYS,
                    FILE_ATTRIBUTE_NORMAL,
                    std::ptr::null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                return Ok(Self { handle });
            }
            let error = io::Error::last_os_error();
            if !matches!(error.raw_os_error(), Some(5 | 32 | 33)) {
                return Err(RegistryError::Io(format!(
                    "cannot acquire registry mutex {}: {error}",
                    path.display()
                )));
            }
            if started.elapsed() >= timeout {
                return Err(RegistryError::LockTimeout(path));
            }
            thread::sleep(retry_delay(attempt));
            attempt = attempt.saturating_add(1);
        }
    }
}

#[cfg(windows)]
impl Drop for RegistryLock {
    fn drop(&mut self) {
        #[link(name = "Kernel32")]
        unsafe extern "system" {
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        // SAFETY: `handle` is uniquely owned by this guard and closed exactly once here.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(not(windows))]
struct RegistryLock {
    path: PathBuf,
}

#[cfg(not(windows))]
impl RegistryLock {
    fn acquire(registry_path: &Path, timeout: Duration) -> Result<Self, RegistryError> {
        let path = registry_lock_path(registry_path);
        let started = Instant::now();
        let mut attempt = 0_usize;
        loop {
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id()).map_err(|error| {
                        RegistryError::Io(format!("cannot initialize registry mutex: {error}"))
                    })?;
                    file.sync_all().map_err(|error| {
                        RegistryError::Io(format!("cannot flush registry mutex: {error}"))
                    })?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if started.elapsed() >= timeout {
                        return Err(RegistryError::LockTimeout(path));
                    }
                    thread::sleep(retry_delay(attempt));
                    attempt = attempt.saturating_add(1);
                }
                Err(error) => {
                    return Err(RegistryError::Io(format!(
                        "cannot acquire registry mutex {}: {error}",
                        path.display()
                    )))
                }
            }
        }
    }
}

#[cfg(not(windows))]
impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn retry_delay(attempt: usize) -> Duration {
    const BACKOFF_MS: [u64; 6] = [2, 4, 8, 16, 25, 40];
    Duration::from_millis(BACKOFF_MS[attempt.min(BACKOFF_MS.len() - 1)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_ID: AtomicU64 = AtomicU64::new(1);

    struct TempRegistry {
        root: PathBuf,
        store: PlannerRegistryStore,
    }

    impl TempRegistry {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "pp-collision-registry-{name}-{}-{}",
                std::process::id(),
                TEST_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).expect("create test root");
            let store = PlannerRegistryStore::new(root.join("registry-v1.json"))
                .expect("valid registry path");
            Self { root, store }
        }
    }

    impl Drop for TempRegistry {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn root(id: &str) -> ConfiguredDiscoveryRoot {
        let path = std::env::temp_dir().join(format!(
            "pp-collision-discovery-{id}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        ConfiguredDiscoveryRoot {
            root_id: id.to_string(),
            canonical_path: path.to_string_lossy().into_owned(),
        }
    }

    fn seed(id: &str) -> PlannerRegistrationSeed {
        let worktree = std::env::temp_dir().join(format!("worktree-{id}"));
        let repository = std::env::temp_dir().join(format!("repository-{id}"));
        let plan_path = worktree.join(".claude/scratch/perfect-plan/plan.json");
        fs::create_dir_all(&repository).unwrap();
        fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
        if !plan_path.exists() {
            fs::write(&plan_path, b"{}\n").unwrap();
        }
        PlannerRegistrationSeed {
            planner_id: id.to_string(),
            repository_id: format!("repo-{id}"),
            repository_root: repository.to_string_lossy().into_owned(),
            worktree_root: worktree.to_string_lossy().into_owned(),
            branch: format!("feature/{id}"),
            plan_id: "PP-002".into(),
            plan_path: plan_path.to_string_lossy().into_owned(),
            files: vec!["src/main.rs".into()],
            resources: vec![format!("mutex:{id}")],
            nodes: vec![PlannerNodeManifest {
                node_id: "B02".into(),
                files: vec!["src/main.rs".into()],
                resources: vec![format!("mutex:{id}")],
            }],
        }
    }

    fn census(generation: u64, root_planners: Vec<(&str, Vec<String>)>) -> DiscoveryCensus {
        DiscoveryCensus {
            registry_generation: generation,
            input_digest: "0".repeat(64),
            captured_at_ms: 2_000,
            expires_at_ms: 10_000,
            roots: root_planners
                .into_iter()
                .map(|(root_id, planner_ids)| DiscoveryRootCensus {
                    root_id: root_id.into(),
                    reachable: true,
                    planner_ids,
                    failure: None,
                })
                .collect(),
        }
    }

    fn initialized_snapshot(name: &str, planner_id: &str) -> (TempRegistry, CensusInputSnapshot) {
        let fixture = TempRegistry::new(name);
        fixture
            .store
            .initialize(vec![root(&format!("root-{name}"))], 1_000)
            .unwrap();
        fixture
            .store
            .register(seed(planner_id), 1_100, 20_000)
            .unwrap();
        let snapshot = fixture.store.census_input_snapshot(1_200).unwrap();
        (fixture, snapshot)
    }

    fn raw_document(store: &PlannerRegistryStore) -> RegistryDocument {
        load_document(store.path()).expect("valid registry document")
    }

    #[test]
    fn persists_versioned_registration_and_exact_census() {
        let fixture = TempRegistry::new("roundtrip");
        fixture
            .store
            .initialize(vec![root("root-a")], 1_000)
            .unwrap();
        assert!(matches!(
            fixture.store.inspect(1_100),
            RegistryRead::Unknown(UnknownRegistry { issues, .. })
                if issues.contains(&RegistryIssue::MissingCensus)
        ));

        let registered = fixture
            .store
            .register(seed("planner-a"), 1_500, 5_000)
            .unwrap();
        assert_eq!(registered.lease_generation, 1);
        let document = raw_document(&fixture.store);
        assert_eq!(document.schema_version, REGISTRY_SCHEMA_VERSION);
        assert_eq!(document.generation, 2);
        assert_eq!(document.registrations[0].identity.nodes[0].node_id, "B02");
        assert_eq!(document.registrations[0].identity.files, ["src/main.rs"]);

        fixture
            .store
            .record_census(census(2, vec![("root-a", vec!["planner-a".into()])]), 2_000)
            .unwrap();
        let RegistryRead::Complete(reloaded) = fixture.store.inspect(2_500) else {
            panic!("exact fresh census should complete the registry")
        };
        assert_eq!(reloaded.registrations, vec![registered]);
        assert_eq!(reloaded.configured_roots, vec![root("root-a")]);
    }

    #[test]
    fn concurrent_register_heartbeat_update_and_unregister_are_lossless() {
        let fixture = TempRegistry::new("concurrent");
        fixture
            .store
            .initialize(vec![root("root-a")], 1_000)
            .unwrap();

        let registrations = (0..16)
            .map(|index| {
                let store = fixture.store.clone();
                thread::spawn(move || {
                    store
                        .register(seed(&format!("planner-{index:02}")), 2_000, 20_000)
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        for handle in registrations {
            handle.join().unwrap();
        }

        let heartbeats = (0..16)
            .map(|index| {
                let store = fixture.store.clone();
                thread::spawn(move || {
                    store
                        .heartbeat(&format!("planner-{index:02}"), 1, 3_000, 20_000)
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        for handle in heartbeats {
            handle.join().unwrap();
        }

        let updates = (0..8)
            .map(|index| {
                let store = fixture.store.clone();
                thread::spawn(move || {
                    let id = format!("planner-{index:02}");
                    let mut replacement = seed(&id);
                    replacement.resources = vec![format!("mutex:{id}"), "port:tcp:5235".into()];
                    store.update(&id, 2, replacement, 4_000, 20_000).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let removals = (8..16)
            .map(|index| {
                let store = fixture.store.clone();
                thread::spawn(move || {
                    let id = format!("planner-{index:02}");
                    store.unregister(&id, 2, 4_000).unwrap()
                })
            })
            .collect::<Vec<_>>();
        for handle in updates.into_iter().chain(removals) {
            handle.join().unwrap();
        }

        let document = raw_document(&fixture.store);
        assert_eq!(document.generation, 49);
        assert_eq!(document.registrations.len(), 8);
        assert!(document
            .registrations
            .iter()
            .all(|entry| entry.lease_generation == 3));
        let ids = document
            .registrations
            .iter()
            .map(|entry| entry.identity.planner_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), 8);
        assert!(fs::read_dir(&fixture.root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")));
    }

    #[test]
    fn missing_malformed_partial_unsupported_duplicate_and_stale_are_unknown() {
        let missing = TempRegistry::new("missing");
        assert_eq!(
            missing.store.inspect(1_000).issues(),
            &[RegistryIssue::MissingRegistry]
        );

        let malformed = TempRegistry::new("malformed");
        fs::write(malformed.store.path(), b"{not-json").unwrap();
        assert!(matches!(
            malformed.store.inspect(1_000).issues(),
            [RegistryIssue::Malformed(_)]
        ));

        let partial = TempRegistry::new("partial");
        fs::write(
            partial.store.path(),
            b"{\"schemaVersion\":1,\"generation\":",
        )
        .unwrap();
        assert!(matches!(
            partial.store.inspect(1_000).issues(),
            [RegistryIssue::Malformed(_)]
        ));

        let unsupported = TempRegistry::new("unsupported");
        fs::write(unsupported.store.path(), b"{\"schemaVersion\":99}").unwrap();
        assert_eq!(
            unsupported.store.inspect(1_000).issues(),
            &[RegistryIssue::UnsupportedSchema(99)]
        );

        let duplicate = TempRegistry::new("duplicate");
        duplicate
            .store
            .initialize(vec![root("root-a")], 1_000)
            .unwrap();
        let registration = duplicate
            .store
            .register(seed("planner-a"), 1_100, 1_000)
            .unwrap();
        let mut duplicated = raw_document(&duplicate.store);
        duplicated.registrations.push(registration);
        fs::write(
            duplicate.store.path(),
            serde_json::to_vec(&duplicated).unwrap(),
        )
        .unwrap();
        assert!(duplicate
            .store
            .inspect(1_200)
            .issues()
            .contains(&RegistryIssue::DuplicatePlanner("planner-a".into())));

        let stale = TempRegistry::new("stale");
        stale.store.initialize(vec![root("root-a")], 1_000).unwrap();
        stale
            .store
            .register(seed("planner-a"), 1_100, 1_000)
            .unwrap();
        stale
            .store
            .record_census(census(2, vec![("root-a", vec!["planner-a".into()])]), 2_000)
            .unwrap();
        assert!(stale
            .store
            .inspect(2_100)
            .issues()
            .contains(&RegistryIssue::StaleRegistration {
                planner_id: "planner-a".into(),
                expired_at_ms: 2_100,
            }));
    }

    #[test]
    fn incomplete_root_census_never_becomes_complete() {
        let fixture = TempRegistry::new("census");
        fixture
            .store
            .initialize(vec![root("root-a"), root("root-b")], 1_000)
            .unwrap();
        fixture
            .store
            .register(seed("planner-a"), 1_100, 8_000)
            .unwrap();
        fixture
            .store
            .register(seed("planner-b"), 1_200, 8_000)
            .unwrap();

        assert!(matches!(
            fixture
                .store
                .record_census(census(3, vec![("root-a", vec!["planner-a".into()])]), 2_000),
            Err(RegistryError::InvalidInput(_))
        ));
        assert!(fixture
            .store
            .inspect(2_100)
            .issues()
            .contains(&RegistryIssue::MissingCensus));

        fixture
            .store
            .record_census(
                DiscoveryCensus {
                    registry_generation: 3,
                    input_digest: "0".repeat(64),
                    captured_at_ms: 2_200,
                    expires_at_ms: 8_000,
                    roots: vec![
                        DiscoveryRootCensus {
                            root_id: "root-a".into(),
                            reachable: true,
                            planner_ids: vec!["planner-a".into()],
                            failure: None,
                        },
                        DiscoveryRootCensus {
                            root_id: "root-b".into(),
                            reachable: false,
                            planner_ids: vec!["planner-b".into()],
                            failure: Some(DiscoveryFailureCode::AccessDenied),
                        },
                    ],
                },
                2_200,
            )
            .unwrap();
        assert!(fixture
            .store
            .inspect(2_300)
            .issues()
            .contains(&RegistryIssue::UnreachableRoot {
                root_id: "root-b".into(),
                failure: Some(DiscoveryFailureCode::AccessDenied),
            }));

        let mut recovered = census(
            3,
            vec![
                ("root-a", vec!["planner-a".into()]),
                ("root-b", vec!["planner-b".into()]),
            ],
        );
        recovered.captured_at_ms = 2_400;
        recovered.expires_at_ms = 10_000;
        fixture.store.record_census(recovered, 2_400).unwrap();
        assert!(fixture.store.inspect(2_500).is_complete());

        fixture
            .store
            .heartbeat("planner-a", 1, 2_600, 8_000)
            .unwrap();
        assert!(fixture.store.inspect(2_700).issues().contains(
            &RegistryIssue::CensusGenerationMismatch {
                registry_generation: 4,
                census_generation: 3,
            }
        ));
    }

    #[test]
    fn malformed_state_cannot_be_silently_reinitialized_or_mutated() {
        let fixture = TempRegistry::new("preserve-corruption");
        let corrupt = b"{\"schemaVersion\":1,\"generation\":";
        fs::write(fixture.store.path(), corrupt).unwrap();

        assert!(matches!(
            fixture.store.initialize(vec![root("root-a")], 1_000),
            Err(RegistryError::Conflict(_))
        ));
        assert!(matches!(
            fixture.store.register(seed("planner-a"), 1_100, 2_000),
            Err(RegistryError::UnknownState(_))
        ));
        assert_eq!(fs::read(fixture.store.path()).unwrap(), corrupt);
    }

    #[test]
    fn first_census_snapshot_is_available_and_digest_ignores_prior_output() {
        let fixture = TempRegistry::new("first-census");
        fixture
            .store
            .initialize(vec![root("root-first")], 1_000)
            .unwrap();
        assert!(matches!(
            fixture.store.census_input_snapshot(1_050),
            Err(RegistryError::UnknownState(_))
        ));
        fixture
            .store
            .register(seed("planner-first"), 1_100, 20_000)
            .unwrap();

        let first = fixture.store.census_input_snapshot(1_200).unwrap();
        assert_eq!(first.attestation.registry_generation, 2);
        assert_eq!(first.configured_roots.len(), 1);
        assert_eq!(first.registrations.len(), 1);
        fixture
            .store
            .record_census_if_unchanged(
                &first.attestation,
                census(2, vec![("root-first", vec!["planner-first".into()])]),
                2_000,
            )
            .unwrap();

        let after_census = fixture.store.census_input_snapshot(2_100).unwrap();
        assert_eq!(first.attestation, after_census.attestation);
        let mut timestamp_only = raw_document(&fixture.store);
        timestamp_only.updated_at_ms += 1;
        timestamp_only.registrations[0].updated_at_ms += 1;
        timestamp_only.registrations[0].heartbeat_at_ms += 1;
        timestamp_only.registrations[0].lease_expires_at_ms += 1;
        fs::write(
            fixture.store.path(),
            serde_json::to_vec(&timestamp_only).unwrap(),
        )
        .unwrap();
        let after_timestamp_only = fixture.store.census_input_snapshot(2_200).unwrap();
        assert_eq!(first.attestation, after_timestamp_only.attestation);
        fixture
            .store
            .heartbeat("planner-first", 1, 3_000, 20_000)
            .unwrap();
        let after_heartbeat = fixture.store.census_input_snapshot(3_100).unwrap();
        assert_ne!(first.attestation, after_heartbeat.attestation);
    }

    #[test]
    fn forged_lifetimes_future_times_and_structural_overflow_are_unknown() {
        let fixture = TempRegistry::new("bounds");
        fixture
            .store
            .initialize(vec![root("root-bounds")], 1_000)
            .unwrap();
        fixture
            .store
            .register(seed("planner-bounds"), 1_100, 20_000)
            .unwrap();
        let valid = raw_document(&fixture.store);

        let mut excessive_lease = valid.clone();
        excessive_lease.registrations[0].lease_expires_at_ms = excessive_lease.registrations[0]
            .heartbeat_at_ms
            .checked_add(MAX_LEASE_MS + 1)
            .unwrap();
        fs::write(
            fixture.store.path(),
            serde_json::to_vec(&excessive_lease).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            fixture.store.census_input_snapshot(1_200),
            Err(RegistryError::UnknownState(_))
        ));

        let mut future = valid.clone();
        future.updated_at_ms = 1_201;
        fs::write(fixture.store.path(), serde_json::to_vec(&future).unwrap()).unwrap();
        assert!(matches!(
            fixture.store.census_input_snapshot(1_200),
            Err(RegistryError::UnknownState(ref issues))
                if issues.contains(&RegistryIssue::FutureDocumentUpdate(1_201))
        ));

        let mut overflow = valid.clone();
        overflow.registrations[0].heartbeat_at_ms = u64::MAX;
        overflow.registrations[0].lease_expires_at_ms = 1;
        fs::write(fixture.store.path(), serde_json::to_vec(&overflow).unwrap()).unwrap();
        assert!(matches!(
            fixture.store.census_input_snapshot(1_200),
            Err(RegistryError::UnknownState(_))
        ));

        for (maximum, label) in [
            (MAX_CONFIGURED_ROOTS, "roots"),
            (MAX_REGISTRATIONS, "registrations"),
            (MAX_NODES_PER_REGISTRATION, "nodes"),
            (MAX_FILES_PER_MANIFEST, "files"),
            (MAX_RESOURCES_PER_MANIFEST, "resources"),
            (MAX_CENSUS_PLANNERS_PER_ROOT, "census planners"),
            (MAX_TOTAL_MANIFEST_ENTRIES, "aggregate entries"),
        ] {
            assert!(require_bound(label, maximum, maximum).is_ok());
            assert!(require_bound(label, maximum + 1, maximum).is_err());
        }
        let excessive_census = DiscoveryCensus {
            registry_generation: 2,
            input_digest: "0".repeat(64),
            captured_at_ms: 1,
            expires_at_ms: 1 + MAX_CENSUS_TTL_MS + 1,
            roots: Vec::new(),
        };
        assert!(validate_census_shape(&excessive_census).is_err());

        let mut too_many_registrations = valid;
        let template = too_many_registrations.registrations[0].clone();
        too_many_registrations.registrations = (0..=MAX_REGISTRATIONS)
            .map(|index| {
                let mut registration = template.clone();
                registration.identity.planner_id = format!("planner-{index:04}");
                registration
            })
            .collect();
        assert!(validate_document_static(&too_many_registrations)
            .iter()
            .any(|issue| matches!(issue, RegistryIssue::InvalidDocument(message) if message.contains("registration count"))));
    }

    #[test]
    fn physical_alias_swaps_change_or_invalidate_the_attestation() {
        let fixture = TempRegistry::new("identity-swap");
        fixture
            .store
            .initialize(vec![root("root-swap")], 1_000)
            .unwrap();
        let registered = fixture
            .store
            .register(seed("planner-swap"), 1_100, 20_000)
            .unwrap();
        let before = fixture.store.census_input_snapshot(1_200).unwrap();
        let plan = PathBuf::from(&registered.identity.plan_path);
        fs::remove_file(&plan).unwrap();
        fs::write(&plan, b"{\"replacement\":true}\n").unwrap();
        let after = fixture.store.census_input_snapshot(1_300).unwrap();
        assert_ne!(
            before.attestation.input_digest,
            after.attestation.input_digest
        );
        assert!(matches!(
            fixture.store.record_census_if_unchanged(
                &before.attestation,
                census(2, vec![("root-swap", vec!["planner-swap".into()])]),
                2_000,
            ),
            Err(RegistryError::Conflict(_))
        ));

        let target = fixture.root.join("junction-target");
        let alias = fixture.root.join("junction-alias");
        fs::create_dir_all(&target).unwrap();
        create_directory_alias(&alias, &target);
        let aliased = TempRegistry::new("aliased-root");
        aliased
            .store
            .initialize(
                vec![ConfiguredDiscoveryRoot {
                    root_id: "root-alias".into(),
                    canonical_path: alias.to_string_lossy().into_owned(),
                }],
                1_000,
            )
            .unwrap();
        assert!(matches!(
            aliased.store.census_input_snapshot(1_100),
            Err(RegistryError::UnknownState(_))
        ));
    }

    #[test]
    fn stable_hardlink_aliases_share_one_plan_authority_identity() {
        let fixture = TempRegistry::new("hardlink");
        fixture
            .store
            .initialize(vec![root("root-hardlink")], 1_000)
            .unwrap();
        let registered = fixture
            .store
            .register(seed("planner-hardlink"), 1_100, 20_000)
            .unwrap();
        let original = PathBuf::from(&registered.identity.plan_path);
        let alias = original.with_file_name("plan-alias.json");
        if alias.exists() {
            fs::remove_file(&alias).unwrap();
        }
        fs::hard_link(&original, &alias).unwrap();
        let original_snapshot = fixture.store.census_input_snapshot(1_200).unwrap();

        let mut document = raw_document(&fixture.store);
        document.registrations[0].identity.plan_path = alias.to_string_lossy().into_owned();
        fs::write(fixture.store.path(), serde_json::to_vec(&document).unwrap()).unwrap();
        let alias_snapshot = fixture.store.census_input_snapshot(1_200).unwrap();
        assert_eq!(original_snapshot.attestation, alias_snapshot.attestation);
    }

    #[test]
    fn conditional_census_write_rejects_mutation_and_duplicate_races() {
        let fixture = TempRegistry::new("conditional-race");
        fixture
            .store
            .initialize(vec![root("root-race")], 1_000)
            .unwrap();
        fixture
            .store
            .register(seed("planner-race"), 1_100, 20_000)
            .unwrap();
        let stale = fixture.store.census_input_snapshot(1_200).unwrap();
        fixture
            .store
            .heartbeat("planner-race", 1, 1_300, 20_000)
            .unwrap();
        assert!(matches!(
            fixture.store.record_census_if_unchanged(
                &stale.attestation,
                census(2, vec![("root-race", vec!["planner-race".into()])]),
                2_000,
            ),
            Err(RegistryError::Conflict(_))
        ));

        let current = fixture.store.census_input_snapshot(1_400).unwrap();
        let store = fixture.store.clone();
        let attestation = current.attestation.clone();
        let first = thread::spawn(move || {
            store.record_census_if_unchanged(
                &attestation,
                census(3, vec![("root-race", vec!["planner-race".into()])]),
                2_000,
            )
        });
        let second = fixture.store.record_census_if_unchanged(
            &current.attestation,
            census(3, vec![("root-race", vec!["planner-race".into()])]),
            2_000,
        );
        let first = first.join().unwrap();
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    }

    #[test]
    fn capability_expiry_while_waiting_for_registry_lock_prevents_acceptance() {
        let (fixture, snapshot) = initialized_snapshot("lock-expiry", "planner-lock-expiry");
        let before = fs::read(fixture.store.path()).unwrap();
        let lock = RegistryLock::acquire(fixture.store.path(), Duration::from_secs(1)).unwrap();
        let store = fixture.store.clone();
        let worker = thread::spawn(move || {
            store.record_census_if_unchanged_before(
                &snapshot.attestation,
                census(
                    snapshot.attestation.registry_generation,
                    vec![("root-lock-expiry", vec!["planner-lock-expiry".to_string()])],
                ),
                3_000,
                || 3_000,
            )
        });

        thread::sleep(Duration::from_millis(40));
        drop(lock);
        assert!(matches!(
            worker.join().expect("registry writer joins"),
            Err(RegistryError::CapabilityExpired)
        ));
        assert_eq!(fs::read(fixture.store.path()).unwrap(), before);
        assert!(raw_document(&fixture.store).census.is_none());
    }

    #[test]
    fn capability_expiry_after_temp_fsync_prevents_atomic_replace() {
        let (fixture, snapshot) = initialized_snapshot("replace-expiry", "planner-replace-expiry");
        let before = fs::read(fixture.store.path()).unwrap();
        let mut trusted_times = [2_100_u64, 2_200, 3_000].into_iter();
        let result = fixture.store.record_census_if_unchanged_before(
            &snapshot.attestation,
            census(
                snapshot.attestation.registry_generation,
                vec![(
                    "root-replace-expiry",
                    vec!["planner-replace-expiry".to_string()],
                )],
            ),
            3_000,
            || {
                trusted_times
                    .next()
                    .expect("all trusted clock gates sampled")
            },
        );

        assert!(matches!(result, Err(RegistryError::CapabilityExpired)));
        assert_eq!(fs::read(fixture.store.path()).unwrap(), before);
        assert!(raw_document(&fixture.store).census.is_none());
        let temporary_prefix = format!(
            ".{}.tmp-",
            fixture.store.path().file_name().unwrap().to_string_lossy()
        );
        assert!(fs::read_dir(&fixture.root)
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(&temporary_prefix)));
    }

    #[test]
    fn digest_encoding_is_domain_and_length_separated() {
        let digest = |domain: &'static [u8], fields: &[&str]| {
            let mut encoder = DigestEncoder::new(domain);
            for field in fields {
                encoder.text(field);
            }
            encoder.finish()
        };
        assert_ne!(
            digest(b"domain-a", &["ab", "c"]),
            digest(b"domain-a", &["a", "bc"])
        );
        assert_ne!(
            digest(b"domain-a", &["same"]),
            digest(b"domain-b", &["same"])
        );
        assert_eq!(
            digest(b"domain-a", &["same"]),
            digest(b"domain-a", &["same"])
        );
    }

    #[cfg(windows)]
    fn create_directory_alias(link: &Path, target: &Path) {
        let output = std::process::Command::new("cmd")
            .args(["/D", "/S", "/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("start mklink");
        assert!(output.status.success(), "mklink /J failed");
    }

    #[cfg(unix)]
    fn create_directory_alias(link: &Path, target: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }
}
