use super::authority_projection::{
    AuthorityBinding, AuthorityPublicationReceipt, CensusClearReceipt, CensusVerdict, ClaimRequest,
    PreclaimReservation,
};
use super::authority_runtime::SchedulerAuthorityRuntime;
use super::delivery::{deliver_run, DeliveryOutcome, DeliveryRequest};
use super::event_bus::{EventBus, EventType, RunEvent};
use super::evidence::{capture_artifact, EvidenceArtifact, EvidenceKind, VerificationResult};
use super::preclaim_store::{PreclaimRecord, PreclaimStore};
use super::preflight::{
    DenyProcessAdapter, PortBinding, PreflightDisposition, PreflightEngine, PreflightReport,
    PreflightRequest, ResourceSnapshot, SystemProbe,
};
use super::reconcile::{reconcile, ReconciliationInput, ReconciliationResult};
use super::release::{evaluate_release, ReleaseGateInput, ReleaseGateResult};
use super::run_scope::{
    declared_required_ports, AllowedFileManifest, CreateRunScope, HotResumeState, RunAuditEvent,
    RunScope,
};
use super::scheduler::{
    AdmissionGitBaseline, NodeCompletion, NodeLease, NodeStatus, PublicNodeLease,
    PublicSchedulerState, ReapAction, ScheduledNode, SchedulerStore,
};
#[cfg(test)]
use super::worker::validate_manifest;
use super::worker::{validate_submission, WorkerGateResult, WorkerManifest, WorkerSubmission};
use crate::collision_assessor::authority::ReservationBinding;
use crate::collision_assessor::registry::{
    PlannerNodeManifest, PlannerRegistrationSeed, PlannerRegistryStore,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SCHEDULER_FILE: &str = "scheduler.json";
const PREFLIGHT_RESULT_FILE: &str = "preflight.json";
const RUN_APPROVAL_FILE: &str = "run-approval.json";
const RECONCILIATION_RESULT_FILE: &str = "reconciliation.json";
const RELEASE_RESULT_FILE: &str = "release.json";
const RECORDED_RESULT_SCHEMA_VERSION: u32 = 1;
const RUN_CATALOG_CAP: usize = 500;
const MAX_EVENT_TAIL_BYTES: u64 = 1024 * 1024;
const MAX_EVENT_TAIL_COUNT: usize = 500;
const MAX_PWSH_OUTPUT_BYTES: usize = 1024 * 1024;
const PWSH_TIMEOUT: Duration = Duration::from_secs(8);
static API_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[cfg(windows)]
const PWSH_7: &str = r"C:\Program Files\PowerShell\7\pwsh.exe";
#[cfg(not(windows))]
const PWSH_7: &str = "/usr/bin/pwsh";

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedRunRequest {
    pub repository_root: PathBuf,
    pub run_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateRunApiRequest {
    pub repository_root: PathBuf,
    pub run_id: String,
    pub plan_path: PathBuf,
    #[serde(default)]
    pub next_actions: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRunApiResponse {
    pub run_dir: PathBuf,
    pub manifest: AllowedFileManifest,
    pub hot_resume: HotResumeState,
    pub scheduler: PublicSchedulerState,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreflightInspectApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    #[serde(default)]
    pub required_ports: BTreeSet<u16>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceProbeApiRequest {
    pub repository_root: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceProbeApiResponse {
    pub provider: &'static str,
    pub executable: PathBuf,
    pub sampled_at_ms: u64,
    pub resources: ResourceSnapshot,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    pub event_offset: Option<u64>,
    pub max_event_bytes: Option<u64>,
    pub max_events: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventTailResponse {
    pub events: Vec<RunEvent>,
    pub start_offset: u64,
    pub next_offset: u64,
    pub skipped_lines: usize,
    pub trailing_partial: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineSnapshotResponse {
    pub manifest: AllowedFileManifest,
    pub hot_resume: HotResumeState,
    pub scheduler: PublicSchedulerState,
    pub preflight: Option<PreflightReport>,
    pub preflight_recorded_at_ms: Option<u64>,
    pub run_approval: Option<RunApprovalReceipt>,
    pub run_approval_recorded_at_ms: Option<u64>,
    pub reconciliation: Option<ReconciliationResult>,
    pub reconciliation_recorded_at_ms: Option<u64>,
    pub release: Option<ReleaseGateResult>,
    pub release_recorded_at_ms: Option<u64>,
    pub event_tail: EventTailResponse,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeCollisionApproval {
    pub node_id: String,
    pub census_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunApprovalReceipt {
    pub manifest_digest: String,
    pub plan_contract_digest: String,
    pub preflight_recorded_at_ms: u64,
    pub registry_generation: u64,
    pub collision_assessments: Vec<NodeCollisionApproval>,
    pub approval_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordedResult<T> {
    schema_version: u32,
    recorded_at_ms: u64,
    result: T,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunCatalogApiRequest {
    pub repository_root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCatalogEntry {
    pub run_id: String,
    pub repository_root: PathBuf,
    pub branch: String,
    pub plan_id: String,
    pub plan_path: PathBuf,
    pub status: String,
    pub completed_nodes: usize,
    pub total_nodes: usize,
    pub updated_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCatalogResponse {
    pub active_runs: Vec<RunCatalogEntry>,
    pub archived_runs: Vec<RunCatalogEntry>,
    pub scanned_entries: usize,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimNodeApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    pub node_id: String,
    pub worker_id: String,
    pub now_ms: u64,
    pub lease_ms: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmitWorkerApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    pub node_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrokeredHeartbeatApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    pub node_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeartbeatApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    pub node_id: String,
    pub token: String,
    pub now_ms: u64,
    pub lease_ms: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FencedCompletionApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    pub node_id: String,
    #[serde(default)]
    pub artifacts: Vec<SupplementalArtifactRequest>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SupplementalArtifactRequest {
    pub kind: EvidenceKind,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FailureApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    pub node_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReapApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "action",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
pub enum ReapActionResponse {
    Reassigned {
        node_id: String,
        worker_id: String,
        preserved_evidence: Option<PathBuf>,
    },
    Blocked {
        node_id: String,
        worker_id: String,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerValidationApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    pub node_id: String,
    #[serde(default)]
    pub artifacts: Vec<SupplementalArtifactRequest>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconcileApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    pub input: ReconciliationInput,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    pub input: ReleaseGateInput,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    pub release: ReleaseGateInput,
    pub delivery: DeliveryRequest,
}

#[derive(Debug)]
struct ScopedContext {
    repository_root: PathBuf,
    run_dir: PathBuf,
    scope: RunScope,
}

fn planner_registration_seed(context: &ScopedContext) -> Result<PlannerRegistrationSeed, String> {
    let mut nodes = context
        .scope
        .manifest
        .nodes
        .iter()
        .map(|node| PlannerNodeManifest {
            node_id: node.node_id.clone(),
            files: node
                .allowed_files
                .iter()
                .map(|file| file.to_string_lossy().replace('\\', "/"))
                .collect(),
            resources: node.allowed_resources.clone(),
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|left, right| left.node_id.cmp(&right.node_id));
    let repository_id = format!(
        "repo-{}",
        digest_text(&[&context.scope.manifest.git_common_dir.to_string_lossy()])
    );
    let planner_id = format!(
        "planner-{}",
        digest_text(&[
            &context.scope.manifest.worktree_id,
            &context.scope.manifest.plan_id,
            &context.scope.manifest.plan_path.to_string_lossy(),
        ])
    );
    Ok(PlannerRegistrationSeed {
        planner_id,
        repository_id,
        repository_root: context.repository_root.to_string_lossy().into_owned(),
        worktree_root: context.repository_root.to_string_lossy().into_owned(),
        branch: context.scope.manifest.branch.clone(),
        plan_id: context.scope.manifest.plan_id.clone(),
        plan_path: context
            .scope
            .manifest
            .plan_path
            .to_string_lossy()
            .into_owned(),
        files: context
            .scope
            .manifest
            .allowed_files
            .iter()
            .map(|file| file.to_string_lossy().replace('\\', "/"))
            .collect(),
        resources: context.scope.manifest.allowed_resources.clone(),
        nodes,
    })
}

fn reject_dirty_target_files(
    repository_root: &Path,
    target_files: &[String],
) -> Result<(), String> {
    let mut dirty = BTreeSet::new();
    for args in [
        vec!["diff", "--name-only", "-z", "--"],
        vec!["diff", "--cached", "--name-only", "-z", "--"],
        vec!["ls-files", "--others", "--exclude-standard", "-z", "--"],
    ] {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository_root)
            .args(args)
            .output()
            .map_err(|error| format!("cannot inspect dirty target files: {error}"))?;
        if !output.status.success() {
            return Err("git refused the dirty target-file inspection".to_string());
        }
        for raw in output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|raw| !raw.is_empty())
        {
            let value = std::str::from_utf8(raw)
                .map_err(|_| "git returned a non-UTF-8 dirty path".to_string())?;
            dirty.insert(value.replace('\\', "/").to_ascii_lowercase());
        }
    }
    let collisions = target_files
        .iter()
        .filter(|file| dirty.contains(&file.to_ascii_lowercase()))
        .cloned()
        .collect::<Vec<_>>();
    if collisions.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "admission refused dirty target ownership: {}",
            collisions.join(", ")
        ))
    }
}

fn git_dirty_paths(repository_root: &Path) -> Result<BTreeSet<String>, String> {
    let mut dirty = BTreeSet::new();
    for args in [
        ["diff", "--name-only", "-z", "--"].as_slice(),
        ["diff", "--cached", "--name-only", "-z", "--"].as_slice(),
        ["ls-files", "--others", "--exclude-standard", "-z", "--"].as_slice(),
    ] {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository_root)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .map_err(|error| format!("cannot inspect repository dirty paths: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "Git refused repository dirty-path inspection: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        for raw in output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|raw| !raw.is_empty())
        {
            let value = std::str::from_utf8(raw)
                .map_err(|_| "Git returned a non-UTF-8 dirty path".to_string())?;
            dirty.insert(value.replace('\\', "/"));
        }
    }
    if dirty.len() > 16_384 {
        return Err("repository dirty-path census exceeds the safety limit".to_string());
    }
    Ok(dirty)
}

fn admission_git_baseline(
    repository_root: &Path,
    target_files: &[String],
) -> Result<AdmissionGitBaseline, String> {
    Ok(AdmissionGitBaseline {
        head_commit: git_head(repository_root)?,
        outside_manifest_digest: outside_manifest_digest(repository_root, target_files)?,
    })
}

fn git_head(repository_root: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository_root)
        .args(["rev-parse", "--verify", "HEAD"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|error| format!("cannot inspect Git HEAD: {error}"))?;
    let head = String::from_utf8(output.stdout)
        .map_err(|_| "Git HEAD is not UTF-8".to_string())?
        .trim()
        .to_ascii_lowercase();
    if !output.status.success()
        || head.len() != 40
        || !head.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("Git HEAD is unavailable or malformed".to_string());
    }
    Ok(head)
}

fn outside_manifest_digest(
    repository_root: &Path,
    target_files: &[String],
) -> Result<String, String> {
    let claims = target_files
        .iter()
        .map(|value| value.replace('\\', "/").to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mut digest = Sha256::new();
    digest.update(b"perfect-planner:outside-manifest-git-state:v1\0");
    for path in git_dirty_paths(repository_root)?
        .into_iter()
        .filter(|path| {
            let normalized = path.replace('\\', "/").to_ascii_lowercase();
            !normalized.starts_with(".claude/scratch/orchestrator/")
                && !claims.iter().any(|claim| path_matches_claim(path, claim))
        })
    {
        let path_bytes = path.as_bytes();
        digest.update((path_bytes.len() as u64).to_le_bytes());
        digest.update(path_bytes);
        for args in [
            vec!["status", "--porcelain=v2", "-z", "--", path.as_str()],
            vec!["diff", "--binary", "HEAD", "--", path.as_str()],
            vec!["ls-files", "-s", "-z", "--", path.as_str()],
        ] {
            let output = Command::new("git")
                .arg("-C")
                .arg(repository_root)
                .args(&args)
                .env("GIT_OPTIONAL_LOCKS", "0")
                .output()
                .map_err(|error| format!("cannot digest Git state for {path}: {error}"))?;
            if !output.status.success() || output.stdout.len() > 32 * 1024 * 1024 {
                return Err(format!("Git state for {path} is unavailable or oversized"));
            }
            digest.update((output.stdout.len() as u64).to_le_bytes());
            digest.update(&output.stdout);
        }
        let absolute = repository_root.join(Path::new(&path));
        match fs::symlink_metadata(&absolute) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("dirty path {path} is a symbolic link"));
            }
            Ok(metadata) if metadata.is_file() => {
                if metadata.len() > 32 * 1024 * 1024 {
                    return Err(format!("dirty path {path} exceeds the evidence limit"));
                }
                let bytes = fs::read(&absolute)
                    .map_err(|error| format!("cannot read dirty path {path}: {error}"))?;
                digest.update((bytes.len() as u64).to_le_bytes());
                digest.update(&bytes);
            }
            Ok(_) => return Err(format!("dirty path {path} is not a regular file")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                digest.update(0_u64.to_le_bytes())
            }
            Err(error) => return Err(format!("cannot inspect dirty path {path}: {error}")),
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn path_matches_claim(path: &str, claim: &str) -> bool {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    let claim = claim
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase();
    path == claim
        || path
            .strip_prefix(&claim)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn digest_bytes(parts: &[&str]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"perfect-planner:native-admission:v1");
    for part in parts {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    digest.finalize().into()
}

fn digest_text(parts: &[&str]) -> String {
    digest_bytes(parts)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode_sha256(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("bound SHA-256 digest is malformed".to_string());
    }
    let mut output = [0_u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "bound SHA-256 digest is malformed".to_string())?;
    }
    Ok(output)
}

#[tauri::command]
pub fn orchestrator_create_run(
    request: CreateRunApiRequest,
) -> Result<CreateRunApiResponse, String> {
    let repository_root = canonical_repository(&request.repository_root)?;
    validate_run_id(&request.run_id)?;
    let scope = RunScope::create(CreateRunScope {
        repository_root: repository_root.clone(),
        run_id: request.run_id.clone(),
        plan_path: request.plan_path,
        next_actions: request.next_actions,
    })?;
    let nodes = scope
        .manifest
        .nodes
        .iter()
        .map(|node| ScheduledNode {
            id: node.node_id.clone(),
            wave: node.wave,
            depends_on: node.depends_on.clone(),
            attempts: 0,
            status: NodeStatus::Ready,
            lease: None,
            stall_alarm_fence: None,
        })
        .collect::<Vec<_>>();
    validate_initial_nodes(&nodes)?;
    let context = open_context(&ScopedRunRequest {
        repository_root,
        run_id: request.run_id,
    })?;
    let scheduler = open_scheduler(&context, nodes)?.public_snapshot()?;
    let hot_resume = scope.read_hot_resume()?;

    Ok(CreateRunApiResponse {
        run_dir: context.run_dir,
        manifest: scope.manifest,
        hot_resume,
        scheduler,
    })
}

#[tauri::command]
pub fn orchestrator_preflight_inspect(
    request: PreflightInspectApiRequest,
) -> Result<PreflightReport, String> {
    let context = open_context(&request.scope)?;
    let required_ports = declared_required_ports(&context.scope.manifest)?;
    if request.required_ports != required_ports {
        return Err(format!(
            "preflight required ports must exactly match immutable manifest claims: expected {:?}, received {:?}",
            required_ports, request.required_ports
        ));
    }
    let probe = Pwsh7SystemProbe::fixed()?;
    let engine = PreflightEngine::new(probe, DenyProcessAdapter);
    let report = engine.run(&PreflightRequest {
        repository_root: context.repository_root.clone(),
        required_ports,
        process_allowlist: BTreeSet::new(),
        stop_allowlisted_conflicts: false,
    })?;
    persist_scoped_json(&context, PREFLIGHT_RESULT_FILE, &report)?;
    append_event(
        &context,
        None,
        "head-orchestrator",
        EventType::Preflight,
        "host preflight inspected without process termination",
        serde_json::to_value(&report).map_err(|error| error.to_string())?,
    )?;
    Ok(report)
}

/// Issue one durable activation receipt only after the user clicks approve and native code
/// rechecks the current system preflight plus every node against the machine-wide ownership
/// registry. No approval boolean, digest, clock, manifest, or collision verdict crosses the
/// renderer boundary.
#[tauri::command]
pub fn orchestrator_approve_run(
    registry: tauri::State<'_, PlannerRegistryStore>,
    request: ScopedRunRequest,
) -> Result<RunApprovalReceipt, String> {
    approve_run(&registry, request)
}

fn approve_run(
    registry: &PlannerRegistryStore,
    request: ScopedRunRequest,
) -> Result<RunApprovalReceipt, String> {
    const REGISTRY_LEASE_MS: u64 = 300_000;
    const PREFLIGHT_MAX_AGE_MS: u64 = 60_000;

    let context = open_context(&request)?;
    let now_ms = unix_ms();
    let preflight = load_optional_scoped_json::<PreflightReport>(&context, PREFLIGHT_RESULT_FILE)?
        .ok_or_else(|| "run approval requires a recorded native preflight".to_string())?;
    if preflight.result.disposition != PreflightDisposition::Ready
        || preflight.result.baseline.repository_root != context.repository_root
        || preflight.recorded_at_ms > now_ms
        || now_ms.saturating_sub(preflight.recorded_at_ms) > PREFLIGHT_MAX_AGE_MS
    {
        return Err("run approval preflight is stale, mismatched or not READY".to_string());
    }

    let seed = planner_registration_seed(&context)?;
    let scheduler_state = open_scheduler(&context, Vec::new())?.snapshot()?;
    for node in &seed.nodes {
        let scheduled = scheduler_state.nodes.get(&node.node_id).ok_or_else(|| {
            format!(
                "approved plan node {} is absent from scheduler state",
                node.node_id
            )
        })?;
        if scheduled.attempts == 0 {
            reject_dirty_target_files(&context.repository_root, &node.files)?;
        }
    }
    let mut registry_generation = None;
    let mut collision_assessments = Vec::with_capacity(seed.nodes.len());
    for node in &seed.nodes {
        let collision = registry
            .prepare_manifest_collision_snapshot(
                seed.clone(),
                &node.node_id,
                now_ms,
                REGISTRY_LEASE_MS,
            )
            .map_err(|error| format!("run approval collision census is UNKNOWN: {error}"))?;
        if !collision.conflict_ids.is_empty() {
            return Err(format!(
                "run approval refused because node {} has {} ownership conflict(s)",
                node.node_id,
                collision.conflict_ids.len()
            ));
        }
        match registry_generation {
            Some(expected) if expected != collision.registry_generation => {
                return Err("run approval collision registry changed during assessment".to_string())
            }
            None => registry_generation = Some(collision.registry_generation),
            _ => {}
        }
        collision_assessments.push(NodeCollisionApproval {
            node_id: node.node_id.clone(),
            census_digest: collision.digest,
        });
    }
    collision_assessments.sort_by(|left, right| left.node_id.cmp(&right.node_id));
    let registry_generation = registry_generation
        .ok_or_else(|| "run approval requires at least one immutable plan node".to_string())?;
    let encoded_assessments = serde_json::to_string(&collision_assessments)
        .map_err(|error| format!("cannot encode approval assessment: {error}"))?;
    let approval_digest = digest_text(&[
        "native-run-approval-v1",
        &context.scope.manifest.manifest_digest,
        &context.scope.manifest.plan_contract_digest,
        &preflight.recorded_at_ms.to_string(),
        &registry_generation.to_string(),
        &encoded_assessments,
    ]);
    let receipt = RunApprovalReceipt {
        manifest_digest: context.scope.manifest.manifest_digest.clone(),
        plan_contract_digest: context.scope.manifest.plan_contract_digest.clone(),
        preflight_recorded_at_ms: preflight.recorded_at_ms,
        registry_generation,
        collision_assessments,
        approval_digest,
    };
    if let Some(existing) =
        load_optional_scoped_json::<RunApprovalReceipt>(&context, RUN_APPROVAL_FILE)?
    {
        if existing.result == receipt {
            return Ok(existing.result);
        }
    }
    persist_scoped_json(&context, RUN_APPROVAL_FILE, &receipt)?;
    append_event(
        &context,
        None,
        "head-orchestrator",
        EventType::GatePass,
        "run explicitly approved after whole-plan collision assessment",
        json!({
            "approvalDigest": receipt.approval_digest,
            "registryGeneration": receipt.registry_generation,
            "assessedNodes": receipt.collision_assessments.len(),
        }),
    )?;
    Ok(receipt)
}

fn validate_run_approval(
    context: &ScopedContext,
    preflight_recorded_at_ms: u64,
    receipt: &RunApprovalReceipt,
) -> Result<(), String> {
    let node_ids = context
        .scope
        .manifest
        .nodes
        .iter()
        .map(|node| node.node_id.as_str())
        .collect::<BTreeSet<_>>();
    let assessment_ids = receipt
        .collision_assessments
        .iter()
        .map(|assessment| assessment.node_id.as_str())
        .collect::<BTreeSet<_>>();
    if receipt.manifest_digest != context.scope.manifest.manifest_digest
        || receipt.plan_contract_digest != context.scope.manifest.plan_contract_digest
        || receipt.preflight_recorded_at_ms != preflight_recorded_at_ms
        || node_ids != assessment_ids
        || receipt.collision_assessments.len() != node_ids.len()
        || receipt.registry_generation == 0
        || receipt.collision_assessments.iter().any(|assessment| {
            assessment.census_digest.len() != 64
                || !assessment
                    .census_digest
                    .chars()
                    .all(|value| value.is_ascii_hexdigit())
        })
    {
        return Err("admission run approval is stale, incomplete or scope-mismatched".to_string());
    }
    let encoded = serde_json::to_string(&receipt.collision_assessments)
        .map_err(|error| format!("cannot encode approval assessment: {error}"))?;
    let expected = digest_text(&[
        "native-run-approval-v1",
        &receipt.manifest_digest,
        &receipt.plan_contract_digest,
        &receipt.preflight_recorded_at_ms.to_string(),
        &receipt.registry_generation.to_string(),
        &encoded,
    ]);
    if expected != receipt.approval_digest {
        return Err("admission run approval digest is invalid".to_string());
    }
    Ok(())
}

/// Read-only capacity telemetry for the always-visible resource guard.
/// Reuses the fixed PowerShell 7 + Windows CIM preflight probe and gains no stop authority.
#[tauri::command]
pub fn orchestrator_resource_probe(
    request: ResourceProbeApiRequest,
) -> Result<ResourceProbeApiResponse, String> {
    let repository_root = canonical_repository(&request.repository_root)?;
    let resources = native_resource_snapshot(&repository_root)?;
    Ok(ResourceProbeApiResponse {
        provider: "Windows native system APIs",
        executable: std::env::current_exe()
            .map_err(|error| format!("cannot resolve native resource probe executable: {error}"))?,
        sampled_at_ms: unix_ms(),
        resources,
    })
}

#[tauri::command]
#[allow(private_interfaces)]
pub fn orchestrator_pipeline_snapshot(
    authority: tauri::State<'_, SchedulerAuthorityRuntime>,
    request: SnapshotApiRequest,
) -> Result<PipelineSnapshotResponse, String> {
    pipeline_snapshot(&authority, request)
}

fn pipeline_snapshot(
    authority: &SchedulerAuthorityRuntime,
    request: SnapshotApiRequest,
) -> Result<PipelineSnapshotResponse, String> {
    let context = open_context(&request.scope)?;
    let scheduler_store = open_scheduler(&context, Vec::new())?;
    if !recover_scheduler_leases(authority, &context, &scheduler_store)?.is_empty() {
        sync_hot_resume(&context, &scheduler_store, None)?;
    }
    for completion in scheduler_store.snapshot()?.completions.values() {
        finalize_completion(&context, &scheduler_store, completion)?;
    }
    let hot_resume = context.scope.read_hot_resume()?;
    let scheduler = scheduler_store.public_snapshot()?;
    let preflight = load_optional_scoped_json::<PreflightReport>(&context, PREFLIGHT_RESULT_FILE)?;
    let persisted_run_approval =
        load_optional_scoped_json::<RunApprovalReceipt>(&context, RUN_APPROVAL_FILE)?;
    // An approval is active authority only for the exact preflight that it approved.
    // Keep an older receipt on disk for audit, but do not project it as current after
    // a later preflight; the operator must explicitly approve the new facts.
    let run_approval = persisted_run_approval.filter(|approval| {
        preflight.as_ref().is_some_and(|current| {
            approval.result.preflight_recorded_at_ms == current.recorded_at_ms
        })
    });
    let reconciliation =
        load_optional_scoped_json::<ReconciliationResult>(&context, RECONCILIATION_RESULT_FILE)?;
    let release = load_optional_scoped_json::<ReleaseGateResult>(&context, RELEASE_RESULT_FILE)?;
    let event_tail = bounded_event_tail(
        &context,
        request.event_offset,
        request.max_event_bytes,
        request.max_events,
    )?;
    Ok(PipelineSnapshotResponse {
        manifest: context.scope.manifest,
        hot_resume,
        scheduler,
        preflight_recorded_at_ms: preflight.as_ref().map(|record| record.recorded_at_ms),
        preflight: preflight.map(|record| record.result),
        run_approval_recorded_at_ms: run_approval.as_ref().map(|record| record.recorded_at_ms),
        run_approval: run_approval.map(|record| record.result),
        reconciliation_recorded_at_ms: reconciliation.as_ref().map(|record| record.recorded_at_ms),
        reconciliation: reconciliation.map(|record| record.result),
        release_recorded_at_ms: release.as_ref().map(|record| record.recorded_at_ms),
        release: release.map(|record| record.result),
        event_tail,
    })
}

/// Admit one bounded local worker through the native-only preclaim, collision and signing chain.
/// The renderer names only an already-bound run and node; it cannot choose a worker identity,
/// clock, lease lifetime, fence, authority receipt or bearer token.
#[allow(private_interfaces)]
#[tauri::command]
pub fn orchestrator_admit_worker(
    authority: tauri::State<'_, SchedulerAuthorityRuntime>,
    registry: tauri::State<'_, PlannerRegistryStore>,
    request: AdmitWorkerApiRequest,
) -> Result<PublicNodeLease, String> {
    admit_worker(&authority, &registry, request)
}

fn admit_worker(
    authority: &SchedulerAuthorityRuntime,
    registry: &PlannerRegistryStore,
    request: AdmitWorkerApiRequest,
) -> Result<PublicNodeLease, String> {
    const LEASE_MS: u64 = 30_000;
    const REGISTRY_LEASE_MS: u64 = 300_000;
    const PREFLIGHT_MAX_AGE_MS: u64 = 60_000;

    validate_text("nodeId", &request.node_id)?;
    let context = open_context(&request.scope)?;
    let now_ms = unix_ms();
    let preflight = load_optional_scoped_json::<PreflightReport>(&context, PREFLIGHT_RESULT_FILE)?
        .ok_or_else(|| "admission requires a recorded native preflight".to_string())?;
    if preflight.result.disposition != PreflightDisposition::Ready
        || preflight.result.baseline.repository_root != context.repository_root
        || preflight.recorded_at_ms > now_ms
        || now_ms.saturating_sub(preflight.recorded_at_ms) > PREFLIGHT_MAX_AGE_MS
    {
        return Err("admission preflight is stale, mismatched or not READY".to_string());
    }
    let approval = load_optional_scoped_json::<RunApprovalReceipt>(&context, RUN_APPROVAL_FILE)?
        .ok_or_else(|| "admission requires explicit native run approval".to_string())?;
    validate_run_approval(&context, preflight.recorded_at_ms, &approval.result)?;

    let seed = planner_registration_seed(&context)?;
    let target = seed
        .nodes
        .iter()
        .find(|node| node.node_id == request.node_id)
        .ok_or_else(|| "admission node is absent from the approved plan".to_string())?;
    let scheduler = open_scheduler(&context, Vec::new())?;
    let scheduler_state = scheduler.snapshot()?;
    let node = scheduler_state
        .nodes
        .get(&request.node_id)
        .ok_or_else(|| format!("unknown node {}", request.node_id))?;
    if node.status != NodeStatus::Ready || node.lease.is_some() {
        return Err(format!("node {} is not claimable", request.node_id));
    }
    if node.attempts == 0 {
        reject_dirty_target_files(&context.repository_root, &target.files)?;
    }

    let collision = registry
        .prepare_manifest_collision_snapshot(
            seed.clone(),
            &request.node_id,
            now_ms,
            REGISTRY_LEASE_MS,
        )
        .map_err(|error| format!("native collision census is UNKNOWN: {error}"))?;
    if !collision.conflict_ids.is_empty() {
        return Err(format!(
            "native collision census refused admission with {} conflict(s)",
            collision.conflict_ids.len()
        ));
    }
    let approved_collision = approval
        .result
        .collision_assessments
        .iter()
        .find(|assessment| assessment.node_id == request.node_id)
        .ok_or_else(|| "admission run approval omitted the target node".to_string())?;
    if collision.registry_generation != approval.result.registry_generation
        || collision.digest != approved_collision.census_digest
    {
        return Err("admission collision state changed after explicit approval".to_string());
    }

    let fence = scheduler_state.next_fence.max(1);
    let authority_generation = u64::from(node.attempts)
        .checked_add(1)
        .ok_or_else(|| "node authority generation exhausted".to_string())?;
    let expires_at_ms = now_ms
        .checked_add(LEASE_MS)
        .ok_or_else(|| "lease expiry overflowed".to_string())?;
    let worker_id = format!(
        "worker-{}",
        digest_text(&[
            &context.scope.manifest.run_id,
            &request.node_id,
            &authority.epoch().to_string(),
            &fence.to_string(),
        ])
    );
    let binding = AuthorityBinding {
        organization_id: "local-machine".to_string(),
        repository_id: seed.repository_id.clone(),
        plan_id: context.scope.manifest.plan_id.clone(),
        node_id: request.node_id.clone(),
        epoch: authority.epoch(),
        generation: collision.planner_lease_generation,
        fence,
        plan_digest: context.scope.manifest.plan_contract_digest.clone(),
        manifest_digest: context.scope.manifest.manifest_digest.clone(),
        collision_digest: collision.digest.clone(),
    };
    let scope_id = digest_text(&[
        &context.scope.manifest.manifest_digest,
        &request.node_id,
        &authority.epoch().to_string(),
        &collision.registry_generation.to_string(),
        &collision.planner_lease_generation.to_string(),
        &fence.to_string(),
    ]);
    let reservation_id = format!("reservation-{scope_id}");
    let publication_id = format!("publication-{scope_id}");
    let clearance_id = format!("clearance-{scope_id}");
    let authorization_id = format!("authorization-{scope_id}");
    let policy_digest = digest_text(&[
        "native-admission-policy-v1",
        &preflight.recorded_at_ms.to_string(),
        &collision.digest,
    ]);
    let preclaims = PreclaimStore::open(context.run_dir.join("preclaims.json"))?;
    let initial = preclaims.expectation()?;
    let current = preclaims.reap_expired(&initial, now_ms)?;
    let reserved = preclaims.reserve(
        &current,
        PreclaimRecord {
            reservation_id: reservation_id.clone(),
            scope_id: scope_id.clone(),
            run_id: context.scope.manifest.run_id.clone(),
            node_id: request.node_id.clone(),
            worker_id: worker_id.clone(),
            lease_generation: collision.planner_lease_generation,
            fence,
            manifest_digest: context.scope.manifest.manifest_digest.clone(),
            policy_digest,
            delivered_approval_digest: context.scope.manifest.approval_receipt_digest.clone(),
            created_at_ms: now_ms,
            expires_at_ms,
        },
    )?;

    let result = (|| -> Result<PublicNodeLease, String> {
        authority.reserve(
            &scope_id,
            PreclaimReservation {
                receipt_id: reservation_id.clone(),
                binding: binding.clone(),
                issued_at_ms: now_ms,
                expires_at_ms,
            },
            now_ms,
        )?;
        authority.publish_authority(
            &scope_id,
            AuthorityPublicationReceipt {
                receipt_id: publication_id.clone(),
                reservation_receipt_id: reservation_id.clone(),
                binding: binding.clone(),
                published_at_ms: now_ms,
                expires_at_ms,
            },
            now_ms,
        )?;

        let verified_collision = registry
            .prepare_manifest_collision_snapshot(
                seed.clone(),
                &request.node_id,
                now_ms,
                REGISTRY_LEASE_MS,
            )
            .map_err(|error| format!("native collision census revalidation is UNKNOWN: {error}"))?;
        if verified_collision.digest != collision.digest
            || verified_collision.registry_generation != collision.registry_generation
            || verified_collision.planner_lease_generation != collision.planner_lease_generation
            || !verified_collision.conflict_ids.is_empty()
        {
            return Err("native collision census changed during admission".to_string());
        }
        authority.accept_clear_census(
            &scope_id,
            CensusClearReceipt {
                receipt_id: clearance_id.clone(),
                reservation_receipt_id: reservation_id.clone(),
                publication_receipt_id: publication_id,
                binding: binding.clone(),
                census_digest: verified_collision.digest,
                verdict: CensusVerdict::Clear,
                observed_at_ms: now_ms,
                expires_at_ms,
            },
            now_ms,
        )?;

        let native_binding = ReservationBinding::from_native_digests(
            digest_bytes(&["scheduler", &authority.epoch().to_string(), &scope_id]),
            digest_bytes(&["repository", &context.scope.manifest.worktree_id]),
            digest_bytes(&["planner", &seed.planner_id]),
            decode_sha256(&context.scope.manifest.plan_contract_digest)?,
            digest_bytes(&[
                "node-set",
                &serde_json::to_string(&seed.nodes).map_err(|error| error.to_string())?,
            ]),
            collision.registry_generation,
            collision.planner_lease_generation,
            authority_generation,
        )
        .map_err(|error| format!("native reservation binding rejected: {error}"))?;
        let payload_digest = digest_bytes(&[
            "claim-payload",
            &context.scope.manifest.manifest_digest,
            &context.scope.manifest.approval_receipt_digest,
            &collision.digest,
        ]);
        let (grant, _) = authority.consume_and_sign_clearance(
            &scope_id,
            ClaimRequest {
                request_id: authorization_id,
                worker_id: worker_id.clone(),
                clearance_receipt_id: clearance_id,
                binding,
                requested_at_ms: now_ms,
                expires_at_ms,
            },
            &native_binding,
            payload_digest,
            now_ms,
        )?;
        let git_baseline = admission_git_baseline(&context.repository_root, &target.files)?;
        let lease = scheduler.claim_authorized(&grant, git_baseline, now_ms)?;
        preclaims.consume(&reserved, &scope_id, &reservation_id, now_ms)?;
        sync_hot_resume(&context, &scheduler, None)?;
        append_event(
            &context,
            Some(request.node_id.clone()),
            &worker_id,
            EventType::Claim,
            "authority-backed worker admitted",
            json!({
                "fence": lease.fence,
                "expiresAtMs": lease.expires_at_ms,
                "authorityEpoch": lease.authority_epoch,
                "authorizationId": lease.authorization_id,
                "collisionDigest": collision.digest,
            }),
        )?;
        Ok(PublicNodeLease::from(&lease))
    })();

    if result.is_err() {
        let _ = authority.invalidate(&scope_id);
        if let Ok(expected) = preclaims.expectation() {
            let _ = preclaims.consume(&expected, &scope_id, &reservation_id, now_ms);
        }
    }
    result
}

/// Renew only a native-held lease. The bearer token remains in the scheduler store and the live
/// issuer epoch is rechecked before every extension.
#[allow(private_interfaces)]
#[tauri::command]
pub fn orchestrator_worker_heartbeat(
    authority: tauri::State<'_, SchedulerAuthorityRuntime>,
    request: BrokeredHeartbeatApiRequest,
) -> Result<PublicNodeLease, String> {
    brokered_heartbeat(&authority, request)
}

fn brokered_heartbeat(
    authority: &SchedulerAuthorityRuntime,
    request: BrokeredHeartbeatApiRequest,
) -> Result<PublicNodeLease, String> {
    validate_text("nodeId", &request.node_id)?;
    let context = open_context(&request.scope)?;
    let scheduler = open_scheduler(&context, Vec::new())?;
    let current = scheduler
        .snapshot()?
        .nodes
        .get(&request.node_id)
        .and_then(|node| node.lease.clone())
        .ok_or_else(|| format!("node {} has no live lease", request.node_id))?;
    if current.authority_epoch != Some(authority.epoch()) || current.authorization_id.is_none() {
        return Err("worker lease belongs to a stale or legacy authority epoch".to_string());
    }
    let now_ms = unix_ms();
    let lease = scheduler.renew(&request.node_id, &current.token, now_ms, 30_000)?;
    append_event(
        &context,
        Some(request.node_id),
        &lease.worker_id,
        EventType::Heartbeat,
        "authority-backed worker heartbeat",
        json!({
            "fence": lease.fence,
            "expiresAtMs": lease.expires_at_ms,
            "authorityEpoch": lease.authority_epoch,
        }),
    )?;
    Ok(PublicNodeLease::from(&lease))
}

// Internal foundation only. B20 must bind this operation to a scheduler-owned,
// consumed collision-clearance receipt before it can be registered as a Tauri command.
pub fn orchestrator_claim_node(request: ClaimNodeApiRequest) -> Result<NodeLease, String> {
    validate_text("nodeId", &request.node_id)?;
    validate_text("workerId", &request.worker_id)?;
    let context = open_context(&request.scope)?;
    let scheduler = open_scheduler(&context, Vec::new())?;
    let lease = scheduler.claim(
        &request.node_id,
        &request.worker_id,
        request.now_ms,
        request.lease_ms,
    )?;
    append_event(
        &context,
        Some(request.node_id),
        &request.worker_id,
        EventType::Claim,
        "node lease claimed",
        json!({ "fence": lease.fence, "expiresAtMs": lease.expires_at_ms }),
    )?;
    Ok(lease)
}

// Renewal is admission authority too. Keep it off the renderer surface until B09/B20
// prove the persisted lease against the current native issuer epoch and clearance.
pub fn orchestrator_heartbeat(request: HeartbeatApiRequest) -> Result<NodeLease, String> {
    validate_text("nodeId", &request.node_id)?;
    validate_token(&request.token)?;
    let context = open_context(&request.scope)?;
    let scheduler = open_scheduler(&context, Vec::new())?;
    let lease = scheduler.renew(
        &request.node_id,
        &request.token,
        request.now_ms,
        request.lease_ms,
    )?;
    append_event(
        &context,
        Some(request.node_id),
        &lease.worker_id,
        EventType::Heartbeat,
        "node lease renewed",
        json!({ "fence": lease.fence, "expiresAtMs": lease.expires_at_ms }),
    )?;
    Ok(lease)
}

#[tauri::command]
#[allow(private_interfaces)]
pub fn orchestrator_complete_worker(
    authority: tauri::State<'_, SchedulerAuthorityRuntime>,
    request: FencedCompletionApiRequest,
) -> Result<WorkerGateResult, String> {
    authorize_fenced_completion(&authority, request)
}

fn authorize_fenced_completion(
    authority: &SchedulerAuthorityRuntime,
    request: FencedCompletionApiRequest,
) -> Result<WorkerGateResult, String> {
    let context = open_context(&request.scope)?;
    let scheduler = open_scheduler(&context, Vec::new())?;
    if let Some(completion) = scheduler.snapshot()?.completions.get(&request.node_id) {
        finalize_completion(&context, &scheduler, completion)?;
        return Ok(completion.gate.clone());
    }
    let (manifest, submission, lease, now_ms) = native_worker_submission(
        authority,
        &context,
        &scheduler,
        &request.node_id,
        request.artifacts,
    )?;
    scheduler.authorize_commit(&request.node_id, &submission.lease_token, now_ms)?;
    verify_submission_artifacts(&context, &submission)?;
    let gate = validate_submission(&manifest, &submission)?;
    if !gate.passed {
        append_event(
            &context,
            Some(request.node_id),
            "worker",
            EventType::GateFail,
            "worker evidence or manifest gate failed",
            serde_json::to_value(&gate).map_err(|error| error.to_string())?,
        )?;
        return Err("completion rejected by the worker evidence gate".to_string());
    }
    let receipt_id = format!(
        "completion-{}",
        digest_text(&[
            &context.scope.manifest.manifest_digest,
            &request.node_id,
            lease.authorization_id.as_deref().unwrap_or_default(),
            &lease.fence.to_string(),
            &serde_json::to_string(&submission.artifacts).map_err(|error| error.to_string())?,
            &serde_json::to_string(&submission.verification).map_err(|error| error.to_string())?,
        ])
    );
    let completion_details = digest_text(&[
        &receipt_id,
        &serde_json::to_string(&submission.changed_files).map_err(|error| error.to_string())?,
        &serde_json::to_string(&submission.artifacts).map_err(|error| error.to_string())?,
        &serde_json::to_string(&gate).map_err(|error| error.to_string())?,
    ]);
    context.scope.append_audit(RunAuditEvent {
        event_id: format!("{receipt_id}-prepared"),
        at_ms: now_ms,
        kind: "NODE_COMPLETION_PREPARED".to_string(),
        node_id: Some(request.node_id.clone()),
        receipt_digest: receipt_id.trim_start_matches("completion-").to_string(),
        details_digest: completion_details,
    })?;
    let mut completing = context.scope.read_hot_resume()?;
    completing.status = format!("completing:{}", request.node_id);
    completing.locked_files = manifest.allowed_files.iter().map(PathBuf::from).collect();
    completing.next_actions = vec!["Finish the durable completion transaction".to_string()];
    context.scope.update_hot_resume(&completing)?;
    let completion = scheduler.complete_authorized(
        &request.node_id,
        &submission.lease_token,
        now_ms,
        NodeCompletion {
            receipt_id: receipt_id.clone(),
            node_id: request.node_id.clone(),
            worker_id: lease.worker_id.clone(),
            fence: lease.fence,
            authority_epoch: lease.authority_epoch.unwrap_or_default(),
            authorization_id: lease.authorization_id.clone().unwrap_or_default(),
            completed_at_ms: now_ms,
            changed_files: submission.changed_files.clone(),
            artifacts: submission.artifacts.clone(),
            verification: submission.verification.clone(),
            gate: gate.clone(),
        },
    )?;
    finalize_completion(&context, &scheduler, &completion)?;
    append_event(
        &context,
        Some(request.node_id),
        &completion.worker_id,
        EventType::NodeDone,
        "fenced completion persisted",
        serde_json::to_value(&completion).map_err(|error| error.to_string())?,
    )?;
    Ok(gate)
}

fn finalize_completion(
    context: &ScopedContext,
    scheduler: &SchedulerStore,
    completion: &NodeCompletion,
) -> Result<(), String> {
    sync_hot_resume(context, scheduler, Some(completion.node_id.clone()))?;
    let details_digest = digest_text(&[
        &completion.receipt_id,
        &serde_json::to_string(&completion.changed_files).map_err(|error| error.to_string())?,
        &serde_json::to_string(&completion.artifacts).map_err(|error| error.to_string())?,
        &serde_json::to_string(&completion.gate).map_err(|error| error.to_string())?,
    ]);
    context.scope.append_audit(RunAuditEvent {
        event_id: format!("{}-committed", completion.receipt_id),
        at_ms: completion.completed_at_ms,
        kind: "NODE_COMPLETION_COMMITTED".to_string(),
        node_id: Some(completion.node_id.clone()),
        receipt_digest: completion
            .receipt_id
            .trim_start_matches("completion-")
            .to_string(),
        details_digest,
    })?;
    Ok(())
}

fn sync_hot_resume(
    context: &ScopedContext,
    scheduler: &SchedulerStore,
    last_completed_step: Option<String>,
) -> Result<HotResumeState, String> {
    let snapshot = scheduler.snapshot()?;
    let mut state = context.scope.read_hot_resume()?;
    let mut locked = BTreeSet::new();
    for node in snapshot
        .nodes
        .values()
        .filter(|node| node.status == NodeStatus::Running)
    {
        if let Some(manifest) = context
            .scope
            .manifest
            .nodes
            .iter()
            .find(|manifest| manifest.node_id == node.id)
        {
            locked.extend(manifest.allowed_files.iter().cloned());
        }
    }
    let completed = snapshot
        .nodes
        .values()
        .filter(|node| node.status == NodeStatus::Done)
        .count();
    state.status = if completed == snapshot.nodes.len() && !snapshot.nodes.is_empty() {
        "completed".to_string()
    } else if snapshot
        .nodes
        .values()
        .any(|node| node.status == NodeStatus::Blocked)
    {
        "blocked".to_string()
    } else if snapshot
        .nodes
        .values()
        .any(|node| node.status == NodeStatus::Running)
    {
        "running".to_string()
    } else {
        "ready".to_string()
    };
    if last_completed_step.is_some() {
        state.last_completed_step = last_completed_step;
    }
    state.locked_files = locked.into_iter().collect();
    state.next_actions = snapshot
        .nodes
        .values()
        .filter(|node| node.status == NodeStatus::Ready)
        .filter(|node| {
            node.depends_on.iter().all(|dependency| {
                snapshot
                    .nodes
                    .get(dependency)
                    .is_some_and(|dependency| dependency.status == NodeStatus::Done)
            })
        })
        .map(|node| format!("Admit {}", node.id))
        .collect();
    context.scope.update_hot_resume(&state)?;
    Ok(state)
}

#[tauri::command]
#[allow(private_interfaces)]
pub fn orchestrator_fail_worker(
    authority: tauri::State<'_, SchedulerAuthorityRuntime>,
    request: FailureApiRequest,
) -> Result<NodeStatus, String> {
    let context = open_context(&request.scope)?;
    let scheduler = open_scheduler(&context, Vec::new())?;
    let lease = live_authority_lease(&authority, &scheduler, &request.node_id)?;
    let status = scheduler.fail(&request.node_id, &lease.token)?;
    sync_hot_resume(&context, &scheduler, None)?;
    append_event(
        &context,
        Some(request.node_id),
        "worker",
        EventType::GateFail,
        "worker failure recorded",
        json!({ "status": status }),
    )?;
    Ok(status)
}

#[tauri::command]
#[allow(private_interfaces)]
pub fn orchestrator_recover_workers(
    authority: tauri::State<'_, SchedulerAuthorityRuntime>,
    request: ReapApiRequest,
) -> Result<Vec<ReapActionResponse>, String> {
    let context = open_context(&request.scope)?;
    let scheduler = open_scheduler(&context, Vec::new())?;
    let responses = recover_scheduler_leases(&authority, &context, &scheduler)?;
    sync_hot_resume(&context, &scheduler, None)?;
    Ok(responses)
}

fn recover_scheduler_leases(
    authority: &SchedulerAuthorityRuntime,
    context: &ScopedContext,
    scheduler: &SchedulerStore,
) -> Result<Vec<ReapActionResponse>, String> {
    let actions = scheduler.recover_stale_authority(authority.epoch(), unix_ms())?;
    let responses = actions
        .into_iter()
        .map(|action| match action {
            ReapAction::Reassigned {
                node_id,
                worker_id,
                preserved_evidence,
            } => ReapActionResponse::Reassigned {
                node_id,
                worker_id,
                preserved_evidence,
            },
            ReapAction::Blocked { node_id, worker_id } => {
                ReapActionResponse::Blocked { node_id, worker_id }
            }
        })
        .collect::<Vec<_>>();
    for response in &responses {
        let (node_id, worker_id) = match response {
            ReapActionResponse::Reassigned {
                node_id, worker_id, ..
            }
            | ReapActionResponse::Blocked { node_id, worker_id } => (node_id, worker_id),
        };
        append_event(
            context,
            Some(node_id.clone()),
            worker_id,
            EventType::Reassign,
            "stale or expired worker lease recovered",
            serde_json::to_value(response).map_err(|error| error.to_string())?,
        )?;
    }
    Ok(responses)
}

#[tauri::command]
#[allow(private_interfaces)]
pub fn orchestrator_validate_worker(
    authority: tauri::State<'_, SchedulerAuthorityRuntime>,
    request: WorkerValidationApiRequest,
) -> Result<WorkerGateResult, String> {
    let context = open_context(&request.scope)?;
    let scheduler = open_scheduler(&context, Vec::new())?;
    let (manifest, submission, _, now_ms) = native_worker_submission(
        &authority,
        &context,
        &scheduler,
        &request.node_id,
        request.artifacts,
    )?;
    scheduler.authorize_commit(&request.node_id, &submission.lease_token, now_ms)?;
    verify_submission_artifacts(&context, &submission)?;
    validate_submission(&manifest, &submission)
}

pub fn orchestrator_reconcile(
    request: ReconcileApiRequest,
) -> Result<ReconciliationResult, String> {
    let context = open_context(&request.scope)?;
    let allowed: BTreeSet<_> = context
        .scope
        .manifest
        .allowed_files
        .iter()
        .map(|path| normalize_path(path))
        .collect();
    for node in &request.input.nodes {
        for file in node.manifest_files.iter().chain(&node.declared_outputs) {
            if !allowed.contains(&normalize_path(Path::new(file))) {
                return Err(format!(
                    "reconciliation node {} names file outside the run manifest: {file}",
                    node.node_id
                ));
            }
        }
    }
    let result = reconcile(&request.input);
    persist_scoped_json(&context, RECONCILIATION_RESULT_FILE, &result)?;
    Ok(result)
}

pub fn orchestrator_evaluate_release(
    request: ReleaseApiRequest,
) -> Result<ReleaseGateResult, String> {
    let context = open_context(&request.scope)?;
    let result = evaluate_release(&request.input);
    persist_scoped_json(&context, RELEASE_RESULT_FILE, &result)?;
    Ok(result)
}

#[tauri::command]
pub fn orchestrator_run_catalog(
    request: RunCatalogApiRequest,
) -> Result<RunCatalogResponse, String> {
    let repository_root = canonical_repository(&request.repository_root)?;
    build_run_catalog(&repository_root)
}

pub fn orchestrator_deliver(request: DeliveryApiRequest) -> Result<DeliveryOutcome, String> {
    let context = open_context(&request.scope)?;
    if request.delivery.run_id != request.scope.run_id {
        return Err("delivery runId does not match the scoped run".to_string());
    }
    validate_delivery_text(&request.delivery)?;
    let scheduler = open_scheduler(&context, Vec::new())?.snapshot()?;
    if scheduler.nodes.is_empty()
        || scheduler
            .nodes
            .values()
            .any(|node| node.status != NodeStatus::Done)
    {
        return Err("delivery requires every scheduled node to be done".to_string());
    }
    let release = evaluate_release(&request.release);
    if !release.merged || !release.issues.is_empty() {
        return Err("delivery requires a merged, issue-free release gate".to_string());
    }
    let checklist = context.repository_root.join("COMPLETE-CHECKLIST.md");
    deliver_run(
        &context.repository_root,
        &context.run_dir,
        &checklist,
        &request.delivery,
    )
}

fn open_context(request: &ScopedRunRequest) -> Result<ScopedContext, String> {
    let repository_root = canonical_repository(&request.repository_root)?;
    validate_run_id(&request.run_id)?;
    let scope = RunScope::open(&repository_root, &request.run_id)?;
    let run_dir = scope.root.clone();
    Ok(ScopedContext {
        repository_root,
        run_dir,
        scope,
    })
}

fn persist_scoped_json<T: Serialize>(
    context: &ScopedContext,
    file_name: &str,
    value: &T,
) -> Result<(), String> {
    validate_state_file_name(file_name)?;
    let target = context.run_dir.join(file_name);
    validate_optional_scoped_target(&context.run_dir, &target)?;
    let record = RecordedResult {
        schema_version: RECORDED_RESULT_SCHEMA_VERSION,
        recorded_at_ms: unix_ms(),
        result: value,
    };
    let mut bytes = serde_json::to_vec_pretty(&record)
        .map_err(|error| format!("cannot serialize {file_name}: {error}"))?;
    bytes.push(b'\n');
    atomic_replace(&target, &bytes)
        .map_err(|error| format!("cannot atomically persist {file_name}: {error}"))
}

fn load_optional_scoped_json<T: DeserializeOwned>(
    context: &ScopedContext,
    file_name: &str,
) -> Result<Option<RecordedResult<T>>, String> {
    validate_state_file_name(file_name)?;
    let target = context.run_dir.join(file_name);
    match fs::symlink_metadata(&target) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(format!(
                    "persisted state {file_name} is not a regular scoped file"
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "cannot inspect persisted state {file_name}: {error}"
            ))
        }
    }
    let resolved = target
        .canonicalize()
        .map_err(|error| format!("cannot resolve persisted state {file_name}: {error}"))?;
    if resolved.parent() != Some(context.run_dir.as_path()) {
        return Err(format!(
            "persisted state {file_name} escapes the scoped run directory"
        ));
    }
    let bytes = fs::read(&resolved)
        .map_err(|error| format!("cannot read persisted state {file_name}: {error}"))?;
    let value: RecordedResult<T> = serde_json::from_slice(&bytes).map_err(|error| {
        format!("persisted state {file_name} is corrupt or incomplete: {error}")
    })?;
    if value.schema_version != RECORDED_RESULT_SCHEMA_VERSION || value.recorded_at_ms == 0 {
        return Err(format!(
            "persisted state {file_name} has an unsupported or incomplete envelope"
        ));
    }
    Ok(Some(value))
}

fn validate_state_file_name(file_name: &str) -> Result<(), String> {
    if !matches!(
        file_name,
        PREFLIGHT_RESULT_FILE
            | RUN_APPROVAL_FILE
            | RECONCILIATION_RESULT_FILE
            | RELEASE_RESULT_FILE
    ) {
        return Err("unsupported persisted result file".to_string());
    }
    Ok(())
}

fn validate_optional_scoped_target(run_dir: &Path, target: &Path) -> Result<(), String> {
    if target.parent() != Some(run_dir) {
        return Err("persisted result target escapes the scoped run".to_string());
    }
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err("persisted result target is not a regular file".to_string())
        }
        Ok(_) => {
            let resolved = target
                .canonicalize()
                .map_err(|error| format!("cannot resolve persisted result target: {error}"))?;
            if resolved.parent() != Some(run_dir) {
                return Err("persisted result target escapes the scoped run".to_string());
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot inspect persisted result target: {error}")),
    }
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no parent"))?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid target name"))?;
    let sequence = API_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
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
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
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
    // SAFETY: both buffers are valid, NUL-terminated UTF-16 paths for this call.
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

fn build_run_catalog(repository_root: &Path) -> Result<RunCatalogResponse, String> {
    let scratch_path = repository_root
        .join(".claude")
        .join("scratch")
        .join("orchestrator");
    match fs::symlink_metadata(&scratch_path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("orchestrator catalog root is not a regular directory".to_string());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RunCatalogResponse {
                active_runs: Vec::new(),
                archived_runs: Vec::new(),
                scanned_entries: 0,
                truncated: false,
            });
        }
        Err(error) => return Err(format!("cannot inspect orchestrator catalog root: {error}")),
    }
    let scratch = scratch_path
        .canonicalize()
        .map_err(|error| format!("cannot resolve repository orchestrator root: {error}"))?;
    if !scratch.starts_with(repository_root) || !scratch.is_dir() {
        return Err("orchestrator catalog root escapes the repository".to_string());
    }
    let archive_path = scratch.join("archive");
    let archive = match fs::symlink_metadata(&archive_path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("orchestrator archive is not a regular scoped directory".to_string());
            }
            let archive = archive_path
                .canonicalize()
                .map_err(|error| format!("cannot resolve orchestrator archive: {error}"))?;
            if archive.parent() != Some(scratch.as_path()) {
                return Err("orchestrator archive escapes the repository".to_string());
            }
            Some(archive)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("cannot inspect orchestrator archive: {error}")),
    };

    let mut scanned_entries = 0;
    let mut truncated = false;
    let active_paths = bounded_catalog_paths(
        &scratch,
        Some("archive"),
        &mut scanned_entries,
        &mut truncated,
    )?;
    let active_entries = active_paths
        .iter()
        .map(|path| load_catalog_entry(repository_root, &scratch, path, false))
        .collect::<Result<Vec<_>, _>>()?;
    let (mut active_runs, mut archived_runs): (Vec<_>, Vec<_>) = active_entries
        .into_iter()
        .partition(|entry| entry.status != "completed");

    if !truncated {
        if let Some(archive) = archive {
            let archived_paths =
                bounded_catalog_paths(&archive, None, &mut scanned_entries, &mut truncated)?;
            archived_runs.extend(
                archived_paths
                    .iter()
                    .map(|path| load_catalog_entry(repository_root, &archive, path, true))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
    }

    active_runs.sort_by(|left, right| left.run_id.cmp(&right.run_id));
    archived_runs.sort_by(|left, right| left.run_id.cmp(&right.run_id));
    Ok(RunCatalogResponse {
        active_runs,
        archived_runs,
        scanned_entries,
        truncated,
    })
}

fn bounded_catalog_paths(
    container: &Path,
    excluded_name: Option<&str>,
    scanned_entries: &mut usize,
    truncated: &mut bool,
) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let entries = fs::read_dir(container).map_err(|error| {
        format!(
            "cannot scan catalog directory {}: {error}",
            container.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read catalog entry: {error}"))?;
        if excluded_name.is_some_and(|name| entry.file_name() == name) {
            continue;
        }
        if *scanned_entries >= RUN_CATALOG_CAP {
            *truncated = true;
            break;
        }
        *scanned_entries += 1;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect catalog entry: {error}"))?;
        if file_type.is_symlink() || !file_type.is_dir() {
            return Err(format!(
                "catalog entry {} is not a regular run directory",
                entry.path().display()
            ));
        }
        let resolved = entry
            .path()
            .canonicalize()
            .map_err(|error| format!("cannot resolve catalog entry: {error}"))?;
        if resolved.parent() != Some(container) {
            return Err(format!(
                "catalog entry {} escapes its repository container",
                entry.path().display()
            ));
        }
        paths.push(resolved);
    }
    Ok(paths)
}

fn load_catalog_entry(
    repository_root: &Path,
    container: &Path,
    run_dir: &Path,
    archived: bool,
) -> Result<RunCatalogEntry, String> {
    if run_dir.parent() != Some(container) || !run_dir.is_dir() {
        return Err("catalog run is outside its exact repository container".to_string());
    }
    let run_id = run_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "catalog run has an invalid directory name".to_string())?
        .to_string();
    validate_run_id(&run_id)?;

    let manifest_path = catalog_file(run_dir, "manifest.json")?;
    let hot_resume_path = catalog_file(run_dir, "hot-resume.json")?;
    let scheduler_path = catalog_file(run_dir, SCHEDULER_FILE)?;
    let manifest: AllowedFileManifest = read_required_json(&manifest_path, "catalog manifest")?;
    let hot_resume: HotResumeState = read_required_json(&hot_resume_path, "catalog hot-resume")?;
    if manifest.run_id != run_id
        || hot_resume.run_id != run_id
        || manifest.branch != hot_resume.branch
        || manifest.repository_root.canonicalize().ok().as_ref()
            != Some(&repository_root.to_path_buf())
        || hot_resume.repository_root.canonicalize().ok().as_ref()
            != Some(&repository_root.to_path_buf())
    {
        return Err(format!(
            "catalog run {run_id} has corrupt repository or run identity"
        ));
    }
    let scheduler =
        SchedulerStore::open(scheduler_path.clone(), run_dir.to_path_buf(), Vec::new())?
            .snapshot()?;
    let completed_nodes = scheduler
        .nodes
        .values()
        .filter(|node| node.status == NodeStatus::Done)
        .count();
    let total_nodes = scheduler.nodes.len();
    if archived {
        let _completion_report = catalog_file(run_dir, "COMPLETION-REPORT.md")?;
        if total_nodes == 0 || completed_nodes != total_nodes {
            return Err(format!("archived run {run_id} is not completely scheduled"));
        }
    }
    let mut update_candidates = [manifest_path, hot_resume_path, scheduler_path]
        .iter()
        .map(|path| modified_ms(path))
        .collect::<Result<Vec<_>, _>>()?;
    for file_name in [
        PREFLIGHT_RESULT_FILE,
        RUN_APPROVAL_FILE,
        RECONCILIATION_RESULT_FILE,
        RELEASE_RESULT_FILE,
    ] {
        if let Some(recorded_at_ms) = catalog_recorded_at(run_dir, file_name)? {
            update_candidates.push(recorded_at_ms);
        }
    }
    let updated_at = update_candidates.into_iter().max().unwrap_or(0);
    Ok(RunCatalogEntry {
        run_id,
        repository_root: repository_root.to_path_buf(),
        branch: manifest.branch,
        plan_id: manifest.plan_id,
        plan_path: manifest.plan_path,
        status: if archived {
            "completed".to_string()
        } else {
            hot_resume.status
        },
        completed_nodes,
        total_nodes,
        updated_at,
    })
}

fn catalog_file(run_dir: &Path, name: &str) -> Result<PathBuf, String> {
    let path = run_dir.join(name);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("catalog run is missing {name}: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("catalog run file {name} is not a regular file"));
    }
    let resolved = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve catalog run file {name}: {error}"))?;
    if resolved.parent() != Some(run_dir) {
        return Err(format!("catalog run file {name} escapes its run directory"));
    }
    Ok(resolved)
}

fn read_required_json<T: DeserializeOwned>(path: &Path, label: &str) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|error| format!("cannot read {label}: {error}"))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("{label} is corrupt or incomplete: {error}"))
}

fn catalog_recorded_at(run_dir: &Path, file_name: &str) -> Result<Option<u64>, String> {
    let path = run_dir.join(file_name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(format!(
                    "catalog gate state {file_name} is not a regular file"
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot inspect catalog gate state: {error}")),
    }
    let resolved = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve catalog gate state: {error}"))?;
    if resolved.parent() != Some(run_dir) {
        return Err(format!("catalog gate state {file_name} escapes its run"));
    }
    let record: RecordedResult<Value> = read_required_json(&resolved, "catalog gate state")?;
    if record.schema_version != RECORDED_RESULT_SCHEMA_VERSION || record.recorded_at_ms == 0 {
        return Err(format!(
            "catalog gate state {file_name} has an invalid envelope"
        ));
    }
    Ok(Some(record.recorded_at_ms))
}

fn modified_ms(path: &Path) -> Result<u64, String> {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("cannot read catalog update time: {error}"))?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "catalog update time predates the Unix epoch".to_string())
        .map(|duration| duration.as_millis() as u64)
}

fn canonical_repository(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("repository root must be absolute".to_string());
    }
    let repository = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve repository root: {error}"))?;
    if !repository.is_dir() || !repository.join(".git").exists() {
        return Err("repository root must be an existing Git worktree".to_string());
    }
    Ok(repository)
}

fn open_scheduler(
    context: &ScopedContext,
    nodes: Vec<ScheduledNode>,
) -> Result<SchedulerStore, String> {
    let state_path = context.run_dir.join(SCHEDULER_FILE);
    if state_path.exists() {
        let resolved = state_path
            .canonicalize()
            .map_err(|error| format!("cannot resolve scheduler state: {error}"))?;
        if resolved.parent() != Some(context.run_dir.as_path()) || !resolved.is_file() {
            return Err("scheduler state escapes the scoped run directory".to_string());
        }
    } else if nodes.is_empty() {
        return Err("scheduler state is missing; refusing implicit reinitialization".to_string());
    }
    let scheduler = SchedulerStore::open(state_path, context.run_dir.clone(), nodes)?;
    let pending_revocations = scheduler.pending_legacy_revocations()?;
    for revocation in &pending_revocations {
        append_event(
            context,
            Some(revocation.node_id.clone()),
            "head-orchestrator",
            EventType::Warning,
            "LEGACY_UNATTESTED_CLAIM_REVOKED",
            json!({
                "workerId": revocation.worker_id,
                "fence": revocation.fence,
                "previousExpiresAtMs": revocation.previous_expires_at_ms,
                "authoritySchemaVersion": 1,
                "remedy": "worker must request a fresh collision-assessed scheduler admission"
            }),
        )?;
    }
    if !pending_revocations.is_empty() {
        scheduler.acknowledge_legacy_revocations(&pending_revocations)?;
    }
    Ok(scheduler)
}

fn bounded_event_tail(
    context: &ScopedContext,
    requested_offset: Option<u64>,
    requested_bytes: Option<u64>,
    requested_count: Option<usize>,
) -> Result<EventTailResponse, String> {
    let max_bytes = requested_bytes
        .unwrap_or(MAX_EVENT_TAIL_BYTES)
        .clamp(1, MAX_EVENT_TAIL_BYTES);
    let max_events = requested_count
        .unwrap_or(MAX_EVENT_TAIL_COUNT)
        .clamp(1, MAX_EVENT_TAIL_COUNT);
    let length = context
        .scope
        .events_path
        .metadata()
        .map_err(|error| format!("cannot inspect run events: {error}"))?
        .len();
    let requested = requested_offset.unwrap_or_else(|| length.saturating_sub(max_bytes));
    if requested > length {
        return Err("event offset is beyond the event log".to_string());
    }
    let floor = length.saturating_sub(max_bytes);
    let default_tail_was_truncated = requested_offset.is_none() && floor > 0;
    let start_offset = requested.max(floor);
    let mut batch = EventBus::new(context.scope.events_path.clone())
        .tail_from(start_offset)
        .map_err(|error| error.to_string())?;
    let count_truncated = batch.events.len() > max_events;
    if count_truncated {
        batch.events = batch.events.split_off(batch.events.len() - max_events);
    }
    Ok(EventTailResponse {
        events: batch.events,
        start_offset,
        next_offset: batch.next_offset,
        skipped_lines: batch.skipped_lines,
        trailing_partial: batch.trailing_partial,
        truncated: default_tail_was_truncated || requested < floor || count_truncated,
    })
}

fn append_event(
    context: &ScopedContext,
    node_id: Option<String>,
    worker: &str,
    event_type: EventType,
    message: &str,
    data: Value,
) -> Result<(), String> {
    validate_text("worker", worker)?;
    EventBus::new(context.scope.events_path.clone())
        .append(&RunEvent {
            ts: unix_ms().to_string(),
            run_id: context.scope.manifest.run_id.clone(),
            node_id,
            worker: worker.to_string(),
            event_type,
            msg: message.to_string(),
            data,
        })
        .map(|_| ())
        .map_err(|error| format!("state changed but audit event append failed: {error}"))
}

fn live_authority_lease(
    authority: &SchedulerAuthorityRuntime,
    scheduler: &SchedulerStore,
    node_id: &str,
) -> Result<NodeLease, String> {
    validate_text("nodeId", node_id)?;
    let lease = scheduler
        .snapshot()?
        .nodes
        .get(node_id)
        .and_then(|node| node.lease.clone())
        .ok_or_else(|| format!("node {node_id} has no live lease"))?;
    if lease.authority_epoch != Some(authority.epoch())
        || lease.authorization_id.is_none()
        || lease.git_baseline.is_none()
        || lease.expires_at_ms <= unix_ms()
    {
        return Err("worker lease belongs to a stale or legacy authority epoch".to_string());
    }
    Ok(lease)
}

fn native_worker_submission(
    authority: &SchedulerAuthorityRuntime,
    context: &ScopedContext,
    scheduler: &SchedulerStore,
    node_id: &str,
    supplied_artifacts: Vec<SupplementalArtifactRequest>,
) -> Result<(WorkerManifest, WorkerSubmission, NodeLease, u64), String> {
    const COMPLETION_VERIFICATION_LEASE_MS: u64 = 330_000;
    let current = live_authority_lease(authority, scheduler, node_id)?;
    let lease = scheduler.renew(
        node_id,
        &current.token,
        unix_ms(),
        COMPLETION_VERIFICATION_LEASE_MS,
    )?;
    append_event(
        context,
        Some(node_id.to_string()),
        &lease.worker_id,
        EventType::Heartbeat,
        "native completion verification window opened",
        json!({
            "fence": lease.fence,
            "expiresAtMs": lease.expires_at_ms,
            "authorityEpoch": lease.authority_epoch,
        }),
    )?;
    if supplied_artifacts.iter().any(|artifact| {
        matches!(
            artifact.kind,
            EvidenceKind::CommandOutput
                | EvidenceKind::ExitCode
                | EvidenceKind::GitDiff
                | EvidenceKind::DocumentDiff
        )
    }) {
        return Err(
            "command, exit-code and diff evidence are native-only completion artifacts".to_string(),
        );
    }
    let node = context
        .scope
        .manifest
        .nodes
        .iter()
        .find(|node| node.node_id == node_id)
        .ok_or_else(|| format!("node {node_id} is absent from the immutable run manifest"))?;
    let allowed_files = node
        .allowed_files
        .iter()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>();
    let baseline = lease
        .git_baseline
        .as_ref()
        .ok_or_else(|| "worker lease has no native Git baseline".to_string())?;
    let changed_files = native_node_changes(context, &allowed_files, baseline)?;
    if changed_files.is_empty() {
        return Err("completion has no native Git change for the node manifest".to_string());
    }

    let evidence_root = context.run_dir.join("evidence");
    let native_root = evidence_root.join(node_id).join("native");
    fs::create_dir_all(&native_root)
        .map_err(|error| format!("cannot create native evidence directory: {error}"))?;
    let mut artifacts = supplied_artifacts
        .into_iter()
        .map(|artifact| capture_artifact(&evidence_root, &artifact.path, artifact.kind))
        .collect::<Result<Vec<_>, String>>()?;
    artifacts.push(capture_native_diff(
        context,
        &changed_files,
        &native_root,
        matches!(
            node.evidence_profile,
            super::evidence::EvidenceProfile::Docs
        ),
    )?);
    let (mut command_artifacts, verification) = run_native_verifications(
        &context.repository_root,
        &native_root,
        &node.verification_commands,
    )?;
    artifacts.append(&mut command_artifacts);
    let manifest = WorkerManifest {
        run_id: context.scope.manifest.run_id.clone(),
        plan_id: context.scope.manifest.plan_id.clone(),
        node_id: node_id.to_string(),
        allowed_files,
        profile: node.evidence_profile.clone(),
        verification_commands: node.verification_commands.clone(),
    };
    let submission = WorkerSubmission {
        lease_token: lease.token.clone(),
        changed_files,
        artifacts,
        verification,
    };
    Ok((manifest, submission, lease, unix_ms()))
}

fn native_node_changes(
    context: &ScopedContext,
    allowed_files: &[String],
    baseline: &AdmissionGitBaseline,
) -> Result<Vec<String>, String> {
    if git_head(&context.repository_root)? != baseline.head_commit {
        return Err("Git HEAD changed after worker admission".to_string());
    }
    if outside_manifest_digest(&context.repository_root, allowed_files)?
        != baseline.outside_manifest_digest
    {
        return Err("completion refused an out-of-manifest Git state change".to_string());
    }
    let changed = git_dirty_paths(&context.repository_root)?
        .into_iter()
        .filter(|path| {
            allowed_files
                .iter()
                .any(|claim| path_matches_claim(path, claim))
        })
        .collect::<Vec<_>>();
    Ok(changed)
}

fn capture_native_diff(
    context: &ScopedContext,
    changed_files: &[String],
    native_root: &Path,
    document_diff: bool,
) -> Result<EvidenceArtifact, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(&context.repository_root)
        .args(["diff", "--binary", "HEAD", "--"])
        .args(changed_files)
        .env("GIT_OPTIONAL_LOCKS", "0");
    let output = command
        .output()
        .map_err(|error| format!("cannot capture native Git diff: {error}"))?;
    if !output.status.success() {
        return Err("native Git diff is unavailable".to_string());
    }
    let mut evidence = format!(
        "PERFECT PLANNER NATIVE DIFF\nHEAD {}\nFILES\n{}\n\n",
        git_head(&context.repository_root)?,
        changed_files.join("\n")
    )
    .into_bytes();
    evidence.extend_from_slice(&output.stdout);
    for changed in changed_files {
        let path = context.repository_root.join(changed);
        let tracked = Command::new("git")
            .arg("-C")
            .arg(&context.repository_root)
            .args(["ls-files", "--error-unmatch", "--", changed])
            .output()
            .map_err(|error| format!("cannot classify changed file {changed}: {error}"))?
            .status
            .success();
        if !tracked && path.is_file() {
            let bytes = fs::read(&path)
                .map_err(|error| format!("cannot capture untracked evidence {changed}: {error}"))?;
            if bytes.len() > 32 * 1024 * 1024 {
                return Err(format!(
                    "untracked evidence {changed} exceeds the size limit"
                ));
            }
            evidence.extend_from_slice(
                format!(
                    "\nUNTRACKED {changed} {} bytes sha256 {:x}\n",
                    bytes.len(),
                    Sha256::digest(&bytes)
                )
                .as_bytes(),
            );
        }
    }
    let path = native_root.join(if document_diff {
        "document.diff"
    } else {
        "git.diff"
    });
    atomic_replace(&path, &evidence)
        .map_err(|error| format!("cannot persist native Git diff: {error}"))?;
    capture_artifact(
        &context.run_dir.join("evidence"),
        &path,
        if document_diff {
            EvidenceKind::DocumentDiff
        } else {
            EvidenceKind::GitDiff
        },
    )
}

fn run_native_verifications(
    repository_root: &Path,
    native_root: &Path,
    commands: &[String],
) -> Result<(Vec<EvidenceArtifact>, Vec<VerificationResult>), String> {
    if commands.is_empty() {
        return Err("node has no machine-verifiable completion command".to_string());
    }
    let executable = PathBuf::from(PWSH_7);
    if !executable.is_file() {
        return Err("PowerShell 7 is unavailable for native verification".to_string());
    }
    let evidence_root = native_root
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "native evidence directory is malformed".to_string())?;
    let mut artifacts = Vec::new();
    let mut verification = Vec::new();
    for (index, verification_command) in commands.iter().enumerate() {
        validate_native_verification_command(verification_command)?;
        let mut command = Command::new(&executable);
        command
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
            .arg(verification_command)
            .current_dir(repository_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let output = run_bounded_capture(command, Duration::from_secs(300), MAX_PWSH_OUTPUT_BYTES)?;
        let mut log = Vec::new();
        log.extend_from_slice(b"COMMAND OUTPUT\n");
        log.extend_from_slice(&output.stdout);
        log.extend_from_slice(b"\nSTANDARD ERROR\n");
        log.extend_from_slice(&output.stderr);
        if log.len() <= 31 {
            log.extend_from_slice(b"(no textual output)\n");
        }
        let log_path = native_root.join(format!("verify-{index:03}.log"));
        atomic_replace(&log_path, &log)
            .map_err(|error| format!("cannot persist verification log: {error}"))?;
        let exit_code = output.status.code().unwrap_or(-1);
        let exit_path = native_root.join(format!("verify-{index:03}.exit.txt"));
        atomic_replace(&exit_path, format!("{exit_code}\n").as_bytes())
            .map_err(|error| format!("cannot persist verification exit code: {error}"))?;
        artifacts.push(capture_artifact(
            evidence_root,
            &log_path,
            EvidenceKind::CommandOutput,
        )?);
        artifacts.push(capture_artifact(
            evidence_root,
            &exit_path,
            EvidenceKind::ExitCode,
        )?);
        verification.push(VerificationResult {
            command_id: verification_command.clone(),
            exit_code,
            output_artifact: log_path.to_string_lossy().into_owned(),
        });
    }
    Ok((artifacts, verification))
}

fn validate_native_verification_command(command: &str) -> Result<(), String> {
    let trimmed = command.trim();
    if trimmed.is_empty()
        || trimmed.len() > 4096
        || trimmed
            .chars()
            .any(|character| matches!(character, '\r' | '\n' | '&' | '|' | '>' | '<' | '`'))
    {
        return Err("verification command contains an unsafe shell construct".to_string());
    }
    let executable = trimmed
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches('"')
        .to_ascii_lowercase();
    let executable = executable
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(executable.as_str())
        .trim_end_matches(".exe");
    if !matches!(
        executable,
        "cargo"
            | "npm"
            | "npx"
            | "node"
            | "pnpm"
            | "yarn"
            | "python"
            | "python3"
            | "py"
            | "pwsh"
            | "powershell"
            | "git"
    ) {
        return Err(format!(
            "verification executable {executable} is not on the native allowlist"
        ));
    }
    let lower = trimmed.to_ascii_lowercase();
    if matches!(executable, "pwsh" | "powershell")
        && (!lower.contains("-file") || lower.contains("-command"))
    {
        return Err("nested PowerShell verification must use -File, not -Command".to_string());
    }
    if executable == "git"
        && !matches!(
            lower.split_whitespace().nth(1).unwrap_or_default(),
            "diff" | "status" | "rev-parse" | "show"
        )
    {
        return Err("native Git verification is restricted to read-only subcommands".to_string());
    }
    Ok(())
}

#[cfg(test)]
fn validate_worker_identity(
    context: &ScopedContext,
    node_id: &str,
    token: &str,
    manifest: &WorkerManifest,
    submission: &WorkerSubmission,
) -> Result<(), String> {
    validate_manifest(manifest)?;
    validate_text("nodeId", node_id)?;
    validate_token(token)?;
    if manifest.run_id != context.scope.manifest.run_id
        || manifest.node_id != node_id
        || submission.lease_token != token
    {
        return Err("worker submission identity does not match the scoped lease".to_string());
    }
    let allowed: BTreeSet<_> = context
        .scope
        .manifest
        .allowed_files
        .iter()
        .map(|path| normalize_path(path))
        .collect();
    if let Some(outside) = manifest
        .allowed_files
        .iter()
        .find(|path| !allowed.contains(&normalize_path(Path::new(path))))
    {
        return Err(format!(
            "worker manifest file is outside the run manifest: {outside}"
        ));
    }
    Ok(())
}

fn verify_submission_artifacts(
    context: &ScopedContext,
    submission: &WorkerSubmission,
) -> Result<(), String> {
    if submission.artifacts.is_empty() {
        return Ok(());
    }
    let evidence_root = context
        .run_dir
        .join("evidence")
        .canonicalize()
        .map_err(|error| format!("cannot resolve scoped evidence directory: {error}"))?;
    if !evidence_root.starts_with(&context.run_dir) || !evidence_root.is_dir() {
        return Err("evidence directory escapes the scoped run".to_string());
    }
    for declared in &submission.artifacts {
        let captured = capture_artifact(&evidence_root, &declared.path, declared.kind.clone())?;
        if captured.sha256 != declared.sha256 || captured.bytes != declared.bytes {
            return Err(format!(
                "evidence metadata does not match captured artifact {}",
                declared.path.display()
            ));
        }
    }
    Ok(())
}

fn validate_initial_nodes(nodes: &[ScheduledNode]) -> Result<(), String> {
    let ids = nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<BTreeSet<_>>();
    if ids.len() != nodes.len() {
        return Err("scheduler initialization contains duplicate node IDs".to_string());
    }
    for node in nodes {
        validate_text("nodeId", &node.id)?;
        if node.status != NodeStatus::Ready
            || node.attempts != 0
            || node.lease.is_some()
            || node.stall_alarm_fence.is_some()
        {
            return Err(format!(
                "scheduler node {} must start READY without attempts, lease or alarm fence",
                node.id
            ));
        }
        for dependency in &node.depends_on {
            if dependency == &node.id || !ids.contains(dependency.as_str()) {
                return Err(format!(
                    "scheduler node {} has an invalid dependency {dependency}",
                    node.id
                ));
            }
        }
    }
    Ok(())
}

fn validate_delivery_text(request: &DeliveryRequest) -> Result<(), String> {
    for (field, value) in [
        ("title", request.title.as_str()),
        ("branch", request.branch.as_str()),
        ("commitSha", request.commit_sha.as_str()),
        ("finishedAt", request.finished_at.as_str()),
    ] {
        validate_text(field, value)?;
    }
    for value in request
        .pull_request_url
        .iter()
        .chain(request.merge_sha.iter())
    {
        validate_text("delivery value", value)?;
    }
    for leftover in &request.leftovers {
        validate_text("leftover id", &leftover.id)?;
        validate_text("leftover description", &leftover.what)?;
        validate_text("leftover location", &leftover.location)?;
        validate_text("leftover severity", &leftover.severity)?;
        validate_text("leftover suggested action", &leftover.suggested_next_action)?;
    }
    for change in &request.changes {
        validate_text("desired change", &change.desired)?;
        validate_text("change status", &change.status)?;
        if let Some(commit) = &change.actual_commit {
            validate_text("actual commit", commit)?;
        }
    }
    Ok(())
}

fn validate_run_id(run_id: &str) -> Result<(), String> {
    if run_id.is_empty()
        || run_id.len() > 120
        || run_id == "."
        || run_id == ".."
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("invalid runId".to_string());
    }
    Ok(())
}

fn validate_token(token: &str) -> Result<(), String> {
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("lease token must be a 64-character hexadecimal fence".to_string());
    }
    Ok(())
}

fn validate_text(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 4096 || value.contains(['\r', '\n', '\0']) {
        return Err(format!("invalid {field}"));
    }
    Ok(())
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Clone, Debug)]
struct Pwsh7SystemProbe {
    executable: PathBuf,
}

impl Pwsh7SystemProbe {
    fn fixed() -> Result<Self, String> {
        let executable = PathBuf::from(PWSH_7);
        if !executable.is_file() {
            return Err(format!(
                "PowerShell 7 is unavailable at the fixed path {}",
                executable.display()
            ));
        }
        Ok(Self { executable })
    }

    fn invoke(&self, script: &'static str, arguments: &[&Path]) -> Result<Vec<u8>, String> {
        let mut command = Command::new(&self.executable);
        command
            .arg("-NoLogo")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-CommandWithArgs")
            .arg(script);
        for argument in arguments {
            command.arg(external_windows_path(argument));
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        run_bounded(command, PWSH_TIMEOUT, MAX_PWSH_OUTPUT_BYTES)
    }
}

fn external_windows_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

#[cfg(windows)]
fn native_resource_snapshot(repository_root: &Path) -> Result<ResourceSnapshot, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::GetSystemTimes;

    fn filetime_value(value: FILETIME) -> u64 {
        ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
    }

    fn sample_cpu_times() -> Result<(u64, u64, u64), String> {
        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: all pointers refer to initialized writable FILETIME values for the
        // duration of this synchronous Windows API call.
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return Err(format!(
                "cannot sample native CPU times: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok((
            filetime_value(idle),
            filetime_value(kernel),
            filetime_value(user),
        ))
    }

    let first = sample_cpu_times()?;
    thread::sleep(Duration::from_millis(100));
    let second = sample_cpu_times()?;
    let idle_delta = second.0.saturating_sub(first.0);
    let total_delta = second
        .1
        .saturating_sub(first.1)
        .saturating_add(second.2.saturating_sub(first.2));
    let cpu_usage_percent = if total_delta == 0 {
        0.0
    } else {
        100.0 * total_delta.saturating_sub(idle_delta) as f64 / total_delta as f64
    };

    // SAFETY: MEMORYSTATUSEX is a plain C data structure; zero initialization followed by
    // setting dwLength is the contract required by GlobalMemoryStatusEx.
    let mut memory: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    memory.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `memory` remains valid and uniquely borrowed for the duration of the call.
    if unsafe { GlobalMemoryStatusEx(&mut memory) } == 0 {
        return Err(format!(
            "cannot sample native memory state: {}",
            std::io::Error::last_os_error()
        ));
    }

    let external_root = external_windows_path(repository_root);
    let disk_root = external_root
        .ancestors()
        .last()
        .ok_or_else(|| "cannot resolve repository disk root".to_string())?;
    let mut disk_root_wide = disk_root.as_os_str().encode_wide().collect::<Vec<_>>();
    disk_root_wide.push(0);
    let mut available = 0u64;
    // SAFETY: `disk_root_wide` is a NUL-terminated UTF-16 string and `available` is a valid
    // output pointer. The unused total outputs are intentionally null.
    if unsafe {
        GetDiskFreeSpaceExW(
            disk_root_wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(format!(
            "cannot sample repository disk capacity: {}",
            std::io::Error::last_os_error()
        ));
    }

    Ok(ResourceSnapshot {
        logical_cpu_count: std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1),
        cpu_usage_percent: cpu_usage_percent.clamp(0.0, 100.0) as f32,
        total_memory_bytes: memory.ullTotalPhys,
        available_memory_bytes: memory.ullAvailPhys,
        repository_disk_available_bytes: available,
    })
}

#[cfg(not(windows))]
fn native_resource_snapshot(_repository_root: &Path) -> Result<ResourceSnapshot, String> {
    Err("native resource probe is available only on Windows".to_string())
}

impl SystemProbe for Pwsh7SystemProbe {
    fn git_status_porcelain_v2(&self, repository_root: &Path) -> Result<String, String> {
        const SCRIPT: &str =
            "$ErrorActionPreference='Stop'; git -C $args[0] status --porcelain=v2 --untracked-files=all; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }";
        let bytes = self.invoke(SCRIPT, &[repository_root])?;
        String::from_utf8(bytes).map_err(|_| "git status returned non-UTF-8 output".to_string())
    }

    fn port_bindings(&self) -> Result<Vec<PortBinding>, String> {
        const SCRIPT: &str = r#"$ErrorActionPreference='Stop'; $processes=@{}; Get-CimInstance Win32_Process | ForEach-Object { $processes[[int]$_.ProcessId]=$_ }; $rows=@(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | ForEach-Object { $pidValue=[int]$_.OwningProcess; $p=$processes[$pidValue]; $started=0; try { $gp=Get-Process -Id $pidValue -ErrorAction Stop; $started=[DateTimeOffset]::new($gp.StartTime.ToUniversalTime()).ToUnixTimeMilliseconds() } catch {}; [pscustomobject]@{ port=[int]$_.LocalPort; address=[string]$_.LocalAddress; process=[pscustomobject]@{ pid=$pidValue; executablePath=[string]$p.ExecutablePath; startedAtEpochMs=[uint64]$started; commandLine=[string]$p.CommandLine } } }); $rows | ConvertTo-Json -Compress -Depth 5"#;
        let bytes = self.invoke(SCRIPT, &[])?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Vec::new());
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("cannot parse PowerShell port probe: {error}"))?;
        match value {
            Value::Array(_) => serde_json::from_value(value)
                .map_err(|error| format!("cannot decode PowerShell port probe: {error}")),
            Value::Object(_) => serde_json::from_value(value)
                .map(|row| vec![row])
                .map_err(|error| format!("cannot decode PowerShell port probe: {error}")),
            Value::Null => Ok(Vec::new()),
            _ => Err("PowerShell port probe returned an unexpected value".to_string()),
        }
    }

    fn resources(&self, repository_root: &Path) -> Result<ResourceSnapshot, String> {
        native_resource_snapshot(repository_root)
    }
}

struct BoundedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    overflowed: bool,
}

fn run_bounded(command: Command, timeout: Duration, max_bytes: usize) -> Result<Vec<u8>, String> {
    let output = run_bounded_capture(command, timeout, max_bytes)?;
    if !output.status.success() {
        return Err(format!(
            "fixed PowerShell probe failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

fn run_bounded_capture(
    mut command: Command,
    timeout: Duration,
    max_bytes: usize,
) -> Result<BoundedOutput, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start fixed PowerShell probe: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "PowerShell stdout was not captured".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "PowerShell stderr was not captured".to_string())?;
    let stdout_reader = thread::spawn(move || read_limited(stdout, max_bytes));
    let stderr_reader = thread::spawn(move || read_limited(stderr, max_bytes));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot inspect PowerShell probe: {error}"))?
        {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err("fixed PowerShell probe exceeded its time limit".to_string());
        }
        thread::sleep(Duration::from_millis(20));
    };
    let (stdout, stdout_overflowed) = stdout_reader
        .join()
        .map_err(|_| "PowerShell stdout reader panicked".to_string())??;
    let (stderr, stderr_overflowed) = stderr_reader
        .join()
        .map_err(|_| "PowerShell stderr reader panicked".to_string())??;
    let output = BoundedOutput {
        status,
        stdout,
        stderr,
        overflowed: stdout_overflowed || stderr_overflowed,
    };
    if output.overflowed {
        return Err("fixed PowerShell probe exceeded its output limit".to_string());
    }
    Ok(output)
}

fn read_limited(reader: impl Read, max_bytes: usize) -> Result<(Vec<u8>, bool), String> {
    let mut bytes = Vec::new();
    reader
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read PowerShell output: {error}"))?;
    let overflowed = bytes.len() > max_bytes;
    if overflowed {
        bytes.truncate(max_bytes);
    }
    Ok((bytes, overflowed))
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
                "pp-orchestrator-api-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            run_git(&path, &["init", "-q", "-b", "feature/api"]);
            run_git(
                &path,
                &["config", "user.email", "perfect-planner@example.invalid"],
            );
            run_git(&path, &["config", "user.name", "Perfect Planner Test"]);
            fs::write(path.join("seed.txt"), "seed\n").unwrap();
            run_git(&path, &["add", "seed.txt"]);
            run_git(&path, &["commit", "-q", "-m", "seed"]);
            let plan_path = path.join(".claude/scratch/perfect-plan/api-plan.json");
            fs::create_dir_all(plan_path.parent().unwrap()).unwrap();
            let plan = serde_json::json!({
                "title": "API test plan",
                "goal": "Exercise native commands",
                "approved": "yes @ test",
                "meta": { "number": "PP-API", "branch": "feature/api" },
                "spine": [{ "id": "P1", "title": "API" }],
                "vertebrae": [{
                    "id": "A01",
                    "spineId": "P1",
                    "title": "Exercise API",
                    "status": "pending",
                    "dependsOn": [],
                    "files": ["src/lib.rs"],
                    "resources": [],
                    "checklist": [{
                        "text": "API passes",
                        "built": false,
                        "tested": false,
                        "verify": "git status --short"
                    }]
                }]
            });
            fs::write(plan_path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
            Self(path)
        }

        fn create_scope(&self, run_id: &str) -> RunScope {
            RunScope::create(CreateRunScope {
                repository_root: self.0.clone(),
                run_id: run_id.to_string(),
                plan_path: PathBuf::from(".claude/scratch/perfect-plan/api-plan.json"),
                next_actions: vec!["claim node".to_string()],
            })
            .unwrap()
        }

        fn initialize_scheduler(&self, scope: &RunScope, completed: bool) {
            let node = ScheduledNode {
                id: "A01".to_string(),
                wave: 1,
                depends_on: Vec::new(),
                attempts: 0,
                status: NodeStatus::Ready,
                lease: None,
                stall_alarm_fence: None,
            };
            let scheduler = SchedulerStore::open(
                scope.root.join(SCHEDULER_FILE),
                scope.root.clone(),
                vec![node],
            )
            .unwrap();
            if completed {
                let lease = scheduler.claim("A01", "worker-1", 1, 10_000).unwrap();
                scheduler.complete("A01", &lease.token, 2).unwrap();
            }
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

    fn admitted_fixture(
        repository: &TempRepo,
        run_id: &str,
    ) -> (
        ScopedRunRequest,
        SchedulerAuthorityRuntime,
        PlannerRegistryStore,
    ) {
        let scope = repository.create_scope(run_id);
        repository.initialize_scheduler(&scope, false);
        let request_scope = ScopedRunRequest {
            repository_root: repository.0.clone(),
            run_id: run_id.to_string(),
        };
        let context = open_context(&request_scope).unwrap();
        persist_scoped_json(
            &context,
            PREFLIGHT_RESULT_FILE,
            &PreflightReport {
                disposition: PreflightDisposition::Ready,
                baseline: super::super::preflight::SystemBaseline {
                    repository_root: repository.0.canonicalize().unwrap(),
                    git_status_porcelain_v2: String::new(),
                    port_bindings: Vec::new(),
                    resources: ResourceSnapshot {
                        logical_cpu_count: 4,
                        cpu_usage_percent: 1.0,
                        total_memory_bytes: 8_000,
                        available_memory_bytes: 4_000,
                        repository_disk_available_bytes: 100_000,
                    },
                },
                conflicts: Vec::new(),
                unknown_conflicts: Vec::new(),
                stopped_processes: Vec::new(),
                reasons: Vec::new(),
            },
        )
        .unwrap();
        let app_data_name = format!("{run_id}-app-data");
        fs::write(
            repository.0.join(".git/info/exclude"),
            format!("{app_data_name}/\n"),
        )
        .unwrap();
        let app_data = repository.0.join(app_data_name);
        fs::create_dir_all(&app_data).unwrap();
        let authority = SchedulerAuthorityRuntime::open(&app_data).unwrap();
        let registry = PlannerRegistryStore::for_app_data(&app_data).unwrap();
        approve_run(&registry, request_scope.clone()).unwrap();
        admit_worker(
            &authority,
            &registry,
            AdmitWorkerApiRequest {
                scope: request_scope.clone(),
                node_id: "A01".to_string(),
            },
        )
        .unwrap();
        (request_scope, authority, registry)
    }

    fn record_ready_preflight(request_scope: &ScopedRunRequest) {
        let context = open_context(request_scope).unwrap();
        persist_scoped_json(
            &context,
            PREFLIGHT_RESULT_FILE,
            &PreflightReport {
                disposition: PreflightDisposition::Ready,
                baseline: super::super::preflight::SystemBaseline {
                    repository_root: request_scope.repository_root.canonicalize().unwrap(),
                    git_status_porcelain_v2: String::new(),
                    port_bindings: Vec::new(),
                    resources: ResourceSnapshot {
                        logical_cpu_count: 4,
                        cpu_usage_percent: 1.0,
                        total_memory_bytes: 8_000,
                        available_memory_bytes: 4_000,
                        repository_disk_available_bytes: 100_000,
                    },
                },
                conflicts: Vec::new(),
                unknown_conflicts: Vec::new(),
                stopped_processes: Vec::new(),
                reasons: Vec::new(),
            },
        )
        .unwrap();
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn scoped_context_accepts_only_direct_repository_run_child() {
        let repository = TempRepo::new();
        repository.create_scope("run-1");
        let context = open_context(&ScopedRunRequest {
            repository_root: repository.0.clone(),
            run_id: "run-1".to_string(),
        })
        .unwrap();
        assert_eq!(
            context.run_dir.parent().unwrap(),
            context.repository_root.join(".claude/scratch/orchestrator")
        );

        assert!(open_context(&ScopedRunRequest {
            repository_root: repository.0.clone(),
            run_id: "../run-1".to_string(),
        })
        .unwrap_err()
        .contains("invalid runId"));
    }

    #[test]
    fn manifest_tampering_fails_closed_before_identity_use() {
        let repository = TempRepo::new();
        let scope = repository.create_scope("run-1");
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&scope.manifest_path).unwrap()).unwrap();
        manifest["runId"] = Value::String("run-other".to_string());
        fs::write(
            &scope.manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        assert!(open_context(&ScopedRunRequest {
            repository_root: repository.0.clone(),
            run_id: "run-1".to_string(),
        })
        .unwrap_err()
        .contains("digest does not verify"));
    }

    #[test]
    fn worker_request_cannot_expand_the_run_manifest() {
        let repository = TempRepo::new();
        repository.create_scope("run-1");
        let context = open_context(&ScopedRunRequest {
            repository_root: repository.0.clone(),
            run_id: "run-1".to_string(),
        })
        .unwrap();
        let token = "a".repeat(64);
        let manifest = WorkerManifest {
            run_id: "run-1".to_string(),
            plan_id: "PP-001".to_string(),
            node_id: "A01".to_string(),
            allowed_files: vec!["src/outside.rs".to_string()],
            profile: super::super::evidence::EvidenceProfile::Headless,
            verification_commands: vec!["fixed-test-id".to_string()],
        };
        let submission = WorkerSubmission {
            lease_token: token.clone(),
            changed_files: vec![],
            artifacts: vec![],
            verification: vec![],
        };

        assert!(
            validate_worker_identity(&context, "A01", &token, &manifest, &submission)
                .unwrap_err()
                .contains("outside the run manifest")
        );
    }

    #[test]
    fn rejects_relative_repository_and_non_fence_token() {
        assert!(canonical_repository(Path::new("relative/repo"))
            .unwrap_err()
            .contains("must be absolute"));
        assert!(validate_token("not-a-fence").is_err());
        assert!(validate_text("field", "line\nbreak").is_err());
    }

    #[test]
    fn standalone_resource_probe_keeps_repository_scope() {
        let relative = orchestrator_resource_probe(ResourceProbeApiRequest {
            repository_root: PathBuf::from("relative/repo"),
        })
        .unwrap_err();
        assert!(relative.contains("must be absolute"));

        let directory = std::env::temp_dir().join(format!(
            "pp-resource-probe-not-git-{}",
            API_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let not_git = orchestrator_resource_probe(ResourceProbeApiRequest {
            repository_root: directory.clone(),
        })
        .unwrap_err();
        assert!(not_git.contains("existing Git worktree"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn fixed_pwsh_probe_receives_the_repository_argument() {
        let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri must have a repository parent")
            .canonicalize()
            .expect("repository root must canonicalize to its native Windows identity");
        let probe = Pwsh7SystemProbe::fixed().expect("PowerShell 7 probe must be installed");

        probe
            .git_status_porcelain_v2(&repository_root)
            .expect("the Git probe must receive repository_root as args[0]");
        let resources = probe
            .resources(&repository_root)
            .expect("the resource probe must receive repository_root as args[0]");

        assert!(resources.logical_cpu_count > 0);
        assert!(resources.total_memory_bytes > 0);
        assert!(resources.repository_disk_available_bytes > 0);

        let command_result =
            orchestrator_resource_probe(ResourceProbeApiRequest { repository_root })
                .expect("the canonical Tauri command path must preserve disk telemetry");
        assert!(command_result.resources.repository_disk_available_bytes > 0);
    }

    #[cfg(windows)]
    #[test]
    fn external_probe_paths_strip_verbatim_prefixes_without_changing_identity() {
        assert_eq!(
            external_windows_path(Path::new(r"\\?\C:\repos\perfect-planner")),
            PathBuf::from(r"C:\repos\perfect-planner")
        );
        assert_eq!(
            external_windows_path(Path::new(r"\\?\UNC\server\share\repo")),
            PathBuf::from(r"\\server\share\repo")
        );
    }

    #[test]
    fn scheduler_initialization_rejects_preclaimed_or_duplicate_nodes() {
        let node = ScheduledNode {
            id: "A01".to_string(),
            wave: 1,
            depends_on: Vec::new(),
            attempts: 0,
            status: NodeStatus::Ready,
            lease: None,
            stall_alarm_fence: None,
        };
        assert!(validate_initial_nodes(&[node.clone(), node.clone()])
            .unwrap_err()
            .contains("duplicate"));

        let mut precompleted = node;
        precompleted.status = NodeStatus::Done;
        assert!(validate_initial_nodes(&[precompleted])
            .unwrap_err()
            .contains("must start READY"));
    }

    #[test]
    fn brokered_admission_redacts_secrets_rejects_duplicate_claims_and_renews_natively() {
        let repository = TempRepo::new();
        let scope = repository.create_scope("run-authority-admission");
        repository.initialize_scheduler(&scope, false);
        let request_scope = ScopedRunRequest {
            repository_root: repository.0.clone(),
            run_id: "run-authority-admission".to_string(),
        };
        let context = open_context(&request_scope).unwrap();
        persist_scoped_json(
            &context,
            PREFLIGHT_RESULT_FILE,
            &PreflightReport {
                disposition: PreflightDisposition::Ready,
                baseline: super::super::preflight::SystemBaseline {
                    repository_root: repository.0.canonicalize().unwrap(),
                    git_status_porcelain_v2: String::new(),
                    port_bindings: Vec::new(),
                    resources: ResourceSnapshot {
                        logical_cpu_count: 4,
                        cpu_usage_percent: 1.0,
                        total_memory_bytes: 8_000,
                        available_memory_bytes: 4_000,
                        repository_disk_available_bytes: 100_000,
                    },
                },
                conflicts: Vec::new(),
                unknown_conflicts: Vec::new(),
                stopped_processes: Vec::new(),
                reasons: Vec::new(),
            },
        )
        .unwrap();
        let app_data = repository.0.join("app-data");
        fs::write(repository.0.join(".git/info/exclude"), "app-data/\n").unwrap();
        fs::create_dir_all(&app_data).unwrap();
        let authority = SchedulerAuthorityRuntime::open(&app_data).unwrap();
        let registry = PlannerRegistryStore::for_app_data(&app_data).unwrap();
        let unapproved = admit_worker(
            &authority,
            &registry,
            AdmitWorkerApiRequest {
                scope: request_scope.clone(),
                node_id: "A01".to_string(),
            },
        )
        .unwrap_err();
        assert!(unapproved.contains("explicit native run approval"));
        let approval = approve_run(&registry, request_scope.clone()).unwrap();
        assert_eq!(approval.collision_assessments.len(), 1);

        let lease = admit_worker(
            &authority,
            &registry,
            AdmitWorkerApiRequest {
                scope: request_scope.clone(),
                node_id: "A01".to_string(),
            },
        )
        .unwrap();
        assert_eq!(lease.authority_epoch, Some(authority.epoch()));
        assert!(lease.authorization_id.is_some());
        let encoded = serde_json::to_string(&lease).unwrap();
        assert!(!encoded.contains("token"));

        let duplicate = admit_worker(
            &authority,
            &registry,
            AdmitWorkerApiRequest {
                scope: request_scope.clone(),
                node_id: "A01".to_string(),
            },
        )
        .unwrap_err();
        assert!(duplicate.contains("not claimable"));

        let renewed = brokered_heartbeat(
            &authority,
            BrokeredHeartbeatApiRequest {
                scope: request_scope,
                node_id: "A01".to_string(),
            },
        )
        .unwrap();
        assert_eq!(renewed.fence, lease.fence);
        assert!(renewed.expires_at_ms >= lease.expires_at_ms);
        let public = open_scheduler(&context, Vec::new())
            .unwrap()
            .public_snapshot()
            .unwrap();
        assert!(!serde_json::to_string(&public).unwrap().contains("token"));
    }

    #[test]
    fn brokered_admission_rejects_a_tampered_persisted_approval() {
        let repository = TempRepo::new();
        let scope = repository.create_scope("run-tampered-approval");
        repository.initialize_scheduler(&scope, false);
        let request_scope = ScopedRunRequest {
            repository_root: repository.0.clone(),
            run_id: "run-tampered-approval".to_string(),
        };
        record_ready_preflight(&request_scope);
        let app_data = repository.0.join("tampered-approval-app-data");
        fs::write(
            repository.0.join(".git/info/exclude"),
            "tampered-approval-app-data/\n",
        )
        .unwrap();
        fs::create_dir_all(&app_data).unwrap();
        let authority = SchedulerAuthorityRuntime::open(&app_data).unwrap();
        let registry = PlannerRegistryStore::for_app_data(&app_data).unwrap();
        approve_run(&registry, request_scope.clone()).unwrap();

        let approval_path = scope.root.join(RUN_APPROVAL_FILE);
        let mut persisted: Value =
            serde_json::from_slice(&fs::read(&approval_path).unwrap()).unwrap();
        persisted["result"]["approvalDigest"] = Value::String("0".repeat(64));
        fs::write(
            &approval_path,
            serde_json::to_vec_pretty(&persisted).unwrap(),
        )
        .unwrap();

        let error = admit_worker(
            &authority,
            &registry,
            AdmitWorkerApiRequest {
                scope: request_scope,
                node_id: "A01".to_string(),
            },
        )
        .unwrap_err();
        assert!(error.contains("approval digest is invalid"), "{error}");
        assert!(open_scheduler(
            &open_context(&ScopedRunRequest {
                repository_root: repository.0.clone(),
                run_id: "run-tampered-approval".to_string(),
            })
            .unwrap(),
            Vec::new()
        )
        .unwrap()
        .snapshot()
        .unwrap()
        .nodes["A01"]
            .lease
            .is_none());
    }

    #[test]
    fn brokered_admission_rejects_collision_registry_change_after_approval() {
        let repository = TempRepo::new();
        let scope = repository.create_scope("run-stale-approval");
        repository.initialize_scheduler(&scope, false);
        let request_scope = ScopedRunRequest {
            repository_root: repository.0.clone(),
            run_id: "run-stale-approval".to_string(),
        };
        record_ready_preflight(&request_scope);
        let app_data = repository.0.join("stale-approval-app-data");
        fs::write(
            repository.0.join(".git/info/exclude"),
            "stale-approval-app-data/\n",
        )
        .unwrap();
        fs::create_dir_all(&app_data).unwrap();
        let authority = SchedulerAuthorityRuntime::open(&app_data).unwrap();
        let registry = PlannerRegistryStore::for_app_data(&app_data).unwrap();
        approve_run(&registry, request_scope.clone()).unwrap();

        let other_repository = TempRepo::new();
        let other_scope = other_repository.create_scope("run-other-participant");
        let other_context = open_context(&ScopedRunRequest {
            repository_root: other_repository.0.clone(),
            run_id: other_scope.manifest.run_id.clone(),
        })
        .unwrap();
        registry
            .prepare_manifest_collision_snapshot(
                planner_registration_seed(&other_context).unwrap(),
                "A01",
                unix_ms(),
                300_000,
            )
            .unwrap();

        let error = admit_worker(
            &authority,
            &registry,
            AdmitWorkerApiRequest {
                scope: request_scope,
                node_id: "A01".to_string(),
            },
        )
        .unwrap_err();
        assert!(
            error.contains("collision state changed after explicit approval"),
            "{error}"
        );
    }

    #[test]
    fn brokered_admission_refuses_preexisting_dirty_target_ownership() {
        let repository = TempRepo::new();
        let scope = repository.create_scope("run-dirty-target");
        repository.initialize_scheduler(&scope, false);
        fs::create_dir_all(repository.0.join("src")).unwrap();
        fs::write(repository.0.join("src/lib.rs"), "user-owned dirty bytes\n").unwrap();
        let request_scope = ScopedRunRequest {
            repository_root: repository.0.clone(),
            run_id: "run-dirty-target".to_string(),
        };
        let context = open_context(&request_scope).unwrap();
        persist_scoped_json(
            &context,
            PREFLIGHT_RESULT_FILE,
            &PreflightReport {
                disposition: PreflightDisposition::Ready,
                baseline: super::super::preflight::SystemBaseline {
                    repository_root: repository.0.canonicalize().unwrap(),
                    git_status_porcelain_v2: "? src/lib.rs".to_string(),
                    port_bindings: Vec::new(),
                    resources: ResourceSnapshot {
                        logical_cpu_count: 4,
                        cpu_usage_percent: 1.0,
                        total_memory_bytes: 8_000,
                        available_memory_bytes: 4_000,
                        repository_disk_available_bytes: 100_000,
                    },
                },
                conflicts: Vec::new(),
                unknown_conflicts: Vec::new(),
                stopped_processes: Vec::new(),
                reasons: Vec::new(),
            },
        )
        .unwrap();
        let app_data = repository.0.join("dirty-app-data");
        fs::create_dir_all(&app_data).unwrap();
        let registry = PlannerRegistryStore::for_app_data(&app_data).unwrap();

        let error = approve_run(&registry, request_scope).unwrap_err();
        assert!(error.contains("dirty target ownership"), "{error}");
        assert!(
            !registry.path().exists(),
            "collision state must not mutate after a dirty refusal"
        );
    }

    #[test]
    fn recovered_attempt_can_reclaim_partial_work_in_its_immutable_manifest() {
        let repository = TempRepo::new();
        let (request_scope, authority, registry) =
            admitted_fixture(&repository, "run-reclaim-partial");
        let context = open_context(&request_scope).unwrap();
        let scheduler = open_scheduler(&context, Vec::new()).unwrap();
        let first_lease = scheduler.snapshot().unwrap().nodes["A01"]
            .lease
            .clone()
            .expect("first authorized attempt must hold a lease");

        fs::create_dir_all(repository.0.join("src")).unwrap();
        fs::write(
            repository.0.join("src/lib.rs"),
            "pub fn partial_bounded_work() {}\n",
        )
        .unwrap();
        let recovered = scheduler
            .recover_stale_authority(authority.epoch(), u64::MAX)
            .unwrap();
        assert!(matches!(
            recovered.as_slice(),
            [ReapAction::Reassigned { .. }]
        ));

        record_ready_preflight(&request_scope);
        approve_run(&registry, request_scope.clone())
            .expect("fresh approval may retain partial work from an earlier bounded attempt");
        let second_lease = admit_worker(
            &authority,
            &registry,
            AdmitWorkerApiRequest {
                scope: request_scope,
                node_id: "A01".to_string(),
            },
        )
        .expect("the same immutable node manifest must be recoverable");

        assert!(second_lease.fence > first_lease.fence);
        assert_eq!(
            open_scheduler(&context, Vec::new())
                .unwrap()
                .snapshot()
                .unwrap()
                .nodes["A01"]
                .attempts,
            2
        );
    }

    #[test]
    fn completion_preserves_unrelated_preexisting_dirty_work() {
        let repository = TempRepo::new();
        fs::create_dir_all(repository.0.join("notes")).unwrap();
        fs::write(repository.0.join("notes/user.txt"), "user-owned\n").unwrap();
        let (scope, authority, _registry) = admitted_fixture(&repository, "run-preserve-unrelated");
        fs::create_dir_all(repository.0.join("src")).unwrap();
        fs::write(repository.0.join("src/lib.rs"), "pub fn bounded() {}\n").unwrap();

        let gate = authorize_fenced_completion(
            &authority,
            FencedCompletionApiRequest {
                scope,
                node_id: "A01".to_string(),
                artifacts: Vec::new(),
            },
        )
        .unwrap();
        assert!(gate.passed);
        assert_eq!(
            fs::read_to_string(repository.0.join("notes/user.txt")).unwrap(),
            "user-owned\n"
        );
    }

    #[test]
    fn completion_refuses_out_of_manifest_mutation_after_admission() {
        let repository = TempRepo::new();
        fs::create_dir_all(repository.0.join("notes")).unwrap();
        fs::write(repository.0.join("notes/user.txt"), "before\n").unwrap();
        let (scope, authority, _registry) = admitted_fixture(&repository, "run-outside-mutation");
        fs::write(repository.0.join("notes/user.txt"), "after\n").unwrap();
        fs::create_dir_all(repository.0.join("src")).unwrap();
        fs::write(repository.0.join("src/lib.rs"), "pub fn bounded() {}\n").unwrap();

        let error = authorize_fenced_completion(
            &authority,
            FencedCompletionApiRequest {
                scope,
                node_id: "A01".to_string(),
                artifacts: Vec::new(),
            },
        )
        .unwrap_err();
        assert!(error.contains("out-of-manifest"), "{error}");
    }

    #[test]
    fn completion_refuses_head_change_after_admission() {
        let repository = TempRepo::new();
        let (scope, authority, _registry) = admitted_fixture(&repository, "run-head-change");
        fs::write(repository.0.join("seed.txt"), "new committed head\n").unwrap();
        run_git(&repository.0, &["add", "seed.txt"]);
        run_git(&repository.0, &["commit", "-q", "-m", "advance head"]);
        fs::create_dir_all(repository.0.join("src")).unwrap();
        fs::write(repository.0.join("src/lib.rs"), "pub fn bounded() {}\n").unwrap();

        let error = authorize_fenced_completion(
            &authority,
            FencedCompletionApiRequest {
                scope,
                node_id: "A01".to_string(),
                artifacts: Vec::new(),
            },
        )
        .unwrap_err();
        assert!(error.contains("HEAD changed"), "{error}");
    }

    #[test]
    fn native_manifest_census_detects_cross_repository_resource_ownership() {
        let first = TempRepo::new();
        let second = TempRepo::new();
        let first_scope = first.create_scope("run-first-resource");
        let second_scope = second.create_scope("run-second-resource");
        let first_context = open_context(&ScopedRunRequest {
            repository_root: first.0.clone(),
            run_id: "run-first-resource".to_string(),
        })
        .unwrap();
        let second_context = open_context(&ScopedRunRequest {
            repository_root: second.0.clone(),
            run_id: "run-second-resource".to_string(),
        })
        .unwrap();
        let mut first_seed = planner_registration_seed(&first_context).unwrap();
        let mut second_seed = planner_registration_seed(&second_context).unwrap();
        for seed in [&mut first_seed, &mut second_seed] {
            seed.resources = vec!["mutex:shared-resource".to_string()];
            seed.nodes[0].resources = seed.resources.clone();
        }
        let app_data = first.0.join("census-app-data");
        fs::create_dir_all(&app_data).unwrap();
        let registry = PlannerRegistryStore::for_app_data(&app_data).unwrap();
        let now_ms = unix_ms();
        let first_snapshot = registry
            .prepare_manifest_collision_snapshot(first_seed, "A01", now_ms, 300_000)
            .unwrap();
        assert!(first_snapshot.conflict_ids.is_empty());
        let second_snapshot = registry
            .prepare_manifest_collision_snapshot(second_seed, "A01", now_ms, 300_000)
            .unwrap();
        assert_eq!(second_snapshot.conflict_ids.len(), 1);
        assert_ne!(
            first_scope.manifest.worktree_id,
            second_scope.manifest.worktree_id
        );
    }

    #[test]
    fn native_manifest_census_revives_only_its_exact_expired_registration() {
        let repository = TempRepo::new();
        let scope = repository.create_scope("run-expired-self-registration");
        let context = open_context(&ScopedRunRequest {
            repository_root: repository.0.clone(),
            run_id: scope.manifest.run_id.clone(),
        })
        .unwrap();
        let seed = planner_registration_seed(&context).unwrap();
        let app_data = repository.0.join("expired-self-census-app-data");
        fs::create_dir_all(&app_data).unwrap();
        let registry = PlannerRegistryStore::for_app_data(&app_data).unwrap();
        let now_ms = unix_ms();

        registry
            .prepare_manifest_collision_snapshot(seed.clone(), "A01", now_ms, 1_000)
            .unwrap();
        let recovered = registry
            .prepare_manifest_collision_snapshot(seed, "A01", now_ms + 1_001, 300_000)
            .unwrap();

        assert!(recovered.conflict_ids.is_empty());
        assert_eq!(recovered.planner_lease_generation, 2);
    }

    #[test]
    fn native_manifest_census_refuses_a_foreign_expired_registration() {
        let first = TempRepo::new();
        let second = TempRepo::new();
        let first_scope = first.create_scope("run-expired-foreign-first");
        let second_scope = second.create_scope("run-expired-foreign-second");
        let first_context = open_context(&ScopedRunRequest {
            repository_root: first.0.clone(),
            run_id: first_scope.manifest.run_id.clone(),
        })
        .unwrap();
        let second_context = open_context(&ScopedRunRequest {
            repository_root: second.0.clone(),
            run_id: second_scope.manifest.run_id.clone(),
        })
        .unwrap();
        let app_data = first.0.join("expired-foreign-census-app-data");
        fs::create_dir_all(&app_data).unwrap();
        let registry = PlannerRegistryStore::for_app_data(&app_data).unwrap();
        let now_ms = unix_ms();

        registry
            .prepare_manifest_collision_snapshot(
                planner_registration_seed(&first_context).unwrap(),
                "A01",
                now_ms,
                1_000,
            )
            .unwrap();
        let error = registry
            .prepare_manifest_collision_snapshot(
                planner_registration_seed(&second_context).unwrap(),
                "A01",
                now_ms + 1_001,
                300_000,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            crate::collision_assessor::registry::RegistryError::UnknownState(_)
        ));
    }

    #[test]
    fn persisted_gate_results_reload_after_context_restart() {
        use super::super::preflight::{PreflightDisposition, SystemBaseline};
        use super::super::release::{CiState, PullRequestState};

        let repository = TempRepo::new();
        let scope = repository.create_scope("run-restart");
        repository.initialize_scheduler(&scope, false);
        let request_scope = ScopedRunRequest {
            repository_root: repository.0.clone(),
            run_id: "run-restart".to_string(),
        };
        let context = open_context(&request_scope).unwrap();
        let preflight = PreflightReport {
            disposition: PreflightDisposition::Ready,
            baseline: SystemBaseline {
                repository_root: repository.0.canonicalize().unwrap(),
                git_status_porcelain_v2: String::new(),
                port_bindings: Vec::new(),
                resources: ResourceSnapshot {
                    logical_cpu_count: 8,
                    cpu_usage_percent: 10.0,
                    total_memory_bytes: 16_000,
                    available_memory_bytes: 8_000,
                    repository_disk_available_bytes: 100_000,
                },
            },
            conflicts: Vec::new(),
            unknown_conflicts: Vec::new(),
            stopped_processes: Vec::new(),
            reasons: Vec::new(),
        };
        let reconciliation = reconcile(&ReconciliationInput {
            plan_id: "PP-001".to_string(),
            nodes: Vec::new(),
            commits: Vec::new(),
            final_tree_files: Vec::new(),
            actual_tree_clean: true,
            uncommitted_files: Vec::new(),
            waivers: Vec::new(),
        });
        let release = evaluate_release(&ReleaseGateInput {
            dirty_worktree: false,
            merge_conflicts: Vec::new(),
            missing_evidence: Vec::new(),
            unplanned: Vec::new(),
            unproven: Vec::new(),
            orphaned: Vec::new(),
            ci: CiState::Passed,
            pushed: true,
            pull_request: PullRequestState::Approved,
        });
        persist_scoped_json(&context, PREFLIGHT_RESULT_FILE, &preflight).unwrap();
        persist_scoped_json(&context, RECONCILIATION_RESULT_FILE, &reconciliation).unwrap();
        persist_scoped_json(&context, RELEASE_RESULT_FILE, &release).unwrap();
        drop(context);

        let authority =
            SchedulerAuthorityRuntime::open(&repository.0.join("snapshot-app-data")).unwrap();
        let snapshot = pipeline_snapshot(
            &authority,
            SnapshotApiRequest {
                scope: request_scope,
                event_offset: None,
                max_event_bytes: None,
                max_events: None,
            },
        )
        .unwrap();
        assert_eq!(snapshot.preflight, Some(preflight));
        assert_eq!(snapshot.reconciliation, Some(reconciliation));
        assert_eq!(snapshot.release, Some(release));
        assert!(snapshot.preflight_recorded_at_ms.is_some());
        assert!(snapshot.reconciliation_recorded_at_ms.is_some());
        assert!(snapshot.release_recorded_at_ms.is_some());
    }

    #[test]
    fn corrupt_persisted_gate_state_fails_snapshot_closed() {
        let repository = TempRepo::new();
        let scope = repository.create_scope("run-corrupt");
        repository.initialize_scheduler(&scope, false);
        fs::write(scope.root.join(RELEASE_RESULT_FILE), b"{\"recordedAtMs\":").unwrap();

        let authority =
            SchedulerAuthorityRuntime::open(&repository.0.join("snapshot-app-data")).unwrap();
        let error = pipeline_snapshot(
            &authority,
            SnapshotApiRequest {
                scope: ScopedRunRequest {
                    repository_root: repository.0.clone(),
                    run_id: "run-corrupt".to_string(),
                },
                event_offset: None,
                max_event_bytes: None,
                max_events: None,
            },
        )
        .unwrap_err();
        assert!(error.contains("corrupt or incomplete"));
    }

    #[test]
    fn catalog_separates_active_and_completed_archives() {
        let repository = TempRepo::new();
        let active = repository.create_scope("run-active");
        repository.initialize_scheduler(&active, false);
        let completed = repository.create_scope("run-completed");
        repository.initialize_scheduler(&completed, true);
        fs::write(completed.root.join("COMPLETION-REPORT.md"), "complete").unwrap();
        let archive = repository.0.join(".claude/scratch/orchestrator/archive");
        fs::create_dir(&archive).unwrap();
        fs::rename(&completed.root, archive.join("run-completed")).unwrap();

        let catalog = orchestrator_run_catalog(RunCatalogApiRequest {
            repository_root: repository.0.clone(),
        })
        .unwrap();
        assert_eq!(catalog.active_runs.len(), 1);
        assert_eq!(catalog.active_runs[0].run_id, "run-active");
        assert_eq!(catalog.archived_runs.len(), 1);
        assert_eq!(catalog.archived_runs[0].run_id, "run-completed");
        assert_eq!(catalog.archived_runs[0].status, "completed");
        assert_eq!(catalog.archived_runs[0].completed_nodes, 1);
    }

    #[test]
    fn catalog_entry_rejects_path_outside_exact_container() {
        let repository = TempRepo::new();
        let scope = repository.create_scope("run-outside");
        repository.initialize_scheduler(&scope, false);
        let container = repository
            .0
            .join(".claude/scratch/orchestrator")
            .canonicalize()
            .unwrap();
        let outside = repository.0.join("outside-run");
        fs::create_dir(&outside).unwrap();

        assert!(load_catalog_entry(
            &repository.0.canonicalize().unwrap(),
            &container,
            &outside,
            false
        )
        .unwrap_err()
        .contains("outside its exact repository container"));
    }

    #[test]
    fn catalog_directory_scan_is_capped_at_five_hundred_entries() {
        let repository = TempRepo::new();
        let container = repository.0.join("catalog-cap");
        fs::create_dir(&container).unwrap();
        for index in 0..=RUN_CATALOG_CAP {
            fs::create_dir(container.join(format!("run-{index:03}"))).unwrap();
        }
        let container = container.canonicalize().unwrap();
        let mut scanned = 0;
        let mut truncated = false;
        let paths = bounded_catalog_paths(&container, None, &mut scanned, &mut truncated).unwrap();

        assert_eq!(paths.len(), RUN_CATALOG_CAP);
        assert_eq!(scanned, RUN_CATALOG_CAP);
        assert!(truncated);
    }

    #[test]
    fn native_lifecycle_completes_idempotently_with_audit_resume_and_catalog_proof() {
        let repository = TempRepo::new();
        let repository_root = repository.0.canonicalize().unwrap();
        let scope_request = ScopedRunRequest {
            repository_root: repository_root.clone(),
            run_id: "run-public-proof".to_string(),
        };
        let create = orchestrator_create_run(CreateRunApiRequest {
            repository_root: repository_root.clone(),
            run_id: scope_request.run_id.clone(),
            plan_path: PathBuf::from(".claude/scratch/perfect-plan/api-plan.json"),
            next_actions: vec!["claim A01".to_string()],
        })
        .unwrap();
        assert!(create.run_dir.starts_with(&repository_root));

        let context = open_context(&scope_request).unwrap();
        persist_scoped_json(
            &context,
            PREFLIGHT_RESULT_FILE,
            &PreflightReport {
                disposition: PreflightDisposition::Ready,
                baseline: super::super::preflight::SystemBaseline {
                    repository_root: repository_root.clone(),
                    git_status_porcelain_v2: String::new(),
                    port_bindings: Vec::new(),
                    resources: ResourceSnapshot {
                        logical_cpu_count: 4,
                        cpu_usage_percent: 1.0,
                        total_memory_bytes: 8_000,
                        available_memory_bytes: 4_000,
                        repository_disk_available_bytes: 100_000,
                    },
                },
                conflicts: Vec::new(),
                unknown_conflicts: Vec::new(),
                stopped_processes: Vec::new(),
                reasons: Vec::new(),
            },
        )
        .unwrap();
        let app_data = repository.0.join("public-proof-app-data");
        fs::write(
            repository.0.join(".git/info/exclude"),
            "public-proof-app-data/\n",
        )
        .unwrap();
        fs::create_dir_all(&app_data).unwrap();
        let authority = SchedulerAuthorityRuntime::open(&app_data).unwrap();
        let registry = PlannerRegistryStore::for_app_data(&app_data).unwrap();
        approve_run(&registry, scope_request.clone()).unwrap();
        admit_worker(
            &authority,
            &registry,
            AdmitWorkerApiRequest {
                scope: scope_request.clone(),
                node_id: "A01".to_string(),
            },
        )
        .unwrap();
        fs::create_dir_all(repository_root.join("src")).unwrap();
        fs::write(repository_root.join("src/lib.rs"), "pub fn proven() {}\n").unwrap();
        let gate = authorize_fenced_completion(
            &authority,
            FencedCompletionApiRequest {
                scope: scope_request.clone(),
                node_id: "A01".to_string(),
                artifacts: Vec::new(),
            },
        )
        .unwrap();
        assert!(gate.passed);
        let duplicate_gate = authorize_fenced_completion(
            &authority,
            FencedCompletionApiRequest {
                scope: scope_request.clone(),
                node_id: "A01".to_string(),
                artifacts: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(duplicate_gate, gate);

        let snapshot = pipeline_snapshot(
            &authority,
            SnapshotApiRequest {
                scope: scope_request.clone(),
                event_offset: None,
                max_event_bytes: None,
                max_events: None,
            },
        )
        .unwrap();
        assert_eq!(snapshot.hot_resume.status, "completed");
        assert_eq!(
            snapshot.hot_resume.last_completed_step.as_deref(),
            Some("A01")
        );
        assert!(snapshot.hot_resume.locked_files.is_empty());
        assert!(snapshot.hot_resume.next_actions.is_empty());
        assert_eq!(snapshot.scheduler.completions.len(), 1);
        let completion = &snapshot.scheduler.completions["A01"];
        assert!(completion.gate.passed);
        assert_eq!(completion.changed_files, vec!["src/lib.rs"]);
        assert!(!completion.artifacts.is_empty());
        assert!(!completion.verification.is_empty());
        let public_json = serde_json::to_string(&snapshot.scheduler).unwrap();
        assert!(!public_json.contains("token"));
        assert!(!public_json.contains("gitBaseline"));

        let audit = fs::read_to_string(&context.scope.audit_path).unwrap();
        let audit_records = audit
            .lines()
            .map(|line| {
                serde_json::from_str::<super::super::run_scope::RunAuditRecord>(line).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(audit_records.len(), 2);
        assert_eq!(audit_records[0].event.kind, "NODE_COMPLETION_PREPARED");
        assert_eq!(audit_records[1].event.kind, "NODE_COMPLETION_COMMITTED");
        assert_eq!(audit_records[0].sequence, 1);
        assert_eq!(audit_records[1].sequence, 2);
        assert_eq!(audit_records[1].previous_hash, audit_records[0].record_hash);
        assert!(create.run_dir.exists());

        let catalog = orchestrator_run_catalog(RunCatalogApiRequest {
            repository_root: repository_root.clone(),
        })
        .unwrap();
        assert!(catalog.active_runs.is_empty());
        assert_eq!(catalog.archived_runs.len(), 1);
        let completed = &catalog.archived_runs[0];
        assert_eq!(completed.run_id, "run-public-proof");
        assert_eq!(completed.status, "completed");
        assert_eq!(completed.completed_nodes, 1);
        assert_eq!(completed.total_nodes, 1);
        assert_eq!(completed.repository_root, repository_root);
    }

    #[test]
    fn snapshot_withholds_an_approval_detached_by_a_newer_preflight() {
        let repository = TempRepo::new();
        let repository_root = repository.0.canonicalize().unwrap();
        let scope_request = ScopedRunRequest {
            repository_root: repository_root.clone(),
            run_id: "run-stale-approval-projection".to_string(),
        };
        orchestrator_create_run(CreateRunApiRequest {
            repository_root: repository_root.clone(),
            run_id: scope_request.run_id.clone(),
            plan_path: PathBuf::from(".claude/scratch/perfect-plan/api-plan.json"),
            next_actions: vec!["preflight".to_string()],
        })
        .unwrap();
        let context = open_context(&scope_request).unwrap();
        let ready = PreflightReport {
            disposition: PreflightDisposition::Ready,
            baseline: super::super::preflight::SystemBaseline {
                repository_root: repository_root.clone(),
                git_status_porcelain_v2: String::new(),
                port_bindings: Vec::new(),
                resources: ResourceSnapshot {
                    logical_cpu_count: 4,
                    cpu_usage_percent: 1.0,
                    total_memory_bytes: 8_000,
                    available_memory_bytes: 4_000,
                    repository_disk_available_bytes: 100_000,
                },
            },
            conflicts: Vec::new(),
            unknown_conflicts: Vec::new(),
            stopped_processes: Vec::new(),
            reasons: Vec::new(),
        };
        persist_scoped_json(&context, PREFLIGHT_RESULT_FILE, &ready).unwrap();
        let app_data = repository.0.join("stale-approval-app-data");
        fs::write(
            repository.0.join(".git/info/exclude"),
            "stale-approval-app-data/\n",
        )
        .unwrap();
        fs::create_dir_all(&app_data).unwrap();
        let authority = SchedulerAuthorityRuntime::open(&app_data).unwrap();
        let registry = PlannerRegistryStore::for_app_data(&app_data).unwrap();
        approve_run(&registry, scope_request.clone()).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(2));
        persist_scoped_json(&context, PREFLIGHT_RESULT_FILE, &ready).unwrap();
        let snapshot = pipeline_snapshot(
            &authority,
            SnapshotApiRequest {
                scope: scope_request,
                event_offset: None,
                max_event_bytes: None,
                max_events: None,
            },
        )
        .unwrap();

        assert!(snapshot.preflight.is_some());
        assert!(snapshot.run_approval.is_none());
        assert!(snapshot.run_approval_recorded_at_ms.is_none());
    }
}
