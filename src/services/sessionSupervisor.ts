export type SourceState = "ACTIVE" | "STALE" | "GONE";
export type LeaseDisposition = "LIVE" | "GRACE" | "CLEARED";

export interface SessionObservation {
  organizationId: string;
  planPath: string;
  vertebra: string;
  sessionId: string;
  sourceState: SourceState;
  lastHeartbeat: string | null;
  files: string[];
  resources: string[];
}

export interface SessionLease extends SessionObservation {
  key: string;
  disposition: LeaseDisposition;
  fence: number;
  firstStaleAtMs: number | null;
  clearedAtMs: number | null;
  lastObservedAtMs: number;
}

export interface ReaperEvent {
  id: string;
  kind: "SESSION_CLEARED";
  atMs: number;
  organizationId: string;
  planPath: string;
  vertebra: string;
  sessionId: string;
  fence: number;
  reason: string;
  files: string[];
  resources: string[];
}

export interface SupervisorSnapshot {
  nowMs: number;
  reaperIntervalMs: number;
  recoveryGraceMs: number;
  leases: SessionLease[];
  events: ReaperEvent[];
  liveCount: number;
  graceCount: number;
  clearedCount: number;
}

const REAPER_INTERVAL_MS = 5_000;
const RECOVERY_GRACE_MS = 120_000;
const fallbackLeases = new Map<string, SessionLease>();
const fallbackEvents: ReaperEvent[] = [];

const inTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export function sessionLeaseKey(observation: Pick<
  SessionObservation,
  "organizationId" | "planPath" | "vertebra" | "sessionId"
>): string {
  return `${observation.organizationId}\u0000${observation.planPath.toLocaleLowerCase()}\u0000${observation.vertebra}\u0000${observation.sessionId}`;
}

function fallbackReconcile(
  observations: SessionObservation[],
  nowMs = Date.now()
): SupervisorSnapshot {
  for (const observation of observations) {
    const key = sessionLeaseKey(observation);
    const current = fallbackLeases.get(key);
    if (current) {
      current.lastObservedAtMs = nowMs;
      current.lastHeartbeat = observation.lastHeartbeat;
      current.sourceState = observation.sourceState;
      current.files = [...observation.files];
      current.resources = [...observation.resources];
      if (observation.sourceState === "ACTIVE" && current.disposition !== "CLEARED") {
        current.disposition = "LIVE";
        current.firstStaleAtMs = null;
      } else if (observation.sourceState !== "ACTIVE" && current.disposition === "LIVE") {
        current.disposition = "GRACE";
        current.firstStaleAtMs = nowMs;
      }
      continue;
    }

    const assignmentFence = Math.max(
      0,
      ...[...fallbackLeases.values()]
        .filter(
          (lease) =>
            lease.organizationId === observation.organizationId &&
            lease.planPath === observation.planPath &&
            lease.vertebra === observation.vertebra
        )
        .map((lease) => lease.fence)
    );
    const stale = observation.sourceState !== "ACTIVE";
    fallbackLeases.set(key, {
      ...observation,
      key,
      disposition: stale ? "GRACE" : "LIVE",
      fence: assignmentFence + 1,
      firstStaleAtMs: stale ? nowMs : null,
      clearedAtMs: null,
      lastObservedAtMs: nowMs,
    });
  }

  for (const lease of fallbackLeases.values()) {
    if (
      lease.disposition !== "GRACE" ||
      lease.firstStaleAtMs === null ||
      nowMs - lease.firstStaleAtMs < RECOVERY_GRACE_MS
    ) {
      continue;
    }
    lease.disposition = "CLEARED";
    lease.clearedAtMs = nowMs;
    lease.fence += 1;
    const files = lease.files;
    const resources = lease.resources;
    lease.files = [];
    lease.resources = [];
    fallbackEvents.push({
      id: `pp-reaper-${nowMs}-${fallbackEvents.length + 1}`,
      kind: "SESSION_CLEARED",
      atMs: nowMs,
      organizationId: lease.organizationId,
      planPath: lease.planPath,
      vertebra: lease.vertebra,
      sessionId: lease.sessionId,
      fence: lease.fence,
      reason: `source remained ${lease.sourceState} for the full recovery grace`,
      files,
      resources,
    });
  }

  const leases = [...fallbackLeases.values()];
  return {
    nowMs,
    reaperIntervalMs: REAPER_INTERVAL_MS,
    recoveryGraceMs: RECOVERY_GRACE_MS,
    leases,
    events: fallbackEvents.slice(-200),
    liveCount: leases.filter((lease) => lease.disposition === "LIVE").length,
    graceCount: leases.filter((lease) => lease.disposition === "GRACE").length,
    clearedCount: leases.filter((lease) => lease.disposition === "CLEARED").length,
  };
}

export async function reconcileSessionLeases(
  observations: SessionObservation[]
): Promise<SupervisorSnapshot> {
  if (inTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<SupervisorSnapshot>("reconcile_session_leases", { observations });
  }
  return fallbackReconcile(observations);
}

export async function recoverClearedSession(
  port: number,
  event: ReaperEvent
): Promise<{ ok: boolean; already?: boolean }> {
  if (!inTauri()) return { ok: false };
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<{ ok: boolean; already?: boolean }>("recover_board_session", {
    port,
    planPath: event.planPath,
    event,
  });
}
