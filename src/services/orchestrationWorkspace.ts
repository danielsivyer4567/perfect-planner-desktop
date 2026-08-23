import type { Board, WorkerAssignment } from "./boards";
import type { ControlMessage, ControlPlaneSnapshot } from "./controlPlane";
import type { OrchestratorSnapshot } from "./orchestratorPipeline";

export type WorkspaceTone = "healthy" | "active" | "waiting" | "blocked" | "unknown";

export interface WorkspaceStatus {
  tone: WorkspaceTone;
  healthLabel: string;
  orchestrationLabel: string;
  messagingLabel: string;
  ciLabel: string;
  nextAction: string;
  latestActivity: string;
}

export interface WorkspaceStatusInput {
  board: Board | null;
  pipeline: OrchestratorSnapshot | null;
  controlPlane: ControlPlaneSnapshot | null;
  workers: WorkerAssignment[];
  decisionCount: number;
  identityError: string | null;
  supervisorError: string | null;
}

function newestScopedMessage(
  snapshot: ControlPlaneSnapshot | null,
  board: Board | null,
): ControlMessage | null {
  if (!snapshot || !board) return null;
  return [...snapshot.messages]
    .filter((message) =>
      message.scope.repositoryRoot === board.repoRoot &&
      message.scope.planPath === board.planPath
    )
    .sort((left, right) => right.updatedAtMs - left.updatedAtMs)[0] || null;
}

/**
 * Projects durable, already-observed state into the compact command surface.
 * Missing data remains unknown: this function never manufactures a run, worker,
 * delivery receipt, verification result, or CI result.
 */
export function deriveWorkspaceStatus(input: WorkspaceStatusInput): WorkspaceStatus {
  const { board, pipeline, controlPlane, workers } = input;
  if (!board) {
    return {
      tone: "unknown",
      healthLabel: "No repository selected",
      orchestrationLabel: "No plan",
      messagingLabel: "Messages unknown",
      ciLabel: "CI unknown",
      nextAction: "Select a running plan in a repository.",
      latestActivity: "No scoped activity is available.",
    };
  }

  const repositoryPlan = `${board.repoName} / ${board.number || "unnumbered plan"}`;
  const deadLetters = controlPlane?.stateCounts.deadLetter || 0;
  const waitingMessages = controlPlane
    ? controlPlane.stateCounts.unrouted + controlPlane.stateCounts.queued + controlPlane.stateCounts.claimed
    : 0;
  const latest = newestScopedMessage(controlPlane, board);
  const nodes = pipeline ? Object.values(pipeline.scheduler.nodes) : [];
  const running = nodes.filter((node) => node.status === "RUNNING").length;
  const blocked = nodes.filter((node) => node.status === "BLOCKED").length;
  const done = nodes.filter((node) => node.status === "DONE").length;
  const staleWorkers = workers.filter((worker) => worker.state !== "ACTIVE").length;
  const localEvidenceReady = Boolean(
    pipeline &&
    nodes.length > 0 &&
    done === nodes.length &&
    pipeline.reconciliation?.passed &&
    !pipeline.warnings.some((warning) => warning.severity === "critical")
  );

  let tone: WorkspaceTone = "waiting";
  let healthLabel = "Waiting for preflight";
  let nextAction = `Open orchestration for ${repositoryPlan} and run preflight.`;

  if (input.identityError || input.supervisorError) {
    tone = "blocked";
    healthLabel = "Native supervision blocked";
    nextAction = `Inspect ${repositoryPlan} diagnostics; do not admit work until native supervision recovers.`;
  } else if (input.decisionCount || deadLetters || blocked || staleWorkers) {
    tone = "blocked";
    healthLabel = "Action required";
    nextAction = deadLetters
      ? `Open activity for ${repositoryPlan} and repair the dead-letter route.`
      : blocked
        ? `Open orchestration for ${repositoryPlan} and resolve the blocked task reason.`
        : `Open orchestration for ${repositoryPlan} and review the interrupted or waiting work.`;
  } else if (pipeline?.release?.readyForPr) {
    tone = "healthy";
    healthLabel = "Release gate passed";
    nextAction = `Review the recorded release evidence for ${repositoryPlan}.`;
  } else if (localEvidenceReady) {
    tone = "healthy";
    healthLabel = "Local evidence complete";
    nextAction = `Run CI for ${repositoryPlan}; local verification and reconciliation are recorded.`;
  } else if (running || workers.some((worker) => worker.state === "ACTIVE")) {
    tone = "active";
    healthLabel = "Work in progress";
    nextAction = `Monitor the active tasks for ${repositoryPlan}.`;
  } else if (pipeline?.preflight?.disposition === "ready") {
    tone = "waiting";
    healthLabel = pipeline.runApproval ? "Ready to admit" : "Approval required";
    nextAction = pipeline.runApproval
      ? `Admit the next ready task for ${repositoryPlan}.`
      : `Review and approve the fresh preflight for ${repositoryPlan}.`;
  }

  const orchestrationLabel = pipeline
    ? `${pipeline.run.status} · ${done}/${nodes.length} tasks`
    : `${workers.filter((worker) => worker.state === "ACTIVE").length} live · run not selected`;
  const messagingLabel = controlPlane
    ? deadLetters
      ? `${deadLetters} dead letter${deadLetters === 1 ? "" : "s"}`
      : waitingMessages
        ? `${waitingMessages} awaiting delivery`
        : `${controlPlane.messages.length} routed`
    : "Messages unknown";
  const ciLabel = pipeline?.release?.readyForPr
    ? "CI gate passed"
    : localEvidenceReady
      ? "Ready for CI"
      : pipeline
        ? "CI not ready"
        : "CI unknown";

  return {
    tone,
    healthLabel,
    orchestrationLabel,
    messagingLabel,
    ciLabel,
    nextAction,
    latestActivity: latest
      ? `${latest.authorId}: ${latest.body}`
      : controlPlane
        ? "No messages recorded for this repository and plan."
        : "Message state has not been loaded.",
  };
}
