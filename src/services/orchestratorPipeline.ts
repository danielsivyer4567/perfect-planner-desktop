export const ORCHESTRATOR_COMMANDS = {
  createRun: "orchestrator_create_run",
  preflight: "orchestrator_preflight_inspect",
  snapshot: "orchestrator_pipeline_snapshot",
  catalog: "orchestrator_run_catalog",
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
  preflight?: PreflightReport | null;
  preflightRecordedAtMs?: number | null;
  reconciliation?: ReconciliationResult | null;
  reconciliationRecordedAtMs?: number | null;
  release?: ReleaseGateResult | null;
  releaseRecordedAtMs?: number | null;
}

export interface RunCatalogRequest {
  repositoryRoot: string;
}

export interface RunCatalogEntry {
  runId: string;
  repositoryRoot: string;
  branch: string;
  status: string;
  completedNodes: number;
  totalNodes: number;
  updatedAt: number;
}

export interface RunCatalogResponse {
  activeRuns: RunCatalogEntry[];
  archivedRuns: RunCatalogEntry[];
  scannedEntries: number;
  truncated: boolean;
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

function normalizedRepositoryRoot(value: string): string {
  return value.trim().replace(/\\/g, "/").replace(/\/+$/, "").toLocaleLowerCase();
}

function pipelineRunStatus(value: string): PipelineRunStatus | null {
  const normalized = value.trim().toLocaleLowerCase().replace(/_/g, "-");
  const aliases: Record<string, PipelineRunStatus> = {
    ready: "pending",
    active: "running",
    archived: "completed",
    done: "completed",
  };
  const candidate = aliases[normalized] || normalized;
  return [
    "pending",
    "preflight",
    "running",
    "decision-required",
    "release-gate",
    "blocked",
    "failed",
    "completed",
  ].includes(candidate)
    ? (candidate as PipelineRunStatus)
    : null;
}

function catalogSummary(
  entry: RunCatalogEntry,
  repositoryRoot: string,
  seed: PipelineSnapshotSeed
): PipelineRunSummary {
  const status = pipelineRunStatus(entry.status);
  if (!status) throw new Error(`Run catalog entry ${entry.runId} has an unknown status`);
  return {
    organizationId: seed.organizationId || "not-recorded",
    repositoryId: seed.repositoryId || repositoryRoot,
    repositoryRoot,
    worktreePath: seed.worktreePath || repositoryRoot,
    branch: entry.branch,
    runId: entry.runId,
    planId: "not-recorded",
    title: entry.runId,
    status,
    completedNodes: entry.completedNodes,
    totalNodes: entry.totalNodes,
    updatedAt: new Date(entry.updatedAt).toISOString(),
  };
}

function reconciliationChanges(result: ReconciliationResult | null): ChangeComparison[] {
  if (!result) return [];
  return [
    ...result.fatal,
    ...result.unplanned,
    ...result.unproven,
    ...result.orphaned,
  ].map((violation) => ({
    id: violation.violationId,
    nodeId: violation.nodeId || null,
    desired: violation.category === "UNPLANNED" ? "Not planned" : violation.summary,
    actualCommit: violation.commitId || null,
    status: violation.category.toLocaleLowerCase() as ChangeComparison["status"],
    details: [violation.file, violation.summary].filter((value): value is string => Boolean(value)),
  }));
}

function mergedCatalogShelves(
  current: PipelineRunSummary,
  catalog: RunCatalogResponse | null,
  seed: PipelineSnapshotSeed
): { activeRuns: PipelineRunSummary[]; completedRuns: PipelineRunSummary[] } {
  if (!catalog) {
    return current.status === "completed"
      ? { activeRuns: [], completedRuns: [current] }
      : { activeRuns: [current], completedRuns: [] };
  }
  const root = current.repositoryRoot;
  const active = catalog.activeRuns.map((entry) => catalogSummary(entry, root, seed));
  const completed = catalog.archivedRuns.map((entry) => catalogSummary(entry, root, seed));
  if (active.some((run) => run.status === "completed")) {
    throw new Error("Run catalog placed a completed run on the active shelf");
  }
  if (completed.some((run) => run.status !== "completed")) {
    throw new Error("Run catalog placed a non-completed run on the Completed shelf");
  }
  const target = current.status === "completed" ? completed : active;
  const index = target.findIndex((run) => run.runId === current.runId);
  if (index >= 0) target[index] = current;
  else target.push(current);
  const uniqueSorted = (runs: PipelineRunSummary[]) =>
    [...new Map(runs.map((run) => [run.runId, run])).values()].sort((left, right) =>
      right.updatedAt.localeCompare(left.updatedAt)
    );
  return { activeRuns: uniqueSorted(active), completedRuns: uniqueSorted(completed) };
}

/**
 * Lift the bounded native transport into the richer console view without inventing gate proof.
 * Persisted gate records become positive UI state; absent gate/delivery records remain
 * explicitly unrecorded.
 */
export function consoleSnapshotFromPipelineResponse(
  response: PipelineSnapshotResponse,
  seed: PipelineSnapshotSeed = {},
  catalog: RunCatalogResponse | null = null
): OrchestratorSnapshot {
  const nodes = Object.values(response.scheduler.nodes);
  const completedNodes = nodes.filter((node) => node.status === "DONE").length;
  const blockedNodes = nodes.filter((node) => node.status === "BLOCKED").length;
  const runningNodes = nodes.filter((node) => node.status === "RUNNING").length;
  const suppliedStatus = pipelineRunStatus(response.hotResume.status);
  const status: PipelineRunStatus = suppliedStatus
    ? suppliedStatus
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
  const preflight = response.preflight ?? null;
  const reconciliation = response.reconciliation ?? null;
  const release = response.release ?? null;
  if (preflight?.disposition === "decisionRequired") {
    preflight.reasons.forEach((reason, index) => {
      warnings.push({
        id: `persisted-preflight-${index}`,
        severity: "critical",
        message: reason,
        decisionRequired: true,
        nodeId: null,
        createdAt: updatedAt,
      });
    });
  }
  release?.issues.forEach((issue, index) => {
    warnings.push({
      id: `persisted-release-${issue.kind}-${index}`,
      severity: issue.decisionRequired ? "critical" : "warning",
      message: issue.message,
      decisionRequired: issue.decisionRequired,
      nodeId: null,
      createdAt: updatedAt,
      issueKind: issue.kind,
    });
  });
  const stages: PipelineStage[] = [
    {
      id: "preflight",
      label: "System preflight",
      status: preflight
        ? preflight.disposition === "decisionRequired"
          ? "blocked"
          : "passed"
        : "waiting",
      summary: preflight
        ? preflight.reasons.join(" · ") || `Preflight ${preflight.disposition}.`
        : "No persisted preflight report is present.",
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
      status: reconciliation ? (reconciliation.passed ? "passed" : "failed") : "waiting",
      summary: reconciliation
        ? reconciliation.passed
          ? "Every persisted reconciliation list is clear or explicitly waived."
          : "Persisted reconciliation violations block release."
        : "No persisted reconciliation result is present.",
    },
    {
      id: "release",
      label: "Pre-merge release gate",
      status: release
        ? release.issues.length
          ? "blocked"
          : release.readyForPr || release.readyToMerge || release.merged
            ? "passed"
            : "waiting"
        : "waiting",
      summary: release
        ? release.issues.map((issue) => issue.message).join(" · ") || "Release gate is clear."
        : "No persisted release result is present.",
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
  const shelves = mergedCatalogShelves(run, catalog, seed);
  return {
    nowMs: Date.now(),
    run,
    stages,
    preflight,
    scheduler: response.scheduler,
    reconciliation,
    changes: reconciliationChanges(reconciliation),
    release,
    delivery: null,
    leftovers: [],
    warnings,
    events: response.eventTail.events,
    activeRuns: shelves.activeRuns,
    completedRuns: shelves.completedRuns,
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
  release: ReleaseGateInput;
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

function recordValue(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value as Record<string, unknown>;
}

function textValue(value: unknown, label: string): string {
  if (typeof value !== "string" || !value.trim()) throw new Error(`${label} must be text`);
  return value;
}

function numberValue(value: unknown, label: string): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    throw new Error(`${label} must be a non-negative finite number`);
  }
  return value;
}

function integerValue(value: unknown, label: string): number {
  const numeric = numberValue(value, label);
  if (!Number.isSafeInteger(numeric)) {
    throw new Error(`${label} must be a non-negative safe integer`);
  }
  return numeric;
}

function booleanValue(value: unknown, label: string): boolean {
  if (typeof value !== "boolean") throw new Error(`${label} must be boolean`);
  return value;
}

function arrayValue(value: unknown, label: string): unknown[] {
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  return value;
}

function textArray(value: unknown, label: string): string[] {
  return arrayValue(value, label).map((item, index) => textValue(item, `${label}[${index}]`));
}

function validateProcessIdentityShape(value: unknown, label: string): void {
  const process = recordValue(value, label);
  integerValue(process.pid, `${label}.pid`);
  textValue(process.executablePath, `${label}.executablePath`);
  integerValue(process.startedAtEpochMs, `${label}.startedAtEpochMs`);
  if (typeof process.commandLine !== "string") {
    throw new Error(`${label}.commandLine must be text`);
  }
}

function validatePortBindingShape(value: unknown, label: string): void {
  const binding = recordValue(value, label);
  integerValue(binding.port, `${label}.port`);
  textValue(binding.address, `${label}.address`);
  validateProcessIdentityShape(binding.process, `${label}.process`);
}

function validatePreflightShape(value: unknown, label: string): void {
  const report = recordValue(value, label);
  if (!["ready", "decisionRequired", "stoppedAllowlistedConflicts"].includes(
    textValue(report.disposition, `${label}.disposition`)
  )) {
    throw new Error(`${label}.disposition is unknown`);
  }
  const baseline = recordValue(report.baseline, `${label}.baseline`);
  textValue(baseline.repositoryRoot, `${label}.baseline.repositoryRoot`);
  if (typeof baseline.gitStatusPorcelainV2 !== "string") {
    throw new Error(`${label}.baseline.gitStatusPorcelainV2 must be text`);
  }
  const resources = recordValue(baseline.resources, `${label}.baseline.resources`);
  [
    "logicalCpuCount",
    "cpuUsagePercent",
    "totalMemoryBytes",
    "availableMemoryBytes",
    "repositoryDiskAvailableBytes",
  ].forEach((field) => numberValue(resources[field], `${label}.baseline.resources.${field}`));
  arrayValue(baseline.portBindings, `${label}.baseline.portBindings`).forEach((item, index) =>
    validatePortBindingShape(item, `${label}.baseline.portBindings[${index}]`)
  );
  ["conflicts", "unknownConflicts"].forEach((field) =>
    arrayValue(report[field], `${label}.${field}`).forEach((item, index) =>
      validatePortBindingShape(item, `${label}.${field}[${index}]`)
    )
  );
  arrayValue(report.stoppedProcesses, `${label}.stoppedProcesses`).forEach((item, index) =>
    validateProcessIdentityShape(item, `${label}.stoppedProcesses[${index}]`)
  );
  textArray(report.reasons, `${label}.reasons`);
}

function validateViolationShape(value: unknown, label: string): void {
  const violation = recordValue(value, label);
  textValue(violation.violationId, `${label}.violationId`);
  if (!["UNPLANNED", "UNPROVEN", "ORPHANED", "FATAL"].includes(
    textValue(violation.category, `${label}.category`)
  )) {
    throw new Error(`${label}.category is unknown`);
  }
  textValue(violation.summary, `${label}.summary`);
  textArray(violation.waivedBy, `${label}.waivedBy`);
  ["planId", "nodeId", "commitId", "file"].forEach((field) => {
    if (violation[field] !== undefined && violation[field] !== null) {
      textValue(violation[field], `${label}.${field}`);
    }
  });
}

function validateReconciliationShape(value: unknown, label: string): void {
  const result = recordValue(value, label);
  booleanValue(result.passed, `${label}.passed`);
  ["unplanned", "unproven", "orphaned", "fatal"].forEach((listName) => {
    arrayValue(result[listName], `${label}.${listName}`).forEach((violation, index) =>
      validateViolationShape(violation, `${label}.${listName}[${index}]`)
    );
  });
  arrayValue(result.waivers, `${label}.waivers`).forEach((item, index) => {
    const waiver = recordValue(item, `${label}.waivers[${index}]`);
    textValue(waiver.name, `${label}.waivers[${index}].name`);
    if (!['UNPLANNED', 'UNPROVEN', 'ORPHANED', 'FATAL'].includes(
      textValue(waiver.category, `${label}.waivers[${index}].category`)
    )) {
      throw new Error(`${label}.waivers[${index}].category is unknown`);
    }
    textValue(waiver.violationId, `${label}.waivers[${index}].violationId`);
    booleanValue(waiver.applied, `${label}.waivers[${index}].applied`);
  });
}

function validateReleaseShape(value: unknown, label: string): void {
  const release = recordValue(value, label);
  booleanValue(release.readyForPr, `${label}.readyForPr`);
  booleanValue(release.readyToMerge, `${label}.readyToMerge`);
  booleanValue(release.merged, `${label}.merged`);
  arrayValue(release.issues, `${label}.issues`).forEach((item, index) => {
    const issue = recordValue(item, `${label}.issues[${index}]`);
    const allowedKinds = new Set<ReleaseIssueKind>([
      "DIRTY_WORKTREE",
      "MERGE_CONFLICT",
      "MISSING_EVIDENCE",
      "RECONCILIATION",
      "CI_NOT_RUN",
      "CODE_FAILURE",
      "CI_INFRASTRUCTURE_FAILURE",
      "NOT_PUSHED",
      "REVIEW_REQUIRED",
      "CHANGES_REQUESTED",
    ]);
    if (!allowedKinds.has(textValue(issue.kind, `${label}.issues[${index}].kind`) as ReleaseIssueKind)) {
      throw new Error(`${label}.issues[${index}].kind is unknown`);
    }
    textValue(issue.message, `${label}.issues[${index}].message`);
    booleanValue(issue.decisionRequired, `${label}.issues[${index}].decisionRequired`);
  });
}

function validateSchedulerShape(value: unknown): void {
  const scheduler = recordValue(value, "pipeline snapshot scheduler");
  integerValue(scheduler.nextFence, "pipeline snapshot scheduler.nextFence");
  const nodes = recordValue(scheduler.nodes, "pipeline snapshot scheduler.nodes");
  Object.entries(nodes).forEach(([key, item]) => {
    const node = recordValue(item, `pipeline snapshot scheduler.nodes.${key}`);
    if (textValue(node.id, `pipeline snapshot scheduler.nodes.${key}.id`) !== key) {
      throw new Error(`pipeline snapshot scheduler node ${key} has a mismatched ID`);
    }
    integerValue(node.wave, `pipeline snapshot scheduler.nodes.${key}.wave`);
    integerValue(node.attempts, `pipeline snapshot scheduler.nodes.${key}.attempts`);
    if (!["READY", "RUNNING", "DONE", "BLOCKED"].includes(
      textValue(node.status, `pipeline snapshot scheduler.nodes.${key}.status`)
    )) {
      throw new Error(`pipeline snapshot scheduler node ${key} has an unknown status`);
    }
    textArray(node.dependsOn, `pipeline snapshot scheduler.nodes.${key}.dependsOn`);
    if (node.title !== undefined) textValue(node.title, `pipeline snapshot scheduler.nodes.${key}.title`);
    if (node.profile !== undefined && !["ui", "headless", "migration", "docs"].includes(
      textValue(node.profile, `pipeline snapshot scheduler.nodes.${key}.profile`)
    )) {
      throw new Error(`pipeline snapshot scheduler node ${key} has an unknown evidence profile`);
    }
    if (node.allowedFiles !== undefined) {
      textArray(node.allowedFiles, `pipeline snapshot scheduler.nodes.${key}.allowedFiles`);
    }
    if (node.stallAlarmFence !== null && node.stallAlarmFence !== undefined) {
      integerValue(
        node.stallAlarmFence,
        `pipeline snapshot scheduler.nodes.${key}.stallAlarmFence`
      );
    }
    if (node.evidence !== undefined) {
      arrayValue(node.evidence, `pipeline snapshot scheduler.nodes.${key}.evidence`).forEach(
        (item, index) => {
          const artifact = recordValue(
            item,
            `pipeline snapshot scheduler.nodes.${key}.evidence[${index}]`
          );
          if (![
            "before-screenshot",
            "after-screenshot",
            "command-output",
            "exit-code",
            "git-diff",
            "ocr-report",
            "migration-status",
            "document-diff",
          ].includes(textValue(
            artifact.kind,
            `pipeline snapshot scheduler.nodes.${key}.evidence[${index}].kind`
          ))) {
            throw new Error(`pipeline snapshot scheduler node ${key} has unknown evidence`);
          }
          textValue(artifact.path, `pipeline snapshot scheduler.nodes.${key}.evidence[${index}].path`);
          textValue(artifact.sha256, `pipeline snapshot scheduler.nodes.${key}.evidence[${index}].sha256`);
          integerValue(artifact.bytes, `pipeline snapshot scheduler.nodes.${key}.evidence[${index}].bytes`);
        }
      );
    }
    if (node.verification !== undefined) {
      arrayValue(node.verification, `pipeline snapshot scheduler.nodes.${key}.verification`).forEach(
        (item, index) => {
          const result = recordValue(
            item,
            `pipeline snapshot scheduler.nodes.${key}.verification[${index}]`
          );
          textValue(result.commandId, `pipeline snapshot scheduler.nodes.${key}.verification[${index}].commandId`);
          if (typeof result.exitCode !== "number" || !Number.isSafeInteger(result.exitCode)) {
            throw new Error(`pipeline snapshot scheduler node ${key} has an invalid exit code`);
          }
          textValue(result.outputArtifact, `pipeline snapshot scheduler.nodes.${key}.verification[${index}].outputArtifact`);
        }
      );
    }
    if (node.lease !== null && node.lease !== undefined) {
      const lease = recordValue(node.lease, `pipeline snapshot scheduler.nodes.${key}.lease`);
      textValue(lease.nodeId, `pipeline snapshot scheduler.nodes.${key}.lease.nodeId`);
      textValue(lease.workerId, `pipeline snapshot scheduler.nodes.${key}.lease.workerId`);
      textValue(lease.token, `pipeline snapshot scheduler.nodes.${key}.lease.token`);
      integerValue(lease.fence, `pipeline snapshot scheduler.nodes.${key}.lease.fence`);
      integerValue(lease.expiresAtMs, `pipeline snapshot scheduler.nodes.${key}.lease.expiresAtMs`);
    }
  });
}

function validatedPipelineSnapshot(
  value: unknown,
  request: SnapshotRequest
): PipelineSnapshotResponse {
  const response = recordValue(value, "pipeline snapshot");
  const manifest = recordValue(response.manifest, "pipeline snapshot manifest");
  if (textValue(manifest.runId, "pipeline snapshot manifest.runId") !== request.runId) {
    throw new Error("pipeline snapshot belongs to a different run");
  }
  const manifestRoot = textValue(
    manifest.repositoryRoot,
    "pipeline snapshot manifest.repositoryRoot"
  );
  if (normalizedRepositoryRoot(manifestRoot) !== normalizedRepositoryRoot(request.repositoryRoot)) {
    throw new Error("pipeline snapshot belongs to a different repository");
  }
  textValue(manifest.branch, "pipeline snapshot manifest.branch");
  textArray(manifest.allowedFiles, "pipeline snapshot manifest.allowedFiles");
  numberValue(manifest.schemaVersion, "pipeline snapshot manifest.schemaVersion");

  const hotResume = recordValue(response.hotResume, "pipeline snapshot hotResume");
  if (textValue(hotResume.runId, "pipeline snapshot hotResume.runId") !== request.runId) {
    throw new Error("pipeline hot-resume state belongs to a different run");
  }
  if (
    normalizedRepositoryRoot(
      textValue(hotResume.repositoryRoot, "pipeline snapshot hotResume.repositoryRoot")
    ) !== normalizedRepositoryRoot(request.repositoryRoot)
  ) {
    throw new Error("pipeline hot-resume state belongs to a different repository");
  }
  textValue(hotResume.status, "pipeline snapshot hotResume.status");
  if (textValue(hotResume.branch, "pipeline snapshot hotResume.branch") !== manifest.branch) {
    throw new Error("pipeline manifest and hot-resume branches do not match");
  }
  integerValue(hotResume.schemaVersion, "pipeline snapshot hotResume.schemaVersion");
  textArray(hotResume.lockedFiles, "pipeline snapshot hotResume.lockedFiles");
  textArray(hotResume.nextActions, "pipeline snapshot hotResume.nextActions");
  if (hotResume.lastCompletedStep !== null && hotResume.lastCompletedStep !== undefined) {
    textValue(hotResume.lastCompletedStep, "pipeline snapshot hotResume.lastCompletedStep");
  }
  validateSchedulerShape(response.scheduler);

  const tail = recordValue(response.eventTail, "pipeline snapshot eventTail");
  const allowedEvents = new Set<OrchestratorEventType>([
    "preflight",
    "claim",
    "heartbeat",
    "progress",
    "evidence",
    "gate-pass",
    "gate-fail",
    "decision-required",
    "node-done",
    "reassign",
    "warning",
    "run-done",
  ]);
  arrayValue(tail.events, "pipeline snapshot eventTail.events").forEach((item, index) => {
    const event = recordValue(item, `pipeline snapshot eventTail.events[${index}]`);
    if (textValue(event.runId, `pipeline snapshot eventTail.events[${index}].runId`) !== request.runId) {
      throw new Error("pipeline event tail contains an event from another run");
    }
    textValue(event.ts, `pipeline snapshot eventTail.events[${index}].ts`);
    textValue(event.worker, `pipeline snapshot eventTail.events[${index}].worker`);
    textValue(event.msg, `pipeline snapshot eventTail.events[${index}].msg`);
    if (!allowedEvents.has(textValue(event.type, `pipeline snapshot eventTail.events[${index}].type`) as OrchestratorEventType)) {
      throw new Error("pipeline event tail contains an unknown event type");
    }
  });
  ["startOffset", "nextOffset", "skippedLines"].forEach((field) =>
    integerValue(tail[field], `pipeline snapshot eventTail.${field}`)
  );
  booleanValue(tail.trailingPartial, "pipeline snapshot eventTail.trailingPartial");
  booleanValue(tail.truncated, "pipeline snapshot eventTail.truncated");

  if (response.preflight !== undefined && response.preflight !== null) {
    validatePreflightShape(response.preflight, "pipeline snapshot preflight");
    const baseline = recordValue(
      recordValue(response.preflight, "pipeline snapshot preflight").baseline,
      "pipeline snapshot preflight.baseline"
    );
    if (
      normalizedRepositoryRoot(
        textValue(baseline.repositoryRoot, "pipeline snapshot preflight.baseline.repositoryRoot")
      ) !== normalizedRepositoryRoot(request.repositoryRoot)
    ) {
      throw new Error("pipeline preflight belongs to a different repository");
    }
  }
  if (response.reconciliation !== undefined && response.reconciliation !== null) {
    validateReconciliationShape(response.reconciliation, "pipeline snapshot reconciliation");
  }
  if (response.release !== undefined && response.release !== null) {
    validateReleaseShape(response.release, "pipeline snapshot release");
  }
  ([
    ["preflight", "preflightRecordedAtMs"],
    ["reconciliation", "reconciliationRecordedAtMs"],
    ["release", "releaseRecordedAtMs"],
  ] as const).forEach(([resultField, recordedAtField]) => {
    const resultPresent = response[resultField] !== undefined && response[resultField] !== null;
    const timestampPresent =
      response[recordedAtField] !== undefined && response[recordedAtField] !== null;
    if (resultPresent !== timestampPresent) {
      throw new Error(`pipeline snapshot ${resultField} result/timestamp pair is incomplete`);
    }
    if (timestampPresent) {
      integerValue(response[recordedAtField], `pipeline snapshot ${recordedAtField}`);
    }
  });
  return value as PipelineSnapshotResponse;
}

function validatedRunCatalog(value: unknown, request: RunCatalogRequest): RunCatalogResponse {
  const response = recordValue(value, "run catalog");
  const root = normalizedRepositoryRoot(request.repositoryRoot);
  const runIds = new Set<string>();
  const validateEntries = (field: "activeRuns" | "archivedRuns") =>
    arrayValue(response[field], `run catalog.${field}`).forEach((item, index) => {
      const entry = recordValue(item, `run catalog.${field}[${index}]`);
      const runId = textValue(entry.runId, `run catalog.${field}[${index}].runId`);
      if (runIds.has(runId)) {
        throw new Error(`run catalog repeats ${runId} across repository shelves`);
      }
      runIds.add(runId);
      if (
        normalizedRepositoryRoot(
          textValue(entry.repositoryRoot, `run catalog.${field}[${index}].repositoryRoot`)
        ) !== root
      ) {
        throw new Error(`run catalog.${field}[${index}] crosses the repository scope`);
      }
      textValue(entry.branch, `run catalog.${field}[${index}].branch`);
      const status = pipelineRunStatus(textValue(entry.status, `run catalog.${field}[${index}].status`));
      if (!status) throw new Error(`run catalog.${field}[${index}] has an unknown status`);
      if (field === "activeRuns" && status === "completed") {
        throw new Error(`run catalog.${field}[${index}] is completed but not archived`);
      }
      if (field === "archivedRuns" && status !== "completed") {
        throw new Error(`run catalog.${field}[${index}] is archived without completed status`);
      }
      integerValue(entry.completedNodes, `run catalog.${field}[${index}].completedNodes`);
      integerValue(entry.totalNodes, `run catalog.${field}[${index}].totalNodes`);
      integerValue(entry.updatedAt, `run catalog.${field}[${index}].updatedAt`);
      if ((entry.completedNodes as number) > (entry.totalNodes as number)) {
        throw new Error(`run catalog.${field}[${index}] has impossible node counts`);
      }
    });
  validateEntries("activeRuns");
  validateEntries("archivedRuns");
  const scannedEntries = integerValue(response.scannedEntries, "run catalog.scannedEntries");
  if (scannedEntries < runIds.size) {
    throw new Error("run catalog scannedEntries is smaller than its returned shelves");
  }
  booleanValue(response.truncated, "run catalog.truncated");
  return value as RunCatalogResponse;
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
  const scoped = requireScope(request);
  const response = await invokePipeline<SnapshotRequest, unknown>(
    ORCHESTRATOR_COMMANDS.snapshot,
    scoped
  );
  return validatedPipelineSnapshot(response, scoped);
}

export async function orchestratorRunCatalog(
  request: RunCatalogRequest
): Promise<RunCatalogResponse> {
  requireText(request.repositoryRoot, "repositoryRoot");
  const response = await invokePipeline<RunCatalogRequest, unknown>(
    ORCHESTRATOR_COMMANDS.catalog,
    request
  );
  return validatedRunCatalog(response, request);
}

export async function orchestratorConsoleSnapshot(
  request: SnapshotRequest,
  seed: PipelineSnapshotSeed = {}
): Promise<OrchestratorSnapshot> {
  requireScope(request);
  const richFixture = richBrowserSnapshot(request);
  if (richFixture) return richFixture;
  const [snapshot, catalog] = await Promise.all([
    orchestratorSnapshot(request),
    orchestratorRunCatalog({ repositoryRoot: request.repositoryRoot }),
  ]);
  return consoleSnapshotFromPipelineResponse(snapshot, seed, catalog);
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
  runCatalog: orchestratorRunCatalog,
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
