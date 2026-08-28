import React, {
  CSSProperties,
  PointerEvent as ReactPointerEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  ChangeComparison,
  OrchestratorEvent,
  OrchestratorEventType,
  OrchestratorSnapshot,
  PipelineRunSummary,
  PipelineRunStatus,
  PipelineSnapshotSeed,
  PipelineStage,
  PipelineStageId,
  PipelineWarning,
  ScheduledNode,
  declaredRequiredPorts,
  orchestratorApproveRun,
  orchestratorClaim,
  orchestratorComplete,
  orchestratorConsoleSnapshot,
  orchestratorCreateRun,
  orchestratorFail,
  orchestratorHeartbeat,
  orchestratorPreflight,
  orchestratorReap,
  orchestratorRunCatalog,
} from "../services/orchestratorPipeline";

const STAGE_ORDER: Array<{ id: PipelineStageId; label: string }> = [
  { id: "preflight", label: "System preflight" },
  { id: "scope", label: "Isolated execution scope" },
  { id: "plan", label: "Perfect Plan" },
  { id: "execution", label: "Scheduled workers" },
  { id: "reconciliation", label: "Planned vs actual" },
  { id: "release", label: "Pre-merge release gate" },
  { id: "delivery", label: "Delivery and handover" },
];

const PROBLEM_EVENTS = new Set<OrchestratorEventType>([
  "gate-fail",
  "decision-required",
  "reassign",
  "warning",
]);

type AuditTab = "logs" | "changes";

export interface PipelineConsoleProps {
  runId?: string;
  repositoryRoot?: string;
  planPath?: string;
  parallelAgents?: boolean;
  snapshotSeed?: PipelineSnapshotSeed;
  snapshot?: OrchestratorSnapshot | null;
  pollIntervalMs?: number;
  className?: string;
  onSnapshotChange?: (snapshot: OrchestratorSnapshot) => void;
  onSelectRun?: (run: PipelineRunSummary) => void;
  onRunCreated?: (scope: { runId: string; repositoryRoot: string }) => void;
  onReviewDecision?: (warning: PipelineWarning) => void;
  onDiagnostic?: (level: "info" | "warning" | "error", message: string) => void;
}

function domToken(value: string): string {
  return value.replace(/[^A-Za-z0-9_-]/g, (character) =>
    `-${character.codePointAt(0)?.toString(16) || "x"}-`
  );
}

function labelize(value: string): string {
  return value
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/[_-]+/g, " ")
    .replace(/\b\w/g, (character) => character.toUpperCase());
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "not recorded";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatTime(value: string | number): string {
  const timestamp =
    typeof value === "number"
      ? value
      : /^\d+$/.test(value)
        ? Number(value)
        : Date.parse(value);
  const date = new Date(timestamp);
  return Number.isFinite(date.getTime()) ? date.toLocaleString() : String(value);
}

function eventDataText(event: OrchestratorEvent, field: string): string | undefined {
  if (!event.data || typeof event.data !== "object" || Array.isArray(event.data)) return undefined;
  const value = (event.data as Record<string, unknown>)[field];
  return typeof value === "string" && value.trim() ? value : undefined;
}

function visibleStages(snapshot: OrchestratorSnapshot): PipelineStage[] {
  const supplied = new Map(snapshot.stages.map((stage) => [stage.id, stage]));
  return STAGE_ORDER.map(({ id, label }) =>
    supplied.get(id) || {
      id,
      label,
      status: "waiting",
      summary: "No gate result has been recorded.",
    }
  );
}

function nodeValues(snapshot: OrchestratorSnapshot): ScheduledNode[] {
  return Object.values(snapshot.scheduler.nodes).sort(
    (left, right) => left.wave - right.wave || left.id.localeCompare(right.id)
  );
}

function comparableWindowsPath(value: string): string {
  let normalized = value.trim().replace(/\//g, "\\");
  const lower = normalized.toLocaleLowerCase();
  if (lower.startsWith("\\\\?\\unc\\")) {
    normalized = `\\\\${normalized.slice(8)}`;
  } else if (lower.startsWith("\\\\?\\")) {
    normalized = normalized.slice(4);
  }
  return normalized.replace(/\\/g, "/").replace(/\/+$/, "").toLocaleLowerCase();
}

function currentAndShelfRuns(
  snapshot: OrchestratorSnapshot,
  shelf: "active" | "completed"
): PipelineRunSummary[] {
  const source = shelf === "active" ? snapshot.activeRuns : snapshot.completedRuns;
  const currentBelongs =
    shelf === "completed"
      ? snapshot.run.status === "completed"
      : snapshot.run.status !== "completed";
  const byId = new Map(source.map((run) => [run.runId, run]));
  if (currentBelongs) byId.set(snapshot.run.runId, snapshot.run);
  return [...byId.values()].sort((left, right) => right.updatedAt.localeCompare(left.updatedAt));
}

function collectedWarnings(snapshot: OrchestratorSnapshot): PipelineWarning[] {
  const warnings = new Map<string, PipelineWarning>(
    snapshot.warnings.map((warning) => {
      const releaseIssue = snapshot.release?.issues.find(
        (issue) => issue.message === warning.message
      );
      return [
        warning.id,
        { ...warning, issueKind: warning.issueKind || releaseIssue?.kind },
      ];
    })
  );
  snapshot.events.forEach((event, index) => {
    if (event.type !== "warning" && event.type !== "decision-required") return;
    const id = `event-${event.type}-${event.nodeId || "run"}-${event.ts}-${index}`;
    if (warnings.has(id)) return;
    warnings.set(id, {
      id,
      severity: event.type === "decision-required" ? "critical" : "warning",
      message: event.msg,
      decisionRequired: event.type === "decision-required",
      nodeId: event.nodeId,
      createdAt: event.ts,
    });
  });
  snapshot.release?.issues.forEach((issue, index) => {
    const id = `release-${issue.kind}-${index}`;
    if (warnings.has(id)) return;
    warnings.set(id, {
      id,
      severity: issue.decisionRequired ? "critical" : "warning",
      message: issue.message,
      decisionRequired: issue.decisionRequired,
      nodeId: null,
      createdAt: new Date(snapshot.nowMs).toISOString(),
      issueKind: issue.kind,
    });
  });
  return [...warnings.values()].sort((left, right) =>
    right.createdAt.localeCompare(left.createdAt)
  );
}

function RunShelf({
  id,
  title,
  runs,
  onSelectRun,
}: {
  id: "active" | "completed";
  title: string;
  runs: PipelineRunSummary[];
  onSelectRun?: (run: PipelineRunSummary) => void;
}) {
  return (
    <section
      className={`pipeline-run-shelf pipeline-run-shelf-${id}`}
      id={`pp-orch-shelf-${id}`}
      aria-labelledby={`pp-orch-heading-shelf-${id}`}
    >
      <header>
        <h3 id={`pp-orch-heading-shelf-${id}`}>{title}</h3>
        <span aria-label={`${runs.length} runs`}>{runs.length}</span>
      </header>
      {runs.length ? (
        <ul className="pipeline-run-list" role="list">
          {runs.map((run) => {
            const token = domToken(run.runId);
            return (
              <li key={run.runId} id={`pp-orch-run-${id}-${token}`}>
                <button
                  type="button"
                  id={`pp-orch-btn-select-${id}-${token}`}
                  onClick={() => onSelectRun?.(run)}
                  disabled={!onSelectRun}
                  aria-label={`Select ${run.runId}, ${run.title}`}
                  data-run-id={run.runId}
                  data-plan-id={run.planId}
                  data-repository-id={run.repositoryId}
                  data-run-status={run.status}
                >
                  <span>{run.runId}</span>
                  <strong>{run.title}</strong>
                  <small>
                    {run.repositoryId} · {run.branch}
                  </small>
                  <span>
                    {run.completedNodes}/{run.totalNodes} nodes · {labelize(run.status)}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      ) : (
        <p id={`pp-orch-empty-${id}`}>No {id} runs in this repository scope.</p>
      )}
    </section>
  );
}

function PipelineSpine({ stages }: { stages: PipelineStage[] }) {
  return (
    <section
      className="pipeline-spine"
      id="pp-orch-region-pipeline-spine"
      aria-labelledby="pp-orch-heading-pipeline-spine"
    >
      <header>
        <p>TOP-DOWN EXECUTION</p>
        <h2 id="pp-orch-heading-pipeline-spine">Guarded orchestration spine</h2>
      </header>
      <ol className="pipeline-stage-list">
        {stages.map((stage, index) => (
          <li
            key={stage.id}
            id={`pp-orch-stage-${stage.id}`}
            className={`pipeline-stage pipeline-stage-${stage.status}`}
            data-stage-status={stage.status}
          >
            <span className="pipeline-stage-index" aria-hidden="true">
              {String(index + 1).padStart(2, "0")}
            </span>
            <div>
              <h3>{stage.label}</h3>
              <p>{stage.summary}</p>
            </div>
            <strong className="pipeline-stage-state">{labelize(stage.status)}</strong>
          </li>
        ))}
      </ol>
    </section>
  );
}

function PreflightPanel({
  snapshot,
  repositoryRoot,
  runId,
  busy,
  onRan,
  onError,
}: {
  snapshot: OrchestratorSnapshot;
  repositoryRoot?: string;
  runId?: string;
  busy?: boolean;
  onRan?: () => void;
  onError?: (message: string) => void;
}) {
  const report = snapshot.preflight;
  const approval = snapshot.runApproval;
  const [running, setRunning] = useState(false);
  let requiredPorts: number[] = [];
  let requiredPortError: string | null = snapshot.manifest
    ? null
    : "Immutable run manifest unavailable; required ports are unknown";
  try {
    if (snapshot.manifest) requiredPorts = declaredRequiredPorts(snapshot.manifest);
  } catch (cause) {
    requiredPortError = cause instanceof Error ? cause.message : String(cause);
  }
  const inspect = async () => {
    if (!repositoryRoot || !runId || running) return;
    if (requiredPortError) {
      onError?.(`${requiredPortError}. Correct the plan resource claim before preflight.`);
      return;
    }
    setRunning(true);
    try {
      await orchestratorPreflight({ repositoryRoot, runId, requiredPorts });
      onRan?.();
    } catch (cause) {
      onError?.(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setRunning(false);
    }
  };
  return (
    <section
      className="pipeline-gate-panel pipeline-preflight"
      id="pp-orch-region-preflight"
      aria-labelledby="pp-orch-heading-preflight"
    >
      <header>
        <h2 id="pp-orch-heading-preflight">Preflight gate</h2>
        <strong>
          {report ? (snapshot.preflightFresh ? labelize(report.disposition) : "Expired") : "Not run"}
        </strong>
        <button
          type="button"
          id="pp-orch-btn-run-preflight"
          disabled={!repositoryRoot || !runId || busy || running}
          onClick={() => void inspect()}
        >
          {running ? "Inspecting…" : "Run preflight"}
        </button>
      </header>
      {!report ? (
        <p id="pp-orch-empty-preflight">
          Execution remains blocked until preflight is recorded. {requiredPortError
            ? requiredPortError
            : requiredPorts.length
              ? `Declared ports checked: ${requiredPorts.join(", ")}.`
              : "This plan declares no port prerequisites."}
        </p>
      ) : (
        <>
          {!snapshot.preflightFresh ? (
            <p id="pp-orch-status-preflight-expired" role="alert">
              Native preflight has expired. Refresh it and re-approve before admission.
            </p>
          ) : approval ? (
            <p id="pp-orch-status-run-approved">
              Explicit approval recorded after {approval.collisionAssessments.length} native
              collision assessment(s). Receipt {approval.approvalDigest.slice(0, 12)}…
            </p>
          ) : report.disposition === "ready" ? (
            <p id="pp-orch-status-run-awaiting-approval">
              Preflight is ready. Workers remain blocked until you explicitly approve this run.
            </p>
          ) : null}
          {report.disposition === "decisionRequired" ? (
            <div id="pp-orch-alert-preflight-decision" role="alert">
              Decision required. Unknown conflicts were not stopped.
            </div>
          ) : null}
          <dl className="pipeline-resource-grid">
            <div>
              <dt>CPU</dt>
              <dd>
                {report.baseline.resources.cpuUsagePercent.toFixed(1)}% ·{" "}
                {report.baseline.resources.logicalCpuCount} logical
              </dd>
            </div>
            <div>
              <dt>Available memory</dt>
              <dd>{formatBytes(report.baseline.resources.availableMemoryBytes)}</dd>
            </div>
            <div>
              <dt>Repository disk</dt>
              <dd>{formatBytes(report.baseline.resources.repositoryDiskAvailableBytes)}</dd>
            </div>
            <div>
              <dt>Git baseline</dt>
              <dd>{report.baseline.gitStatusPorcelainV2.trim() || "Clean"}</dd>
            </div>
          </dl>
          {report.reasons.length ? (
            <ul id="pp-orch-list-preflight-reasons">
              {report.reasons.map((reason, index) => (
                <li key={`${reason}-${index}`}>{reason}</li>
              ))}
            </ul>
          ) : null}
          {report.conflicts.length ? (
            <div className="pipeline-table-scroll">
              <table id="pp-orch-table-preflight-conflicts">
                <caption>Processes occupying required ports</caption>
                <thead>
                  <tr>
                    <th scope="col">Port</th>
                    <th scope="col">PID</th>
                    <th scope="col">Executable</th>
                    <th scope="col">Disposition</th>
                  </tr>
                </thead>
                <tbody>
                  {report.conflicts.map((binding) => {
                    const unknown = report.unknownConflicts.some(
                      (candidate) =>
                        candidate.port === binding.port &&
                        candidate.process.pid === binding.process.pid &&
                        candidate.process.startedAtEpochMs === binding.process.startedAtEpochMs
                    );
                    return (
                      <tr key={`${binding.port}-${binding.process.pid}`}>
                        <td>{binding.port}</td>
                        <td>{binding.process.pid}</td>
                        <td>
                          <code>{binding.process.executablePath}</code>
                        </td>
                        <td>{unknown ? "Decision required" : "Allowlisted"}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          ) : (
            <p>No required-port conflicts recorded.</p>
          )}
        </>
      )}
    </section>
  );
}

function NodePanel({
  nodes,
  approved,
  preflightFresh,
  repositoryRoot,
  runId,
  busy,
  onChanged,
  onError,
}: {
  nodes: ScheduledNode[];
  approved: boolean;
  preflightFresh: boolean;
  repositoryRoot?: string;
  runId?: string;
  busy?: boolean;
  onChanged?: () => void;
  onError?: (message: string) => void;
}) {
  const [pending, setPending] = useState<string | null>(null);
  const scoped = Boolean(repositoryRoot && runId);
  const runNative = async (
    node: ScheduledNode,
    kind: "admit" | "heartbeat" | "complete" | "fail"
  ) => {
    if (!repositoryRoot || !runId || pending) return;
    setPending(`${kind}:${node.id}`);
    try {
      if (kind === "admit") {
        await orchestratorClaim({ repositoryRoot, runId, nodeId: node.id });
      } else if (kind === "heartbeat") {
        await orchestratorHeartbeat({ repositoryRoot, runId, nodeId: node.id });
      } else if (kind === "complete") {
        await orchestratorComplete({
          repositoryRoot,
          runId,
          nodeId: node.id,
          artifacts: [],
        });
      } else {
        await orchestratorFail({ repositoryRoot, runId, nodeId: node.id });
      }
      onChanged?.();
    } catch (cause) {
      onError?.(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPending(null);
    }
  };
  return (
    <section
      className="pipeline-gate-panel pipeline-nodes"
      id="pp-orch-region-nodes"
      aria-labelledby="pp-orch-heading-nodes"
    >
      <header>
        <h2 id="pp-orch-heading-nodes">Workers, leases and evidence</h2>
        <strong>{nodes.length} nodes</strong>
      </header>
      {nodes.length ? (
        <div className="pipeline-node-list">
          {nodes.map((node) => {
            const token = domToken(node.id);
            const evidence = node.evidence || [];
            const nativeCompletionSupported =
              node.profile === "headless" || node.profile === "docs";
            const verificationConfigured = Boolean(node.verificationCommands?.length);
            return (
      <details
                className={`pipeline-node pipeline-node-${node.status.toLowerCase()}`}
                id={`pp-orch-node-${token}`}
                key={node.id}
                data-node-id={node.id}
                data-worker-id={node.lease?.workerId || "unclaimed"}
                data-lease-fence={node.lease ? String(node.lease.fence) : "none"}
                data-attempts={node.attempts}
                data-node-status={node.status}
                data-context-kind="node"
                data-context-id={node.id}
                data-context-label={`${node.id} · ${node.title || `Plan node ${node.id}`}`}
              >
                <summary id={`pp-orch-btn-toggle-node-${token}`}>
                  <span>{node.id}</span>
                  <strong>{node.title || `Plan node ${node.id}`}</strong>
                  <span>Wave {node.wave}</span>
                  <span>{node.attempts}/3 attempts</span>
                  <span>{labelize(node.status)}</span>
                </summary>
                <div className="pipeline-node-detail">
                  <dl>
                    <div>
                      <dt>Worker</dt>
                      <dd>{node.lease?.workerId || "Unclaimed"}</dd>
                    </div>
                    <div>
                      <dt>Fence</dt>
                      <dd>{node.lease?.fence ?? "No live fence"}</dd>
                    </div>
                    <div>
                      <dt>Lease expiry</dt>
                      <dd>
                        {node.lease
                          ? formatTime(node.lease.expiresAtMs)
                          : "No active lease"}
                      </dd>
                    </div>
                    <div>
                      <dt>Evidence profile</dt>
                      <dd>{node.profile ? labelize(node.profile) : "Not recorded"}</dd>
                    </div>
                  </dl>
                  <div className="pipeline-node-actions">
                    <button
                      type="button"
                      id={`pp-orch-btn-admit-${token}`}
                      disabled={
                        !scoped ||
                        busy ||
                        pending !== null ||
                        !approved ||
                        !preflightFresh ||
                        Boolean(node.lease) ||
                        node.status.toLowerCase() !== "ready"
                      }
                      onClick={() => void runNative(node, "admit")}
                    >
                      {pending === `admit:${node.id}` ? "Admitting…" : "Admit worker"}
                    </button>
                    <button
                      type="button"
                      id={`pp-orch-btn-heartbeat-${token}`}
                      disabled={!scoped || busy || pending !== null || !node.lease}
                      onClick={() => void runNative(node, "heartbeat")}
                    >
                      {pending === `heartbeat:${node.id}` ? "Renewing…" : "Heartbeat"}
                    </button>
                    <button
                      type="button"
                      id={`pp-orch-btn-complete-${token}`}
                      disabled={
                        !scoped ||
                        busy ||
                        pending !== null ||
                        !node.lease ||
                        !nativeCompletionSupported ||
                        !verificationConfigured
                      }
                      onClick={() => void runNative(node, "complete")}
                    >
                      {pending === `complete:${node.id}` ? "Validating…" : "Validate & complete"}
                    </button>
                    <button
                      type="button"
                      id={`pp-orch-btn-fail-${token}`}
                      disabled={!scoped || busy || pending !== null || !node.lease}
                      onClick={() => void runNative(node, "fail")}
                    >
                      {pending === `fail:${node.id}` ? "Recording…" : "Record failure"}
                    </button>
                    <p>
                      Admission is native-only. The UI names the bound run and node; it cannot
                      pick a worker id, lease, clock, changed-file list, or verification result.
                    </p>
                    {!preflightFresh ? (
                      <p role="status">
                        Admission is disabled until native preflight is refreshed and this run is
                        re-approved.
                      </p>
                    ) : !approved ? (
                      <p role="status">Admission is disabled until this exact run is approved.</p>
                    ) : null}
                    {!verificationConfigured ? (
                      <p role="status">
                        Completion is disabled because this approved node has no verification
                        command.
                      </p>
                    ) : !nativeCompletionSupported ? (
                      <p role="status">
                        Completion is disabled for {labelize(node.profile || "unknown")} evidence
                        until required screenshots or migration proof are attached through a native
                        evidence picker.
                      </p>
                    ) : null}
                  </div>
                  <div>
                    <h3>Allowed files</h3>
                    {node.allowedFiles?.length ? (
                      <ul>
                        {node.allowedFiles.map((file) => (
                          <li key={file}>
                            <code>{file}</code>
                          </li>
                        ))}
                      </ul>
                    ) : (
                      <p>Manifest files are not present in this snapshot.</p>
                    )}
                  </div>
                  <div>
                    <h3>Evidence</h3>
                    {evidence.length ? (
                      <ul className="pipeline-evidence-list">
                        {evidence.map((artifact, index) => (
                          <li
                            key={`${artifact.kind}-${artifact.sha256}-${index}`}
                            data-context-kind="evidence"
                            data-context-id={`${node.id}:${artifact.kind}:${artifact.sha256}`}
                            data-context-label={`${labelize(artifact.kind)} · ${artifact.path}`}
                          >
                            <span
                              data-evidence-id={`${node.id}:${artifact.kind}:${artifact.sha256}`}
                              data-evidence-kind={artifact.kind}
                            >
                              <strong>{labelize(artifact.kind)}</strong>
                            </span>
                            <code>{artifact.path}</code>
                            <span>{formatBytes(artifact.bytes)}</span>
                            <small title={artifact.sha256}>
                              SHA-256 {artifact.sha256.slice(0, 12)}…
                            </small>
                          </li>
                        ))}
                      </ul>
                    ) : (
                      <p>No evidence artifacts are attached.</p>
                    )}
                  </div>
                </div>
              </details>
            );
          })}
        </div>
      ) : (
        <p id="pp-orch-empty-nodes">No scheduler nodes were recorded.</p>
      )}
    </section>
  );
}

function ReconciliationPanel({ snapshot }: { snapshot: OrchestratorSnapshot }) {
  const result = snapshot.reconciliation;
  const violations = result
    ? [...result.fatal, ...result.unplanned, ...result.unproven, ...result.orphaned]
    : [];
  return (
    <section
      className="pipeline-gate-panel pipeline-reconciliation"
      id="pp-orch-region-reconciliation"
      aria-labelledby="pp-orch-heading-reconciliation"
    >
      <header>
        <h2 id="pp-orch-heading-reconciliation">Reconciliation gate</h2>
        <strong>{result ? (result.passed ? "Passed" : "Blocked") : "Not run"}</strong>
      </header>
      {!result ? (
        <p>No planned-versus-actual reconciliation is recorded.</p>
      ) : violations.length ? (
        <ul id="pp-orch-list-reconciliation-violations">
          {violations.map((violation) => (
            <li key={violation.violationId}>
              <strong>{labelize(violation.category)}</strong>
              <span>{violation.summary}</span>
              {violation.waivedBy.length ? (
                <small>Waived by {violation.waivedBy.join(", ")}</small>
              ) : (
                <small>Active violation</small>
              )}
            </li>
          ))}
        </ul>
      ) : (
        <p>Every planned output has an exact committed relationship.</p>
      )}
    </section>
  );
}

function ReleasePanel({ snapshot }: { snapshot: OrchestratorSnapshot }) {
  const release = snapshot.release;
  return (
    <section
      className="pipeline-gate-panel pipeline-release"
      id="pp-orch-region-release"
      aria-labelledby="pp-orch-heading-release"
    >
      <header>
        <h2 id="pp-orch-heading-release">Release gate</h2>
        <strong>
          {release
            ? release.merged
              ? "Merged"
              : release.readyToMerge
                ? "Ready to merge"
                : release.readyForPr
                  ? "Ready for PR"
                  : "Blocked"
            : "Not run"}
        </strong>
      </header>
      {release?.issues.length ? (
        <ul id="pp-orch-list-release-issues">
          {release.issues.map((issue, index) => (
            <li key={`${issue.kind}-${index}`} data-issue-kind={issue.kind}>
              <strong>{labelize(issue.kind)}</strong>
              <span>{issue.message}</span>
              {issue.decisionRequired ? <mark>Decision required</mark> : null}
            </li>
          ))}
        </ul>
      ) : (
        <p>{release ? "No release issues are recorded." : "Release checks have not run."}</p>
      )}
      {snapshot.delivery ? (
        <>
          <dl className="pipeline-delivery-summary" id="pp-orch-region-delivery-summary">
            <div>
              <dt>Handover</dt>
              <dd>
                <code>{snapshot.delivery.handoverDir}</code>
              </dd>
            </div>
            <div>
              <dt>Archive</dt>
              <dd>
                <code>{snapshot.delivery.archiveDir}</code>
              </dd>
            </div>
            <div>
              <dt>Leftovers</dt>
              <dd>{snapshot.delivery.leftoversCount}</dd>
            </div>
          </dl>
          {snapshot.leftovers.length ? (
            <div className="pipeline-leftovers" id="pp-orch-region-leftovers">
              <h3>Outstanding items and errors</h3>
              <ul>
                {snapshot.leftovers.map((leftover) => (
                  <li key={leftover.id} id={`pp-orch-leftover-${domToken(leftover.id)}`}>
                    <strong>{leftover.what}</strong>
                    <code>{leftover.location}</code>
                    <span>{labelize(leftover.severity)}</span>
                    <p>{leftover.suggestedNextAction}</p>
                  </li>
                ))}
              </ul>
            </div>
          ) : snapshot.delivery.leftoversCount ? (
            <p role="alert">
              {snapshot.delivery.leftoversCount} leftovers were counted, but their structured
              records are missing from this snapshot.
            </p>
          ) : (
            <p>No outstanding items were delivered.</p>
          )}
        </>
      ) : null}
    </section>
  );
}

function EventRows({ events }: { events: OrchestratorEvent[] }) {
  return events.length ? (
    <ol className="pipeline-audit-events" id="pp-orch-list-audit-events">
      {events.map((event, index) => (
        <li
          key={`${event.ts}-${event.type}-${event.nodeId || "run"}-${index}`}
          data-event-type={event.type}
          data-audit-event-id={
            eventDataText(event, "eventId") ||
            `${event.runId}:${event.type}:${event.nodeId || "run"}:${event.ts}`
          }
          data-repository-id={eventDataText(event, "repositoryId")}
          data-plan-id={eventDataText(event, "planId")}
        >
          <time dateTime={event.ts}>{formatTime(event.ts)}</time>
          <strong>{labelize(event.type)}</strong>
          <span>{event.worker}</span>
          <code>{event.nodeId || "RUN"}</code>
          <p>{event.msg}</p>
        </li>
      ))}
    </ol>
  ) : (
    <p id="pp-orch-empty-audit-events">No events match these filters.</p>
  );
}

function ChangeRows({ changes }: { changes: ChangeComparison[] }) {
  return changes.length ? (
    <div className="pipeline-table-scroll">
      <table id="pp-orch-table-changes-succeeded">
        <caption>Desired changes compared with actual committed work</caption>
        <thead>
          <tr>
            <th scope="col">Desired change</th>
            <th scope="col">Actual committed</th>
            <th scope="col">Result</th>
          </tr>
        </thead>
        <tbody>
          {changes.map((change) => (
            <tr
              key={change.id}
              data-change-status={change.status}
              data-reconciliation-row-id={change.id}
              data-reconciliation-status={change.status}
            >
              <td data-reconciliation-column="desired">
                {change.nodeId ? <code>{change.nodeId}</code> : null}
                <span>{change.desired}</span>
              </td>
              <td data-reconciliation-column="actual">
                {change.actualCommit ? (
                  <code>{change.actualCommit}</code>
                ) : (
                  <strong>Missing</strong>
                )}
                {change.details?.length ? <small>{change.details.join(" · ")}</small> : null}
              </td>
              <td>
                <strong>{labelize(change.status)}</strong>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  ) : (
    <p id="pp-orch-empty-changes">No changes match these filters.</p>
  );
}

function AuditDrawer({ snapshot }: { snapshot: OrchestratorSnapshot }) {
  const [tab, setTab] = useState<AuditTab>("logs");
  const [height, setHeight] = useState(320);
  const [maximized, setMaximized] = useState(false);
  const [collapsed, setCollapsed] = useState(false);
  const [search, setSearch] = useState("");
  const [worker, setWorker] = useState("all");
  const [eventType, setEventType] = useState<"all" | OrchestratorEventType>("all");
  const [problemsOnly, setProblemsOnly] = useState(false);
  const dragRef = useRef<{ y: number; height: number } | null>(null);

  const clampHeight = useCallback((value: number) => {
    const maximum = typeof window === "undefined" ? 900 : Math.max(240, window.innerHeight - 32);
    return Math.min(maximum, Math.max(180, value));
  }, []);

  const resizeBy = (delta: number) => {
    setCollapsed(false);
    setMaximized(false);
    setHeight((current) => clampHeight(current + delta));
  };

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    dragRef.current = { y: event.clientY, height };
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!dragRef.current) return;
    setCollapsed(false);
    setMaximized(false);
    setHeight(clampHeight(dragRef.current.height + dragRef.current.y - event.clientY));
  };

  const onPointerEnd = (event: ReactPointerEvent<HTMLDivElement>) => {
    dragRef.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };

  const onSeparatorKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "ArrowUp") resizeBy(24);
    else if (event.key === "ArrowDown") resizeBy(-24);
    else if (event.key === "Home") {
      setMaximized(false);
      setCollapsed(false);
      setHeight(180);
    } else if (event.key === "End") {
      setCollapsed(false);
      setMaximized(true);
    } else return;
    event.preventDefault();
  };

  const activateTab = (next: AuditTab) => {
    setTab(next);
    window.setTimeout(() => document.getElementById(`pp-orch-tab-audit-${next}`)?.focus(), 0);
  };

  const onTabKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === "Home") {
      event.preventDefault();
      activateTab("logs");
    } else if (event.key === "End") {
      event.preventDefault();
      activateTab("changes");
    } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      activateTab(tab === "logs" ? "changes" : "logs");
    }
  };

  const workers = useMemo(
    () => [...new Set(snapshot.events.map((event) => event.worker))].sort(),
    [snapshot.events]
  );
  const normalizedSearch = search.trim().toLocaleLowerCase();
  const filteredEvents = useMemo(
    () =>
      [...snapshot.events]
        .filter((event) => worker === "all" || event.worker === worker)
        .filter((event) => eventType === "all" || event.type === eventType)
        .filter((event) => !problemsOnly || PROBLEM_EVENTS.has(event.type))
        .filter(
          (event) =>
            !normalizedSearch ||
            `${event.msg} ${event.worker} ${event.nodeId || ""} ${event.type}`
              .toLocaleLowerCase()
              .includes(normalizedSearch)
        )
        .sort((left, right) => right.ts.localeCompare(left.ts)),
    [eventType, normalizedSearch, problemsOnly, snapshot.events, worker]
  );
  const filteredChanges = useMemo(
    () =>
      snapshot.changes.filter((change) => {
        if (problemsOnly && change.status === "succeeded") return false;
        if (!normalizedSearch) return true;
        return `${change.nodeId || ""} ${change.desired} ${change.actualCommit || ""} ${change.status}`
          .toLocaleLowerCase()
          .includes(normalizedSearch);
      }),
    [normalizedSearch, problemsOnly, snapshot.changes]
  );

  const drawerHeight = collapsed
    ? "auto"
    : maximized
      ? "calc(100vh - 24px)"
      : `${height}px`;
  const style = {
    height: drawerHeight,
    "--pp-orch-audit-height": drawerHeight,
  } as CSSProperties;

  return (
    <section
      className={`pipeline-audit-drawer${maximized ? " is-maximized" : ""}${collapsed ? " is-collapsed" : ""}`}
      id="pp-orch-region-audit-drawer"
      aria-labelledby="pp-orch-heading-audit-drawer"
      style={style}
      data-state={collapsed ? "collapsed" : maximized ? "maximized" : "open"}
      data-context-kind="modal"
      data-context-id="audit-drawer"
      data-context-label="Bottom audit log"
      data-context-close="#pp-orch-btn-audit-collapse"
    >
      <div
        className="pipeline-audit-resize-handle"
        id="pp-orch-control-audit-resize"
        role="separator"
        tabIndex={0}
        aria-label="Resize audit drawer"
        aria-orientation="horizontal"
        aria-valuemin={180}
        aria-valuemax={typeof window === "undefined" ? 900 : Math.max(240, window.innerHeight - 32)}
        aria-valuenow={maximized ? Math.max(240, window.innerHeight - 32) : height}
        aria-valuetext={maximized ? "Full screen" : `${height} pixels high`}
        aria-controls="pp-orch-panel-audit-active"
        onDoubleClick={() => {
          setCollapsed(false);
          setMaximized((current) => !current);
        }}
        onKeyDown={onSeparatorKeyDown}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerEnd}
        onPointerCancel={onPointerEnd}
      >
        <span aria-hidden="true" />
      </div>
      <header className="pipeline-audit-header">
        <div>
          <p>AUDIT STREAM</p>
          <h2 id="pp-orch-heading-audit-drawer">Recorded proof and delivery truth</h2>
        </div>
        <div className="pipeline-audit-size-controls" aria-label="Audit drawer size controls">
          <button
            type="button"
            id="pp-orch-btn-audit-smaller"
            onClick={() => resizeBy(-80)}
            aria-label="Make audit drawer smaller"
          >
            Smaller
          </button>
          <button
            type="button"
            id="pp-orch-btn-audit-larger"
            onClick={() => resizeBy(80)}
            aria-label="Make audit drawer larger"
          >
            Larger
          </button>
          <button
            type="button"
            id="pp-orch-btn-audit-maximize"
            aria-pressed={maximized}
            onClick={() => {
              setCollapsed(false);
              setMaximized((current) => !current);
            }}
          >
            {maximized ? "Restore" : "Maximize"}
          </button>
          <button
            type="button"
            id="pp-orch-btn-audit-collapse"
            aria-expanded={!collapsed}
            aria-controls="pp-orch-panel-audit-active"
            onClick={() => {
              setMaximized(false);
              setCollapsed((current) => !current);
            }}
          >
            {collapsed ? "Open" : "Collapse"}
          </button>
        </div>
      </header>
      {!collapsed ? (
        <>
          <div role="tablist" aria-label="Audit views" className="pipeline-audit-tabs">
            <button
              type="button"
              role="tab"
              id="pp-orch-tab-audit-logs"
              aria-selected={tab === "logs"}
              aria-controls="pp-orch-panel-audit-active"
              tabIndex={tab === "logs" ? 0 : -1}
              onClick={() => setTab("logs")}
              onKeyDown={onTabKeyDown}
            >
              LOGS
            </button>
            <button
              type="button"
              role="tab"
              id="pp-orch-tab-audit-changes"
              aria-selected={tab === "changes"}
              aria-controls="pp-orch-panel-audit-active"
              tabIndex={tab === "changes" ? 0 : -1}
              onClick={() => setTab("changes")}
              onKeyDown={onTabKeyDown}
            >
              CHANGES / SUCCEEDED
            </button>
          </div>
          <form className="pipeline-audit-filters" onSubmit={(event) => event.preventDefault()}>
            <label htmlFor="pp-orch-input-audit-search">Search</label>
            <input
              id="pp-orch-input-audit-search"
              type="search"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder="Message, node, commit or result"
            />
            <label htmlFor="pp-orch-select-audit-worker">Worker</label>
            <select
              id="pp-orch-select-audit-worker"
              value={worker}
              onChange={(event) => setWorker(event.target.value)}
            >
              <option value="all">All workers</option>
              {workers.map((name) => (
                <option value={name} key={name}>
                  {name}
                </option>
              ))}
            </select>
            <label htmlFor="pp-orch-select-audit-event">Event</label>
            <select
              id="pp-orch-select-audit-event"
              value={eventType}
              onChange={(event) =>
                setEventType(event.target.value as "all" | OrchestratorEventType)
              }
              disabled={tab !== "logs"}
            >
              <option value="all">All event types</option>
              {[
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
              ].map((type) => (
                <option value={type} key={type}>
                  {labelize(type)}
                </option>
              ))}
            </select>
            <label className="pipeline-audit-problems" htmlFor="pp-orch-check-audit-problems">
              <input
                id="pp-orch-check-audit-problems"
                type="checkbox"
                checked={problemsOnly}
                onChange={(event) => setProblemsOnly(event.target.checked)}
              />
              Problems only
            </label>
          </form>
          <div
            role="tabpanel"
            id="pp-orch-panel-audit-active"
            aria-labelledby={`pp-orch-tab-audit-${tab}`}
            className="pipeline-audit-panel"
          >
            {tab === "logs" ? (
              <EventRows events={filteredEvents} />
            ) : (
              <ChangeRows changes={filteredChanges} />
            )}
          </div>
        </>
      ) : null}
    </section>
  );
}

export function PipelineConsole({
  runId,
  repositoryRoot,
  planPath,
  parallelAgents = true,
  snapshotSeed,
  snapshot: suppliedSnapshot,
  pollIntervalMs = 5_000,
  className = "",
  onSnapshotChange,
  onSelectRun,
  onRunCreated,
  onReviewDecision,
  onDiagnostic,
}: PipelineConsoleProps) {
  const [loadedSnapshot, setLoadedSnapshot] = useState<OrchestratorSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [approving, setApproving] = useState(false);
  const [reaping, setReaping] = useState(false);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [catalogRuns, setCatalogRuns] = useState<{
    active: PipelineRunSummary[];
    completed: PipelineRunSummary[];
  }>({ active: [], completed: [] });
  const catalogGeneration = useRef(0);
  const catalogRequest = useRef<{ generation: number; key: string } | null>(null);
  const refreshGeneration = useRef(0);
  const refreshRequest = useRef<{ generation: number; key: string } | null>(null);
  const onDiagnosticRef = useRef(onDiagnostic);
  const onSnapshotChangeRef = useRef(onSnapshotChange);
  useEffect(() => {
    onDiagnosticRef.current = onDiagnostic;
  }, [onDiagnostic]);
  useEffect(() => {
    onSnapshotChangeRef.current = onSnapshotChange;
  }, [onSnapshotChange]);
  const snapshot = suppliedSnapshot === undefined ? loadedSnapshot : suppliedSnapshot;
  const snapshotOrganizationId = snapshotSeed?.organizationId;
  const snapshotRepositoryId = snapshotSeed?.repositoryId;
  const snapshotWorktreePath = snapshotSeed?.worktreePath;
  const snapshotPlanId = snapshotSeed?.planId;
  const snapshotTitle = snapshotSeed?.title;
  const stableSnapshotSeed = useMemo<PipelineSnapshotSeed | undefined>(() => {
    if (
      snapshotOrganizationId === undefined &&
      snapshotRepositoryId === undefined &&
      snapshotWorktreePath === undefined &&
      snapshotPlanId === undefined &&
      snapshotTitle === undefined
    ) return undefined;
    return {
      organizationId: snapshotOrganizationId,
      repositoryId: snapshotRepositoryId,
      worktreePath: snapshotWorktreePath,
      planId: snapshotPlanId,
      title: snapshotTitle,
    };
  }, [
    snapshotOrganizationId,
    snapshotPlanId,
    snapshotRepositoryId,
    snapshotTitle,
    snapshotWorktreePath,
  ]);
  const catalogScopeKey = `${repositoryRoot ? comparableWindowsPath(repositoryRoot) : ""}\u0000${
    planPath ? comparableWindowsPath(planPath) : ""
  }`;
  const refreshScopeKey = `${catalogScopeKey}\u0000${runId || ""}`;
  const catalogScopeKeyRef = useRef(catalogScopeKey);
  const refreshScopeKeyRef = useRef(refreshScopeKey);
  catalogScopeKeyRef.current = catalogScopeKey;
  refreshScopeKeyRef.current = refreshScopeKey;

  const loadCatalog = useCallback(async () => {
    if (!repositoryRoot || catalogRequest.current?.key === catalogScopeKey) return;
    const generation = ++catalogGeneration.current;
    catalogRequest.current = { generation, key: catalogScopeKey };
    setCatalogLoading(true);
    try {
      const catalog = await orchestratorRunCatalog({ repositoryRoot });
      if (
        catalogScopeKeyRef.current !== catalogScopeKey ||
        catalogRequest.current?.generation !== generation
      ) return;
      const activePlan = planPath ? comparableWindowsPath(planPath) : undefined;
      const summary = (entry: (typeof catalog.activeRuns)[number]): PipelineRunSummary => {
        const normalized = entry.status.toLocaleLowerCase();
        const status: PipelineRunStatus = normalized === "completed"
          ? "completed"
          : normalized === "blocked"
            ? "blocked"
            : normalized === "running"
              ? "running"
              : "pending";
        return {
          organizationId: snapshotOrganizationId || "local-machine",
          repositoryId: snapshotRepositoryId || repositoryRoot,
          repositoryRoot,
          worktreePath: repositoryRoot,
          branch: entry.branch,
          runId: entry.runId,
          planId: entry.planId,
          title: entry.planId || entry.runId,
          status,
          completedNodes: entry.completedNodes,
          totalNodes: entry.totalNodes,
          updatedAt: new Date(entry.updatedAt).toISOString(),
        };
      };
      const belongsToPlan = (entry: (typeof catalog.activeRuns)[number]) =>
        !activePlan || comparableWindowsPath(entry.planPath) === activePlan;
      setCatalogRuns({
        active: catalog.activeRuns.filter(belongsToPlan).map(summary),
        completed: catalog.archivedRuns.filter(belongsToPlan).map(summary),
      });
    } catch (cause) {
      if (
        catalogScopeKeyRef.current !== catalogScopeKey ||
        catalogRequest.current?.generation !== generation
      ) return;
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message);
      onDiagnosticRef.current?.("error", message);
    } finally {
      if (catalogRequest.current?.generation === generation) {
        catalogRequest.current = null;
        setCatalogLoading(false);
      }
    }
  }, [
    catalogScopeKey,
    planPath,
    repositoryRoot,
    snapshotOrganizationId,
    snapshotRepositoryId,
  ]);

  useEffect(() => {
    catalogGeneration.current += 1;
    catalogRequest.current = null;
    setCatalogLoading(false);
    setCatalogRuns({ active: [], completed: [] });
  }, [catalogScopeKey]);

  useEffect(() => {
    if (repositoryRoot && !runId) void loadCatalog();
  }, [loadCatalog, repositoryRoot, runId]);

  const refresh = useCallback(async () => {
    if (!runId || !repositoryRoot) {
      if (!repositoryRoot) setError("Select a repository before requesting an orchestrator snapshot.");
      else if (!runId) setError(null);
      return;
    }
    if (refreshRequest.current?.key === refreshScopeKey) return;
    const generation = ++refreshGeneration.current;
    refreshRequest.current = { generation, key: refreshScopeKey };
    setLoading(true);
    try {
      const next = await orchestratorConsoleSnapshot(
        { repositoryRoot, runId },
        stableSnapshotSeed
      );
      if (
        refreshScopeKeyRef.current !== refreshScopeKey ||
        refreshRequest.current?.generation !== generation
      ) return;
      setLoadedSnapshot(next);
      onSnapshotChangeRef.current?.(next);
      setError(null);
      onDiagnosticRef.current?.("info", `Verified orchestrator snapshot ${next.run.runId}.`);
    } catch (cause) {
      if (
        refreshScopeKeyRef.current !== refreshScopeKey ||
        refreshRequest.current?.generation !== generation
      ) return;
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message);
      onDiagnosticRef.current?.("error", message);
    } finally {
      if (refreshRequest.current?.generation === generation) {
        refreshRequest.current = null;
        setLoading(false);
      }
    }
  }, [refreshScopeKey, repositoryRoot, runId, stableSnapshotSeed]);

  useEffect(() => {
    if (!runId) {
      refreshGeneration.current += 1;
      refreshRequest.current = null;
      setLoadedSnapshot(null);
      setLoading(false);
      setError(null);
      return;
    }
    void refresh();
    if (!Number.isFinite(pollIntervalMs) || pollIntervalMs < 2_000) return;
    const timer = window.setInterval(() => void refresh(), pollIntervalMs);
    return () => window.clearInterval(timer);
  }, [pollIntervalMs, refresh, runId]);

  const stages = useMemo(() => (snapshot ? visibleStages(snapshot) : []), [snapshot]);
  const nodes = useMemo(() => (snapshot ? nodeValues(snapshot) : []), [snapshot]);
  const warnings = useMemo(() => (snapshot ? collectedWarnings(snapshot) : []), [snapshot]);
  const activeRuns = useMemo(
    () => (snapshot ? currentAndShelfRuns(snapshot, "active") : []),
    [snapshot]
  );
  const completedRuns = useMemo(
    () => (snapshot ? currentAndShelfRuns(snapshot, "completed") : []),
    [snapshot]
  );

  const createRun = useCallback(async () => {
    if (!repositoryRoot || !planPath || creating) return;
    setCreating(true);
    setError(null);
    try {
      const runId = `run-${Date.now().toString(36)}`;
      await orchestratorCreateRun({
        repositoryRoot,
        runId,
        planPath,
        parallelAgents,
        nextActions: [
          "Run the native system and collision preflight",
          parallelAgents
            ? "Admit dependency-ready workers through the bounded parallel scheduler"
            : "Admit one dependency-ready worker at a time through the serial scheduler",
          "Validate and durably record evidence before completion",
        ],
      });
      onRunCreated?.({ repositoryRoot, runId });
      onDiagnostic?.("info", `Created native orchestrator run ${runId}.`);
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message);
      onDiagnostic?.("error", message);
    } finally {
      setCreating(false);
    }
  }, [creating, onDiagnostic, onRunCreated, parallelAgents, planPath, repositoryRoot]);

  const reapExpired = useCallback(async () => {
    if (!repositoryRoot || !runId || reaping) return;
    setReaping(true);
    try {
      const actions = await orchestratorReap({ repositoryRoot, runId });
      onDiagnostic?.("info", `Native lease reaper recorded ${actions.length} action(s).`);
      await refresh();
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message);
      onDiagnostic?.("error", message);
    } finally {
      setReaping(false);
    }
  }, [onDiagnostic, reaping, refresh, repositoryRoot, runId]);

  const approveRun = useCallback(async () => {
    if (!repositoryRoot || !runId || approving) return;
    setApproving(true);
    setError(null);
    try {
      const receipt = await orchestratorApproveRun({ repositoryRoot, runId });
      onDiagnostic?.(
        "info",
        `Approved ${receipt.collisionAssessments.length} node(s) under native collision census.`
      );
      await refresh();
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message);
      onDiagnostic?.("error", message);
    } finally {
      setApproving(false);
    }
  }, [approving, onDiagnostic, refresh, repositoryRoot, runId]);

  return (
    <section
      className={`pipeline-console ${className}`.trim()}
      id="pp-orch-pipeline-console"
      aria-labelledby="pp-orch-heading-console"
      data-repository-id={snapshot?.run.repositoryId}
      data-context-surface="pipeline"
      data-context-label="Head orchestrator pipeline"
    >
      <header className="pipeline-console-header">
        <div>
          <p>HEAD ORCHESTRATOR</p>
          <h1 id="pp-orch-heading-console">
            {snapshot?.run.title || "Orchestrator pipeline"}
          </h1>
          <p id="pp-orch-console-scope">
            {snapshot
              ? `${snapshot.run.repositoryId} · ${snapshot.run.branch} · ${snapshot.run.runId}`
              : "No run snapshot loaded"}
          </p>
        </div>
        <div className="pipeline-console-header-actions">
          {repositoryRoot && planPath && !runId ? (
            <button
              type="button"
              id="pp-orch-btn-create-run"
              onClick={() => void createRun()}
              disabled={creating}
              aria-describedby={error ? "pp-orch-error-pipeline" : undefined}
            >
              {creating ? "Creating…" : "Create native run"}
            </button>
          ) : null}
          {repositoryRoot && !runId ? (
            <button
              type="button"
              id="pp-orch-btn-load-runs"
              onClick={() => void loadCatalog()}
              disabled={catalogLoading}
            >
              {catalogLoading ? "Loading…" : "Load saved runs"}
            </button>
          ) : null}
          {repositoryRoot && runId ? (
            <button
              type="button"
              id="pp-orch-btn-approve-run"
              onClick={() => void approveRun()}
              disabled={
                loading ||
                approving ||
                Boolean(snapshot?.runApproval) ||
                !snapshot?.preflightFresh ||
                snapshot?.preflight?.disposition !== "ready"
              }
            >
              {snapshot?.runApproval
                ? "Run approved"
                : approving
                  ? "Approving…"
                  : "Approve run"}
            </button>
          ) : null}
          {repositoryRoot && runId ? (
            <button
              type="button"
              id="pp-orch-btn-reap-expired"
              onClick={() => void reapExpired()}
              disabled={loading || reaping}
            >
              {reaping ? "Recovering…" : "Recover expired leases"}
            </button>
          ) : null}
          <button
            type="button"
            id="pp-orch-btn-refresh-pipeline"
            onClick={() => void refresh()}
            disabled={!runId || !repositoryRoot || loading}
            aria-describedby={error ? "pp-orch-error-pipeline" : undefined}
          >
            {loading ? "Refreshing…" : "Refresh pipeline"}
          </button>
        </div>
      </header>

      {error ? (
        <div id="pp-orch-error-pipeline" role="alert" className="pipeline-console-error">
          Pipeline unavailable: {error}
        </div>
      ) : null}

      {!snapshot ? (
        <div className="pipeline-console-empty" id="pp-orch-empty-console" role="status">
          {repositoryRoot && !runId ? (
            <>
              <strong>Pipeline not initialized for this plan</strong>
              <span>No verified orchestrator run is bound to the selected Perfect Plan.</span>
              <dl>
                <div><dt>Where</dt><dd>{repositoryRoot}</dd></div>
                <div><dt>Remedy</dt><dd>Create a native run or explicitly select a recorded run. Plan IDs are never guessed as run IDs.</dd></div>
              </dl>
            </>
          ) : "No evidence is displayed because no verified pipeline snapshot is available."}
          {catalogRuns.active.length || catalogRuns.completed.length ? (
            <div className="pipeline-shelves" id="pp-orch-region-saved-run-shelves">
              <RunShelf
                id="active"
                title="Saved in progress"
                runs={catalogRuns.active}
                onSelectRun={onSelectRun}
              />
              <RunShelf
                id="completed"
                title="Saved completed"
                runs={catalogRuns.completed}
                onSelectRun={onSelectRun}
              />
            </div>
          ) : null}
        </div>
      ) : (
        <>
          {warnings.length ? (
            <aside
              className="pipeline-warning-stack"
              id="pp-orch-region-persistent-warnings"
              aria-labelledby="pp-orch-heading-persistent-warnings"
            >
              <h2 id="pp-orch-heading-persistent-warnings">
                Persistent warnings · {warnings.length}
              </h2>
              <ul role="list">
                {warnings.map((warning) => {
                  const token = domToken(warning.id);
                  return (
                    <li
                      key={warning.id}
                      id={`pp-orch-warning-${token}`}
                      role={warning.decisionRequired ? "alert" : undefined}
                      data-severity={warning.severity}
                      data-decision-status={warning.decisionRequired ? "pending" : undefined}
                      data-issue-kind={warning.issueKind}
                    >
                      <strong>
                        {warning.decisionRequired ? "Decision required" : "Warning"}
                      </strong>
                      <span>{warning.message}</span>
                      {warning.nodeId ? <code>{warning.nodeId}</code> : null}
                      <time dateTime={warning.createdAt}>{formatTime(warning.createdAt)}</time>
                      {warning.decisionRequired && onReviewDecision ? (
                        <button
                          type="button"
                          id={`pp-orch-btn-review-warning-${token}`}
                          onClick={() => onReviewDecision(warning)}
                        >
                          Review decision
                        </button>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
            </aside>
          ) : (
            <p className="pipeline-no-warnings" id="pp-orch-status-no-warnings">
              No active warnings are recorded.
            </p>
          )}

          <div className="pipeline-shelves" id="pp-orch-region-run-shelves">
            <RunShelf
              id="active"
              title="In progress"
              runs={activeRuns}
              onSelectRun={onSelectRun}
            />
            <RunShelf
              id="completed"
              title="Completed"
              runs={completedRuns}
              onSelectRun={onSelectRun}
            />
          </div>

          <PipelineSpine stages={stages} />
          <PreflightPanel
            snapshot={snapshot}
            repositoryRoot={repositoryRoot}
            runId={runId}
            busy={loading}
            onRan={() => void refresh()}
            onError={setError}
          />
          <NodePanel
            nodes={nodes}
            approved={Boolean(snapshot.runApproval)}
            preflightFresh={snapshot.preflightFresh}
            repositoryRoot={repositoryRoot}
            runId={runId}
            busy={loading}
            onChanged={() => void refresh()}
            onError={setError}
          />
          <ReconciliationPanel snapshot={snapshot} />
          <ReleasePanel snapshot={snapshot} />
          <AuditDrawer snapshot={snapshot} />
        </>
      )}
    </section>
  );
}

export default PipelineConsole;
