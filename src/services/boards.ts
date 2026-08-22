import { stableEntityId } from "./identityRegistry";
import { EvidenceArtifact, PlanSnapshot } from "../types/plan";

/**
 * Board discovery.
 *
 * perfect-planning does NOT write static board files — it serves the board from
 * `assets/perfect-plan-server.cjs`, one detached server per plan, and identifies itself
 * on `/whoami`. So a "tab" here is a running board server, not a file on disk.
 *
 * The board guards itself: any request carrying a foreign `Origin` header is 403'd, and
 * the `Host` header must be exactly `127.0.0.1:<port>`. A `fetch()` straight from this
 * page would therefore always be rejected. Probing has to happen off-page:
 *   - in the Tauri build, through the Rust `discover_boards` command;
 *   - in `npm run dev`, through the board-probe middleware in vite.config.ts.
 * Neither sends an Origin, so neither trips the guard, and neither spawns a competing
 * server — the skill's rule is to reuse `/whoami`, never to start a second one.
 */

export const PORT_START = 5230;
export const PORT_END = 5249;

export interface Board {
  port: number;
  url: string;
  planPath: string;
  number: string | null;
  topic: string | null;
  approved: string;
  awaiting: unknown;
  pid: number | null;
  project: string | null;
  repoName: string;
  repoRoot: string;
  worktreeName: string;
  branch: string;
  approvalBridge?: ApprovalBridgeStatus | null;
}

export type ApprovalBridgeState =
  | "PENDING"
  | "UNREGISTERED"
  | "QUEUED"
  | "CLAIMED"
  | "RETRYING"
  | "DELIVERED"
  | "ACKNOWLEDGED"
  | "DEAD_LETTER"
  | "ROUTE_EXPIRED"
  | "ROUTE_REVOKED"
  | "IDENTITY_MISMATCH";

export interface ApprovalBridgeStatus {
  planPath: string;
  registrationId: string | null;
  routeId: string | null;
  taskId: string | null;
  messageId: string | null;
  state: ApprovalBridgeState;
  admissionReleased: boolean;
  deliveryReceipt: string | null;
  lastError: string | null;
  routeExpiresAtMs: number | null;
}

export interface WorkerAssignment {
  vertebra: string;
  session: string;
  state: "ACTIVE" | "STALE" | "GONE";
  lastHeartbeat?: string | null;
  ageMs?: number | null;
  model?: string | null;
  user?: string | null;
}

export interface WorkerSnapshot {
  planPath: string;
  port: number;
  asOf: string | null;
  activeWindowMs: number | null;
  workers: Record<string, WorkerAssignment>;
}

export interface OrganizationScope {
  id: string;
  label: string;
  workspaceRoot: string;
}

export interface BranchGroup {
  id: string;
  name: string;
  boards: Board[];
}

export interface RepositoryGroup {
  scope: OrganizationScope;
  branches: BranchGroup[];
  boardCount: number;
}

export interface VertebraManifest {
  id: string;
  files: string[];
  resources: string[];
}

export interface PlanManifestSnapshot {
  planPath: string;
  port: number;
  vertebrae: Record<string, VertebraManifest>;
  plan: PlanSnapshot;
}

export interface DecisionRequest {
  kind: string;
  item: string | null;
  since: string | null;
  problem: string | null;
  where: string | null;
  remedy: string | null;
}

interface WhoAmI {
  ok?: boolean;
  planPath?: string;
  number?: string | null;
  topic?: string | null;
  approved?: string;
  awaiting?: unknown;
  port?: number;
  pid?: number;
  project?: string | null;
  repoName?: string;
  repoRoot?: string;
  worktreeName?: string;
  branch?: string;
  approvalBridge?: ApprovalBridgeStatus | null;
}

interface RawWorkerSnapshot {
  asOf?: string;
  activeWindowMs?: number;
  workers?: Record<string, Partial<WorkerAssignment>>;
}

type RawPlanSnapshot = Partial<PlanSnapshot>;

export type CollisionCensusUnknownCode =
  | "REGISTRY_UNAVAILABLE"
  | "CAPABILITY_REJECTED"
  | "CAPABILITY_EXPIRED"
  | "CLOCK_ROLLBACK"
  | "COLLECTOR_UNAVAILABLE"
  | "COLLECTION_TIMEOUT"
  | "PARSE_FAILED"
  | "METADATA_LIMIT_EXCEEDED"
  | "IDENTITY_CHANGED"
  | "OBSERVATION_TIME_INVALID"
  | "COLLECTION_FAILED"
  | "REGISTRY_DRIFT"
  | "PERSISTENCE_FAILED"
  | "NATIVE_WORKER_UNAVAILABLE";

export interface CollisionCensusUnknown {
  status: "UNKNOWN";
  code: CollisionCensusUnknownCode;
}

export interface IssuedCollisionCensusCapability {
  token: string;
  runId: string;
  issuedAtMs: number;
  expiresAtMs: number;
}

export interface RecordedCollisionCensus {
  status: "RECORDED";
  capturedAtMs: number;
  expiresAtMs: number;
  rootCount: number;
  observedPlannerCount: number;
}

interface RawEvidenceArtifact {
  name?: string;
  mime?: string;
  text?: string;
  dataBase64?: string;
}

const inTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Native-only assessor transport. Board HTTP and the development proxy are display surfaces and
 * can never issue or satisfy the global census capability. */
export async function issueCollisionCensusCapability(
  runId: string
): Promise<IssuedCollisionCensusCapability> {
  if (!inTauri()) throw new Error("native collision assessor unavailable");
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<IssuedCollisionCensusCapability>(
    "collision_assessor_issue_discovery_capability",
    { request: { runId } }
  );
}

export async function collectCollisionCensus(
  runId: string,
  token: string
): Promise<RecordedCollisionCensus> {
  if (!inTauri()) throw new Error("native collision assessor unavailable");
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<RecordedCollisionCensus>("collision_assessor_collect_census", {
    request: { runId, token },
  });
}

export async function revokeCollisionCensusCapability(token: string): Promise<boolean> {
  if (!inTauri()) throw new Error("native collision assessor unavailable");
  const { invoke } = await import("@tauri-apps/api/core");
  const response = await invoke<{ revoked: boolean }>(
    "collision_assessor_revoke_discovery_capability",
    { request: { token } }
  );
  return response.revoked === true;
}

function toBoard(port: number, who: WhoAmI): Board | null {
  if (!who || who.ok !== true) return null;
  const fallback = organizationForPlanPath(who.planPath || "");
  const repoRoot = who.repoRoot?.trim() || fallback.workspaceRoot;
  const repoName = who.repoName?.trim() || who.project?.trim() || fallback.label;
  const worktreeName = who.worktreeName?.trim() || fallback.label;
  return {
    port: who.port || port,
    url: `http://127.0.0.1:${who.port || port}/`,
    planPath: who.planPath || "",
    number: who.number ?? null,
    topic: who.topic ?? null,
    approved: who.approved || "",
    awaiting: who.awaiting ?? null,
    pid: who.pid ?? null,
    project: who.project ?? null,
    repoName,
    repoRoot,
    worktreeName,
    branch: who.branch?.trim() || "unknown branch",
    approvalBridge: who.approvalBridge || null,
  };
}

/** Native-only approval observation. The native command re-reads `/whoami`; the renderer does
 * not supply the approval value, process ID, task ID, route, message text, or receipt. */
export async function observeBoardApproval(
  board: Board
): Promise<ApprovalBridgeStatus | null> {
  if (!inTauri()) return board.approvalBridge || null;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<ApprovalBridgeStatus>("observe_board_approval", {
      port: board.port,
      planPath: board.planPath,
    });
  } catch (error) {
    console.warn("[perfect-planner] approval bridge observation failed:", error);
    return null;
  }
}

async function probeViaDevServer(port: number): Promise<Board | null> {
  try {
    const res = await fetch(`/board-probe/${port}/whoami`, { cache: "no-store" });
    if (!res.ok) return null;
    return toBoard(port, await res.json());
  } catch {
    return null;
  }
}

async function discoverViaTauri(): Promise<Board[]> {
  const { invoke } = await import("@tauri-apps/api/core");
  const found = await invoke<WhoAmI[]>("discover_boards", {
    start: PORT_START,
    end: PORT_END,
  });
  return (found || [])
    .map((who) => toBoard(who.port || 0, who))
    .filter((b): b is Board => b !== null);
}

function normalizeWorkerSnapshot(
  board: Board,
  raw: RawWorkerSnapshot | null | undefined
): WorkerSnapshot | null {
  if (!raw?.workers || typeof raw.workers !== "object" || Array.isArray(raw.workers)) {
    return null;
  }

  const workers: Record<string, WorkerAssignment> = {};
  for (const [vertebra, candidate] of Object.entries(raw.workers)) {
    if (!candidate || typeof candidate !== "object") continue;
    const session = typeof candidate.session === "string" ? candidate.session : "";
    const state = candidate.state;
    if (!vertebra || !session || !["ACTIVE", "STALE", "GONE"].includes(state || "")) {
      continue;
    }
    workers[vertebra] = {
      vertebra,
      session,
      state: state as WorkerAssignment["state"],
      lastHeartbeat:
        typeof candidate.lastHeartbeat === "string" ? candidate.lastHeartbeat : null,
      ageMs: typeof candidate.ageMs === "number" ? candidate.ageMs : null,
      model: typeof candidate.model === "string" ? candidate.model : null,
      user: typeof candidate.user === "string" ? candidate.user : null,
    };
  }

  return {
    planPath: board.planPath,
    port: board.port,
    asOf: typeof raw.asOf === "string" ? raw.asOf : null,
    activeWindowMs: typeof raw.activeWindowMs === "number" ? raw.activeWindowMs : null,
    workers,
  };
}

/**
 * Heartbeat state from the board's authoritative, read-only endpoint.
 * `null` means the identity check or request failed; callers must retain their previous
 * transition state so a transient miss cannot re-arm and replay an alarm.
 */
export async function readBoardWorkers(board: Board): Promise<WorkerSnapshot | null> {
  try {
    let raw: RawWorkerSnapshot;
    if (inTauri()) {
      const { invoke } = await import("@tauri-apps/api/core");
      raw = await invoke<RawWorkerSnapshot>("read_board_workers", {
        port: board.port,
        planPath: board.planPath,
      });
    } else {
      const query = new URLSearchParams({ planPath: board.planPath });
      const res = await fetch(`/board-probe/${board.port}/workers?${query}`, {
        cache: "no-store",
      });
      if (!res.ok) return null;
      raw = await res.json();
    }
    return normalizeWorkerSnapshot(board, raw);
  } catch {
    return null;
  }
}

function stringList(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((entry): entry is string => typeof entry === "string" && !!entry.trim());
}

export async function readBoardPlan(board: Board): Promise<PlanManifestSnapshot | null> {
  try {
    let raw: RawPlanSnapshot;
    if (inTauri()) {
      const { invoke } = await import("@tauri-apps/api/core");
      raw = await invoke<RawPlanSnapshot>("read_board_plan", {
        port: board.port,
        planPath: board.planPath,
      });
    } else {
      const query = new URLSearchParams({ planPath: board.planPath });
      const res = await fetch(`/board-probe/${board.port}/plan?${query}`, {
        cache: "no-store",
      });
      if (!res.ok) return null;
      raw = await res.json();
    }
    if (!Array.isArray(raw?.vertebrae)) return null;
    const vertebrae: Record<string, VertebraManifest> = {};
    for (const candidate of raw.vertebrae) {
      if (typeof candidate?.id !== "string" || !candidate.id.trim()) continue;
      vertebrae[candidate.id] = {
        id: candidate.id,
        files: stringList(candidate.files),
        resources: stringList(candidate.resources),
      };
    }
    return {
      planPath: board.planPath,
      port: board.port,
      vertebrae,
      plan: {
        ...(raw as PlanSnapshot),
        spine: Array.isArray(raw.spine) ? raw.spine : [],
        vertebrae: raw.vertebrae as PlanSnapshot["vertebrae"],
      },
    };
  } catch {
    return null;
  }
}

/** Read one proof artifact through the same identity fence as the plan itself. */
export async function readBoardEvidence(
  board: Board,
  fileName: string
): Promise<EvidenceArtifact | null> {
  if (!fileName || fileName.includes("/") || fileName.includes("\\") || fileName.includes("..")) {
    return null;
  }
  try {
    let raw: RawEvidenceArtifact;
    if (inTauri()) {
      const { invoke } = await import("@tauri-apps/api/core");
      raw = await invoke<RawEvidenceArtifact>("read_board_evidence", {
        port: board.port,
        planPath: board.planPath,
        fileName,
      });
    } else {
      const query = new URLSearchParams({ planPath: board.planPath });
      const res = await fetch(
        `/board-probe/${board.port}/evidence/${encodeURIComponent(fileName)}?${query}`,
        { cache: "no-store" }
      );
      if (!res.ok) return null;
      raw = await res.json();
    }
    if (!raw || raw.name !== fileName || typeof raw.mime !== "string") return null;
    return {
      name: fileName,
      mime: raw.mime,
      text: typeof raw.text === "string" ? raw.text : undefined,
      dataUrl:
        typeof raw.dataBase64 === "string"
          ? `data:${raw.mime};base64,${raw.dataBase64}`
          : undefined,
    };
  } catch {
    return null;
  }
}

export function organizationForPlanPath(planPath: string): OrganizationScope {
  const normalized = planPath.replace(/\\/g, "/");
  const marker = "/.claude/scratch/perfect-plan/";
  const markerIndex = normalized.toLocaleLowerCase().indexOf(marker);
  const workspaceRoot = (markerIndex >= 0 ? normalized.slice(0, markerIndex) : normalized)
    .replace(/\/$/, "");
  const parts = workspaceRoot.split("/").filter(Boolean);
  const label = parts.at(-1) || "unscoped workspace";
  return {
    id: stableEntityId("org", workspaceRoot.toLocaleLowerCase()),
    label,
    workspaceRoot,
  };
}

/** Canonical repository boundary reported by Git through `/whoami`.
 * Falls back to the old path scope only for older board servers that have not restarted.
 */
export function repositoryForBoard(board: Board): OrganizationScope {
  const root = board.repoRoot?.trim() || organizationForPlanPath(board.planPath).workspaceRoot;
  const label = board.repoName?.trim() || organizationForPlanPath(board.planPath).label;
  return {
    id: stableEntityId("repo", root.replace(/\\/g, "/").toLocaleLowerCase()),
    label,
    workspaceRoot: root,
  };
}

export function groupBoardsByRepository(boards: Board[]): RepositoryGroup[] {
  const repositories = new Map<string, { scope: OrganizationScope; boards: Board[] }>();
  for (const board of boards) {
    const scope = repositoryForBoard(board);
    const current = repositories.get(scope.id) || { scope, boards: [] };
    current.boards.push(board);
    repositories.set(scope.id, current);
  }

  return [...repositories.values()]
    .sort((a, b) => a.scope.label.localeCompare(b.scope.label))
    .map(({ scope, boards: repoBoards }) => {
      const branches = new Map<string, Board[]>();
      for (const board of repoBoards) {
        const name = board.branch || "unknown branch";
        const branchBoards = branches.get(name) || [];
        branchBoards.push(board);
        branches.set(name, branchBoards);
      }
      return {
        scope,
        boardCount: repoBoards.length,
        branches: [...branches.entries()]
          .sort(([a], [b]) => a.localeCompare(b))
          .map(([name, branchBoards]) => ({
            id: stableEntityId("branch", `${scope.id}\u0000${name}`),
            name,
            boards: branchBoards.sort((a, b) =>
              String(a.number || "").localeCompare(String(b.number || "")) ||
              String(a.topic || "").localeCompare(String(b.topic || ""))
            ),
          })),
      };
    });
}

export function stalledWorkerKey(planPath: string, worker: WorkerAssignment): string {
  return `${planPath}\u0000${worker.vertebra}\u0000${worker.session}`;
}

/** Plan numbers are only directory-local, so UI identity includes the canonical plan path. */
export function boardEntitySource(board: Board): string {
  return `${board.planPath}\u0000${board.port}`;
}

export function decisionRequest(board: Board): DecisionRequest | null {
  const value = board.awaiting;
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  if (typeof record.kind !== "string" || !record.kind.trim()) return null;
  const firstText = (...keys: string[]): string | null => {
    for (const key of keys) {
      const candidate = record[key];
      if (typeof candidate === "string" && candidate.trim()) return candidate.trim();
    }
    return null;
  };
  return {
    kind: record.kind.trim(),
    item: typeof record.item === "string" ? record.item : null,
    since: typeof record.since === "string" ? record.since : null,
    problem: firstText("problem", "message", "why", "reason"),
    where: firstText("where", "location", "scope"),
    remedy: firstText("remedy", "solution", "needs", "nextAction"),
  };
}

export async function discoverBoards(): Promise<Board[]> {
  let boards: Board[];

  if (inTauri()) {
    try {
      boards = await discoverViaTauri();
    } catch (e) {
      console.warn("[perfect-planner] Rust discovery failed, probing over HTTP:", e);
      boards = [];
    }
  } else {
    const ports: number[] = [];
    for (let p = PORT_START; p <= PORT_END; p++) ports.push(p);
    const results = await Promise.all(ports.map(probeViaDevServer));
    boards = results.filter((b): b is Board => b !== null);
  }

  const uniqueByPlan = new Map<string, Board>();
  for (const board of boards.sort((a, b) => a.port - b.port)) {
    // A second server can expose the same plan through a larger application shell. It is
    // another presentation endpoint, not another plan. Selecting it would make that shell
    // appear inside this shell, so keep one deterministic canonical row per plan path.
    const planIdentity = board.planPath
      ? board.planPath.replace(/\//g, "\\").toLocaleLowerCase()
      : `port:${board.port}`;
    if (!uniqueByPlan.has(planIdentity)) uniqueByPlan.set(planIdentity, board);
  }

  return [...uniqueByPlan.values()];
}

/** Two plans can both be "PP-001" — they are numbered per plan directory, not globally. */
export function boardLabel(b: Board): string {
  return b.number ? `${b.number} · ${b.topic || "untitled"}` : b.topic || `port ${b.port}`;
}

/** The board reports approval as `yes @ <date> (<reason>)`; the sidebar only needs the state. */
export function approvalState(b: Board): "approved" | "pending" | "unknown" {
  const a = (b.approved || "").trim().toLowerCase();
  if (a.startsWith("yes")) return "approved";
  if (a.startsWith("pending") || a.startsWith("no")) return "pending";
  return "unknown";
}
