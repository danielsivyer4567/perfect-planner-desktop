use super::capability::{
    validate_run_id, CapabilityError, CapabilityStore, DiscoveryCancellation, DiscoveryScope,
    IssuedDiscoveryCapability,
};
use super::registry::{CensusInputSnapshot, DiscoveryCensus, PlannerRegistryStore, RegistryError};
use crate::supervisor::unix_ms;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const DISCOVERY_CAPABILITY_TTL_MS: u64 = 60_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IssueDiscoveryCapabilityRequest {
    pub run_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectCensusRequest {
    pub run_id: String,
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevokeDiscoveryCapabilityRequest {
    pub token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedDiscoveryCapabilityResponse {
    pub token: String,
    pub run_id: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}

impl From<IssuedDiscoveryCapability> for IssuedDiscoveryCapabilityResponse {
    fn from(capability: IssuedDiscoveryCapability) -> Self {
        Self {
            token: capability.token,
            run_id: capability.run_id,
            issued_at_ms: capability.issued_at_ms,
            expires_at_ms: capability.expires_at_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CensusUnknownCode {
    RegistryUnavailable,
    CapabilityRejected,
    CapabilityExpired,
    ClockRollback,
    CollectorUnavailable,
    CollectionTimeout,
    ParseFailed,
    MetadataLimitExceeded,
    IdentityChanged,
    ObservationTimeInvalid,
    CollectionFailed,
    RegistryDrift,
    PersistenceFailed,
    NativeWorkerUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CensusUnknown {
    pub status: &'static str,
    pub code: CensusUnknownCode,
}

impl CensusUnknown {
    fn new(code: CensusUnknownCode) -> Self {
        Self {
            status: "UNKNOWN",
            code,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectCensusResponse {
    pub status: &'static str,
    pub captured_at_ms: u64,
    pub expires_at_ms: u64,
    pub root_count: usize,
    pub observed_planner_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeDiscoveryCapabilityResponse {
    pub revoked: bool,
}

// B04's killable native collector will produce the non-Unavailable outcomes. They are part of the
// bounded B18 protocol now even though production deliberately stays unavailable until B04 lands.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CensusCollectionFailure {
    Unavailable,
    Timeout,
    Malformed,
    LimitExceeded,
    IdentityChanged,
    Failed,
}

/// The collector is supplied only by native Rust during Tauri setup. There is deliberately no
/// setter, IPC request field, development fallback or renderer-selected implementation. B18's
/// production implementation is unavailable; B04 must supply a killable child-process collector
/// that owns its timeout, kill and wait boundary before returning from this synchronous call.
pub(crate) trait MetadataCensusCollector: Send + Sync + 'static {
    fn collect(
        &self,
        input: CensusInputSnapshot,
        capability_deadline_ms: u64,
        cancellation: DiscoveryCancellation,
    ) -> Result<DiscoveryCensus, CensusCollectionFailure>;
}

trait CensusClock: Send + Sync + 'static {
    fn now_ms(&self) -> u64;
}

struct SystemCensusClock;

impl CensusClock for SystemCensusClock {
    fn now_ms(&self) -> u64 {
        unix_ms()
    }
}

struct UnavailableMetadataCollector;

impl MetadataCensusCollector for UnavailableMetadataCollector {
    fn collect(
        &self,
        _input: CensusInputSnapshot,
        _capability_deadline_ms: u64,
        _cancellation: DiscoveryCancellation,
    ) -> Result<DiscoveryCensus, CensusCollectionFailure> {
        Err(CensusCollectionFailure::Unavailable)
    }
}

pub struct CensusCommandState {
    registry: PlannerRegistryStore,
    collector: Arc<dyn MetadataCensusCollector>,
    clock: Arc<dyn CensusClock>,
}

impl CensusCommandState {
    /// Fail closed until B04 supplies the audited native metadata collector during app setup.
    pub(crate) fn unavailable(registry: PlannerRegistryStore) -> Self {
        Self::with_native_collector(registry, Arc::new(UnavailableMetadataCollector))
    }

    pub(crate) fn with_native_collector(
        registry: PlannerRegistryStore,
        collector: Arc<dyn MetadataCensusCollector>,
    ) -> Self {
        Self {
            registry,
            collector,
            clock: Arc::new(SystemCensusClock),
        }
    }

    #[cfg(test)]
    fn with_test_components(
        registry: PlannerRegistryStore,
        collector: Arc<dyn MetadataCensusCollector>,
        clock: Arc<dyn CensusClock>,
    ) -> Self {
        Self {
            registry,
            collector,
            clock,
        }
    }
}

#[tauri::command]
pub fn collision_assessor_issue_discovery_capability(
    capabilities: tauri::State<'_, CapabilityStore>,
    census: tauri::State<'_, CensusCommandState>,
    request: IssueDiscoveryCapabilityRequest,
) -> Result<IssuedDiscoveryCapabilityResponse, CensusUnknown> {
    issue_discovery_capability(&capabilities, &census, request)
}

fn issue_discovery_capability(
    capabilities: &CapabilityStore,
    census: &CensusCommandState,
    request: IssueDiscoveryCapabilityRequest,
) -> Result<IssuedDiscoveryCapabilityResponse, CensusUnknown> {
    validate_run_id(&request.run_id).map_err(capability_unknown)?;
    let now_ms = census.clock.now_ms();
    let input = census
        .registry
        .census_input_snapshot(now_ms)
        .map_err(|_| CensusUnknown::new(CensusUnknownCode::RegistryUnavailable))?;
    let scope = native_scope(request.run_id, &input);
    capabilities
        .issue(scope, now_ms, DISCOVERY_CAPABILITY_TTL_MS)
        .map(IssuedDiscoveryCapabilityResponse::from)
        .map_err(capability_unknown)
}

#[tauri::command]
pub fn collision_assessor_collect_census(
    capabilities: tauri::State<'_, CapabilityStore>,
    census: tauri::State<'_, CensusCommandState>,
    request: CollectCensusRequest,
) -> Result<CollectCensusResponse, CensusUnknown> {
    collect_census(&capabilities, &census, request)
}

fn collect_census(
    capabilities: &CapabilityStore,
    state: &CensusCommandState,
    request: CollectCensusRequest,
) -> Result<CollectCensusResponse, CensusUnknown> {
    let started_at_ms = state.clock.now_ms();
    let permit = capabilities
        .begin_discovery_for_run(&request.token, &request.run_id, started_at_ms)
        .map_err(capability_unknown)?;
    let input = match state.registry.census_input_snapshot(started_at_ms) {
        Ok(input) => input,
        Err(_) => {
            revoke_terminal(capabilities, &request.token);
            return Err(CensusUnknown::new(CensusUnknownCode::RegistryUnavailable));
        }
    };
    let initial_attestation = input.attestation.clone();
    let initial_scope = native_scope(request.run_id.clone(), &input);
    if permit.registry_generation != initial_scope.registry_generation
        || permit.repository_census_hash != initial_scope.repository_census_hash
    {
        revoke_terminal(capabilities, &request.token);
        return Err(CensusUnknown::new(CensusUnknownCode::RegistryDrift));
    }

    let mut observed =
        match state
            .collector
            .collect(input, permit.expires_at_ms, permit.cancellation())
        {
            Ok(census) => census,
            Err(failure) => {
                revoke_terminal(capabilities, &request.token);
                return Err(collection_unknown(failure));
            }
        };
    observed.input_digest = initial_attestation.digest_hex();

    let completed_at_ms = state.clock.now_ms();
    if completed_at_ms < started_at_ms {
        revoke_terminal(capabilities, &request.token);
        return Err(CensusUnknown::new(CensusUnknownCode::ClockRollback));
    }
    if observed.captured_at_ms < started_at_ms || observed.captured_at_ms > completed_at_ms {
        revoke_terminal(capabilities, &request.token);
        return Err(CensusUnknown::new(
            CensusUnknownCode::ObservationTimeInvalid,
        ));
    }
    let current_input = match state.registry.census_input_snapshot(completed_at_ms) {
        Ok(input) => input,
        Err(_) => {
            revoke_terminal(capabilities, &request.token);
            return Err(CensusUnknown::new(CensusUnknownCode::RegistryUnavailable));
        }
    };
    let current_scope = native_scope(request.run_id, &current_input);
    let mut terminal_code = None;
    let recorded = capabilities.consume_with_snapshot_at(
        &request.token,
        &current_scope,
        || state.clock.now_ms(),
        |permit, accepted_at_ms| {
            if accepted_at_ms < completed_at_ms {
                terminal_code = Some(CensusUnknownCode::ClockRollback);
                return Err("trusted clock moved backwards before persistence".to_string());
            }
            state
                .registry
                .record_census_if_unchanged_before(
                    &initial_attestation,
                    observed,
                    permit.expires_at_ms,
                    || state.clock.now_ms(),
                )
                .map_err(|error| {
                    terminal_code = Some(registry_persistence_code(&error));
                    "conditional census persistence rejected".to_string()
                })
        },
    );
    let recorded = match recorded {
        Ok(recorded) => recorded,
        Err(CapabilityError::SnapshotCreationFailed) => {
            return Err(CensusUnknown::new(
                terminal_code.unwrap_or(CensusUnknownCode::PersistenceFailed),
            ));
        }
        Err(error) => return Err(capability_unknown(error)),
    };

    let observed_planner_count = recorded
        .roots
        .iter()
        .try_fold(0_usize, |total, root| {
            total.checked_add(root.planner_ids.len())
        })
        .ok_or_else(|| CensusUnknown::new(CensusUnknownCode::MetadataLimitExceeded))?;

    Ok(CollectCensusResponse {
        status: "RECORDED",
        captured_at_ms: recorded.captured_at_ms,
        expires_at_ms: recorded.expires_at_ms,
        root_count: recorded.roots.len(),
        observed_planner_count,
    })
}

#[tauri::command]
pub fn collision_assessor_revoke_discovery_capability(
    state: tauri::State<'_, CapabilityStore>,
    request: RevokeDiscoveryCapabilityRequest,
) -> Result<RevokeDiscoveryCapabilityResponse, CensusUnknown> {
    state.revoke(&request.token).map_err(capability_unknown)?;
    Ok(RevokeDiscoveryCapabilityResponse { revoked: true })
}

fn native_scope(run_id: String, input: &CensusInputSnapshot) -> DiscoveryScope {
    DiscoveryScope {
        run_id,
        registry_generation: input.attestation.registry_generation,
        repository_census_hash: input.attestation.digest_hex(),
    }
}

fn revoke_terminal(capabilities: &CapabilityStore, token: &str) {
    let _ = capabilities.revoke(token);
}

fn collection_unknown(failure: CensusCollectionFailure) -> CensusUnknown {
    let code = match failure {
        CensusCollectionFailure::Unavailable => CensusUnknownCode::CollectorUnavailable,
        CensusCollectionFailure::Timeout => CensusUnknownCode::CollectionTimeout,
        CensusCollectionFailure::Malformed => CensusUnknownCode::ParseFailed,
        CensusCollectionFailure::LimitExceeded => CensusUnknownCode::MetadataLimitExceeded,
        CensusCollectionFailure::IdentityChanged => CensusUnknownCode::IdentityChanged,
        CensusCollectionFailure::Failed => CensusUnknownCode::CollectionFailed,
    };
    CensusUnknown::new(code)
}

fn capability_unknown(error: CapabilityError) -> CensusUnknown {
    let code = match error {
        CapabilityError::Expired => CensusUnknownCode::CapabilityExpired,
        CapabilityError::ClockRollback => CensusUnknownCode::ClockRollback,
        CapabilityError::BindingMismatch => CensusUnknownCode::RegistryDrift,
        CapabilityError::SnapshotCreationFailed => CensusUnknownCode::PersistenceFailed,
        CapabilityError::StoreUnavailable | CapabilityError::EntropyUnavailable => {
            CensusUnknownCode::NativeWorkerUnavailable
        }
        CapabilityError::InvalidRunId
        | CapabilityError::InvalidRegistryGeneration
        | CapabilityError::InvalidCensusHash
        | CapabilityError::InvalidTtl
        | CapabilityError::ExpiryOverflow
        | CapabilityError::EntropyCollision
        | CapabilityError::CapacityExceeded
        | CapabilityError::InvalidToken
        | CapabilityError::UnknownToken
        | CapabilityError::AlreadyUsed
        | CapabilityError::DiscoveryNotStarted => CensusUnknownCode::CapabilityRejected,
    };
    CensusUnknown::new(code)
}

fn registry_persistence_code(error: &RegistryError) -> CensusUnknownCode {
    match error {
        RegistryError::CapabilityExpired => CensusUnknownCode::CapabilityExpired,
        RegistryError::ClockRollback => CensusUnknownCode::ClockRollback,
        RegistryError::Conflict(_) | RegistryError::UnknownState(_) => {
            CensusUnknownCode::RegistryDrift
        }
        RegistryError::InvalidInput(_) | RegistryError::LockTimeout(_) | RegistryError::Io(_) => {
            CensusUnknownCode::PersistenceFailed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision_assessor::registry::{
        ConfiguredDiscoveryRoot, DiscoveryFailureCode, DiscoveryRootCensus, PlannerNodeManifest,
        PlannerRegistrationSeed, RegistryIssue, RegistryRead,
    };
    use serde_json::{json, Value};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    struct Fixture {
        root: PathBuf,
        registry: PlannerRegistryStore,
        discovery_root: PathBuf,
        plan_path: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("pp-b18-{label}-{}-{sequence}", std::process::id()));
            let discovery_root = root.join("discovery");
            let repository_root = root.join("repository");
            let worktree_root = root.join("worktree");
            let plan_path = worktree_root.join(".claude/scratch/perfect-plan/plan.json");
            fs::create_dir_all(&discovery_root).unwrap();
            fs::create_dir_all(&repository_root).unwrap();
            fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
            fs::write(&plan_path, b"{}\n").unwrap();

            let registry = PlannerRegistryStore::new(root.join("registry-v1.json")).unwrap();
            registry
                .initialize(
                    vec![ConfiguredDiscoveryRoot {
                        root_id: "root-a".into(),
                        canonical_path: discovery_root.to_string_lossy().into_owned(),
                    }],
                    1_000,
                )
                .unwrap();
            registry
                .register(
                    PlannerRegistrationSeed {
                        planner_id: "planner-a".into(),
                        repository_id: "repo-a".into(),
                        repository_root: repository_root.to_string_lossy().into_owned(),
                        worktree_root: worktree_root.to_string_lossy().into_owned(),
                        branch: "feature/a".into(),
                        plan_id: "PP-002".into(),
                        plan_path: plan_path.to_string_lossy().into_owned(),
                        files: vec!["src/main.rs".into()],
                        resources: vec!["mutex:planner-a".into()],
                        nodes: vec![PlannerNodeManifest {
                            node_id: "B18".into(),
                            files: vec!["src/main.rs".into()],
                            resources: vec!["mutex:planner-a".into()],
                        }],
                    },
                    1_100,
                    100_000,
                )
                .unwrap();
            Self {
                root,
                registry,
                discovery_root,
                plan_path,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    struct TestClock(AtomicU64);

    impl TestClock {
        fn new(now_ms: u64) -> Self {
            Self(AtomicU64::new(now_ms))
        }

        fn set(&self, now_ms: u64) {
            self.0.store(now_ms, Ordering::Release);
        }
    }

    impl CensusClock for TestClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::Acquire)
        }
    }

    fn observed_census(input: &CensusInputSnapshot, now_ms: u64) -> DiscoveryCensus {
        let planner_ids = input
            .registrations
            .iter()
            .map(|registration| registration.registration.identity.planner_id.clone())
            .collect::<Vec<_>>();
        DiscoveryCensus {
            registry_generation: input.attestation.registry_generation,
            input_digest: "0".repeat(64),
            captured_at_ms: now_ms,
            expires_at_ms: now_ms + 5_000,
            roots: input
                .configured_roots
                .iter()
                .enumerate()
                .map(|(index, root)| DiscoveryRootCensus {
                    root_id: root.root_id.clone(),
                    reachable: true,
                    planner_ids: if index == 0 {
                        planner_ids.clone()
                    } else {
                        Vec::new()
                    },
                    failure: None,
                })
                .collect(),
        }
    }

    struct SuccessCollector {
        calls: AtomicUsize,
        clock: Arc<TestClock>,
    }

    impl MetadataCensusCollector for SuccessCollector {
        fn collect(
            &self,
            input: CensusInputSnapshot,
            deadline_at_ms: u64,
            cancellation: DiscoveryCancellation,
        ) -> Result<DiscoveryCensus, CensusCollectionFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if cancellation.is_cancelled() || self.clock.now_ms() >= deadline_at_ms {
                return Err(CensusCollectionFailure::Timeout);
            }
            Ok(observed_census(&input, self.clock.now_ms()))
        }
    }

    struct ErrorCollector(CensusCollectionFailure);

    impl MetadataCensusCollector for ErrorCollector {
        fn collect(
            &self,
            _input: CensusInputSnapshot,
            _deadline_at_ms: u64,
            _cancellation: DiscoveryCancellation,
        ) -> Result<DiscoveryCensus, CensusCollectionFailure> {
            Err(self.0)
        }
    }

    struct InvalidTimeOutputCollector(u64);

    impl MetadataCensusCollector for InvalidTimeOutputCollector {
        fn collect(
            &self,
            input: CensusInputSnapshot,
            _deadline_at_ms: u64,
            _cancellation: DiscoveryCancellation,
        ) -> Result<DiscoveryCensus, CensusCollectionFailure> {
            Ok(observed_census(&input, self.0))
        }
    }

    fn state(
        fixture: &Fixture,
        collector: Arc<dyn MetadataCensusCollector>,
        clock: Arc<TestClock>,
    ) -> CensusCommandState {
        CensusCommandState::with_test_components(fixture.registry.clone(), collector, clock)
    }

    fn issue(
        capabilities: &CapabilityStore,
        state: &CensusCommandState,
    ) -> IssuedDiscoveryCapabilityResponse {
        issue_discovery_capability(
            capabilities,
            state,
            IssueDiscoveryCapabilityRequest {
                run_id: "run-PP-002-B18".into(),
            },
        )
        .unwrap()
    }

    fn request(token: String) -> CollectCensusRequest {
        CollectCensusRequest {
            run_id: "run-PP-002-B18".into(),
            token,
        }
    }

    #[test]
    fn ipc_requests_are_exact_and_renderer_cannot_assert_native_scope() {
        assert!(
            serde_json::from_value::<IssueDiscoveryCapabilityRequest>(json!({
                "runId": "run-1"
            }))
            .is_ok()
        );
        assert!(serde_json::from_value::<CollectCensusRequest>(json!({
            "runId": "run-1",
            "token": "a".repeat(64)
        }))
        .is_ok());
        for forbidden in [
            "registryGeneration",
            "repositoryCensusHash",
            "ttlMs",
            "path",
            "root",
            "url",
            "host",
            "port",
            "endpoint",
            "shell",
            "process",
        ] {
            let mut collect = json!({ "runId": "run-1", "token": "a".repeat(64) });
            collect[forbidden] = Value::from("forbidden");
            assert!(serde_json::from_value::<CollectCensusRequest>(collect).is_err());

            let mut issue = json!({ "runId": "run-1" });
            issue[forbidden] = Value::from("forbidden");
            assert!(serde_json::from_value::<IssueDiscoveryCapabilityRequest>(issue).is_err());
        }
    }

    #[test]
    fn successful_collection_persists_attestation_consumes_once_and_is_path_free() {
        let fixture = Fixture::new("success");
        let clock = Arc::new(TestClock::new(2_000));
        let collector = Arc::new(SuccessCollector {
            calls: AtomicUsize::new(0),
            clock: Arc::clone(&clock),
        });
        let state = state(&fixture, collector.clone(), clock);
        let capabilities = CapabilityStore::default();
        let issued = issue(&capabilities, &state);
        let response =
            collect_census(&capabilities, &state, request(issued.token.clone())).unwrap();
        assert_eq!(response.status, "RECORDED");
        assert_eq!(response.root_count, 1);
        assert_eq!(response.observed_planner_count, 1);
        assert_eq!(collector.calls.load(Ordering::SeqCst), 1);
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains(&issued.token));
        assert!(!serialized.contains(&fixture.root.to_string_lossy().to_string()));

        let RegistryRead::Complete(document) = fixture.registry.inspect(2_100) else {
            panic!("recorded census must inspect complete")
        };
        let census = document.census.unwrap();
        assert_ne!(census.input_digest, "0".repeat(64));
        assert_eq!(census.input_digest.len(), 64);

        let replay = collect_census(&capabilities, &state, request(issued.token)).unwrap_err();
        assert_eq!(replay.code, CensusUnknownCode::CapabilityRejected);
        assert_eq!(collector.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn invalid_or_unknown_token_causes_zero_registry_or_collector_reads() {
        let missing = std::env::temp_dir().join(format!(
            "pp-b18-missing-{}-{}",
            std::process::id(),
            FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let registry = PlannerRegistryStore::new(missing.join("registry-v1.json")).unwrap();
        let clock = Arc::new(TestClock::new(2_000));
        let collector = Arc::new(SuccessCollector {
            calls: AtomicUsize::new(0),
            clock: Arc::clone(&clock),
        });
        let state = CensusCommandState::with_test_components(registry, collector.clone(), clock);
        let capabilities = CapabilityStore::default();
        for token in ["bad".to_string(), "a".repeat(64)] {
            let error = collect_census(&capabilities, &state, request(token)).unwrap_err();
            assert_eq!(error.code, CensusUnknownCode::CapabilityRejected);
        }
        assert_eq!(collector.calls.load(Ordering::SeqCst), 0);
        assert!(!missing.exists());
    }

    #[test]
    fn invalid_issue_run_id_is_rejected_before_any_registry_read() {
        let missing = std::env::temp_dir().join(format!(
            "pp-b18-invalid-issue-{}-{}",
            std::process::id(),
            FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let registry = PlannerRegistryStore::new(missing.join("registry-v1.json")).unwrap();
        let clock = Arc::new(TestClock::new(2_000));
        let collector = Arc::new(SuccessCollector {
            calls: AtomicUsize::new(0),
            clock: Arc::clone(&clock),
        });
        let state = CensusCommandState::with_test_components(registry, collector.clone(), clock);
        let capabilities = CapabilityStore::default();
        for invalid in [String::new(), "../escape".into(), "x".repeat(129)] {
            let error = issue_discovery_capability(
                &capabilities,
                &state,
                IssueDiscoveryCapabilityRequest { run_id: invalid },
            )
            .err()
            .expect("invalid run ID must be rejected");
            assert_eq!(error.code, CensusUnknownCode::CapabilityRejected);
        }
        assert_eq!(collector.calls.load(Ordering::SeqCst), 0);
        assert!(!missing.exists());
    }

    #[test]
    fn collector_output_outside_this_discovery_window_is_rejected_and_revoked() {
        for (name, observed_at_ms) in [("stale-output", 1_999), ("future-output", 2_001)] {
            let fixture = Fixture::new(name);
            let clock = Arc::new(TestClock::new(2_000));
            let state = state(
                &fixture,
                Arc::new(InvalidTimeOutputCollector(observed_at_ms)),
                clock,
            );
            let capabilities = CapabilityStore::default();
            let issued = issue(&capabilities, &state);

            let error =
                collect_census(&capabilities, &state, request(issued.token.clone())).unwrap_err();
            assert_eq!(error.code, CensusUnknownCode::ObservationTimeInvalid);
            assert!(matches!(
                fixture.registry.inspect(2_100),
                RegistryRead::Unknown(unknown)
                    if unknown.issues.contains(&RegistryIssue::MissingCensus)
            ));
            assert_eq!(
                collect_census(&capabilities, &state, request(issued.token))
                    .unwrap_err()
                    .code,
                CensusUnknownCode::CapabilityRejected
            );
        }
    }

    #[test]
    fn every_collector_failure_revokes_and_returns_a_bounded_unknown_code() {
        let cases = [
            (
                CensusCollectionFailure::Unavailable,
                CensusUnknownCode::CollectorUnavailable,
            ),
            (
                CensusCollectionFailure::Timeout,
                CensusUnknownCode::CollectionTimeout,
            ),
            (
                CensusCollectionFailure::Malformed,
                CensusUnknownCode::ParseFailed,
            ),
            (
                CensusCollectionFailure::LimitExceeded,
                CensusUnknownCode::MetadataLimitExceeded,
            ),
            (
                CensusCollectionFailure::IdentityChanged,
                CensusUnknownCode::IdentityChanged,
            ),
            (
                CensusCollectionFailure::Failed,
                CensusUnknownCode::CollectionFailed,
            ),
        ];
        for (index, (failure, expected)) in cases.into_iter().enumerate() {
            let fixture = Fixture::new(&format!("failure-{index}"));
            let clock = Arc::new(TestClock::new(2_000));
            let state = state(&fixture, Arc::new(ErrorCollector(failure)), clock);
            let capabilities = CapabilityStore::default();
            let issued = issue(&capabilities, &state);
            let error =
                collect_census(&capabilities, &state, request(issued.token.clone())).unwrap_err();
            assert_eq!(error.code, expected);
            let serialized = serde_json::to_string(&error).unwrap();
            assert_eq!(
                serialized,
                format!(
                    "{{\"status\":\"UNKNOWN\",\"code\":\"{}\"}}",
                    serde_json::to_value(expected).unwrap().as_str().unwrap()
                )
            );
            assert!(!serialized.contains(&issued.token));
            assert_eq!(
                collect_census(&capabilities, &state, request(issued.token))
                    .unwrap_err()
                    .code,
                CensusUnknownCode::CapabilityRejected
            );
        }
    }

    #[test]
    fn concurrent_replay_runs_exactly_one_native_collection() {
        let fixture = Fixture::new("concurrent");
        let clock = Arc::new(TestClock::new(2_000));
        let collector = Arc::new(SuccessCollector {
            calls: AtomicUsize::new(0),
            clock: Arc::clone(&clock),
        });
        let state = Arc::new(state(&fixture, collector.clone(), clock));
        let capabilities = Arc::new(CapabilityStore::default());
        let issued = issue(&capabilities, &state);
        let handles = (0..16)
            .map(|_| {
                let state = Arc::clone(&state);
                let capabilities = Arc::clone(&capabilities);
                let request = request(issued.token.clone());
                thread::spawn(move || collect_census(&capabilities, &state, request))
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(collector.calls.load(Ordering::SeqCst), 1);
    }

    struct RevocationCollector {
        entered: Arc<Barrier>,
        cancellation_seen: Arc<AtomicUsize>,
    }

    impl MetadataCensusCollector for RevocationCollector {
        fn collect(
            &self,
            _input: CensusInputSnapshot,
            _deadline_at_ms: u64,
            cancellation: DiscoveryCancellation,
        ) -> Result<DiscoveryCensus, CensusCollectionFailure> {
            self.entered.wait();
            for _ in 0..1_000 {
                if cancellation.is_cancelled() {
                    self.cancellation_seen.store(1, Ordering::Release);
                    return Err(CensusCollectionFailure::Failed);
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(CensusCollectionFailure::Timeout)
        }
    }

    #[test]
    fn explicit_revoke_during_collection_cancels_native_boundary_and_discards_result() {
        let fixture = Fixture::new("revoke");
        let clock = Arc::new(TestClock::new(2_000));
        let entered = Arc::new(Barrier::new(2));
        let cancellation_seen = Arc::new(AtomicUsize::new(0));
        let collector = Arc::new(RevocationCollector {
            entered: Arc::clone(&entered),
            cancellation_seen: Arc::clone(&cancellation_seen),
        });
        let state = Arc::new(state(&fixture, collector, clock));
        let capabilities = Arc::new(CapabilityStore::default());
        let issued = issue(&capabilities, &state);
        let thread_state = Arc::clone(&state);
        let thread_capabilities = Arc::clone(&capabilities);
        let token = issued.token.clone();
        let handle = thread::spawn(move || {
            collect_census(&thread_capabilities, &thread_state, request(token))
        });
        entered.wait();
        capabilities.revoke(&issued.token).unwrap();
        let result = handle.join().unwrap().unwrap_err();
        assert_eq!(result.code, CensusUnknownCode::CollectionFailed);
        assert_eq!(cancellation_seen.load(Ordering::Acquire), 1);
        assert!(fixture
            .registry
            .inspect(2_100)
            .issues()
            .contains(&RegistryIssue::MissingCensus));
        assert_eq!(
            collect_census(&capabilities, &state, request(issued.token))
                .unwrap_err()
                .code,
            CensusUnknownCode::CapabilityRejected
        );
    }

    struct ExpiringCollector {
        clock: Arc<TestClock>,
    }

    impl MetadataCensusCollector for ExpiringCollector {
        fn collect(
            &self,
            input: CensusInputSnapshot,
            deadline_at_ms: u64,
            _cancellation: DiscoveryCancellation,
        ) -> Result<DiscoveryCensus, CensusCollectionFailure> {
            let census = observed_census(&input, self.clock.now_ms());
            self.clock.set(deadline_at_ms);
            Ok(census)
        }
    }

    #[test]
    fn expiry_during_collection_is_rechecked_and_never_persisted() {
        let fixture = Fixture::new("expiry");
        let clock = Arc::new(TestClock::new(2_000));
        let state = state(
            &fixture,
            Arc::new(ExpiringCollector {
                clock: Arc::clone(&clock),
            }),
            clock,
        );
        let capabilities = CapabilityStore::default();
        let issued = issue(&capabilities, &state);
        let error =
            collect_census(&capabilities, &state, request(issued.token.clone())).unwrap_err();
        assert_eq!(error.code, CensusUnknownCode::CapabilityExpired);
        assert!(fixture
            .registry
            .inspect(issued.expires_at_ms)
            .issues()
            .contains(&RegistryIssue::MissingCensus));
        assert_eq!(
            collect_census(&capabilities, &state, request(issued.token))
                .unwrap_err()
                .code,
            CensusUnknownCode::CapabilityRejected
        );
    }

    struct RootSwapCollector {
        root: PathBuf,
        clock: Arc<TestClock>,
    }

    impl MetadataCensusCollector for RootSwapCollector {
        fn collect(
            &self,
            input: CensusInputSnapshot,
            _deadline_at_ms: u64,
            _cancellation: DiscoveryCancellation,
        ) -> Result<DiscoveryCensus, CensusCollectionFailure> {
            let census = observed_census(&input, self.clock.now_ms());
            fs::remove_dir(&self.root).unwrap();
            fs::create_dir(&self.root).unwrap();
            Ok(census)
        }
    }

    #[test]
    fn same_generation_native_identity_drift_revokes_before_persistence() {
        let fixture = Fixture::new("drift");
        let clock = Arc::new(TestClock::new(2_000));
        let state = state(
            &fixture,
            Arc::new(RootSwapCollector {
                root: fixture.discovery_root.clone(),
                clock: Arc::clone(&clock),
            }),
            clock,
        );
        let capabilities = CapabilityStore::default();
        let issued = issue(&capabilities, &state);
        let error =
            collect_census(&capabilities, &state, request(issued.token.clone())).unwrap_err();
        assert_eq!(error.code, CensusUnknownCode::RegistryDrift);
        assert!(fixture
            .registry
            .inspect(2_100)
            .issues()
            .contains(&RegistryIssue::MissingCensus));
    }

    struct InvalidCensusCollector {
        clock: Arc<TestClock>,
    }

    impl MetadataCensusCollector for InvalidCensusCollector {
        fn collect(
            &self,
            input: CensusInputSnapshot,
            _deadline_at_ms: u64,
            _cancellation: DiscoveryCancellation,
        ) -> Result<DiscoveryCensus, CensusCollectionFailure> {
            let mut census = observed_census(&input, self.clock.now_ms());
            census.roots.clear();
            Ok(census)
        }
    }

    #[test]
    fn conditional_persistence_failure_revokes_without_leaking_detail() {
        let fixture = Fixture::new("persist-failure");
        let clock = Arc::new(TestClock::new(2_000));
        let state = state(
            &fixture,
            Arc::new(InvalidCensusCollector {
                clock: Arc::clone(&clock),
            }),
            clock,
        );
        let capabilities = CapabilityStore::default();
        let issued = issue(&capabilities, &state);
        let error =
            collect_census(&capabilities, &state, request(issued.token.clone())).unwrap_err();
        assert_eq!(error.code, CensusUnknownCode::PersistenceFailed);
        let serialized = serde_json::to_string(&error).unwrap();
        assert!(!serialized.contains(&fixture.root.to_string_lossy().to_string()));
        assert!(!serialized.contains(&issued.token));
    }

    #[test]
    fn collision_permission_and_generated_manifest_expose_exactly_three_named_commands() {
        fn allow_commands(document: &str) -> Vec<String> {
            let marker = "commands.allow = [";
            let start = document.find(marker).unwrap() + marker.len();
            let end = document[start..].find(']').unwrap() + start;
            document[start..end]
                .split(',')
                .map(|value| value.trim().trim_matches('"'))
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect()
        }

        let permission = include_str!("../../permissions/collision-assessor.toml");
        assert_eq!(
            allow_commands(permission),
            [
                "collision_assessor_issue_discovery_capability",
                "collision_assessor_collect_census",
                "collision_assessor_revoke_discovery_capability",
            ]
        );
        let generated =
            include_str!("../../permissions/autogenerated/collision_assessor_collect_census.toml");
        assert_eq!(
            allow_commands(generated),
            ["collision_assessor_collect_census"]
        );
        let capability: Value =
            serde_json::from_str(include_str!("../../capabilities/main-read-only.json")).unwrap();
        assert!(capability["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry == "allow-collision-assessor"));
    }

    #[test]
    fn legacy_census_without_input_digest_is_rejected_on_restart() {
        let fixture = Fixture::new("legacy");
        let clock = Arc::new(TestClock::new(2_000));
        let collector = Arc::new(SuccessCollector {
            calls: AtomicUsize::new(0),
            clock: Arc::clone(&clock),
        });
        let state = state(&fixture, collector, clock);
        let capabilities = CapabilityStore::default();
        let issued = issue(&capabilities, &state);
        collect_census(&capabilities, &state, request(issued.token)).unwrap();

        let mut value: Value =
            serde_json::from_slice(&fs::read(fixture.registry.path()).unwrap()).unwrap();
        value["census"]
            .as_object_mut()
            .unwrap()
            .remove("inputDigest");
        fs::write(
            fixture.registry.path(),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
        let reopened = PlannerRegistryStore::new(fixture.registry.path().to_path_buf()).unwrap();
        assert!(matches!(
            reopened.inspect(2_100),
            RegistryRead::Unknown(unknown)
                if unknown.issues.iter().any(|issue| matches!(issue, RegistryIssue::Malformed(_)))
        ));
    }

    #[test]
    fn same_generation_plan_replacement_invalidates_persisted_census_after_restart() {
        let fixture = Fixture::new("restart-drift");
        let clock = Arc::new(TestClock::new(2_000));
        let collector = Arc::new(SuccessCollector {
            calls: AtomicUsize::new(0),
            clock: Arc::clone(&clock),
        });
        let state = state(&fixture, collector, clock);
        let capabilities = CapabilityStore::default();
        let issued = issue(&capabilities, &state);
        collect_census(&capabilities, &state, request(issued.token)).unwrap();

        fs::remove_file(&fixture.plan_path).unwrap();
        fs::write(&fixture.plan_path, b"{\"replacement\":true}\n").unwrap();
        let reopened = PlannerRegistryStore::new(fixture.registry.path().to_path_buf()).unwrap();
        assert!(reopened
            .inspect(2_100)
            .issues()
            .contains(&RegistryIssue::CensusInputDigestMismatch));
    }

    #[test]
    fn failure_codes_do_not_serialize_native_details() {
        let _typed_failure = DiscoveryFailureCode::AccessDenied;
        for code in [
            CensusUnknownCode::RegistryUnavailable,
            CensusUnknownCode::CapabilityRejected,
            CensusUnknownCode::CapabilityExpired,
            CensusUnknownCode::ClockRollback,
            CensusUnknownCode::CollectorUnavailable,
            CensusUnknownCode::CollectionTimeout,
            CensusUnknownCode::ParseFailed,
            CensusUnknownCode::MetadataLimitExceeded,
            CensusUnknownCode::IdentityChanged,
            CensusUnknownCode::ObservationTimeInvalid,
            CensusUnknownCode::CollectionFailed,
            CensusUnknownCode::RegistryDrift,
            CensusUnknownCode::PersistenceFailed,
            CensusUnknownCode::NativeWorkerUnavailable,
        ] {
            let serialized = serde_json::to_string(&CensusUnknown::new(code)).unwrap();
            assert!(serialized.starts_with("{\"status\":\"UNKNOWN\",\"code\":"));
            assert!(!serialized.contains(':') || !serialized.contains("\\\\"));
        }
    }
}
