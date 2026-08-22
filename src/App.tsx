import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Board,
  PORT_START,
  PORT_END,
  approvalState,
  boardEntitySource,
  boardLabel,
  decisionRequest,
  discoverBoards,
  groupBoardsByRepository,
  repositoryForBoard,
  OrganizationScope,
  readBoardPlan,
  observeBoardApproval,
  readBoardWorkers,
  stalledWorkerKey,
  WorkerAssignment,
  PlanManifestSnapshot,
} from "./services/boards";
import { LocalOutputWitness } from "./components/LocalOutputWitness";
import { OrchestratorMessenger } from "./components/OrchestratorMessenger";
import { PipelineConsole } from "./components/PipelineConsole";
import { ResourceGuard, type ResourceGuardState } from "./components/ResourceGuard";
import { ActionContextMenu, type ContextActionLog } from "./components/ActionContextMenu";
import { DiagnosticsConsole, type DiagnosticEntry } from "./components/DiagnosticsConsole";
import {
  browserOrchestratorScope,
  OrchestratorSnapshot,
  PipelineRunSummary,
  PipelineSnapshotSeed,
} from "./services/orchestratorPipeline";
import { PlanSnapshot } from "./types/plan";
import { alarmDurationMs, playRisingAlarm } from "./services/stallAlarm";
import { probeResourceGuard } from "./services/resourceGuard";
import {
  IdentityLease,
  assignRepositoryCallSigns,
  reserveIdentity,
  shortEntityId,
  stableEntityId,
} from "./services/identityRegistry";
import {
  LeaseDisposition,
  reconcileSessionLeases,
  recoverClearedSession,
  sessionLeaseKey,
  SupervisorSnapshot,
} from "./services/sessionSupervisor";
import { ControlPlaneScope } from "./services/controlPlane";

const SOUND_KEY = "perfect-planner:stall-sound";
const VOLUME_KEY = "perfect-planner:stall-volume";
const DISMISSED_BOARDS_KEY = "perfect-planner:dismissed-plans";

interface WorkerReport {
  boardPort: number;
  boardLabel: string;
  planPath: string;
  organization: OrganizationScope;
  worker: WorkerAssignment;
  files: string[];
  resources: string[];
  disposition: LeaseDisposition;
  fence: number;
}

type HeadOrchestratorActorState = "working" | "holding" | "standby" | "stopped";

interface HeadOrchestratorGuidance {
  problem: string;
  where: string;
  remedy: string;
}

interface HeadOrchestratorActorProps {
  entityId: string;
  state: HeadOrchestratorActorState;
  guidance: HeadOrchestratorGuidance;
}

const HeadOrchestratorActor: React.FC<HeadOrchestratorActorProps> = ({
  entityId,
  state,
  guidance,
}) => (
  <div
    className={`head-orchestrator-command-post ${state}`}
    data-speaking-to="pp-list-worker-reports"
    data-delivery="visual-status-only"
  >
    <div
      id="pp-entity-head-orchestrator-character"
      className="head-orchestrator-character"
      data-orchestrator-id={entityId || "unassigned"}
      data-role="head-orchestrator-character"
      role="img"
      aria-label={`Head orchestrator character, ${state}`}
    >
      <svg viewBox="0 0 68 82" aria-hidden="true" focusable="false">
        <path className="orch-antenna" d="M34 10V4M28 4h12" />
        <rect className="orch-head" x="11" y="11" width="46" height="31" rx="5" />
        <rect className="orch-eye" x="20" y="21" width="7" height="7" rx="1" />
        <rect className="orch-eye" x="41" y="21" width="7" height="7" rx="1" />
        <path className="orch-mouth" d="M24 34h20" />
        <path className="orch-body" d="M20 46h28v21H20z" />
        <path className="orch-arm" d="M20 50 10 60M48 50l9 7" />
        <path className="orch-leg" d="M27 67v9h-8M41 67v9h8" />
        <rect className="orch-radio" x="52" y="50" width="9" height="15" rx="2" />
        <path className="orch-radio-aerial" d="m57 50 3-7" />
        <circle className="orch-radio-light" cx="56.5" cy="54.5" r="1.5" />
      </svg>
      <span>ORCH</span>
    </div>
    <div
      id="pp-status-head-orchestrator-speech"
      className="head-orchestrator-speech"
      role="status"
      aria-live="polite"
      aria-atomic="true"
    >
      <span>HEAD ORCH → WORKERS</span>
      <dl>
        <div className="orch-guidance-problem">
          <dt>Problem</dt>
          <dd>{guidance.problem}</dd>
        </div>
        <div>
          <dt>Where</dt>
          <dd>{guidance.where}</dd>
        </div>
        <div>
          <dt>Remedy</dt>
          <dd>{guidance.remedy}</dd>
        </div>
      </dl>
      <small>recommended action only · delivered commands appear below</small>
    </div>
  </div>
);

function storedSoundEnabled(): boolean {
  try {
    return localStorage.getItem(SOUND_KEY) !== "off";
  } catch {
    return true;
  }
}

function storedVolume(): number {
  try {
    const saved = Number(localStorage.getItem(VOLUME_KEY));
    return Number.isFinite(saved) && saved >= 0.1 && saved <= 1 ? saved : 0.5;
  } catch {
    return 0.5;
  }
}

function storedDismissedPlans(): Set<string> {
  try {
    const value = JSON.parse(localStorage.getItem(DISMISSED_BOARDS_KEY) || "[]");
    return new Set(
      Array.isArray(value)
        ? value.filter((item): item is string => typeof item === "string")
        : []
    );
  } catch {
    return new Set();
  }
}

function isPlanComplete(plan: PlanSnapshot | undefined): boolean {
  const items = plan?.vertebrae.flatMap((vertebra) => vertebra.checklist || []) || [];
  if (!items.length) return false;
  return items.every((item) => {
    const proof = item.proof;
    const captured = Boolean(
      proof && (
        proof.by === "user" ||
        (proof.log && (proof.exit === undefined || proof.exit === 0))
      )
    );
    const visual = !item.ui || Boolean(
      proof?.screenshot && proof.screenshotCheck?.ok !== false
    );
    return item.built === true && item.tested === true && captured && visual;
  });
}

function pipelineNodeDomToken(value: string): string {
  return value.replace(/[^A-Za-z0-9_-]/g, (character) =>
    `-${character.codePointAt(0)?.toString(16) || "x"}-`
  );
}

/**
 * The real skill board remains the main app surface. The shell adds repository coordination
 * and a compact left-rail witness for captured localhost output.
 */
export const App: React.FC = () => {
  const [boards, setBoards] = useState<Board[]>([]);
  const [activePort, setActivePort] = useState<number | null>(null);
  const [scanning, setScanning] = useState(true);
  const [scannedOnce, setScannedOnce] = useState(false);
  const [scanGeneration, setScanGeneration] = useState(0);
  const [nonce, setNonce] = useState(0);
  const [soundEnabled, setSoundEnabled] = useState(storedSoundEnabled);
  const [volume, setVolume] = useState(storedVolume);
  const [soundStatus, setSoundStatus] = useState<"armed" | "playing" | "blocked">(
    "armed"
  );
  const [stalledCount, setStalledCount] = useState(0);
  const [stalledByPlan, setStalledByPlan] = useState<Record<string, number>>({});
  const [workerReports, setWorkerReports] = useState<WorkerReport[]>([]);
  const [orchestratorIds, setOrchestratorIds] = useState<Record<string, string>>({});
  const [supervisor, setSupervisor] = useState<SupervisorSnapshot | null>(null);
  const [supervisorError, setSupervisorError] = useState<string | null>(null);
  const [identityError, setIdentityError] = useState<string | null>(null);
  const [planSnapshots, setPlanSnapshots] = useState<Record<string, PlanManifestSnapshot>>({});
  const [selectedPipelineScope, setSelectedPipelineScope] = useState<{
    runId: string;
    repositoryRoot: string;
    planPath: string;
  } | null>(null);
  const [pipelineSnapshot, setPipelineSnapshot] = useState<OrchestratorSnapshot | null>(null);
  const [dismissedPlans, setDismissedPlans] = useState<Set<string>>(storedDismissedPlans);
  const [resourceGuard, setResourceGuard] = useState<ResourceGuardState>({
    status: "checking",
    result: null,
    error: null,
  });
  const [diagnosticEntries, setDiagnosticEntries] = useState<DiagnosticEntry[]>([]);
  const frameRef = useRef<HTMLIFrameElement>(null);
  const scanRunningRef = useRef(false);
  const soundEnabledRef = useRef(soundEnabled);
  const volumeRef = useRef(volume);
  const alarmPlayingRef = useRef(false);
  const stallsByPlanRef = useRef(new Map<string, Set<string>>());
  const orchestratorLeasesRef = useRef(new Map<string, IdentityLease>());
  const mirroredRecoveryEventsRef = useRef(new Set<string>());
  const dismissedPlansRef = useRef(dismissedPlans);
  const diagnosticSequenceRef = useRef(0);
  const lastResourceDiagnosticRef = useRef("");
  const activePortRef = useRef<number | null>(null);
  const activePortMissesRef = useRef(0);

  const activateBoardPort = useCallback((port: number | null) => {
    activePortRef.current = port;
    activePortMissesRef.current = 0;
    setActivePort(port);
  }, []);

  const recordDiagnostic = useCallback((entry: Omit<DiagnosticEntry, "id" | "at">) => {
    diagnosticSequenceRef.current += 1;
    setDiagnosticEntries((current) => [...current.slice(-199), {
      ...entry,
      id: `diagnostic-${diagnosticSequenceRef.current}`,
      at: Date.now(),
    }]);
  }, []);

  const ring = useCallback(async (force = false) => {
    if ((!force && !soundEnabledRef.current) || alarmPlayingRef.current) return;
    alarmPlayingRef.current = true;
    const played = await playRisingAlarm(volumeRef.current);
    if (!played) {
      alarmPlayingRef.current = false;
      setSoundStatus("blocked");
      return;
    }
    setSoundStatus("playing");
    window.setTimeout(() => {
      alarmPlayingRef.current = false;
      setSoundStatus("armed");
    }, alarmDurationMs + 100);
  }, []);

  const scan = useCallback(async () => {
    if (scanRunningRef.current) return;
    scanRunningRef.current = true;
    setScanning(true);
    try {
      const found = await discoverBoards();
      const visible = found.filter((board) => !dismissedPlansRef.current.has(board.planPath));
      const currentPort = activePortRef.current;
      let nextPort = currentPort;
      if (currentPort !== null && visible.some((board) => board.port === currentPort)) {
        activePortMissesRef.current = 0;
      } else if (currentPort !== null && activePortMissesRef.current < 3) {
        // Board discovery is a network census and one endpoint can miss a single poll while
        // its sibling boards remain visible. Do not silently switch plans during that gap:
        // retain the explicit user selection and its last trusted metadata for three scans.
        activePortMissesRef.current += 1;
      } else {
        activePortMissesRef.current = 0;
        nextPort = visible.length ? visible[0].port : null;
      }
      activePortRef.current = nextPort;
      setActivePort(nextPort);

      const [snapshots, manifests, approvalBridges] = await Promise.all([
        Promise.all(found.map(readBoardWorkers)),
        Promise.all(found.map(readBoardPlan)),
        Promise.all(found.map(observeBoardApproval)),
      ]);
      const observedBoards = found.map((board, index) => ({
          ...board,
          approvalBridge: approvalBridges[index],
        }));
      setBoards((currentBoards) => {
        const selectedPort = activePortRef.current;
        if (
          selectedPort === null ||
          observedBoards.some((board) => board.port === selectedPort) ||
          activePortMissesRef.current === 0
        ) {
          return observedBoards;
        }
        const retained = currentBoards.find((board) => board.port === selectedPort);
        return retained ? [...observedBoards, retained] : observedBoards;
      });
      setPlanSnapshots(
        Object.fromEntries(
          found.flatMap((board, index) => manifests[index] ? [[board.planPath, manifests[index]]] : [])
        )
      );
      let newlyStalled = 0;
      let currentStalls = 0;
      const currentByPlan: Record<string, number> = {};
      const provisionalReports: Array<Omit<WorkerReport, "disposition" | "fence">> = [];
      const observedIdsByOrganization = new Map<string, Set<string>>();
      for (let index = 0; index < found.length; index++) {
        const board = found[index];
        if (dismissedPlansRef.current.has(board.planPath)) {
          continue;
        }
        const organization = repositoryForBoard(board);
        const observedIds = observedIdsByOrganization.get(organization.id) || new Set<string>();
        observedIds.add(boardEntitySource(board));
        observedIdsByOrganization.set(organization.id, observedIds);
        const previous = stallsByPlanRef.current.get(board.planPath) || new Set<string>();
        const snapshot = snapshots[index];
        if (!snapshot) {
          // An unreadable poll is unknown, not recovery. Retaining the old set prevents a
          // brief server miss from re-arming and replaying the same alarm five seconds later.
          currentStalls += previous.size;
          currentByPlan[board.planPath] = previous.size;
          continue;
        }
        const next = new Set(
          Object.values(snapshot.workers)
            .filter((worker) => worker.state !== "ACTIVE")
            .map((worker) => stalledWorkerKey(board.planPath, worker))
        );
        for (const worker of Object.values(snapshot.workers)) {
          observedIds.add(worker.session);
          const manifest = manifests[index]?.vertebrae[worker.vertebra];
          provisionalReports.push({
            boardPort: board.port,
            boardLabel: boardLabel(board),
            planPath: board.planPath,
            organization,
            worker,
            files: manifest?.files || [],
            resources: manifest?.resources || [],
          });
        }
        for (const key of next) if (!previous.has(key)) newlyStalled += 1;
        stallsByPlanRef.current.set(board.planPath, next);
        currentStalls += next.size;
        currentByPlan[board.planPath] = next.size;
      }

      let nextSupervisor: SupervisorSnapshot | null = null;
      try {
        nextSupervisor = await reconcileSessionLeases(
          provisionalReports.map((report) => ({
            organizationId: report.organization.id,
            planPath: report.planPath,
            vertebra: report.worker.vertebra,
            sessionId: report.worker.session,
            sourceState: report.worker.state,
            lastHeartbeat: report.worker.lastHeartbeat || null,
            files: report.files,
            resources: report.resources,
          }))
        );
        setSupervisor(nextSupervisor);
        setSupervisorError(null);

        // A reaper tombstone that exists only in app memory leaves the board in split-brain:
        // the shell says CLEARED while `/workers` keeps deriving STALE from the untouched
        // plan. Mirror each durable event into its identity-fenced board exactly once per app
        // run; the endpoint is also idempotent, so restarts are safe.
        const boardsByPlan = new Map(found.map((board) => [board.planPath, board]));
        for (const event of nextSupervisor.events) {
          if (event.kind !== "SESSION_CLEARED" || mirroredRecoveryEventsRef.current.has(event.id)) continue;
          const board = boardsByPlan.get(event.planPath);
          if (!board) continue;
          const recovered = await recoverClearedSession(board.port, event);
          if (recovered.ok) mirroredRecoveryEventsRef.current.add(event.id);
        }
      } catch (error) {
        setSupervisorError(
          error instanceof Error ? error.message : "session supervisor unavailable"
        );
      }
      const leasesByKey = new Map(
        (nextSupervisor?.leases || []).map((lease) => [lease.key, lease])
      );
      const reports: WorkerReport[] = provisionalReports.map((report) => {
        const key = sessionLeaseKey({
          organizationId: report.organization.id,
          planPath: report.planPath,
          vertebra: report.worker.vertebra,
          sessionId: report.worker.session,
        });
        const lease = leasesByKey.get(key);
        return {
          ...report,
          disposition:
            lease?.disposition || (report.worker.state === "ACTIVE" ? "LIVE" : "GRACE"),
          fence: lease?.fence || 0,
        };
      });
      for (const report of reports) {
        if (report.worker.state === "ACTIVE" || report.disposition !== "CLEARED") continue;
        currentByPlan[report.planPath] = Math.max(
          0,
          (currentByPlan[report.planPath] || 0) - 1
        );
        currentStalls = Math.max(0, currentStalls - 1);
      }
      setStalledCount(currentStalls);
      setStalledByPlan(currentByPlan);
      setWorkerReports(reports);
      try {
        const liveOrganizationIds = new Set(observedIdsByOrganization.keys());
        for (const [organizationId, lease] of orchestratorLeasesRef.current) {
          if (liveOrganizationIds.has(organizationId)) continue;
          lease.release();
          orchestratorLeasesRef.current.delete(organizationId);
        }
        for (const [organizationId, observedIds] of observedIdsByOrganization) {
          let lease = orchestratorLeasesRef.current.get(organizationId);
          if (!lease) {
            // Reserve only after this organization's boards and workers have been inspected.
            lease = reserveIdentity("orchestrator", observedIds);
            orchestratorLeasesRef.current.set(organizationId, lease);
          } else {
            lease.heartbeat();
          }
        }
        setOrchestratorIds(
          Object.fromEntries(
            [...orchestratorLeasesRef.current].map(([organizationId, lease]) => [
              organizationId,
              lease.id,
            ])
          )
        );
        setIdentityError(null);
      } catch (error) {
        setIdentityError(error instanceof Error ? error.message : "identity reservation failed");
      }
      // One sound for the whole scan, even if several workers crossed the threshold together.
      if (newlyStalled > 0) void ring();
    } finally {
      setScanning(false);
      setScannedOnce(true);
      setScanGeneration((generation) => generation + 1);
      scanRunningRef.current = false;
    }
  }, [ring]);

  useEffect(() => {
    scan();
    // A board appears when a chat starts one, and disappears when that session ends.
    const t = setInterval(scan, 5000);
    return () => clearInterval(t);
  }, [scan]);

  useEffect(
    () => () => {
      for (const lease of orchestratorLeasesRef.current.values()) lease.release();
      orchestratorLeasesRef.current.clear();
    },
    []
  );

  const visibleBoards = useMemo(
    () => boards.filter((board) => !dismissedPlans.has(board.planPath)),
    [boards, dismissedPlans]
  );
  const hiddenBoardCount = boards.length - visibleBoards.length;
  const active = visibleBoards.find((b) => b.port === activePort) || null;
  const isNativeTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  const activePlan = active ? planSnapshots[active.planPath]?.plan || null : null;
  const repositoryGroups = useMemo(() => {
    const groups = groupBoardsByRepository(visibleBoards);
    const callSigns = assignRepositoryCallSigns(groups.map((repository) => repository.scope.id));
    return groups.map((repository) => ({
      ...repository,
      callSign: callSigns.get(repository.scope.id) || "?",
    }));
  }, [visibleBoards]);
  const activeRepository = active ? repositoryForBoard(active) : null;
  const activeRepositoryCallSign = activeRepository
    ? repositoryGroups.find((repository) => repository.scope.id === activeRepository.id)?.callSign || "?"
    : "?";
  const orchestratorId = activeRepository
    ? orchestratorIds[activeRepository.id] || null
    : null;
  const firstStalledBoard = visibleBoards.find((board) => (stalledByPlan[board.planPath] || 0) > 0);
  const decisionBoards = visibleBoards
    .map((board) => ({ board, decision: decisionRequest(board) }))
    .filter(
      (entry) =>
        entry.decision !== null &&
        (!activeRepository ||
          repositoryForBoard(entry.board).id === activeRepository.id)
    );
  const scopedWorkerReports = workerReports.filter(
    (report) => !activeRepository || report.organization.id === activeRepository.id
  );
  const visibleWorkerReports = scopedWorkerReports.filter(
    (report) => report.disposition !== "CLEARED"
  );
  const legacyActiveWorkers = visibleWorkerReports.filter(
    (report) => report.worker.state === "ACTIVE"
  ).length;
  const legacyScopedStalled = visibleWorkerReports.filter(
    (report) => report.worker.state !== "ACTIVE"
  ).length;
  const legacyScopedCleared = supervisor?.leases.filter(
    (lease) =>
      lease.disposition === "CLEARED" &&
      (!activeRepository || lease.organizationId === activeRepository.id)
  ).length || 0;
  const boundPipelineSnapshot = pipelineSnapshot && (
    !isNativeTauri || (
      selectedPipelineScope?.planPath === active?.planPath &&
      selectedPipelineScope?.runId === pipelineSnapshot.run.runId
    )
  ) ? pipelineSnapshot : null;
  const pipelineNodes = boundPipelineSnapshot
    ? Object.values(boundPipelineSnapshot.scheduler.nodes).sort(
      (left, right) => left.wave - right.wave || left.id.localeCompare(right.id)
    )
    : [];
  const pipelineRunningNodes = pipelineNodes.filter(
    (node) => node.status === "RUNNING" && node.lease
  );
  const pipelineBlockedNodes = pipelineNodes.filter((node) => node.status === "BLOCKED");
  const pipelineCompletedNodes = pipelineNodes.filter((node) => node.status === "DONE");
  const pipelineReadyNodes = pipelineNodes.filter((node) => node.status === "READY");
  const workerReportCount = boundPipelineSnapshot ? pipelineNodes.length : visibleWorkerReports.length;
  const activeWorkers = boundPipelineSnapshot ? pipelineRunningNodes.length : legacyActiveWorkers;
  const scopedStalled = boundPipelineSnapshot ? pipelineBlockedNodes.length : legacyScopedStalled;
  const scopedCleared = boundPipelineSnapshot ? pipelineCompletedNodes.length : legacyScopedCleared;
  const pipelineAdmissionBlocked = Boolean(
    boundPipelineSnapshot &&
    pipelineReadyNodes.length &&
    (!boundPipelineSnapshot.runApproval || !boundPipelineSnapshot.preflightFresh)
  );
  const headActorState: HeadOrchestratorActorState = boundPipelineSnapshot
    ? pipelineRunningNodes.length
      ? "working"
      : pipelineBlockedNodes.length || pipelineAdmissionBlocked
        ? "holding"
        : "standby"
    : identityError || supervisorError
      ? "stopped"
      : decisionBoards.length || scopedStalled
        ? "holding"
        : activeWorkers
          ? "working"
          : "standby";
  const firstDecisionEntry = decisionBoards[0];
  const firstDecision = firstDecisionEntry?.decision;
  const firstProblemWorker = visibleWorkerReports.find(
    (report) => report.worker.state !== "ACTIVE"
  );
  const selectedScope = [activeRepository?.label, active?.number, active?.branch]
    .filter((part): part is string => Boolean(part))
    .join(" / ") || "No repository or plan selected";
  const headActorGuidance: HeadOrchestratorGuidance = (() => {
    if (boundPipelineSnapshot) {
      const runScope = `${selectedScope} / ${boundPipelineSnapshot.run.runId}`;
      const running = pipelineRunningNodes[0];
      if (running?.lease) {
        return {
          problem: "No blocking issue detected in the bound native run.",
          where: `${runScope} / ${running.id} / worker ${running.lease.workerId}`,
          remedy: "Monitor the native heartbeat, keep edits inside the immutable manifest, then validate evidence before completion.",
        };
      }
      if (pipelineNodes.length > 0 && pipelineCompletedNodes.length === pipelineNodes.length) {
        return {
          problem: "Native run completed with durable evidence.",
          where: runScope,
          remedy: `${pipelineCompletedNodes.length} node${pipelineCompletedNodes.length === 1 ? "" : "s"} passed fenced completion; controls remain disabled and hot-resume state is compact.`,
        };
      }
      if (pipelineBlockedNodes.length) {
        return {
          problem: `${pipelineBlockedNodes[0].id} is blocked by the native scheduler.`,
          where: `${runScope} / ${pipelineBlockedNodes[0].id}`,
          remedy: "Inspect the recorded failure and audit trail; do not issue new authority until the node is explicitly recovered or re-planned.",
        };
      }
      if (!boundPipelineSnapshot.preflightFresh) {
        return {
          problem: "Native preflight has expired; admission is blocked.",
          where: `${runScope} / preflight gate`,
          remedy: "Refresh the native preflight and explicitly approve this exact run before admitting a dependency-ready node.",
        };
      }
      if (!boundPipelineSnapshot.runApproval) {
        return {
          problem: "The exact native run has not been explicitly approved.",
          where: `${runScope} / approval gate`,
          remedy: "Review the collision census and record explicit approval; renderer state alone cannot release admission.",
        };
      }
      return {
        problem: "A dependency-ready node is waiting for native admission.",
        where: `${runScope} / ${pipelineReadyNodes[0]?.id || "scheduler"}`,
        remedy: "Admit only through the native control; worker identity, fence, lease, clock, and manifest remain native-owned.",
      };
    }
    if (identityError) {
      return {
        problem: "Orchestrator identity could not be reserved; workers remain blocked.",
        where: `${selectedScope} / identity registry`,
        remedy: "Release the conflicting lease or allocate a new unique ID, then rescan before admitting workers.",
      };
    }
    if (supervisorError) {
      return {
        problem: "Worker supervision could not be loaded; legacy board claim state is untrusted.",
        where: `${selectedScope} / board lease supervisor`,
        remedy: "Keep legacy board claims blocked, restore the supervisor, inspect its log, and rescan before retrying.",
      };
    }
    if (firstDecisionEntry && firstDecision) {
      return {
        problem: firstDecision.problem || `Decision required: ${firstDecision.kind}.`,
        where: firstDecision.where || [
          firstDecisionEntry.board.repoName,
          firstDecisionEntry.board.number,
          firstDecision.item,
          firstDecisionEntry.board.branch,
        ].filter((part): part is string => Boolean(part)).join(" / "),
        remedy: firstDecision.remedy || "Keep the affected route on hold, review the decision request, then approve or re-plan it.",
      };
    }
    if (firstProblemWorker) {
      return {
        problem: `${firstProblemWorker.worker.state} worker heartbeat; its claim cannot progress safely.`,
        where: [
          firstProblemWorker.organization.label,
          active?.number,
          firstProblemWorker.worker.vertebra,
          `worker ${firstProblemWorker.worker.session}`,
        ].filter((part): part is string => Boolean(part)).join(" / "),
        remedy: "Pause new claims, check the worker heartbeat, then recover or release its claim and re-plan before restarting.",
      };
    }
    if (activeWorkers) {
      return {
        problem: "No blocking issue detected.",
        where: `${selectedScope} / clockwise worker route`,
        remedy: `Continue ${activeWorkers} active worker${activeWorkers === 1 ? "" : "s"} and require a report after each node.`,
      };
    }
    return scannedOnce
      ? {
          problem: "No worker claims are currently reporting.",
          where: `${selectedScope} / worker route`,
          remedy: "No recovery is required; keep the orchestrator standing by until a validated claim arrives.",
        }
      : {
          problem: "Fleet assessment has not completed yet.",
          where: `${selectedScope} / initial census`,
          remedy: "Keep workers blocked until the first read-only census and collision assessment finish.",
        };
  })();
  const controlPlaneScope = useMemo<ControlPlaneScope | null>(() => {
    if (!active || !activeRepository || !orchestratorId) return null;
    const normalizedPlanPath = active.planPath.replace(/\\/g, "/");
    const planDirectoryMarker = "/.claude/scratch/perfect-plan/";
    const markerIndex = normalizedPlanPath.toLocaleLowerCase().indexOf(planDirectoryMarker);
    const worktreePath = markerIndex >= 0
      ? normalizedPlanPath.slice(0, markerIndex)
      : active.repoRoot;
    return {
      organizationId: activeRepository.id,
      repositoryId: activeRepository.id,
      repositoryRoot: active.repoRoot,
      worktreePath,
      branch: active.branch,
      planId: stableEntityId("plan", active.planPath.toLocaleLowerCase()),
      planPath: active.planPath,
      nodeId: null,
      itemId: null,
      workerId: null,
      orchestratorId,
    };
  }, [active, activeRepository, orchestratorId]);
  const browserPipelineScope = useMemo(() => browserOrchestratorScope(), []);
  const pipelineScope = useMemo(() => {
    if (browserPipelineScope) return browserPipelineScope;
    if (selectedPipelineScope?.planPath === active?.planPath) return selectedPipelineScope;
    return null;
  }, [active?.planPath, browserPipelineScope, selectedPipelineScope]);
  useEffect(() => {
    if (
      selectedPipelineScope &&
      active?.repoRoot &&
      (selectedPipelineScope.repositoryRoot.toLocaleLowerCase() !==
        active.repoRoot.toLocaleLowerCase() ||
        selectedPipelineScope.planPath !== active.planPath)
    ) {
      setSelectedPipelineScope(null);
    }
  }, [active?.planPath, active?.repoRoot, selectedPipelineScope]);
  useEffect(() => {
    setSelectedPipelineScope(null);
    setPipelineSnapshot(null);
  }, [active?.planPath]);
  const selectPipelineRun = useCallback((run: PipelineRunSummary) => {
    if (!active?.planPath) return;
    setSelectedPipelineScope({
      runId: run.runId,
      repositoryRoot: run.repositoryRoot,
      planPath: active.planPath,
    });
  }, [active?.planPath]);
  const selectCreatedPipelineRun = useCallback((scope: { runId: string; repositoryRoot: string }) => {
    if (!active?.planPath) return;
    setSelectedPipelineScope({ ...scope, planPath: active.planPath });
  }, [active?.planPath]);
  const recordPipelineDiagnostic = useCallback(
    (level: "info" | "warning" | "error", message: string) =>
      recordDiagnostic({ level, source: "pipeline", message }),
    [recordDiagnostic]
  );
  const pipelineRepositoryLabel =
    activeRepository?.label || pipelineSnapshot?.run.repositoryId || "UNSCOPED";
  const pipelineRepositoryId =
    activeRepository?.id || pipelineSnapshot?.run.repositoryId || "unscoped";
  const pipelineProjectLabel =
    active?.project || pipelineSnapshot?.run.title || pipelineRepositoryLabel;
  const pipelineBranchLabel = active?.branch || pipelineSnapshot?.run.branch || "unscoped";
  const pipelineSnapshotSeed = useMemo<PipelineSnapshotSeed | undefined>(() => {
    if (!active || !activeRepository) return undefined;
    return {
      organizationId: activeRepository.id,
      repositoryId: activeRepository.id,
      worktreePath: controlPlaneScope?.worktreePath || active.repoRoot,
      planId: controlPlaneScope?.planId || active.number || undefined,
      title: active.topic || active.project || boardLabel(active),
    };
  }, [active, activeRepository, controlPlaneScope]);
  const controlPlaneWorkers = useMemo(
    () => visibleWorkerReports
      .filter((report) => report.planPath === active?.planPath)
      .map((report) => ({
        id: report.worker.session,
        nodeId: report.worker.vertebra,
        label: `${report.worker.session} · ${report.worker.vertebra}`,
        state: report.worker.state === "ACTIVE" ? "LIVE" : report.disposition,
      })),
    [active?.planPath, visibleWorkerReports]
  );

  const toggleSound = () => {
    const next = !soundEnabledRef.current;
    soundEnabledRef.current = next;
    setSoundEnabled(next);
    try {
      localStorage.setItem(SOUND_KEY, next ? "on" : "off");
    } catch {
      // Preferences are a convenience; alert detection must continue if storage is blocked.
    }
  };

  const changeVolume = (next: number) => {
    volumeRef.current = next;
    setVolume(next);
    try {
      localStorage.setItem(VOLUME_KEY, String(next));
    } catch {
      // See toggleSound: a storage failure must not break the live monitor.
    }
  };

  const testSound = () => {
    if (!soundEnabledRef.current) toggleSound();
    void ring(true);
  };

  const refreshResourceGuard = useCallback(() => {
    if (!active?.repoRoot) {
      setResourceGuard({
        status: "unavailable",
        result: null,
        error: "Select a repository to check system resources",
      });
      return;
    }
    setResourceGuard({ status: "checking", result: null, error: null });
    void probeResourceGuard(active.repoRoot).then(
      (result) => setResourceGuard({ status: "active", result, error: null }),
      (error) => setResourceGuard({
        status: "unavailable",
        result: null,
        error: error instanceof Error ? error.message : "Windows resource probe failed",
      })
    );
  }, [active?.repoRoot]);

  useEffect(() => {
    refreshResourceGuard();
    const timer = window.setInterval(refreshResourceGuard, 30_000);
    return () => window.clearInterval(timer);
  }, [refreshResourceGuard]);

  useEffect(() => {
    if (resourceGuard.status === "checking") return;
    const signature = resourceGuard.status === "active"
      ? `active:${resourceGuard.result.provider}`
      : `unavailable:${resourceGuard.error}`;
    if (signature === lastResourceDiagnosticRef.current) return;
    lastResourceDiagnosticRef.current = signature;
    const browserOnly = resourceGuard.status === "unavailable" && resourceGuard.error.includes("Tauri desktop app");
    recordDiagnostic({
      level: resourceGuard.status === "active" || browserOnly ? "info" : "error",
      source: "resource-guard",
      message: resourceGuard.status === "active"
        ? `Windows resource probe active via ${resourceGuard.result.provider}.`
        : resourceGuard.error,
    });
  }, [recordDiagnostic, resourceGuard]);

  const dismissPlan = useCallback((planPath: string) => {
    const next = new Set(dismissedPlansRef.current);
    next.add(planPath);
    dismissedPlansRef.current = next;
    setDismissedPlans(next);
    try {
      localStorage.setItem(DISMISSED_BOARDS_KEY, JSON.stringify([...next]));
    } catch {
      // Dismissal remains reversible for this session when storage is unavailable.
    }
    const replacement = boards.find((board) => !next.has(board.planPath));
    activateBoardPort(replacement?.port ?? null);
    recordDiagnostic({ level: "info", source: "plan", message: `Removed ${planPath} from the live rail; the plan file was not changed.` });
    void scan();
  }, [activateBoardPort, boards, recordDiagnostic, scan]);

  const restoreDismissedPlans = () => {
    const next = new Set<string>();
    dismissedPlansRef.current = next;
    setDismissedPlans(next);
    try {
      localStorage.removeItem(DISMISSED_BOARDS_KEY);
    } catch {
      // The in-memory restore still takes effect.
    }
    activateBoardPort(boards[0]?.port ?? null);
    void scan();
  };

  const handleContextPlanAction = useCallback((action: "select" | "remove" | "open", planPath: string) => {
    const board = boards.find((candidate) => candidate.planPath === planPath);
    if (!board) {
      recordDiagnostic({ level: "warning", source: "context-menu", message: `Plan action refused because ${planPath} is no longer in the current board census.` });
      return;
    }
    if (action === "select") activateBoardPort(board.port);
    if (action === "remove") dismissPlan(planPath);
    if (action === "open") window.open(board.url, "_blank", "noopener,noreferrer");
  }, [activateBoardPort, boards, dismissPlan, recordDiagnostic]);

  const recordContextAction = useCallback((entry: ContextActionLog) => {
    recordDiagnostic({ level: entry.level, source: entry.source, message: entry.message });
  }, [recordDiagnostic]);

  return (
    <div className="shell" id="pp-app-shell" data-orchestrator-id={orchestratorId || "pending"}>
      <ActionContextMenu onPlanAction={handleContextPlanAction} onLog={recordContextAction} />
      <aside className="rail" id="pp-region-board-rail">
        <div className="rail-head" id="pp-region-board-rail-heading">
          <h1>
            perfect <span>planning</span>
          </h1>
          <p className="rail-sub">
            boards on 127.0.0.1:{PORT_START}–{PORT_END}
          </p>
        </div>

        <div className="rail-list" id="pp-list-boards" aria-label="Running boards by repository">
          {repositoryGroups.map((repository) => (
            <section
              key={repository.scope.id}
              id={`pp-repository-${repository.scope.id}`}
              className="repo-section"
              data-repository-id={repository.scope.id}
              data-repository-name={repository.scope.label}
              data-repository-call-sign={repository.callSign}
              data-context-kind="surface"
              data-context-id={repository.scope.id}
              data-context-label={`Repository ${repository.callSign} ${repository.scope.label}`}
              aria-labelledby={`pp-repository-heading-${repository.scope.id}`}
            >
              <header className="repo-heading">
                <span className="repo-call-sign" aria-hidden="true">{repository.callSign}</span>
                <span className="repo-heading-copy">
                  <span className="repo-kicker">repository {repository.callSign}</span>
                  <span className="repo-title-line">
                    <h2 id={`pp-repository-heading-${repository.scope.id}`}>
                      {repository.scope.label}
                    </h2>
                    <span className="repo-count">{repository.boardCount}</span>
                  </span>
                </span>
              </header>

              <div className="repo-branches">
                {repository.branches.map((branch) => (
                  <div
                    key={branch.id}
                    className="branch-group"
                    data-branch-id={branch.id}
                    data-branch-name={branch.name}
                    data-context-kind="surface"
                    data-context-id={branch.id}
                    data-context-label={`Branch ${branch.name}`}
                  >
                    <div className="branch-heading" title={branch.name}>
                      <span aria-hidden="true">⎇</span>
                      <span>{branch.name}</span>
                      <b>{branch.boards.length}</b>
                    </div>
                    <div className="branch-status-groups">
                      {[
                        {
                          key: "progress",
                          label: "IN PROGRESS",
                          boards: branch.boards.filter((board) => !isPlanComplete(planSnapshots[board.planPath]?.plan)),
                        },
                        {
                          key: "complete",
                          label: "COMPLETED",
                          boards: branch.boards.filter((board) => isPlanComplete(planSnapshots[board.planPath]?.plan)),
                        },
                      ].filter((group) => group.boards.length > 0).map((group) => (
                        <section className={`plan-status-group ${group.key}`} key={group.key} data-plan-status={group.key}>
                          <div className="plan-status-divider"><span>{group.label}</span><b>{group.boards.length}</b></div>
                          <div role="list" aria-label={`${repository.scope.label}, ${branch.name}, ${group.label.toLowerCase()}`}>
                      {group.boards.map((b) => {
                        const state = approvalState(b);
                        const boardStalls = stalledByPlan[b.planPath] || 0;
                        const boardId = stableEntityId("board", boardEntitySource(b));
                        const boardPlan = planSnapshots[b.planPath]?.plan;
                        const isComplete = isPlanComplete(boardPlan);
                        const projectLabel = b.project?.trim() || repository.scope.label;
                        const hasDistinctProject = projectLabel.localeCompare(
                          repository.scope.label,
                          undefined,
                          { sensitivity: "base" }
                        ) !== 0;
                        return (
                          <div key={b.port} role="listitem" className="rail-plan-entry">
                            <button
                              id={`pp-btn-select-${boardId}`}
                              type="button"
                              data-entity-id={boardId}
                              data-organization-id={repository.scope.id}
                              data-repository-name={repository.scope.label}
                              data-repository-call-sign={repository.callSign}
                              data-project-name={projectLabel}
                              data-branch-name={branch.name}
                              data-board-port={b.port}
                              data-context-kind="plan"
                              data-context-id={boardId}
                              data-context-label={`${b.number || "Unnumbered plan"} · ${b.topic || "untitled plan"}`}
                              data-plan-path={b.planPath}
                              className={`rail-item${b.port === activePort ? " on" : ""}${boardStalls ? " stalled" : ""}${isComplete ? " complete" : ""}`}
                              onClick={() => activateBoardPort(b.port)}
                              title={`Repository ${repository.callSign} · ${repository.scope.label}${hasDistinctProject ? ` · Project ${projectLabel}` : ""} · ${branch.name}\n${b.planPath}`}
                              aria-label={`${b.number || "Unnumbered plan"} Repository ${repository.callSign} ${repository.scope.label}${hasDistinctProject ? ` Project ${projectLabel}` : ""} ${branch.name} ${b.topic || "untitled plan"}`}
                              aria-pressed={b.port === activePort}
                              aria-controls="pp-frame-active-board"
                            >
                              <span className="rail-item-top">
                                <span className="rail-num">{b.number || "—"}</span>
                                {isComplete ? (
                                  <span className="complete-flag" aria-label="Plan complete">✓ COMPLETE</span>
                                ) : boardStalls ? (
                                  <span className="stall-flag">STALLED</span>
                                ) : (
                                  <span className={`dot ${state}`} title={b.approved || "approval unknown"} />
                                )}
                              </span>
                              <span className="rail-repo-repeat">
                                <b className="rail-call-sign" aria-hidden="true">{repository.callSign}</b>
                                {repository.scope.label}
                              </span>
                              {hasDistinctProject ? (
                                <span className="rail-project-repeat">project · {projectLabel}</span>
                              ) : null}
                              <span className="rail-branch-repeat">⎇ {branch.name}</span>
                              <span className="rail-topic">{b.topic || "untitled plan"}</span>
                              <span className="rail-meta">
                                {b.worktreeName} · :{b.port}
                                {b.awaiting ? " · awaiting" : ""}
                                {boardStalls ? ` · ${boardStalls} stalled` : ""}
                              </span>
                              <span
                                className={`rail-chat-route ${isNativeTauri && b.approvalBridge?.admissionReleased ? "delivered" : "blocked"}`}
                                data-approval-bridge-state={isNativeTauri ? b.approvalBridge?.state || "UNAVAILABLE" : "UNVERIFIED_BOARD_CLAIM"}
                                title={isNativeTauri
                                  ? b.approvalBridge?.lastError || b.approvalBridge?.routeId || "No native task route is registered"
                                  : "Browser mode cannot verify a board-reported chat route"}
                              >
                                chat · {isNativeTauri
                                  ? b.approvalBridge?.state?.replace(/_/g, " ").toLowerCase() || "unavailable"
                                  : "unverified board claim"}
                              </span>
                            </button>
                          </div>
                        );
                      })}
                          </div>
                        </section>
                      ))}
                    </div>
                  </div>
                ))}
              </div>
            </section>
          ))}

          {scannedOnce && !boards.length && (
            <div className="rail-empty">
              <p>No board is running.</p>
              <p>Start one from the skill:</p>
              <code>
                node perfect-plan-server.cjs --plan &lt;plan.json&gt; --port {PORT_START}
              </code>
              <p className="rail-empty-note">
                It appears here on its own — a chat joins by serving a board, not by wiring
                anything up.
              </p>
            </div>
          )}
          {scannedOnce && boards.length > 0 && !visibleBoards.length && (
            <div className="rail-empty">
              <p>All running boards are hidden.</p>
              <button id="pp-btn-restore-dismissed-empty" type="button" className="chip" onClick={restoreDismissedPlans}>
                restore {hiddenBoardCount}
              </button>
            </div>
          )}
        </div>

        <LocalOutputWitness board={active} plan={activePlan} />

        <div
          className={`alarm-panel${stalledCount ? " stalled" : ""}`}
          id="pp-region-stall-alarm"
        >
          <div className="alarm-line">
            <span className="alarm-label">alarm · gentle rise</span>
            <span className="alarm-state" role="status" aria-live="polite">
              {stalledCount && firstStalledBoard ? (
                <button
                  id="pp-btn-show-stalled"
                  type="button"
                  className="alarm-jump"
                  onClick={() => activateBoardPort(firstStalledBoard.port)}
                  title="Show the first stalled board"
                >
                  {stalledCount} stalled
                </button>
              ) : soundStatus === "blocked" ? (
                "click test"
              ) : (
                soundStatus
              )}
            </span>
          </div>
          <div className="alarm-controls">
            <button
              id="pp-btn-toggle-stall-sound"
              type="button"
              className="chip alarm-toggle"
              onClick={toggleSound}
              aria-pressed={soundEnabled}
              title="Mute or enable automatic stall alarms"
            >
              {soundEnabled ? "sound on" : "muted"}
            </button>
            <button
              id="pp-btn-test-stall-sound"
              type="button"
              className="chip"
              onClick={testSound}
              title="Play the alarm once"
            >
              test
            </button>
            <label className="volume" title={`Alarm volume ${Math.round(volume * 100)}%`}>
              <span className="sr-only">Alarm volume</span>
              <input
                id="pp-input-stall-volume"
                type="range"
                min="0.1"
                max="1"
                step="0.05"
                value={volume}
                onChange={(event) => changeVolume(Number(event.target.value))}
                aria-label="Alarm volume"
              />
            </label>
          </div>
        </div>

        <div className="rail-foot" id="pp-region-board-scan">
          <button
            id="pp-btn-rescan-boards"
            type="button"
            className="chip"
            onClick={scan}
            disabled={scanning}
            data-scan-generation={scanGeneration}
          >
            {scanning ? "scanning…" : "rescan"}
          </button>
          <span className="rail-count">
            {visibleBoards.length} board{visibleBoards.length === 1 ? "" : "s"}
          </span>
          {hiddenBoardCount ? (
            <button id="pp-btn-restore-dismissed" type="button" className="chip" onClick={restoreDismissedPlans}>
              restore {hiddenBoardCount}
            </button>
          ) : null}
        </div>
      </aside>

      <main className="stage" id="pp-region-command-stage">
        <section
          id="pp-entity-head-orchestrator"
          data-entity-id={orchestratorId || "unassigned"}
          data-organization-id={pipelineSnapshot?.run.organizationId || pipelineRepositoryId}
          data-repository-name={pipelineRepositoryLabel}
          data-repository-call-sign={activeRepositoryCallSign}
          data-project-name={pipelineProjectLabel}
          data-branch-name={pipelineBranchLabel}
          className={`orchestrator${decisionBoards.length ? " decision" : ""}${!boundPipelineSnapshot && (identityError || supervisorError) ? " identity-error" : ""}`}
          aria-label="Head orchestrator"
        >
          <div className="orchestrator-head">
            <HeadOrchestratorActor
              entityId={orchestratorId || ""}
              state={headActorState}
              guidance={headActorGuidance}
            />
            <span className="head-copy">
              <span className="head-eyebrow">
                HEAD ORCHESTRATOR · REPOSITORY {activeRepositoryCallSign} · {pipelineRepositoryLabel}
                {active?.project && activeRepository && active.project.localeCompare(activeRepository.label, undefined, { sensitivity: "base" }) !== 0
                  ? ` · PROJECT ${active.project}`
                  : ""}
              </span>
              <strong>
                {identityError ? "ID RESERVATION FAILED" : shortEntityId(orchestratorId)}
              </strong>
            </span>
            <span className="head-lease" id="pp-status-head-lease">
              {boundPipelineSnapshot
                ? pipelineNodes.length > 0 && pipelineCompletedNodes.length === pipelineNodes.length
                  ? "PIPELINE COMPLETE"
                  : pipelineRunningNodes.length
                    ? "NATIVE LEASE ACTIVE"
                    : pipelineBlockedNodes.length
                      ? "NATIVE RUN BLOCKED"
                      : pipelineAdmissionBlocked
                        ? "NATIVE GATE BLOCKED"
                        : "NATIVE GATE READY"
                : supervisorError
                  ? "LEGACY REAPER STOPPED"
                  : orchestratorId
                    ? "LEGACY LEASE + REAPER ACTIVE"
                    : identityError
                      ? "STOPPED"
                      : "INSPECTING SETUP"}
            </span>
            <span className="head-stat" id="pp-stat-worker-reports"><b>{workerReportCount}</b> {boundPipelineSnapshot ? "nodes" : "reports"}</span>
            <span className="head-stat" id="pp-stat-active"><b>{activeWorkers}</b> active</span>
            <span className={`head-stat${scopedStalled ? " bad" : ""}`} id="pp-stat-held"><b>{scopedStalled}</b> {boundPipelineSnapshot ? "blocked" : "grace"}</span>
            <span className="head-stat" id="pp-stat-completed"><b>{scopedCleared}</b> {boundPipelineSnapshot ? "done" : "cleared"}</span>
            <span className={`head-stat${decisionBoards.length ? " needs" : ""}`}><b>{decisionBoards.length}</b> decisions</span>
            <ResourceGuard state={resourceGuard} onRefresh={refreshResourceGuard} />
          </div>

          <div className="worker-wire" id="pp-list-worker-reports" role="list" aria-label="Worker reports">
            {boundPipelineSnapshot ? pipelineNodes.length ? pipelineNodes.map((node) => {
              const completion = boundPipelineSnapshot.scheduler.completions?.[node.id];
              const workerId = node.lease?.workerId || completion?.workerId || "unclaimed";
              const fence = node.lease?.fence || completion?.fence || 0;
              const state = node.lease
                ? "LIVE"
                : node.status === "READY" && node.attempts > 0
                  ? "RECOVERED"
                  : node.status;
              return (
                <button
                  id={`pp-btn-open-pipeline-${stableEntityId("assignment", `${boundPipelineSnapshot.run.runId}\u0000${node.id}`)}`}
                  type="button"
                  key={node.id}
                  role="listitem"
                  className={`worker-report ${state.toLowerCase()}`}
                  data-pipeline-node-id={node.id}
                  data-worker-id={workerId}
                  data-fence={fence || "none"}
                  onClick={() => document
                    .getElementById(`pp-orch-node-${pipelineNodeDomToken(node.id)}`)
                    ?.scrollIntoView({ block: "center" })}
                  title={`${boundPipelineSnapshot.run.runId} · ${node.id} · ${workerId} · fence ${fence || "none"}`}
                >
                  <span className="worker-id">{workerId}</span>
                  <span>{node.id}</span>
                  <em>{state}</em>
                </button>
              );
            }) : (
              <span className="worker-empty">The verified native run has no scheduler nodes.</span>
            ) : visibleWorkerReports.length ? visibleWorkerReports.map((report) => {
              const assignmentSource = `${report.organization.id}\u0000${report.planPath}\u0000${report.worker.vertebra}\u0000${report.worker.session}`;
              const assignmentId = stableEntityId("assignment", assignmentSource);
              return (
                <button
                  id={`pp-btn-open-${assignmentId}`}
                  type="button"
                  key={assignmentSource}
                  role="listitem"
                  className={`worker-report ${report.worker.state.toLowerCase()}`}
                  data-entity-id={assignmentId}
                  data-worker-id={report.worker.session}
                  data-fence={report.fence}
                  onClick={() => activateBoardPort(report.boardPort)}
                  title={`${report.boardLabel} · ${report.worker.session} · fence ${report.fence}`}
                >
                  <span className="worker-id">{report.worker.session}</span>
                  <span>{report.worker.vertebra}</span>
                  <em>{report.worker.state === "ACTIVE" ? "LIVE" : report.disposition}</em>
                </button>
              );
            }) : (
              <span className="worker-empty">
                {scopedCleared
                  ? `No live claims · ${scopedCleared} stale session${scopedCleared === 1 ? "" : "s"} cleared`
                  : "No worker claims are reporting."}
              </span>
            )}
            {decisionBoards.map(({ board, decision }) => {
              const decisionId = stableEntityId(
                "decision",
                `${boardEntitySource(board)}\u0000${decision?.kind || "unknown"}\u0000${decision?.item || ""}`
              );
              return (
                <button
                  id={`pp-btn-open-${decisionId}`}
                  type="button"
                  key={decisionId}
                  className="decision-report"
                  data-entity-id={decisionId}
                  onClick={() => activateBoardPort(board.port)}
                  title={`Decision requested by ${boardLabel(board)}`}
                >
                  DECISION · {decision?.item || decision?.kind}
                </button>
              );
            })}
          </div>
          <OrchestratorMessenger
            scope={controlPlaneScope}
            orchestratorId={orchestratorId}
            workers={controlPlaneWorkers}
          />
        </section>

        <PipelineConsole
          runId={pipelineScope?.runId}
          repositoryRoot={pipelineScope?.repositoryRoot || active?.repoRoot}
          planPath={active?.planPath}
          snapshotSeed={pipelineSnapshotSeed}
          onSelectRun={selectPipelineRun}
          onRunCreated={selectCreatedPipelineRun}
          onSnapshotChange={setPipelineSnapshot}
          onDiagnostic={recordPipelineDiagnostic}
        />

        {active ? (
          <>
            <div
              className="stage-bar"
              id="pp-region-active-board-heading"
              data-repository-name={active.repoName}
              data-repository-call-sign={activeRepositoryCallSign}
              data-project-name={active.project || active.repoName}
              data-branch-name={active.branch}
              data-context-kind="plan"
              data-context-id={stableEntityId("board", boardEntitySource(active))}
              data-context-label={boardLabel(active)}
              data-plan-path={active.planPath}
            >
              <span className="active-scope" aria-label="Active repository, branch and plan">
                <span className="scope-call-sign" aria-hidden="true">{activeRepositoryCallSign}</span>
                <strong className="scope-repo">{active.repoName}</strong>
                {active.project && active.project.localeCompare(active.repoName, undefined, { sensitivity: "base" }) !== 0 ? (
                  <>
                    <span className="scope-divider">›</span>
                    <strong className="scope-project">{active.project}</strong>
                  </>
                ) : null}
                <span className="scope-divider">›</span>
                <span className="scope-branch">⎇ {active.branch}</span>
                <span className="scope-divider">›</span>
                <span className="crumb">{boardLabel(active)}</span>
              </span>
              <span className="path" title={active.planPath}>
                {active.worktreeName} · {active.planPath}
              </span>
              <span className="stage-actions">
                <span className="context-action-hint" id="pp-hint-plan-context-actions">right-click plan for actions</span>
                <button
                  id="pp-btn-reload-active-board"
                  type="button"
                  className="chip"
                  onClick={() => {
                    // Re-navigating the frame reloads the board from its own server;
                    // reading into a cross-origin frame is not permitted, and not needed.
                    setNonce((n) => n + 1);
                    void scan();
                  }}
                >
                  refresh
                </button>
                <a
                  id="pp-link-open-active-board"
                  className="chip"
                  href={active.url}
                  target="_blank"
                  rel="noreferrer"
                >
                  open in browser
                </a>
              </span>
            </div>
            <iframe
              id="pp-frame-active-board"
              key={`${active.port}:${nonce}`}
              ref={frameRef}
              className="board"
              src={active.url}
              title={boardLabel(active)}
            />
          </>
        ) : (
          <div className="stage-empty" id="pp-region-empty-stage">
            <div className="stage-empty-inner">
              <h2>Waiting for a board</h2>
              <p>
                This window is a container. It renders the board perfect-planning already
                serves — it does not draw its own.
              </p>
            </div>
          </div>
        )}
      </main>
      <DiagnosticsConsole
        activeBoard={active}
        activePlan={activePlan}
        resourceGuard={resourceGuard}
        pipelineSnapshot={boundPipelineSnapshot}
        entries={diagnosticEntries}
        onClear={() => setDiagnosticEntries([])}
      />
    </div>
  );
};
