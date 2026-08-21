//! Machine-wide registry for Perfect Planner collision assessment.
//!
//! The registry is deliberately independent from Tauri commands. `for_app_data` fixes its
//! location beneath Tauri's per-user app-data directory, while every read-modify-write operation
//! is serialized by an operating-system-backed lock and published with atomic replacement.
//! Invalid state is reported as `UNKNOWN`; it is never interpreted as an empty registry.

use serde::{Deserialize, Serialize};
use serde_json::Value;
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
const MAX_TEXT_BYTES: usize = 4_096;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoveryRootCensus {
    pub root_id: String,
    pub reachable: bool,
    #[serde(default)]
    pub planner_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoveryCensus {
    pub registry_generation: u64,
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
    MissingCensus,
    CensusGenerationMismatch {
        registry_generation: u64,
        census_generation: u64,
    },
    StaleCensus(u64),
    FutureCensus(u64),
    MissingRootCensus(String),
    UnexpectedRootCensus(String),
    DuplicateRootCensus(String),
    UnreachableRoot {
        root_id: String,
        failure: Option<String>,
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
    pub fn record_census(
        &self,
        census: DiscoveryCensus,
        now_ms: u64,
    ) -> Result<DiscoveryCensus, RegistryError> {
        validate_census_shape(&census)?;
        self.mutate(now_ms, false, move |document| {
            if census.registry_generation != document.generation {
                return Err(RegistryError::Conflict(format!(
                    "census generation {} does not match registry generation {}",
                    census.registry_generation, document.generation
                )));
            }
            document.census = Some(census.clone());
            Ok(census)
        })
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
        let mut document = load_document_for_mutation(self.path.as_path())?;
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

fn load_document_for_mutation(path: &Path) -> Result<RegistryDocument, RegistryError> {
    let document = load_document(path).map_err(RegistryError::UnknownState)?;
    let issues = validate_document_static(&document);
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

    let mut planner_ids = BTreeSet::new();
    for registration in &document.registrations {
        if !planner_ids.insert(registration.identity.planner_id.clone()) {
            issues.push(RegistryIssue::DuplicatePlanner(
                registration.identity.planner_id.clone(),
            ));
        }
        if let Err(error) = validate_registration(registration) {
            issues.push(RegistryIssue::InvalidDocument(error.to_string()));
        }
    }

    let mut root_ids = BTreeSet::new();
    let mut root_paths = BTreeSet::new();
    for root in &document.configured_roots {
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
    for registration in &document.registrations {
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
    validate_sorted_unique_relative_paths("plan files", &seed.files)?;
    validate_sorted_unique_text("plan resources", &seed.resources)?;
    if seed.nodes.is_empty() {
        return Err(RegistryError::InvalidInput(
            "registration must contain at least one node manifest".into(),
        ));
    }
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
        validate_sorted_unique_relative_paths("node files", &node.files)?;
        validate_sorted_unique_text("node resources", &node.resources)?;
        last_node = Some(node.node_id.as_str());
    }
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
    let mut last_root = None::<&str>;
    for root in &census.roots {
        validate_id("census root id", &root.root_id)?;
        if last_root.is_some_and(|last| last >= root.root_id.as_str()) {
            return Err(RegistryError::InvalidInput(
                "census roots must be sorted and unique by root id".into(),
            ));
        }
        validate_sorted_unique_ids("census planner ids", &root.planner_ids)?;
        match (root.reachable, root.failure.as_deref()) {
            (true, None) => {}
            (false, Some(failure)) => validate_text("root census failure", failure)?,
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
    let mut last = None::<&str>;
    for value in values {
        validate_text(label, value)?;
        let normalized = value.replace('\\', "/");
        if normalized.starts_with('/')
            || normalized
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || normalized.as_bytes().get(1) == Some(&b':')
        {
            return Err(RegistryError::InvalidInput(format!(
                "{label} contains a non-normalized repository-relative path: {value}"
            )));
        }
        if last.is_some_and(|previous| previous >= value.as_str()) {
            return Err(RegistryError::InvalidInput(format!(
                "{label} must be sorted and unique"
            )));
        }
        last = Some(value.as_str());
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
    let mut bytes = serde_json::to_vec_pretty(document).map_err(|error| {
        RegistryError::Io(format!("cannot serialize registry document: {error}"))
    })?;
    bytes.push(b'\n');
    atomic_replace(path, &bytes).map_err(|error| {
        RegistryError::Io(format!(
            "cannot atomically persist registry {}: {error}",
            path.display()
        ))
    })
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no parent"))?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "registry target is not a regular file",
            ));
        }
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid target name"))?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{file_name}.tmp-{}-{sequence}",
        std::process::id()
    ));
    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        replace_file(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
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
        ConfiguredDiscoveryRoot {
            root_id: id.to_string(),
            canonical_path: std::env::temp_dir().join(id).to_string_lossy().into_owned(),
        }
    }

    fn seed(id: &str) -> PlannerRegistrationSeed {
        let worktree = std::env::temp_dir().join(format!("worktree-{id}"));
        PlannerRegistrationSeed {
            planner_id: id.to_string(),
            repository_id: format!("repo-{id}"),
            repository_root: std::env::temp_dir()
                .join(format!("repository-{id}"))
                .to_string_lossy()
                .into_owned(),
            worktree_root: worktree.to_string_lossy().into_owned(),
            branch: format!("feature/{id}"),
            plan_id: "PP-002".into(),
            plan_path: worktree
                .join(".claude/scratch/perfect-plan/plan.json")
                .to_string_lossy()
                .into_owned(),
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
                        .register(seed(&format!("planner-{index:02}")), 2_000 + index, 20_000)
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
                        .heartbeat(&format!("planner-{index:02}"), 1, 3_000 + index, 20_000)
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
                    replacement.resources = vec![format!("mutex:{id}"), "port:5235".into()];
                    store
                        .update(&id, 2, replacement, 4_000 + index, 20_000)
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        let removals = (8..16)
            .map(|index| {
                let store = fixture.store.clone();
                thread::spawn(move || {
                    let id = format!("planner-{index:02}");
                    store.unregister(&id, 2, 4_000 + index).unwrap()
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

        fixture
            .store
            .record_census(census(3, vec![("root-a", vec!["planner-a".into()])]), 2_000)
            .unwrap();
        let issues = fixture.store.inspect(2_100).issues().to_vec();
        assert!(issues.contains(&RegistryIssue::MissingRootCensus("root-b".into())));
        assert!(issues.contains(&RegistryIssue::UnaccountedPlanner("planner-b".into())));

        fixture
            .store
            .record_census(
                DiscoveryCensus {
                    registry_generation: 3,
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
                            failure: Some("access denied".into()),
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
                failure: Some("access denied".into()),
            }));

        fixture
            .store
            .record_census(
                census(
                    3,
                    vec![
                        ("root-a", vec!["planner-a".into()]),
                        ("root-b", vec!["planner-b".into()]),
                    ],
                ),
                2_400,
            )
            .unwrap();
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
}
