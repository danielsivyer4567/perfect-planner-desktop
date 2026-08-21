use super::delivery::{deliver_run, DeliveryOutcome, DeliveryRequest};
use super::event_bus::{EventBus, EventType, RunEvent};
use super::evidence::capture_artifact;
use super::preflight::{
    DenyProcessAdapter, PortBinding, PreflightEngine, PreflightReport, PreflightRequest,
    ResourceSnapshot, SystemProbe,
};
use super::reconcile::{reconcile, ReconciliationInput, ReconciliationResult};
use super::release::{evaluate_release, ReleaseGateInput, ReleaseGateResult};
use super::run_scope::{AllowedFileManifest, CreateRunScope, HotResumeState, RunScope};
use super::scheduler::{
    NodeLease, NodeStatus, ReapAction, ScheduledNode, SchedulerState, SchedulerStore,
};
use super::worker::{
    validate_manifest, validate_submission, WorkerGateResult, WorkerManifest, WorkerSubmission,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SCHEDULER_FILE: &str = "scheduler.json";
const MAX_EVENT_TAIL_BYTES: u64 = 1024 * 1024;
const MAX_EVENT_TAIL_COUNT: usize = 500;
const MAX_PWSH_OUTPUT_BYTES: usize = 1024 * 1024;
const PWSH_TIMEOUT: Duration = Duration::from_secs(8);

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
    pub branch: String,
    pub allowed_files: Vec<PathBuf>,
    #[serde(default)]
    pub next_actions: Vec<String>,
    pub nodes: Vec<ScheduledNode>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRunApiResponse {
    pub run_dir: PathBuf,
    pub manifest: AllowedFileManifest,
    pub hot_resume: HotResumeState,
    pub scheduler: SchedulerState,
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
    pub scheduler: SchedulerState,
    pub event_tail: EventTailResponse,
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
    pub token: String,
    pub now_ms: u64,
    pub manifest: WorkerManifest,
    pub submission: WorkerSubmission,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FailureApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    pub node_id: String,
    pub token: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReapApiRequest {
    #[serde(flatten)]
    pub scope: ScopedRunRequest,
    pub now_ms: u64,
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
    pub now_ms: u64,
    pub manifest: WorkerManifest,
    pub submission: WorkerSubmission,
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

#[tauri::command]
pub fn orchestrator_create_run(
    request: CreateRunApiRequest,
) -> Result<CreateRunApiResponse, String> {
    let repository_root = canonical_repository(&request.repository_root)?;
    validate_run_id(&request.run_id)?;
    validate_text("branch", &request.branch)?;
    if request.nodes.is_empty() {
        return Err("scheduler requires at least one node".to_string());
    }
    validate_initial_nodes(&request.nodes)?;

    let scope = RunScope::create(CreateRunScope {
        repository_root: repository_root.clone(),
        run_id: request.run_id.clone(),
        branch: request.branch,
        allowed_files: request.allowed_files,
        next_actions: request.next_actions,
    })?;
    let context = open_context(&ScopedRunRequest {
        repository_root,
        run_id: request.run_id,
    })?;
    let scheduler = open_scheduler(&context, request.nodes)?.snapshot()?;
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
    let probe = Pwsh7SystemProbe::fixed()?;
    let engine = PreflightEngine::new(probe, DenyProcessAdapter);
    let report = engine.run(&PreflightRequest {
        repository_root: context.repository_root.clone(),
        required_ports: request.required_ports,
        process_allowlist: BTreeSet::new(),
        stop_allowlisted_conflicts: false,
    })?;
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

#[tauri::command]
pub fn orchestrator_pipeline_snapshot(
    request: SnapshotApiRequest,
) -> Result<PipelineSnapshotResponse, String> {
    let context = open_context(&request.scope)?;
    let hot_resume = context.scope.read_hot_resume()?;
    let scheduler = open_scheduler(&context, Vec::new())?.snapshot()?;
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
        event_tail,
    })
}

#[tauri::command]
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

#[tauri::command]
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
pub fn orchestrator_authorize_fenced_completion(
    request: FencedCompletionApiRequest,
) -> Result<WorkerGateResult, String> {
    validate_token(&request.token)?;
    let context = open_context(&request.scope)?;
    validate_worker_identity(
        &context,
        &request.node_id,
        &request.token,
        &request.manifest,
        &request.submission,
    )?;
    let scheduler = open_scheduler(&context, Vec::new())?;
    scheduler.authorize_commit(&request.node_id, &request.token, request.now_ms)?;
    verify_submission_artifacts(&context, &request.submission)?;
    let gate = validate_submission(&request.manifest, &request.submission)?;
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
    scheduler.complete(&request.node_id, &request.token, request.now_ms)?;
    append_event(
        &context,
        Some(request.node_id),
        "worker",
        EventType::NodeDone,
        "fenced completion persisted",
        serde_json::to_value(&gate).map_err(|error| error.to_string())?,
    )?;
    Ok(gate)
}

#[tauri::command]
pub fn orchestrator_record_failure(request: FailureApiRequest) -> Result<NodeStatus, String> {
    validate_token(&request.token)?;
    let context = open_context(&request.scope)?;
    let status = open_scheduler(&context, Vec::new())?.fail(&request.node_id, &request.token)?;
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
pub fn orchestrator_reap_expired(
    request: ReapApiRequest,
) -> Result<Vec<ReapActionResponse>, String> {
    let context = open_context(&request.scope)?;
    let actions = open_scheduler(&context, Vec::new())?.reap_expired(request.now_ms)?;
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
            &context,
            Some(node_id.clone()),
            worker_id,
            EventType::Reassign,
            "expired worker lease reaped",
            serde_json::to_value(response).map_err(|error| error.to_string())?,
        )?;
    }
    Ok(responses)
}

#[tauri::command]
pub fn orchestrator_validate_worker_submission(
    request: WorkerValidationApiRequest,
) -> Result<WorkerGateResult, String> {
    let context = open_context(&request.scope)?;
    validate_worker_identity(
        &context,
        &request.manifest.node_id,
        &request.submission.lease_token,
        &request.manifest,
        &request.submission,
    )?;
    open_scheduler(&context, Vec::new())?.authorize_commit(
        &request.manifest.node_id,
        &request.submission.lease_token,
        request.now_ms,
    )?;
    verify_submission_artifacts(&context, &request.submission)?;
    validate_submission(&request.manifest, &request.submission)
}

#[tauri::command]
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
    Ok(reconcile(&request.input))
}

#[tauri::command]
pub fn orchestrator_evaluate_release(
    request: ReleaseApiRequest,
) -> Result<ReleaseGateResult, String> {
    let _context = open_context(&request.scope)?;
    Ok(evaluate_release(&request.input))
}

#[tauri::command]
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
    let scratch = repository_root
        .join(".claude")
        .join("scratch")
        .join("orchestrator")
        .canonicalize()
        .map_err(|error| format!("cannot resolve repository orchestrator root: {error}"))?;
    if !scratch.starts_with(&repository_root) {
        return Err("orchestrator scratch root escapes the repository".to_string());
    }
    let run_dir = scratch
        .join(&request.run_id)
        .canonicalize()
        .map_err(|error| format!("cannot resolve scoped run directory: {error}"))?;
    if run_dir.parent() != Some(scratch.as_path()) || !run_dir.is_dir() {
        return Err(
            "run directory is not a direct child of the repository orchestrator root".to_string(),
        );
    }

    let manifest_path = contained_existing_file(&run_dir, "manifest.json")?;
    let audit_path = contained_existing_file(&run_dir, "audit.jsonl")?;
    let events_path = contained_existing_file(&run_dir, "events.jsonl")?;
    let hot_resume_path = contained_existing_file(&run_dir, "hot-resume.json")?;
    let manifest: AllowedFileManifest = serde_json::from_slice(
        &fs::read(&manifest_path).map_err(|error| format!("cannot read run manifest: {error}"))?,
    )
    .map_err(|error| format!("cannot parse run manifest: {error}"))?;
    if manifest.run_id != request.run_id
        || manifest.repository_root.canonicalize().ok().as_ref() != Some(&repository_root)
    {
        return Err("run manifest identity does not match its repository scope".to_string());
    }
    let scope = RunScope {
        root: run_dir.clone(),
        manifest_path,
        audit_path,
        events_path,
        hot_resume_path,
        manifest,
    };
    Ok(ScopedContext {
        repository_root,
        run_dir,
        scope,
    })
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

fn contained_existing_file(run_dir: &Path, name: &str) -> Result<PathBuf, String> {
    let path = run_dir
        .join(name)
        .canonicalize()
        .map_err(|error| format!("cannot resolve run file {name}: {error}"))?;
    if path.parent() != Some(run_dir) || !path.is_file() {
        return Err(format!("run file {name} escapes the scoped run directory"));
    }
    Ok(path)
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
    SchedulerStore::open(state_path, context.run_dir.clone(), nodes)
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
            .arg("-Command")
            .arg(script);
        for argument in arguments {
            command.arg(argument);
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
        const SCRIPT: &str = r#"$ErrorActionPreference='Stop'; $os=Get-CimInstance Win32_OperatingSystem; $cpu=@(Get-CimInstance Win32_Processor | ForEach-Object { [double]$_.LoadPercentage }); $drive=(Get-Item -LiteralPath $args[0]).PSDrive; [pscustomobject]@{ logicalCpuCount=[int][Environment]::ProcessorCount; cpuUsagePercent=[double](($cpu | Measure-Object -Average).Average); totalMemoryBytes=[uint64]$os.TotalVisibleMemorySize*1024; availableMemoryBytes=[uint64]$os.FreePhysicalMemory*1024; repositoryDiskAvailableBytes=[uint64]$drive.Free } | ConvertTo-Json -Compress"#;
        serde_json::from_slice(&self.invoke(SCRIPT, &[repository_root])?)
            .map_err(|error| format!("cannot decode PowerShell resource probe: {error}"))
    }
}

struct BoundedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    overflowed: bool,
}

fn run_bounded(
    mut command: Command,
    timeout: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
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
    if !output.status.success() {
        return Err(format!(
            "fixed PowerShell probe failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
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
            fs::create_dir_all(path.join(".git")).unwrap();
            Self(path)
        }

        fn create_scope(&self, run_id: &str) -> RunScope {
            RunScope::create(CreateRunScope {
                repository_root: self.0.clone(),
                run_id: run_id.to_string(),
                branch: "feature/api".to_string(),
                allowed_files: vec![PathBuf::from("src/lib.rs")],
                next_actions: vec!["claim node".to_string()],
            })
            .unwrap()
        }
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
    fn manifest_identity_mismatch_fails_closed() {
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
        .contains("identity does not match"));
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
}
