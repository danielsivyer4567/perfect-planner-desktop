export const ORCHESTRATOR_COMMANDS = {
  createRun: "orchestrator_create_run",
  preflight: "orchestrator_preflight_inspect",
  snapshot: "orchestrator_pipeline_snapshot",
  claim: "orchestrator_claim_node",
  heartbeat: "orchestrator_heartbeat",
  complete: "orchestrator_authorize_fenced_completion",
  fail: "orchestrator_record_failure",
  reap: "orchestrator_reap_expired",
  validateSubmission: "orchestrator_validate_worker_submission",
  reconcile: "orchestrator_reconcile",
  release: "orchestrator_evaluate_release",
  deliver: "orchestrator_deliver",
} as const;

export type OrchestratorCommand =
  (typeof ORCHESTRATOR_COMMANDS)[keyof typeof ORCHESTRATOR_COMMANDS];
export const ORCHESTRATOR_RICH_FIXTURE_COMMAND = "orchestrator_snapshot" as const;
export type OrchestratorFixtureCommand =
  | OrchestratorCommand
  | typeof ORCHESTRATOR_RICH_FIXTURE_COMMAND;

export type OrchestratorEventType =
  | "preflight"
  | "claim"
  | "heartbeat"
  | "progress"
  | "evidence"
  | "gate-pass"
  | "gate-fail"
  | "decision-required"
  | "node-done"
  | "reassign"
  | "warning"
  | "run-done";

export interface OrchestratorEvent {
  ts: string;
  runId: string;
  nodeId: string | null;
  worker: string;
  type: OrchestratorEventType;
  msg: string;
  data: unknown;
}

export interface ProcessIdentity {
  pid: number;
  executablePath: string;
  startedAtEpochMs: number;
  commandLine: string;
}

export interface PortBinding {
  port: number;
  address: string;
  process: ProcessIdentity;
}

export interface ResourceSnapshot {
  logicalCpuCount: number;
  cpuUsagePercent: number;
  totalMemoryBytes: number;
  availableMemoryBytes: number;
  repositoryDiskAvailableBytes: number;
}

export interface SystemBaseline {
  repositoryRoot: string;
  gitStatusPorcelainV2: string;
  portBindings: PortBinding[];
  resources: ResourceSnapshot;
}

export type PreflightDisposition =
  | "ready"
  | "decisionRequired"
  | "stoppedAllowlistedConflicts";

export interface PreflightReport {
  disposition: PreflightDisposition;
  baseline: SystemBaseline;
  conflicts: PortBinding[];
  unknownConflicts: PortBinding[];
  stoppedProcesses: ProcessIdentity[];
  reasons: string[];
}

export type EvidenceProfile = "ui" | "headless" | "migration" | "docs";
export type EvidenceKind =
  | "before-screenshot"
  | "after-screenshot"
  | "command-output"
  | "exit-code"
  | "git-diff"
  | "ocr-report"
  | "migration-status"
  | "document-diff";

export interface EvidenceArtifact {
  kind: EvidenceKind;
  path: string;
  sha256: string;
  bytes: number;
}

export interface VerificationResult {
  commandId: string;
  exitCode: number;
  outputArtifact: string;
}

export interface EvidenceGateResult {
  passed: boolean;
  missing: EvidenceKind[];
  failedCommands: string[];
  hashes: string[];
}

export interface WorkerManifest {
  runId: string;
  planId: string;
  nodeId: string;
  allowedFiles: string[];
  profile: EvidenceProfile;
  verificationCommands: string[];
}

export interface WorkerSubmission {
  leaseToken: string;
  changedFiles: string[];
  artifacts: EvidenceArtifact[];
  verification: VerificationResult[];
}

export interface WorkerGateResult {
  passed: boolean;
  manifestEscapes: string[];
  evidence: EvidenceGateResult;
}

export interface NodeLease {
  nodeId: string;
  workerId: string;
  token: string;
  fence: number;
  expiresAtMs: number;
}

export type ScheduledNodeStatus = "READY" | "RUNNING" | "DONE" | "BLOCKED";

export interface ScheduledNode {
  id: string;
  title?: string;
  wave: number;
  dependsOn: string[];
  attempts: number;
  status: ScheduledNodeStatus;
  lease: NodeLease | null;
  stallAlarmFence: number | null;
  profile?: EvidenceProfile;
  evidence?: EvidenceArtifact[];
  allowedFiles?: string[];
  verification?: VerificationResult[];
}

export interface SchedulerState {
  nextFence: number;
  nodes: Record<string, ScheduledNode>;
}

export type ReapAction =
  | {
      action: "REASSIGNED";
      nodeId: string;
      workerId: string;
      preservedEvidence: string | null;
    }
  | { action: "BLOCKED"; nodeId: string; workerId: string };

export interface AllowedFileManifest {
  schemaVersion: number;
  runId: string;
  repositoryRoot: string;
  branch: string;
  allowedFiles: string[];
}

export interface HotResumeState {
  schemaVersion: number;
  runId: string;
  repositoryRoot: string;
  branch: string;
  status: string;
  lastCompletedStep: string | null;
  lockedFiles: string[];
  nextActions: string[];
}

export interface EventTailResponse {
  events: OrchestratorEvent[];
  startOffset: number;
  nextOffset: number;
  skippedLines: number;
  trailingPartial: boolean;
  truncated: boolean;
}

export interface PipelineSnapshotResponse {
  manifest: AllowedFileManifest;
  hotResume: HotResumeState;
  scheduler: SchedulerState;
  eventTail: EventTailResponse;
}

export type ViolationCategory = "UNPLANNED" | "UNPROVEN" | "ORPHANED" | "FATAL";

export interface ReconciliationPlanNode {
  nodeId: string;
  manifestFiles: string[];
  declaredOutputs: string[];
}

export interface CommitHunk {
  file: string;
}

export interface CommitRecord {
  commitId: string;
  message: string;
  hunks: CommitHunk[];
}

export interface ReconciliationWaiver {
  name: string;
  category: ViolationCategory;
  violationId: string;
}

export interface ReconciliationInput {
  planId: string;
  nodes: ReconciliationPlanNode[];
  commits: CommitRecord[];
  finalTreeFiles: string[];
  actualTreeClean: boolean;
  uncommittedFiles: string[];
  waivers: ReconciliationWaiver[];
}

export interface ReconciliationViolation {
  violationId: string;
  category: ViolationCategory;
  summary: string;
  planId?: string;
  nodeId?: string;
  commitId?: string;
  file?: string;
  waivedBy: string[];
}

export interface WaiverAudit {
  name: string;
  category: ViolationCategory;
  violationId: string;
  applied: boolean;
}

export interface ReconciliationResult {
  passed: boolean;
  unplanned: ReconciliationViolation[];
  unproven: ReconciliationViolation[];
  orphaned: ReconciliationViolation[];
  fatal: ReconciliationViolation[];
  waivers: WaiverAudit[];
}

export interface ChangeComparison {
  id: string;
  nodeId: string | null;
  desired: string;
  actualCommit: string | null;
  status: "succeeded" | "missing" | "unplanned" | "unproven" | "orphaned" | "fatal";
  details?: string[];
}

export type CiState =
  | "NOT_RUN"
  | "PASSED"
  | "CODE_FAILURE"
  | "INFRASTRUCTURE_FAILURE";
export type PullRequestState =
  | "NOT_CREATED"
  | "OPEN"
  | "APPROVED"
  | "CHANGES_REQUESTED"
  | "MERGED";

export type ReleaseIssueKind =
  | "DIRTY_WORKTREE"
  | "MERGE_CONFLICT"
  | "MISSING_EVIDENCE"
  | "RECONCILIATION"
  | "CI_NOT_RUN"
  | "CODE_FAILURE"
  | "CI_INFRASTRUCTURE_FAILURE"
  | "NOT_PUSHED"
  | "REVIEW_REQUIRED"
  | "CHANGES_REQUESTED";

export interface ReleaseIssue {
  kind: ReleaseIssueKind;
  message: string;
  decisionRequired: boolean;
}

export interface ReleaseGateResult {
  readyForPr: boolean;
  readyToMerge: boolean;
  merged: boolean;
  issues: ReleaseIssue[];
}

export interface ReleaseGateInput {
  dirtyWorktree: boolean;
  mergeConflicts: string[];
  missingEvidence: string[];
  unplanned: string[];
  unproven: string[];
  orphaned: string[];
  ci: CiState;
  pushed: boolean;
  pullRequest: PullRequestState;
}

export interface DeliveryChange {
  desired: string;
  actualCommit: string | null;
  status: string;
}

export interface LeftoverItem {
  id: string;
  what: string;
  location: string;
  severity: string;
  suggestedNextAction: string;
}

export interface DeliveryRequest {
  runId: string;
  planId: string;
  title: string;
  branch: string;
  commitSha: string;
  pullRequestUrl: string | null;
  mergeSha: string | null;
  finishedAt: string;
  changes: DeliveryChange[];
  leftovers: LeftoverItem[];
}

export interface DeliveryOutcome {
  handoverDir: string;
  archiveDir: string;
  checklistLine: string;
  leftoversCount: number;
}

export type PipelineRunStatus =
  | "pending"
  | "preflight"
  | "running"
  | "decision-required"
  | "release-gate"
  | "blocked"
  | "failed"
  | "completed";

export interface PipelineRunIdentity {
  organizationId: string;
  repositoryId: string;
  repositoryRoot: string;
  worktreePath: string;
  branch: string;
  runId: string;
  planId: string;
  title: string;
}

export interface PipelineRunSummary extends PipelineRunIdentity {
  status: PipelineRunStatus;
  completedNodes: number;
  totalNodes: number;
  updatedAt: string;
}

export interface PipelineWarning {
  id: string;
  severity: "warning" | "critical";
  message: string;
  decisionRequired: boolean;
  nodeId: string | null;
  createdAt: string;
  issueKind?: ReleaseIssueKind;
}

export type PipelineStageId =
  | "preflight"
  | "scope"
  | "plan"
  | "execution"
  | "reconciliation"
  | "release"
  | "delivery";

export interface PipelineStage {
  id: PipelineStageId;
  label: string;
  status: "waiting" | "running" | "passed" | "blocked" | "failed";
  summary: string;
}

export interface OrchestratorSnapshot {
  nowMs: number;
  run: PipelineRunSummary;
  stages: PipelineStage[];
  preflight: PreflightReport | null;
  scheduler: SchedulerState;
  reconciliation: ReconciliationResult | null;
  changes: ChangeComparison[];
  release: ReleaseGateResult | null;
  delivery: DeliveryOutcome | null;
  leftovers: LeftoverItem[];
  warnings: PipelineWarning[];
  events: OrchestratorEvent[];
  activeRuns: PipelineRunSummary[];
  completedRuns: PipelineRunSummary[];
}

export interface PipelineSnapshotSeed {
  organizationId?: string;
  repositoryId?: string;
  worktreePath?: string;
  planId?: string;
  title?: string;
}

/**
 * Lift the bounded native transport into the richer console view without inventing gate proof.
 * Only the manifest, scheduler, hot-resume state, and event tail can become positive UI state;
 * absent preflight/reconciliation/release/delivery records remain explicitly unrecorded.
 */
export function consoleSnapshotFromPipelineResponse(
  response: PipelineSnapshotResponse,
  seed: PipelineSnapshotSeed = {}
): OrchestratorSnapshot {
  const nodes = Object.values(response.scheduler.nodes);
  const completedNodes = nodes.filter((node) => node.status === "DONE").length;
  const blockedNodes = nodes.filter((node) => node.status === "BLOCKED").length;
  const runningNodes = nodes.filter((node) => node.status === "RUNNING").length;
  const suppliedStatus = response.hotResume.status.trim().toLocaleLowerCase();
  const knownStatuses = new Set<PipelineRunStatus>([
    "pending",
    "preflight",
    "running",
    "decision-required",
    "release-gate",
    "blocked",
    "failed",
    "completed",
  ]);
  const status: PipelineRunStatus = knownStatuses.has(suppliedStatus as PipelineRunStatus)
    ? (suppliedStatus as PipelineRunStatus)
    : blockedNodes
      ? "blocked"
      : runningNodes
        ? "running"
        : "pending";
  const latestEvent = [...response.eventTail.events].sort((left, right) =>
    right.ts.localeCompare(left.ts)
  )[0];
  const updatedAt = latestEvent?.ts || new Date(0).toISOString();
  const run: PipelineRunSummary = {
    organizationId: seed.organizationId || "not-recorded",
    repositoryId: seed.repositoryId || response.manifest.repositoryRoot,
    repositoryRoot: response.manifest.repositoryRoot,
    worktreePath: seed.worktreePath || response.manifest.repositoryRoot,
    branch: response.manifest.branch,
    runId: response.manifest.runId,
    planId: seed.planId || "not-recorded",
    title: seed.title || response.manifest.runId,
    status,
    completedNodes,
    totalNodes: nodes.length,
    updatedAt,
  };
  const warnings: PipelineWarning[] = response.eventTail.events
    .filter((event) => event.type === "warning" || event.type === "decision-required")
    .map((event, index) => ({
      id: `transport-${event.type}-${event.nodeId || "run"}-${event.ts}-${index}`,
      severity: event.type === "decision-required" ? "critical" : "warning",
      message: event.msg,
      decisionRequired: event.type === "decision-required",
      nodeId: event.nodeId,
      createdAt: event.ts,
    }));
  const stages: PipelineStage[] = [
    {
      id: "preflight",
      label: "System preflight",
      status: "waiting",
      summary: "The bounded snapshot does not contain a preflight report.",
    },
    {
      id: "scope",
      label: "Isolated execution scope",
      status: "passed",
      summary: `${response.manifest.allowedFiles.length} manifest files are repository-fenced.`,
    },
    {
      id: "plan",
      label: "Perfect Plan",
      status: nodes.length ? "passed" : "blocked",
      summary: nodes.length
        ? `${nodes.length} scheduler nodes are persisted.`
        : "No scheduler nodes are persisted.",
    },
    {
      id: "execution",
      label: "Scheduled workers",
      status: blockedNodes
        ? "blocked"
        : nodes.length > 0 && completedNodes === nodes.length
          ? "passed"
          : runningNodes
            ? "running"
            : "waiting",
      summary: `${completedNodes}/${nodes.length} done · ${runningNodes} running · ${blockedNodes} blocked.`,
    },
    {
      id: "reconciliation",
      label: "Planned vs actual",
      status: "waiting",
      summary: "No reconciliation result is present in the bounded snapshot.",
    },
    {
      id: "release",
      label: "Pre-merge release gate",
      status: "waiting",
      summary: "No release result is present in the bounded snapshot.",
    },
    {
      id: "delivery",
      label: "Delivery and handover",
      status: status === "completed" ? "passed" : "waiting",
      summary:
        status === "completed"
          ? "Hot-resume state records the run as completed."
          : "No completed delivery is recorded.",
    },
  ];
  return {
    nowMs: Date.now(),
    run,
    stages,
    preflight: null,
    scheduler: response.scheduler,
    reconciliation: null,
    changes: [],
    release: null,
    delivery: null,
    leftovers: [],
    warnings,
    events: response.eventTail.events,
    activeRuns: status === "completed" ? [] : [run],
    completedRuns: status === "completed" ? [run] : [],
  };
}

export interface ScopedRunRequest {
  repositoryRoot: string;
  runId: string;
}

export interface CreateRunRequest extends ScopedRunRequest {
  branch: string;
  allowedFiles: string[];
  nextActions: string[];
  nodes: ScheduledNode[];
}

export interface CreateRunResult {
  runDir: string;
  manifest: AllowedFileManifest;
  hotResume: HotResumeState;
  scheduler: SchedulerState;
}

export interface OrchestratorPreflightRequest extends ScopedRunRequest {
  requiredPorts: number[];
}

export interface SnapshotRequest extends ScopedRunRequest {
  eventOffset?: number | null;
  maxEventBytes?: number | null;
  maxEvents?: number | null;
}

export interface ClaimNodeRequest extends ScopedRunRequest {
  nodeId: string;
  workerId: string;
  nowMs: number;
  leaseMs: number;
}

export interface HeartbeatRequest extends ScopedRunRequest {
  nodeId: string;
  token: string;
  nowMs: number;
  leaseMs: number;
}

export interface CompleteNodeRequest extends ScopedRunRequest {
  nodeId: string;
  token: string;
  nowMs: number;
  manifest: WorkerManifest;
  submission: WorkerSubmission;
}

export interface FailNodeRequest extends ScopedRunRequest {
  nodeId: string;
  token: string;
}

export interface ReapRequest extends ScopedRunRequest {
  nowMs: number;
}

export interface ValidateSubmissionRequest extends ScopedRunRequest {
  nowMs: number;
  manifest: WorkerManifest;
  submission: WorkerSubmission;
}

export interface ReconcileRequest extends ScopedRunRequest {
  input: ReconciliationInput;
}

export interface ReleaseRequest extends ScopedRunRequest {
  input: ReleaseGateInput;
}

export interface DeliverRequest extends ScopedRunRequest {
  delivery: DeliveryRequest;
}

type FixtureSuccess<T> = { ok: true; value: T };
type FixtureFailure = { ok: false; error: string };
export type FixtureResponse<T = unknown> = FixtureSuccess<T> | FixtureFailure;

export interface OrchestratorPipelineFixture {
  version: 1;
  responses: Partial<Record<OrchestratorFixtureCommand, FixtureResponse>>;
}

declare global {
  interface Window {
    readonly __ORCHESTRATOR_PIPELINE__?: Readonly<OrchestratorPipelineFixture>;
    readonly __TAURI_INTERNALS__?: unknown;
  }
}

const inTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function requireText(value: string, field: string): string {
  const normalized = value.trim();
  if (!normalized) throw new Error(`${field} is required`);
  return normalized;
}

function requireRun<T extends { runId: string }>(request: T): T {
  requireText(request.runId, "runId");
  return request;
}

function requireScope<T extends ScopedRunRequest>(request: T): T {
  requireRun(request);
  requireText(request.repositoryRoot, "repositoryRoot");
  return request;
}

function cloneFixtureValue<T>(value: T): T {
  if (typeof structuredClone === "function") return structuredClone(value);
  return JSON.parse(JSON.stringify(value)) as T;
}

function isDeepFrozen(value: unknown, seen = new Set<object>()): boolean {
  if (value === null || (typeof value !== "object" && typeof value !== "function")) return true;
  const object = value as object;
  if (seen.has(object)) return true;
  seen.add(object);
  if (!Object.isFrozen(object)) return false;
  return Reflect.ownKeys(object).every((key) =>
    isDeepFrozen((object as Record<PropertyKey, unknown>)[key], seen)
  );
}

/** Create the immutable browser-development fixture required by the fail-closed client. */
export function freezeOrchestratorPipelineFixture(
  fixture: OrchestratorPipelineFixture
): Readonly<OrchestratorPipelineFixture> {
  const seen = new Set<object>();
  const freeze = (value: unknown): void => {
    if (value === null || (typeof value !== "object" && typeof value !== "function")) return;
    const object = value as object;
    if (seen.has(object)) return;
    seen.add(object);
    Reflect.ownKeys(object).forEach((key) =>
      freeze((object as Record<PropertyKey, unknown>)[key])
    );
    Object.freeze(object);
  };
  freeze(fixture);
  return fixture;
}

function requireBrowserFixture(command: OrchestratorFixtureCommand): Readonly<OrchestratorPipelineFixture> {
  if (typeof window === "undefined") {
    throw new Error(`Cannot run ${command}: no Tauri runtime or browser fixture is available`);
  }
  const fixture = window.__ORCHESTRATOR_PIPELINE__;
  if (!fixture) {
    throw new Error(
      `Cannot run ${command} outside Tauri: set a frozen window.__ORCHESTRATOR_PIPELINE__ fixture`
    );
  }
  if (fixture.version !== 1 || !isDeepFrozen(fixture)) {
    throw new Error("Browser orchestrator fixture must be version 1 and deeply frozen");
  }
  return fixture;
}

function browserFixtureResponse<T>(command: OrchestratorFixtureCommand): T {
  const fixture = requireBrowserFixture(command);
  const response = fixture.responses[command];
  if (!response) {
    throw new Error(`Browser orchestrator fixture has no explicit response for ${command}`);
  }
  if (!response.ok) throw new Error(response.error || `${command} failed in the browser fixture`);
  return cloneFixtureValue(response.value as T);
}

export function browserOrchestratorScope(): ScopedRunRequest | null {
  if (inTauri() || typeof window === "undefined" || !window.__ORCHESTRATOR_PIPELINE__) {
    return null;
  }
  const fixture = requireBrowserFixture(ORCHESTRATOR_RICH_FIXTURE_COMMAND);
  const rich = fixture.responses[ORCHESTRATOR_RICH_FIXTURE_COMMAND];
  if (rich?.ok) {
    const value = rich.value as Partial<OrchestratorSnapshot>;
    if (
      value.run &&
      typeof value.run.runId === "string" &&
      typeof value.run.repositoryRoot === "string"
    ) {
      return { runId: value.run.runId, repositoryRoot: value.run.repositoryRoot };
    }
    throw new Error("Frozen rich orchestrator fixture has no repository-scoped run identity");
  }
  if (rich && !rich.ok) throw new Error(rich.error || "Frozen rich orchestrator fixture failed");
  const transport = fixture.responses[ORCHESTRATOR_COMMANDS.snapshot];
  if (transport?.ok) {
    const value = transport.value as Partial<PipelineSnapshotResponse>;
    if (
      value.manifest &&
      typeof value.manifest.runId === "string" &&
      typeof value.manifest.repositoryRoot === "string"
    ) {
      return {
        runId: value.manifest.runId,
        repositoryRoot: value.manifest.repositoryRoot,
      };
    }
    throw new Error("Frozen transport fixture has no repository-scoped run manifest");
  }
  if (transport && !transport.ok) {
    throw new Error(transport.error || "Frozen transport orchestrator fixture failed");
  }
  return null;
}

function richBrowserSnapshot(request: SnapshotRequest): OrchestratorSnapshot | null {
  if (inTauri() || typeof window === "undefined") return null;
  const fixture = requireBrowserFixture(ORCHESTRATOR_RICH_FIXTURE_COMMAND);
  if (!fixture.responses[ORCHESTRATOR_RICH_FIXTURE_COMMAND]) return null;
  const snapshot = browserFixtureResponse<OrchestratorSnapshot>(
    ORCHESTRATOR_RICH_FIXTURE_COMMAND
  );
  if (
    !snapshot ||
    typeof snapshot !== "object" ||
    !snapshot.run ||
    typeof snapshot.run.runId !== "string" ||
    typeof snapshot.run.repositoryRoot !== "string" ||
    snapshot.run.runId !== request.runId ||
    snapshot.run.repositoryRoot.toLocaleLowerCase() !== request.repositoryRoot.toLocaleLowerCase() ||
    !Array.isArray(snapshot.stages) ||
    !snapshot.scheduler ||
    !snapshot.scheduler.nodes ||
    !Array.isArray(snapshot.events) ||
    !Array.isArray(snapshot.activeRuns) ||
    !Array.isArray(snapshot.completedRuns) ||
    !Array.isArray(snapshot.warnings) ||
    !Array.isArray(snapshot.changes)
  ) {
    throw new Error("Frozen rich orchestrator snapshot failed its repository/run shape fence");
  }
  return { ...snapshot, leftovers: Array.isArray(snapshot.leftovers) ? snapshot.leftovers : [] };
}

async function invokePipeline<TRequest extends object, TResult>(
  command: OrchestratorCommand,
  request: TRequest
): Promise<TResult> {
  if (!inTauri()) return browserFixtureResponse<TResult>(command);
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<TResult>(command, { request });
}

export async function orchestratorCreateRun(
  request: CreateRunRequest
): Promise<CreateRunResult> {
  requireRun(request);
  requireText(request.repositoryRoot, "repositoryRoot");
  requireText(request.branch, "branch");
  if (!request.allowedFiles.length) throw new Error("allowedFiles must not be empty");
  if (!request.nodes.length) throw new Error("nodes must not be empty");
  return invokePipeline(ORCHESTRATOR_COMMANDS.createRun, request);
}

export async function orchestratorPreflight(
  request: OrchestratorPreflightRequest
): Promise<PreflightReport> {
  return invokePipeline(ORCHESTRATOR_COMMANDS.preflight, requireScope(request));
}

export async function orchestratorSnapshot(
  request: SnapshotRequest
): Promise<PipelineSnapshotResponse> {
  return invokePipeline(ORCHESTRATOR_COMMANDS.snapshot, requireScope(request));
}

export async function orchestratorConsoleSnapshot(
  request: SnapshotRequest,
  seed: PipelineSnapshotSeed = {}
): Promise<OrchestratorSnapshot> {
  requireScope(request);
  const richFixture = richBrowserSnapshot(request);
  if (richFixture) return richFixture;
  return consoleSnapshotFromPipelineResponse(await orchestratorSnapshot(request), seed);
}

export async function orchestratorClaim(request: ClaimNodeRequest): Promise<NodeLease> {
  requireScope(request);
  requireText(request.nodeId, "nodeId");
  requireText(request.workerId, "workerId");
  if (!Number.isSafeInteger(request.leaseMs) || request.leaseMs < 1_000) {
    throw new Error("leaseMs must be at least 1000");
  }
  return invokePipeline(ORCHESTRATOR_COMMANDS.claim, request);
}

export async function orchestratorHeartbeat(request: HeartbeatRequest): Promise<NodeLease> {
  requireScope(request);
  requireText(request.nodeId, "nodeId");
  requireText(request.token, "token");
  if (!Number.isSafeInteger(request.leaseMs) || request.leaseMs < 1_000) {
    throw new Error("leaseMs must be at least 1000");
  }
  return invokePipeline(ORCHESTRATOR_COMMANDS.heartbeat, request);
}

export async function orchestratorComplete(
  request: CompleteNodeRequest
): Promise<WorkerGateResult> {
  requireScope(request);
  requireText(request.nodeId, "nodeId");
  requireText(request.token, "token");
  if (
    request.manifest.runId !== request.runId ||
    request.manifest.nodeId !== request.nodeId ||
    request.submission.leaseToken !== request.token
  ) {
    throw new Error("completion manifest, submission, and lease belong to different scopes");
  }
  return invokePipeline(ORCHESTRATOR_COMMANDS.complete, request);
}

export async function orchestratorFail(
  request: FailNodeRequest
): Promise<ScheduledNodeStatus> {
  requireScope(request);
  requireText(request.nodeId, "nodeId");
  requireText(request.token, "token");
  return invokePipeline(ORCHESTRATOR_COMMANDS.fail, request);
}

export async function orchestratorReap(request: ReapRequest): Promise<ReapAction[]> {
  return invokePipeline(ORCHESTRATOR_COMMANDS.reap, requireScope(request));
}

export async function orchestratorValidateSubmission(
  request: ValidateSubmissionRequest
): Promise<WorkerGateResult> {
  requireScope(request);
  if (request.manifest.runId !== request.runId) {
    throw new Error("worker manifest belongs to a different run");
  }
  return invokePipeline(ORCHESTRATOR_COMMANDS.validateSubmission, request);
}

export async function orchestratorReconcile(
  request: ReconcileRequest
): Promise<ReconciliationResult> {
  requireText(request.input.planId, "input.planId");
  return invokePipeline(ORCHESTRATOR_COMMANDS.reconcile, requireScope(request));
}

export async function orchestratorRelease(
  request: ReleaseRequest
): Promise<ReleaseGateResult> {
  return invokePipeline(ORCHESTRATOR_COMMANDS.release, requireScope(request));
}

export async function orchestratorDeliver(
  request: DeliverRequest
): Promise<DeliveryOutcome> {
  requireScope(request);
  if (request.delivery.runId !== request.runId) {
    throw new Error("delivery request belongs to a different run");
  }
  return invokePipeline(ORCHESTRATOR_COMMANDS.deliver, request);
}

export const orchestratorPipelineClient = Object.freeze({
  createRun: orchestratorCreateRun,
  preflight: orchestratorPreflight,
  snapshot: orchestratorSnapshot,
  consoleSnapshot: orchestratorConsoleSnapshot,
  claim: orchestratorClaim,
  heartbeat: orchestratorHeartbeat,
  complete: orchestratorComplete,
  fail: orchestratorFail,
  reap: orchestratorReap,
  validateSubmission: orchestratorValidateSubmission,
  reconcile: orchestratorReconcile,
  release: orchestratorRelease,
  deliver: orchestratorDeliver,
});
