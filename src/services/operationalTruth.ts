import type { Board } from "./boards";
import type { ControlPlaneSnapshot } from "./controlPlane";
import type {
  EvidenceArtifact,
  NodeCompletion,
  OrchestratorSnapshot,
  ScheduledNode,
  VerificationResult,
} from "./orchestratorPipeline";

export type TruthTone = "ready" | "active" | "warning" | "blocked" | "unknown";

export interface ReleaseVerdict {
  label: "NOT READY" | "READY FOR PR" | "READY TO MERGE" | "MERGED";
  tone: TruthTone;
  blockers: string[];
  source: string;
  checkedAtMs: number | null;
}

export interface CollisionTruth {
  label: string;
  tone: TruthTone;
  impact: string;
  source: string;
  checkedAtMs: number | null;
  nextAction: string;
  affected: string[];
}

export interface EvidenceCell {
  label: "PASSED" | "FAILED" | "NOT RECORDED";
  tone: TruthTone;
  detail: string;
}

export interface TaskEvidenceTruth {
  nodeId: string;
  title: string;
  status: string;
  tests: EvidenceCell;
  build: EvidenceCell;
  runtime: EvidenceCell;
  artifacts: EvidenceCell;
  risks: string[];
}

export interface ActivityTruth {
  id: string;
  atMs: number;
  actor: string;
  action: string;
  result: string;
  scope: string;
  source: "native run" | "message ledger";
}

export interface OperationalTruth {
  release: ReleaseVerdict;
  collision: CollisionTruth;
  evidence: TaskEvidenceTruth[];
  activity: ActivityTruth[];
  currentRunParallelLimit: number | null;
  futureRunParallelLimit: number;
  scopeLabel: string;
  provenance: string;
  checkedAtMs: number | null;
}

function scopeLabel(board: Board | null): string {
  if (!board) return "No repository or plan selected";
  return `${board.repoName} / ${board.number || "unnumbered plan"} / ${board.branch}`;
}

export function deriveReleaseVerdict(
  pipeline: OrchestratorSnapshot | null,
  board: Board | null,
): ReleaseVerdict {
  if (!pipeline) {
    return {
      label: "NOT READY",
      tone: "unknown",
      blockers: board
        ? [
            "No exact native run is selected for this repository and plan.",
            "Release and CI evidence have not been loaded.",
          ]
        : ["No repository, plan, or exact native run is selected."],
      source: board ? "No native release receipt" : "Selection state",
      checkedAtMs: null,
    };
  }
  if (!pipeline.release) {
    return {
      label: "NOT READY",
      tone: "warning",
      blockers: ["The native release gate has not recorded a result."],
      source: `Native run ${pipeline.run.runId}`,
      checkedAtMs: pipeline.nowMs,
    };
  }
  if (pipeline.release.merged) {
    return {
      label: "MERGED",
      tone: "ready",
      blockers: [],
      source: `Native release receipt for ${pipeline.run.runId}`,
      checkedAtMs: pipeline.nowMs,
    };
  }
  if (pipeline.release.readyToMerge) {
    return {
      label: "READY TO MERGE",
      tone: "ready",
      blockers: [],
      source: `Native release receipt for ${pipeline.run.runId}`,
      checkedAtMs: pipeline.nowMs,
    };
  }
  if (pipeline.release.readyForPr) {
    return {
      label: "READY FOR PR",
      tone: "ready",
      blockers: [],
      source: `Native release receipt for ${pipeline.run.runId}`,
      checkedAtMs: pipeline.nowMs,
    };
  }
  return {
    label: "NOT READY",
    tone: "blocked",
    blockers: pipeline.release.issues.map((issue) => issue.message),
    source: `Native release receipt for ${pipeline.run.runId}`,
    checkedAtMs: pipeline.nowMs,
  };
}

function deriveCollisionTruth(
  pipeline: OrchestratorSnapshot | null,
  board: Board | null,
): CollisionTruth {
  const scope = scopeLabel(board);
  if (!pipeline) {
    return {
      label: "UNKNOWN · NO VERIFIED RUN",
      tone: "unknown",
      impact: "No native collision census or approval receipt is bound to this repository and plan.",
      source: "Native preflight unavailable",
      checkedAtMs: null,
      nextAction: `Select or create an exact native run for ${scope}, then run preflight.`,
      affected: [scope],
    };
  }
  const preflight = pipeline.preflight;
  if (!preflight) {
    return {
      label: "BLOCKED · PREFLIGHT NOT RECORDED",
      tone: "blocked",
      impact: "Worker admission cannot prove repository identity, dirty state, ports, or running-process conflicts.",
      source: `Native run ${pipeline.run.runId}`,
      checkedAtMs: pipeline.nowMs,
      nextAction: "Run native preflight before approving or admitting any task.",
      affected: [scope, pipeline.run.runId],
    };
  }
  const unknown = preflight.unknownConflicts.map(
    (binding) => `port ${binding.port} · ${binding.process.executablePath || `PID ${binding.process.pid}`}`,
  );
  const known = preflight.conflicts.map(
    (binding) => `port ${binding.port} · ${binding.process.executablePath || `PID ${binding.process.pid}`}`,
  );
  if (unknown.length) {
    return {
      label: "BLOCKED · UNKNOWN PROCESS CONFLICT",
      tone: "blocked",
      impact: `${unknown.length} running process${unknown.length === 1 ? "" : "es"} cannot be safely attributed. Admission remains blocked.`,
      source: `Native preflight for ${pipeline.run.runId}`,
      checkedAtMs: pipeline.nowMs,
      nextAction: "Identify the listed process owner, then refresh preflight; do not stop it from the planner.",
      affected: unknown,
    };
  }
  if (!pipeline.preflightFresh) {
    return {
      label: "BLOCKED · PREFLIGHT EXPIRED",
      tone: "blocked",
      impact: "The recorded census may no longer match the repository, worktree, or running processes.",
      source: `Expired native preflight for ${pipeline.run.runId}`,
      checkedAtMs: pipeline.nowMs,
      nextAction: "Refresh preflight and approve the new receipt before another admission.",
      affected: [scope, ...known],
    };
  }
  if (!pipeline.runApproval) {
    return {
      label: "BLOCKED · APPROVAL RECEIPT MISSING",
      tone: "blocked",
      impact: "The renderer cannot authorize work from a green-looking preflight alone.",
      source: `Native preflight for ${pipeline.run.runId}`,
      checkedAtMs: pipeline.nowMs,
      nextAction: "Review this exact census and record native approval for this exact run.",
      affected: [scope, ...known],
    };
  }
  const approvedNodes = pipeline.runApproval.collisionAssessments.map(
    (assessment) => `${assessment.nodeId} · census ${assessment.censusDigest.slice(0, 12)}…`,
  );
  return {
    label: known.length ? "ENFORCED · CONFLICTS RESOLVED" : "ENFORCED · APPROVED CENSUS",
    tone: "ready",
    impact: known.length
      ? `${known.length} allowlisted conflict${known.length === 1 ? " was" : "s were"} handled by native preflight; admissions remain fenced to the approved manifest.`
      : "No port conflict was recorded and native approval is bound to the immutable run manifest.",
    source: `Approval ${pipeline.runApproval.approvalDigest.slice(0, 12)}…`,
    checkedAtMs: pipeline.nowMs,
    nextAction: "Admit only dependency-ready tasks through the native control.",
    affected: approvedNodes.length ? approvedNodes : [scope],
  };
}

function commandEvidence(
  commands: string[],
  results: VerificationResult[],
  matcher: RegExp,
): EvidenceCell {
  const matched = commands.filter((command) => matcher.test(command));
  if (!matched.length) return { label: "NOT RECORDED", tone: "unknown", detail: "No matching verification command is declared." };
  const matchedResults = results.filter((result) => matched.includes(result.commandId));
  if (!matchedResults.length) return { label: "NOT RECORDED", tone: "unknown", detail: matched.join(", ") };
  const failed = matchedResults.filter((result) => result.exitCode !== 0);
  return failed.length
    ? { label: "FAILED", tone: "blocked", detail: failed.map((result) => result.commandId).join(", ") }
    : { label: "PASSED", tone: "ready", detail: matchedResults.map((result) => result.commandId).join(", ") };
}

function artifactEvidence(artifacts: EvidenceArtifact[]): EvidenceCell {
  if (!artifacts.length) return { label: "NOT RECORDED", tone: "unknown", detail: "No hashed artifact receipt is attached." };
  return {
    label: "PASSED",
    tone: "ready",
    detail: `${artifacts.length} hashed artifact${artifacts.length === 1 ? "" : "s"}`,
  };
}

function evidenceForNode(
  node: ScheduledNode,
  completion: NodeCompletion | undefined,
  pipeline: OrchestratorSnapshot,
): TaskEvidenceTruth {
  const manifestNode = pipeline.manifest?.nodes.find((candidate) => candidate.nodeId === node.id);
  const commands = node.verificationCommands || manifestNode?.verificationCommands || [];
  const results = completion?.verification || node.verification || [];
  const artifacts = completion?.artifacts || node.evidence || [];
  const risks = pipeline.warnings
    .filter((warning) => warning.nodeId === null || warning.nodeId === node.id)
    .map((warning) => warning.message);
  if (completion && !completion.gate.passed) risks.unshift("The native completion gate did not pass.");
  return {
    nodeId: node.id,
    title: node.title || node.id,
    status: node.status,
    tests: commandEvidence(commands, results, /(test|spec|pytest|cargo test)/i),
    build: commandEvidence(commands, results, /(build|typecheck|tsc|clippy|cargo check)/i),
    runtime: commandEvidence(commands, results, /(smoke|browser|playwright|e2e|runtime)/i),
    artifacts: artifactEvidence(artifacts),
    risks,
  };
}

function deriveActivity(
  pipeline: OrchestratorSnapshot | null,
  controlPlane: ControlPlaneSnapshot | null,
  board: Board | null,
): ActivityTruth[] {
  const scope = scopeLabel(board);
  const events: ActivityTruth[] = pipeline
    ? pipeline.events.map((event, index) => ({
        id: `run-${event.runId}-${event.ts}-${index}`,
        atMs: Date.parse(event.ts) || pipeline.nowMs,
        actor: event.worker || "native orchestrator",
        action: event.type.replace(/-/g, " "),
        result: event.msg,
        scope: `${scope}${event.nodeId ? ` / ${event.nodeId}` : ""}`,
        source: "native run" as const,
      }))
    : [];
  if (controlPlane && board) {
    controlPlane.messages
      .filter((message) =>
        message.scope.repositoryRoot === board.repoRoot && message.scope.planPath === board.planPath
      )
      .forEach((message) => events.push({
        id: `message-${message.id}`,
        atMs: message.updatedAtMs,
        actor: message.authorId,
        action: `${message.kind.replace(/([A-Z])/g, " $1").toLowerCase()} · ${message.state}`,
        result: message.body,
        scope: `${scope}${message.scope.nodeId ? ` / ${message.scope.nodeId}` : ""}`,
        source: "message ledger",
      }));
  }
  return events.sort((left, right) => right.atMs - left.atMs).slice(0, 20);
}

export function deriveOperationalTruth({
  board,
  pipeline,
  controlPlane,
  parallelAgents,
}: {
  board: Board | null;
  pipeline: OrchestratorSnapshot | null;
  controlPlane: ControlPlaneSnapshot | null;
  parallelAgents: boolean;
}): OperationalTruth {
  const checkedAtMs = Math.max(pipeline?.nowMs || 0, controlPlane?.nowMs || 0) || null;
  const sources = [
    pipeline ? `native run ${pipeline.run.runId}` : "native run not selected",
    controlPlane ? `message ledger ${controlPlane.repositoryId}` : "message ledger not loaded",
  ];
  const evidence = pipeline
    ? Object.values(pipeline.scheduler.nodes)
        .sort((left, right) => left.wave - right.wave || left.id.localeCompare(right.id))
        .map((node) => evidenceForNode(node, pipeline.scheduler.completions?.[node.id], pipeline))
    : [];
  return {
    release: deriveReleaseVerdict(pipeline, board),
    collision: deriveCollisionTruth(pipeline, board),
    evidence,
    activity: deriveActivity(pipeline, controlPlane, board),
    currentRunParallelLimit: pipeline?.scheduler.maxParallelWorkers ?? null,
    futureRunParallelLimit: parallelAgents ? 4 : 1,
    scopeLabel: scopeLabel(board),
    provenance: sources.join(" · "),
    checkedAtMs,
  };
}
