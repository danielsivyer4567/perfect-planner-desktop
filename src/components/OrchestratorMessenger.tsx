import {
  FormEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  acknowledgeControlMessage,
  claimControlDeliveries,
  getControlPlaneSnapshot,
  postControlMessage,
  recordControlDelivery,
  type ControlMessage,
  type ControlPlaneSnapshot,
  type ControlPlaneScope,
} from "../services/controlPlane";

export interface OrchestratorWorker {
  id: string;
  nodeId: string;
  label: string;
  state: string;
}

export interface OrchestratorMessengerProps {
  scope: ControlPlaneScope | null;
  orchestratorId: string | null;
  workers: OrchestratorWorker[];
  refreshToken?: number;
  onSnapshotChange?: (snapshot: ControlPlaneSnapshot | null) => void;
  chatDestination?: {
    registrationId?: string;
    address?: string;
    enabled?: boolean;
    label?: string;
    requiresAcknowledgement?: boolean;
    maxAttempts?: number;
    retryBaseMs?: number;
    registeredAtMs?: number;
    metadata?: Record<string, string>;
  };
}

type Snapshot = Awaited<ReturnType<typeof getControlPlaneSnapshot>>;
type UnknownRecord = Record<string, unknown>;

interface RetryEnvelope {
  fingerprint: string;
  correlationId: string;
  noteKey: string;
  escalationKey: string;
}

const DELIVERY_STATES = [
  "QUEUED",
  "UNROUTED",
  "CLAIMED",
  "DELIVERED",
  "ACKNOWLEDGED",
  "DEAD_LETTER",
] as const;

type LoopletFeedState = "live" | "waiting" | "blocked" | "idle";
type Point3 = { x: number; y: number; z: number };

const BRAILLE_BLANK = "\u2800";
const LOOPLET_DOT_WEIGHTS = [
  [0x01, 0x08],
  [0x02, 0x10],
  [0x04, 0x20],
  [0x40, 0x80],
] as const;

function rotateX(point: Point3, angle: number): Point3 {
  const cosine = Math.cos(angle);
  const sine = Math.sin(angle);
  return { x: point.x, y: point.y * cosine - point.z * sine, z: point.y * sine + point.z * cosine };
}

function rotateY(point: Point3, angle: number): Point3 {
  const cosine = Math.cos(angle);
  const sine = Math.sin(angle);
  return { x: point.x * cosine + point.z * sine, y: point.y, z: -point.x * sine + point.z * cosine };
}

function rotateZ(point: Point3, angle: number): Point3 {
  const cosine = Math.cos(angle);
  const sine = Math.sin(angle);
  return { x: point.x * cosine - point.y * sine, y: point.x * sine + point.y * cosine, z: point.z };
}

function makeLoopletRings(): Array<{ outer: boolean; points: Point3[] }> {
  const circle = (transform: (point: Point3) => Point3, outer = false) => ({
    outer,
    points: Array.from({ length: 181 }, (_, index) => {
      const angle = index / 180 * Math.PI * 2;
      return transform({ x: Math.cos(angle), y: Math.sin(angle), z: 0 });
    }),
  });
  return [
    circle((point) => rotateX(point, Math.PI / 2)),
    circle((point) => rotateY(point, Math.PI / 2)),
    circle((point) => rotateX(point, Math.PI / 6)),
    circle((point) => rotateX(point, -Math.PI / 6)),
    circle((point) => point, true),
  ];
}

const LOOPLET_RINGS = makeLoopletRings();

function renderLoopletBraille(time: number, width = 12, height = 6): string {
  const subWidth = width * 2;
  const subHeight = height * 4;
  const pixels = Array.from({ length: subHeight }, () => Array<number>(subWidth).fill(0));
  const angleY = time * 1.54;
  const angleX = 0.32 + 0.08 * Math.sin(time * 0.44);
  const angleZ = 0.06 * Math.cos(time * 0.33);
  const scaleX = (subWidth - 5) * 0.45;
  const scaleY = (subHeight - 3) * 0.46;
  const centerX = subWidth / 2;
  const centerY = subHeight / 2;
  const cameraDistance = 2.8;

  const plot = (point: Point3) => {
    const depth = point.z + cameraDistance;
    if (depth <= 0.1) return;
    const x = Math.round(centerX + point.x * scaleX * cameraDistance / depth);
    const y = Math.round(centerY - point.y * scaleY * cameraDistance / depth);
    if (x >= 0 && x < subWidth && y >= 0 && y < subHeight) pixels[y][x] = 1;
  };

  for (const ring of LOOPLET_RINGS) {
    for (const source of ring.points) {
      let point = ring.outer ? rotateZ(source, -time * 0.44) : rotateY(source, angleY);
      if (!ring.outer) {
        point = rotateX(point, angleX);
        point = rotateZ(point, angleZ);
      }
      plot(point);
    }
  }

  const loopPhase = time * 2.2 % (Math.PI * 2);
  let ball = rotateX({ x: Math.cos(loopPhase), y: Math.sin(loopPhase), z: 0 }, Math.PI / 7);
  ball = rotateZ(rotateX(rotateY(ball, angleY), angleX), angleZ);
  const ballDepth = ball.z + cameraDistance;
  const ballX = Math.round(centerX + ball.x * scaleX * cameraDistance / ballDepth);
  const ballY = Math.round(centerY - ball.y * scaleY * cameraDistance / ballDepth);
  for (let yOffset = -1; yOffset <= 1; yOffset += 1) {
    for (let xOffset = -1; xOffset <= 1; xOffset += 1) {
      const x = ballX + xOffset;
      const y = ballY + yOffset;
      if (x >= 0 && x < subWidth && y >= 0 && y < subHeight) pixels[y][x] = 1;
    }
  }

  return Array.from({ length: height }, (_, cellY) =>
    Array.from({ length: width }, (_, cellX) => {
      let code = 0;
      for (let dotY = 0; dotY < 4; dotY += 1) {
        for (let dotX = 0; dotX < 2; dotX += 1) {
          if (pixels[cellY * 4 + dotY][cellX * 2 + dotX]) code |= LOOPLET_DOT_WEIGHTS[dotY][dotX];
        }
      }
      return code ? String.fromCharCode(0x2800 + code) : BRAILLE_BLANK;
    }).join("")
  ).join("\n");
}

function LoopletLiveFeedMark({ state }: { state: LoopletFeedState }) {
  const frameRef = useRef<HTMLPreElement | null>(null);
  const label = state === "blocked"
    ? "DELIVERY BLOCKED"
    : state === "waiting"
      ? "ROUTE ATTENTION"
      : state === "live"
        ? "CONTROL FEED LIVE"
        : "CONTROL FEED STANDBY";

  useEffect(() => {
    const frame = frameRef.current;
    if (!frame) return;
    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
    let requestId = 0;
    let lastPaint = 0;
    const start = performance.now();

    const paint = (now: number) => {
      if (!document.hidden && now - lastPaint >= 125) {
        const elapsed = reducedMotion.matches ? 0.55 : (now - start) / 1_000;
        frame.textContent = renderLoopletBraille(elapsed);
        lastPaint = now;
      }
      if (!reducedMotion.matches) requestId = window.requestAnimationFrame(paint);
    };

    paint(performance.now());
    const onPreferenceChange = () => {
      window.cancelAnimationFrame(requestId);
      requestId = 0;
      paint(performance.now());
    };
    reducedMotion.addEventListener("change", onPreferenceChange);
    return () => {
      window.cancelAnimationFrame(requestId);
      reducedMotion.removeEventListener("change", onPreferenceChange);
    };
  }, []);

  return (
    <div
      className={`looplet-live-feed-mark ${state}`}
      id="pp-status-looplet-live-feed"
      role="img"
      aria-label={`Looplet ${label.toLowerCase()}`}
      data-feed-state={state}
    >
      <pre ref={frameRef} aria-hidden="true">{renderLoopletBraille(0.55)}</pre>
      <span>
        <b>LOOPLET LIVE</b>
        <em>{label}</em>
      </span>
    </div>
  );
}

function asRecord(value: unknown): UnknownRecord {
  return value && typeof value === "object" ? value as UnknownRecord : {};
}

function stringValue(value: unknown): string {
  return typeof value === "string" ? value : value == null ? "" : String(value);
}

function messageField(message: ControlMessage, field: string): unknown {
  return asRecord(message)[field];
}

function messageId(message: ControlMessage): string {
  return stringValue(messageField(message, "id") || messageField(message, "messageId"));
}

function messageAuthor(message: ControlMessage): string {
  return stringValue(messageField(message, "authorId") || messageField(message, "sourceActorId")) || "unknown actor";
}

function messageBody(message: ControlMessage): string {
  return stringValue(messageField(message, "body") || messageField(message, "text"));
}

function messageKind(message: ControlMessage): string {
  return stringValue(messageField(message, "kind")) || "status";
}

function messageScope(message: ControlMessage): UnknownRecord {
  return asRecord(messageField(message, "scope"));
}

function messageTimestamp(message: ControlMessage): number {
  const value = messageField(message, "createdAtMs") ?? messageField(message, "createdAt");
  if (typeof value === "number") return value;
  const parsed = Date.parse(stringValue(value));
  return Number.isFinite(parsed) ? parsed : 0;
}

function deliveryState(message: ControlMessage): typeof DELIVERY_STATES[number] | string {
  const raw = stringValue(messageField(message, "state")) || "queued";
  const collapsed = raw.replace(/[\s_-]+/g, "").toLowerCase();
  if (collapsed === "queued") return "QUEUED";
  if (collapsed === "unrouted") return "UNROUTED";
  if (collapsed === "claimed") return "CLAIMED";
  if (collapsed === "delivered") return "DELIVERED";
  if (collapsed === "acknowledged") return "ACKNOWLEDGED";
  if (collapsed === "deadletter" || collapsed === "deadlettered") return "DEAD_LETTER";
  return raw.replace(/([a-z])([A-Z])/g, "$1_$2").replace(/[\s-]+/g, "_").toUpperCase();
}

function formatKind(kind: string): string {
  return kind.replace(/([a-z])([A-Z])/g, "$1 $2").replace(/[_-]+/g, " ").toUpperCase();
}

function formatTime(timestamp: number): string {
  if (!timestamp) return "time not recorded";
  return new Date(timestamp).toLocaleString();
}

function domToken(value: string): string {
  return value.replace(/[^A-Za-z0-9_-]/g, (character) => `-${character.codePointAt(0)?.toString(16) || "x"}-`);
}

function newKey(prefix: string): string {
  const random = typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return `${prefix}:${random}`;
}

function primitiveScopeEntries(scope: ControlPlaneScope): Array<[string, string | number | boolean]> {
  return Object.entries(asRecord(scope)).filter((entry): entry is [string, string | number | boolean] => {
    // The orchestrator process ID is intentionally ephemeral. Durable worker notes must
    // remain visible after a head-orchestrator restart, while repository/plan/node/worker
    // identity continues to provide the hard isolation boundary.
    if (entry[0] === "orchestratorId") return false;
    const value = entry[1];
    return value !== undefined && value !== null && (typeof value === "string" || typeof value === "number" || typeof value === "boolean");
  });
}

function belongsToScope(message: ControlMessage, scope: ControlPlaneScope): boolean {
  const actual = messageScope(message);
  return primitiveScopeEntries(scope).every(([key, value]) => actual[key] === value);
}

export function OrchestratorMessenger({
  scope,
  orchestratorId,
  workers,
  refreshToken,
  onSnapshotChange,
  chatDestination,
}: OrchestratorMessengerProps) {
  const isNativeTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [selectedWorkerId, setSelectedWorkerId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [escalate, setEscalate] = useState(false);
  const [loading, setLoading] = useState(false);
  const [acknowledgingId, setAcknowledgingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [syncError, setSyncError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const textAreaRef = useRef<HTMLTextAreaElement | null>(null);
  const panelRef = useRef<HTMLElement | null>(null);
  const returnFocusRef = useRef<HTMLButtonElement | null>(null);
  const retryEnvelopeRef = useRef<RetryEnvelope | null>(null);
  const refreshRunningRef = useRef(false);

  const selectedWorker = useMemo(
    () => workers.find((worker) => worker.id === selectedWorkerId) || null,
    [selectedWorkerId, workers],
  );

  const refresh = useCallback(async () => {
    if (!scope) {
      setSnapshot(null);
      onSnapshotChange?.(null);
      return;
    }
    if (refreshRunningRef.current) return;
    refreshRunningRef.current = true;
    try {
      let next = await getControlPlaneSnapshot({
        repositoryId: scope.repositoryId,
        organizationId: scope.organizationId,
      });
      if (orchestratorId) {
        const claimed = await claimControlDeliveries({
          repositoryId: scope.repositoryId,
          organizationId: scope.organizationId,
          consumerId: orchestratorId,
          destinationKinds: ["localUi"],
          limit: 50,
          leaseMs: 15_000,
          filter: { planId: scope.planId },
        });
        for (const message of claimed) {
          const attempt = [...message.attempts]
            .reverse()
            .find((candidate) => candidate.state === "claimed");
          if (!attempt) continue;
          await recordControlDelivery({
            repositoryId: scope.repositoryId,
            messageId: message.id,
            attemptId: attempt.attemptId,
            consumerId: orchestratorId,
            outcome: "delivered",
          });
        }
        if (claimed.length) {
          next = await getControlPlaneSnapshot({
            repositoryId: scope.repositoryId,
            organizationId: scope.organizationId,
          });
        }
      }
      setSnapshot(next);
      onSnapshotChange?.(next);
      setSyncError(null);
    } catch (cause) {
      setSyncError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      refreshRunningRef.current = false;
    }
  }, [onSnapshotChange, orchestratorId, scope]);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 4_000);
    return () => window.clearInterval(timer);
  }, [refresh, refreshToken]);

  useEffect(() => {
    if (selectedWorkerId && !selectedWorker) setSelectedWorkerId(null);
  }, [selectedWorker, selectedWorkerId]);

  useEffect(() => {
    if (!selectedWorker) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setSelectedWorkerId(null);
        window.setTimeout(() => returnFocusRef.current?.focus(), 0);
        return;
      }
      if (event.key !== "Tab" || !panelRef.current) return;
      const focusable = [...panelRef.current.querySelectorAll<HTMLElement>(
        "button:not([disabled]), input:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex='-1'])",
      )].filter((element) => !element.hasAttribute("hidden"));
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    window.setTimeout(() => textAreaRef.current?.focus(), 0);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selectedWorker]);

  const messages = useMemo(
    () => [...(snapshot?.messages || [])].sort((left, right) => messageTimestamp(right) - messageTimestamp(left)),
    [snapshot],
  );
  const effectiveChatDestination = useMemo(
    () => chatDestination || snapshot?.registrations.find(
      (registration) =>
        registration.kind === "codexChat" &&
        registration.enabled &&
        Boolean(registration.registrationId && registration.address)
    ),
    [chatDestination, snapshot]
  );

  const scopedMessages = useMemo(
    () => scope ? messages.filter((message) => belongsToScope(message, scope)) : [],
    [messages, scope],
  );

  const inboxCount = scopedMessages.filter((message) =>
    messageAuthor(message) !== orchestratorId && deliveryState(message) !== "ACKNOWLEDGED"
  ).length;
  const outboxCount = scopedMessages.filter((message) =>
    messageAuthor(message) === orchestratorId && deliveryState(message) !== "ACKNOWLEDGED"
  ).length;
  const feedState: LoopletFeedState = syncError || (snapshot?.stateCounts.deadLetter || 0) > 0
    ? "blocked"
    : (snapshot?.stateCounts.unrouted || 0) > 0 || (snapshot?.stateCounts.queued || 0) > 0 || (snapshot?.stateCounts.claimed || 0) > 0
      ? "waiting"
      : snapshot
        ? "live"
        : "idle";

  const workerScope = useMemo(() => {
    if (!scope || !selectedWorker) return null;
    return {
      ...scope,
      workerId: selectedWorker.id,
      nodeId: selectedWorker.nodeId,
    } as ControlPlaneScope;
  }, [scope, selectedWorker]);

  const workerMessages = useMemo(() => {
    if (!workerScope) return [];
    return messages.filter((message) => belongsToScope(message, workerScope));
  }, [messages, workerScope]);

  const closePanel = useCallback(() => {
    setSelectedWorkerId(null);
    setError(null);
    setNotice(null);
    window.setTimeout(() => returnFocusRef.current?.focus(), 0);
  }, []);

  const updateDraft = (value: string) => {
    setDraft(value);
    retryEnvelopeRef.current = null;
  };

  const updateEscalation = (value: boolean) => {
    setEscalate(value);
    retryEnvelopeRef.current = null;
  };

  const submitNote = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const body = draft.trim();
    if (!workerScope || !selectedWorker || !orchestratorId || !body || loading) return;

    const targetFingerprint = JSON.stringify(effectiveChatDestination || {});
    const fingerprint = `${selectedWorker.id}\u0000${selectedWorker.nodeId}\u0000${body}\u0000${escalate}\u0000${targetFingerprint}`;
    let envelope = retryEnvelopeRef.current;
    if (!envelope || envelope.fingerprint !== fingerprint) {
      const correlationId = newKey("pp-correlation");
      envelope = {
        fingerprint,
        correlationId,
        noteKey: `${correlationId}:worker-note`,
        escalationKey: `${correlationId}:chat-notification`,
      };
      retryEnvelopeRef.current = envelope;
    }

    setLoading(true);
    setError(null);
    setNotice(null);
    try {
      await postControlMessage({
        scope: workerScope,
        idempotencyKey: envelope.noteKey,
        correlationId: envelope.correlationId,
        kind: "workerNote",
        authorId: orchestratorId,
        body,
        destination: {
          registrationId: `pp-local-ui:${workerScope.repositoryId}`,
          kind: "localUi",
          label: "Perfect Planner orchestrator messenger",
          address: "pp-region-orchestrator-messenger",
          enabled: true,
          requiresAcknowledgement: false,
          maxAttempts: 1,
          retryBaseMs: 5_000,
          registeredAtMs: Date.now(),
          metadata: {
            workerId: selectedWorker.id,
            nodeId: selectedWorker.nodeId,
          },
        },
      });

      if (escalate) {
        await postControlMessage({
          scope: workerScope,
          idempotencyKey: envelope.escalationKey,
          correlationId: envelope.correlationId,
          kind: "alert",
          authorId: orchestratorId,
          body,
          destination: {
            kind: "codexChat",
            registrationId: effectiveChatDestination?.registrationId || null,
            label: effectiveChatDestination?.label || "Codex chat notification",
            address: effectiveChatDestination?.address || null,
            enabled: effectiveChatDestination?.enabled ?? true,
            requiresAcknowledgement: effectiveChatDestination?.requiresAcknowledgement ?? true,
            maxAttempts: effectiveChatDestination?.maxAttempts ?? 3,
            retryBaseMs: effectiveChatDestination?.retryBaseMs ?? 5_000,
            registeredAtMs: effectiveChatDestination?.registeredAtMs ?? null,
            metadata: { ...effectiveChatDestination?.metadata },
          },
        });
      }

      retryEnvelopeRef.current = null;
      setDraft("");
      setEscalate(false);
      setNotice(escalate
        ? "Worker note recorded. Chat notification created; verify its delivery state below."
        : "Worker note recorded in the control-plane ledger.");
      await refresh();
    } catch (cause) {
      setError(`${cause instanceof Error ? cause.message : String(cause)} The same idempotency keys will be reused if you retry this unchanged note.`);
      await refresh();
    } finally {
      setLoading(false);
    }
  };

  const acknowledge = async (message: ControlMessage) => {
    const id = messageId(message);
    if (!id || !orchestratorId || acknowledgingId) return;
    setAcknowledgingId(id);
    setError(null);
    try {
      await acknowledgeControlMessage({
        repositoryId: message.scope.repositoryId,
        messageId: id,
        acknowledgedBy: orchestratorId,
      });
      await refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setAcknowledgingId(null);
    }
  };

  return (
    <section
      className="orchestrator-messenger"
      id="pp-region-orchestrator-messenger"
      aria-label="Orchestrator messages"
      data-entity-id={orchestratorId || "unassigned"}
    >
      <header className="orchestrator-messenger-header" id="pp-header-orchestrator-messenger">
        <LoopletLiveFeedMark state={feedState} />
        <div className="orchestrator-messenger-title">
          <span>ORCHESTRATOR MESSAGES</span>
          <code id="pp-value-orchestrator-message-id">{orchestratorId || "ID UNASSIGNED"}</code>
        </div>
        <div className="orchestrator-message-badges" aria-label="Persistent inbox and outbox counts">
          <span className="orchestrator-message-badge inbox" id="pp-badge-orchestrator-inbox">
            INBOX <b>{inboxCount}</b>
          </span>
          <span className="orchestrator-message-badge outbox" id="pp-badge-orchestrator-outbox">
            OUTBOX <b>{outboxCount}</b>
          </span>
          <span className="orchestrator-message-badge unrouted" id="pp-badge-orchestrator-unrouted">
            UNROUTED <b>{snapshot?.stateCounts.unrouted || 0}</b>
          </span>
        </div>
      </header>
      {syncError ? <p className="orchestrator-message-error" id="pp-error-orchestrator-sync" role="alert">SYNC FAILED · {syncError}</p> : null}

      <div className="orchestrator-worker-message-list" id="pp-list-orchestrator-workers" role="list">
        {workers.map((worker) => {
          const workerDomId = domToken(worker.id);
          const workerMessageCount = scope
            ? messages.filter((message) => belongsToScope(message, {
              ...scope,
              workerId: worker.id,
              nodeId: worker.nodeId,
            } as ControlPlaneScope)).length
            : 0;
          return (
            <div
              className="orchestrator-worker-message-item"
              id={`pp-item-orchestrator-worker-${workerDomId}`}
              key={`${worker.id}:${worker.nodeId}`}
              role="listitem"
              data-worker-id={worker.id}
              data-node-id={worker.nodeId}
              data-context-kind="node"
              data-context-id={`${worker.id}:${worker.nodeId}`}
              data-context-label={`${worker.label} · ${worker.nodeId}`}
            >
              <button
                className="orchestrator-worker-message-button"
                id={`pp-btn-open-worker-notes-${workerDomId}`}
                type="button"
                aria-haspopup="dialog"
                aria-controls="pp-panel-worker-notes"
                onClick={(event) => {
                  returnFocusRef.current = event.currentTarget;
                  setSelectedWorkerId(worker.id);
                  setDraft("");
                  setEscalate(false);
                  setError(null);
                  setNotice(null);
                  retryEnvelopeRef.current = null;
                }}
              >
                <span className="orchestrator-worker-message-label">{worker.label}</span>
                <code>{worker.nodeId}</code>
                <span className="orchestrator-worker-message-state">{worker.state}</span>
                <span className="orchestrator-worker-message-count" aria-label={`${workerMessageCount} messages`}>
                  {workerMessageCount}
                </span>
              </button>
            </div>
          );
        })}
        {!workers.length ? <p id="pp-empty-orchestrator-workers">No messaging destinations are registered in this scope.</p> : null}
      </div>

      <div className="orchestrator-delivery-state-legend" id="pp-legend-orchestrator-delivery-states" aria-label="Delivery state legend">
        {DELIVERY_STATES.map((state) => <span key={state} className={`delivery-state ${state.toLowerCase().replace("_", "-")}`}>{state}</span>)}
      </div>

      {selectedWorker && workerScope ? (
        <div className="orchestrator-worker-notes-backdrop" id="pp-backdrop-worker-notes" onMouseDown={(event) => {
          if (event.target === event.currentTarget) closePanel();
        }}>
          <section
            className="orchestrator-worker-notes-panel"
            id="pp-panel-worker-notes"
            ref={panelRef}
            role="dialog"
            aria-modal="true"
            aria-labelledby="pp-title-worker-notes"
            aria-describedby="pp-description-worker-notes"
            data-worker-id={selectedWorker.id}
            data-node-id={selectedWorker.nodeId}
            data-context-kind="modal"
            data-context-id={`${selectedWorker.id}:${selectedWorker.nodeId}`}
            data-context-label={`Worker notes · ${selectedWorker.label}`}
            data-context-close="#pp-btn-close-worker-notes"
          >
            <header className="orchestrator-worker-notes-header">
              <div>
                <h2 id="pp-title-worker-notes">{selectedWorker.label}</h2>
                <p id="pp-description-worker-notes">
                  Worker <code>{selectedWorker.id}</code> · node <code>{selectedWorker.nodeId}</code> · {selectedWorker.state}
                </p>
              </div>
              <button className="orchestrator-worker-notes-close" id="pp-btn-close-worker-notes" type="button" onClick={closePanel} aria-label="Close worker notes">
                CLOSE
              </button>
            </header>

            <div className="orchestrator-worker-notes-history" id="pp-list-worker-notes" aria-label="Worker note history" aria-live="polite">
              {workerMessages.map((message) => {
                const id = messageId(message);
                const state = deliveryState(message);
                const safeId = domToken(id || `${messageTimestamp(message)}-${messageAuthor(message)}`);
                return (
                  <article
                    className="orchestrator-worker-note"
                    id={`pp-message-${safeId}`}
                    key={id || safeId}
                    data-message-id={id || "unassigned"}
                    data-delivery-state={state}
                    data-context-surface="worker-note"
                    data-context-id={id || safeId}
                    data-context-label={`${formatKind(messageKind(message))} · ${messageAuthor(message)}`}
                  >
                    <header>
                      <span className="orchestrator-worker-note-kind">{formatKind(messageKind(message))}</span>
                      <span className={`delivery-state ${state.toLowerCase().replace("_", "-")}`} id={`pp-status-${safeId}`}>{state}</span>
                    </header>
                    <p className="orchestrator-worker-note-body">{messageBody(message)}</p>
                    <footer>
                      <span>{messageAuthor(message)}</span>
                      <time dateTime={messageTimestamp(message) ? new Date(messageTimestamp(message)).toISOString() : undefined}>
                        {formatTime(messageTimestamp(message))}
                      </time>
                      {state === "DELIVERED" && message.destination.requiresAcknowledgement ? (
                        <button
                          className="orchestrator-worker-note-acknowledge"
                          id={`pp-btn-ack-${safeId}`}
                          type="button"
                          disabled={!orchestratorId || acknowledgingId !== null}
                          onClick={() => void acknowledge(message)}
                        >
                          {acknowledgingId === id ? "ACKNOWLEDGING…" : "ACKNOWLEDGE"}
                        </button>
                      ) : null}
                    </footer>
                  </article>
                );
              })}
              {!workerMessages.length ? <p id="pp-empty-worker-notes">No notes or routed messages exist for this exact worker and node.</p> : null}
            </div>

            <form className="orchestrator-worker-note-form" id="pp-form-worker-note" onSubmit={submitNote}>
              <label htmlFor="pp-input-worker-note">Leave a worker note</label>
              <textarea
                id="pp-input-worker-note"
                ref={textAreaRef}
                value={draft}
                rows={4}
                maxLength={8_000}
                disabled={loading || !orchestratorId}
                aria-describedby="pp-help-worker-note"
                onChange={(event) => updateDraft(event.currentTarget.value)}
              />
              <p id="pp-help-worker-note">
                This records an immutable, worker-and-node-scoped note. It does not claim a chat notification was sent.
              </p>
              <label className="orchestrator-worker-note-escalation" htmlFor="pp-check-escalate-chat">
                <input
                  id="pp-check-escalate-chat"
                  type="checkbox"
                  checked={escalate}
                  disabled={loading || !orchestratorId || isNativeTauri}
                  onChange={(event) => updateEscalation(event.currentTarget.checked)}
                />
                {isNativeTauri
                  ? "Chat escalation awaiting B16 native route"
                  : "Also create an unverified browser chat-delivery record"}
              </label>
              {isNativeTauri ? (
                <p className="orchestrator-chat-route-status" id="pp-status-chat-route-native-blocked">
                  The renderer is forbidden from selecting a chat destination. B16 must derive and deliver the registered route inside the native orchestrator.
                </p>
              ) : null}
              {escalate ? (
                <p className="orchestrator-chat-route-status" id="pp-status-chat-route">
                  {effectiveChatDestination?.registrationId && effectiveChatDestination?.address && effectiveChatDestination?.enabled !== false
                    ? "A registered chat destination was supplied. Delivery still requires a DELIVERED state."
                    : "No chat destination is registered. This notification will remain UNROUTED. Register a connector, then retry the escalation."}
                </p>
              ) : null}
              {error ? <p className="orchestrator-message-error" id="pp-error-worker-note" role="alert">{error}</p> : null}
              {notice ? <p className="orchestrator-message-notice" id="pp-notice-worker-note" role="status">{notice}</p> : null}
              <button
                className="orchestrator-worker-note-submit"
                id="pp-btn-send-worker-note"
                type="submit"
                disabled={loading || !orchestratorId || !draft.trim()}
              >
                {loading ? "RECORDING…" : escalate ? "RECORD NOTE + CREATE CHAT DELIVERY" : "RECORD NOTE"}
              </button>
            </form>
          </section>
        </div>
      ) : null}
    </section>
  );
}
