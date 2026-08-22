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
import {
  browserOrchestratorScope,
  OrchestratorSnapshot,
  PipelineRunSummary,
  PipelineSnapshotSeed,
} from "./services/orchestratorPipeline";
import { PlanSnapshot } from "./types/plan";
import { alarmDurationMs, playRisingAlarm } from "./services/stallAlarm";
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

interface HeadOrchestratorActorProps {
  entityId: string;
  state: HeadOrchestratorActorState;
  speech: string;
}

const HeadOrchestratorActor: React.FC<HeadOrchestratorActorProps> = ({
  entityId,
  state,
  speech,
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
      <strong>{speech}</strong>
      <small>visual status · delivered messages appear below</small>
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

/**
 * The real skill board remains the main app surface. The shell adds repository coordination
 * and a compact left-rail witness for captured localhost output.
 */
export const App: React.FC = () => {
  const [boards, setBoards] = useState<Board[]>([]);
  const [activePort, setActivePort] = useState<number | null>(null);
  const [scanning, setScanning] = useState(true);
  const [scannedOnce, setScannedOnce] = useState(false);
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
  } | null>(null);
  const [pipelineSnapshot, setPipelineSnapshot] = useState<OrchestratorSnapshot | null>(null);
  const frameRef = useRef<HTMLIFrameElement>(null);
  const scanRunningRef = useRef(false);
  const soundEnabledRef = useRef(soundEnabled);
  const volumeRef = useRef(volume);
  const alarmPlayingRef = useRef(false);
  const stallsByPlanRef = useRef(new Map<string, Set<string>>());
  const orchestratorLeasesRef = useRef(new Map<string, IdentityLease>());
  const mirroredRecoveryEventsRef = useRef(new Set<string>());

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
      setActivePort((current) => {
        if (current !== null && found.some((b) => b.port === current)) return current;
        return found.length ? found[0].port : null;
      });

      const [snapshots, manifests, approvalBridges] = await Promise.all([
        Promise.all(found.map(readBoardWorkers)),
        Promise.all(found.map(readBoardPlan)),
        Promise.all(found.map(observeBoardApproval)),
      ]);
      setBoards(
        found.map((board, index) => ({
          ...board,
          approvalBridge: approvalBridges[index],
        }))
      );
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

  const active = boards.find((b) => b.port === activePort) || null;
  const activePlan = active ? planSnapshots[active.planPath]?.plan || null : null;
  const repositoryGroups = useMemo(() => {
    const groups = groupBoardsByRepository(boards);
    const callSigns = assignRepositoryCallSigns(groups.map((repository) => repository.scope.id));
    return groups.map((repository) => ({
      ...repository,
      callSign: callSigns.get(repository.scope.id) || "?",
    }));
  }, [boards]);
  const activeRepository = active ? repositoryForBoard(active) : null;
  const activeRepositoryCallSign = activeRepository
    ? repositoryGroups.find((repository) => repository.scope.id === activeRepository.id)?.callSign || "?"
    : "?";
  const orchestratorId = activeRepository
    ? orchestratorIds[activeRepository.id] || null
    : null;
  const firstStalledBoard = boards.find((board) => (stalledByPlan[board.planPath] || 0) > 0);
  const decisionBoards = boards
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
  const activeWorkers = visibleWorkerReports.filter(
    (report) => report.worker.state === "ACTIVE"
  ).length;
  const scopedStalled = visibleWorkerReports.filter(
    (report) => report.worker.state !== "ACTIVE"
  ).length;
  const scopedCleared = supervisor?.leases.filter(
    (lease) =>
      lease.disposition === "CLEARED" &&
      (!activeRepository || lease.organizationId === activeRepository.id)
  ).length || 0;
  const headActorState: HeadOrchestratorActorState = identityError || supervisorError
    ? "stopped"
    : decisionBoards.length || scopedStalled
      ? "holding"
      : activeWorkers
        ? "working"
        : "standby";
  const headActorSpeech = identityError
    ? "STOP. Identity is not proven; no worker may proceed."
    : supervisorError
      ? "STOP. Worker supervision is offline; claims remain blocked."
      : decisionBoards.length
        ? `Hold the route. ${decisionBoards.length} decision${decisionBoards.length === 1 ? "" : "s"} need Daniel.`
        : scopedStalled
          ? `Hold position. ${scopedStalled} worker${scopedStalled === 1 ? " is" : "s are"} inside the grace check.`
          : activeWorkers
            ? `Keep moving clockwise. ${activeWorkers} active worker${activeWorkers === 1 ? "" : "s"}; report after each node.`
            : scannedOnce
              ? "Standing by. No worker claims are reporting in this repository."
              : "Checking the fleet before any worker is admitted.";
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
    if (selectedPipelineScope) return selectedPipelineScope;
    const runId = active?.number?.trim().replace(/^#/, "");
    if (!runId || !active?.repoRoot) return null;
    return { runId, repositoryRoot: active.repoRoot };
  }, [active, browserPipelineScope, selectedPipelineScope]);
  useEffect(() => {
    if (
      selectedPipelineScope &&
      active?.repoRoot &&
      selectedPipelineScope.repositoryRoot.toLocaleLowerCase() !==
        active.repoRoot.toLocaleLowerCase()
    ) {
      setSelectedPipelineScope(null);
    }
  }, [active?.repoRoot, selectedPipelineScope]);
  useEffect(() => {
    setSelectedPipelineScope(null);
  }, [active?.planPath]);
  const selectPipelineRun = useCallback((run: PipelineRunSummary) => {
    setSelectedPipelineScope({
      runId: run.runId,
      repositoryRoot: run.repositoryRoot,
    });
  }, []);
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

  return (
    <div className="shell" id="pp-app-shell" data-orchestrator-id={orchestratorId || "pending"}>
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
                              className={`rail-item${b.port === activePort ? " on" : ""}${boardStalls ? " stalled" : ""}${isComplete ? " complete" : ""}`}
                              onClick={() => setActivePort(b.port)}
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
                                className={`rail-chat-route ${b.approvalBridge?.admissionReleased ? "delivered" : "blocked"}`}
                                data-approval-bridge-state={b.approvalBridge?.state || "UNAVAILABLE"}
                                title={b.approvalBridge?.lastError || b.approvalBridge?.routeId || "No native task route is registered"}
                              >
                                chat · {b.approvalBridge?.state?.replace(/_/g, " ").toLowerCase() || "unavailable"}
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
                  onClick={() => setActivePort(firstStalledBoard.port)}
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
          >
            {scanning ? "scanning…" : "rescan"}
          </button>
          <span className="rail-count">
            {boards.length} board{boards.length === 1 ? "" : "s"}
          </span>
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
          className={`orchestrator${decisionBoards.length ? " decision" : ""}${identityError || supervisorError ? " identity-error" : ""}`}
          aria-label="Head orchestrator"
        >
          <div className="orchestrator-head">
            <HeadOrchestratorActor
              entityId={orchestratorId || ""}
              state={headActorState}
              speech={headActorSpeech}
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
            <span className="head-lease">
              {supervisorError
                ? "REAPER STOPPED"
                : orchestratorId
                  ? "LEASE + REAPER ACTIVE"
                  : identityError
                    ? "STOPPED"
                    : "INSPECTING SETUP"}
            </span>
            <span className="head-stat"><b>{visibleWorkerReports.length}</b> reports</span>
            <span className="head-stat"><b>{activeWorkers}</b> active</span>
            <span className={`head-stat${scopedStalled ? " bad" : ""}`}><b>{scopedStalled}</b> grace</span>
            <span className="head-stat"><b>{scopedCleared}</b> cleared</span>
            <span className={`head-stat${decisionBoards.length ? " needs" : ""}`}><b>{decisionBoards.length}</b> decisions</span>
          </div>

          <div className="worker-wire" id="pp-list-worker-reports" role="list" aria-label="Worker reports">
            {visibleWorkerReports.length ? visibleWorkerReports.map((report) => {
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
                  onClick={() => setActivePort(report.boardPort)}
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
                  onClick={() => setActivePort(board.port)}
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
          repositoryRoot={pipelineScope?.repositoryRoot}
          snapshotSeed={pipelineSnapshotSeed}
          onSelectRun={selectPipelineRun}
          onSnapshotChange={setPipelineSnapshot}
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
    </div>
  );
};
