const REGISTRY_KEY = "perfect-planner:entity-leases:v1";
const REPOSITORY_CALL_SIGN_KEY = "perfect-planner:repository-call-signs:v1";
const LEASE_MS = 15_000;

interface RegistryEntry {
  id: string;
  kind: string;
  lastSeen: number;
}

export interface IdentityLease {
  id: string;
  heartbeat: () => void;
  release: () => void;
}

function alphabeticCallSign(index: number): string {
  let value = index + 1;
  let result = "";
  while (value > 0) {
    value -= 1;
    result = String.fromCharCode(65 + (value % 26)) + result;
    value = Math.floor(value / 26);
  }
  return result;
}

function readRepositoryCallSigns(): Record<string, string> {
  try {
    const parsed = JSON.parse(localStorage.getItem(REPOSITORY_CALL_SIGN_KEY) || "{}") as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed).filter(
        (entry): entry is [string, string] =>
          typeof entry[0] === "string" &&
          typeof entry[1] === "string" &&
          /^[A-Z]+$/.test(entry[1])
      )
    );
  } catch {
    return {};
  }
}

/**
 * Give each canonical repository a permanent alphabetic call-sign. Historical mappings stay
 * reserved so a familiar letter can never silently start referring to a different repository.
 */
export function assignRepositoryCallSigns(repositoryIds: Iterable<string>): Map<string, string> {
  const stored = readRepositoryCallSigns();
  const normalized: Record<string, string> = {};
  const occupied = new Set<string>();

  for (const [repositoryId, callSign] of Object.entries(stored).sort(([a], [b]) => a.localeCompare(b))) {
    if (occupied.has(callSign)) continue;
    normalized[repositoryId] = callSign;
    occupied.add(callSign);
  }

  let candidateIndex = 0;
  for (const repositoryId of [...new Set(repositoryIds)]) {
    if (normalized[repositoryId]) continue;
    let candidate = alphabeticCallSign(candidateIndex);
    while (occupied.has(candidate)) {
      candidateIndex += 1;
      candidate = alphabeticCallSign(candidateIndex);
    }
    normalized[repositoryId] = candidate;
    occupied.add(candidate);
    candidateIndex += 1;
  }

  try {
    localStorage.setItem(REPOSITORY_CALL_SIGN_KEY, JSON.stringify(normalized));
  } catch {
    // A deterministic current-session mapping still renders when storage is unavailable.
  }

  return new Map(Object.entries(normalized));
}

function readRegistry(now = Date.now()): RegistryEntry[] {
  try {
    const parsed = JSON.parse(localStorage.getItem(REGISTRY_KEY) || "[]") as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (entry): entry is RegistryEntry =>
        !!entry &&
        typeof entry.id === "string" &&
        typeof entry.kind === "string" &&
        typeof entry.lastSeen === "number" &&
        now - entry.lastSeen <= LEASE_MS
    );
  } catch {
    return [];
  }
}

function writeRegistry(entries: RegistryEntry[]): void {
  try {
    localStorage.setItem(REGISTRY_KEY, JSON.stringify(entries));
  } catch {
    // Storage can be disabled in a hardened browser. UUID entropy still prevents practical
    // collision; the live setup scan remains the primary guard for visible entity IDs.
  }
}

function uuid(): string {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  return [...bytes]
    .map((value, index) =>
      [4, 6, 8, 10].includes(index)
        ? `-${value.toString(16).padStart(2, "0")}`
        : value.toString(16).padStart(2, "0")
    )
    .join("");
}

/**
 * Inspect every supplied live ID plus every unexpired local lease before reserving a new ID.
 * Prefixes make roles readable; UUIDs keep independent PCs and parallel app instances apart.
 */
export function reserveIdentity(kind: string, observedIds: Iterable<string>): IdentityLease {
  const now = Date.now();
  const registry = readRegistry(now);
  const occupied = new Set([...observedIds, ...registry.map((entry) => entry.id)]);
  let id = "";
  for (let attempt = 0; attempt < 32; attempt += 1) {
    const candidate = `pp-${kind}-${uuid()}`;
    if (!occupied.has(candidate)) {
      id = candidate;
      break;
    }
  }
  if (!id) throw new Error(`unable to reserve a unique ${kind} ID after 32 attempts`);

  writeRegistry([...registry, { id, kind, lastSeen: now }]);
  const heartbeat = () => {
    const current = readRegistry();
    const withoutSelf = current.filter((entry) => entry.id !== id);
    writeRegistry([...withoutSelf, { id, kind, lastSeen: Date.now() }]);
  };
  const release = () => writeRegistry(readRegistry().filter((entry) => entry.id !== id));
  return { id, heartbeat, release };
}

export function stableEntityId(prefix: string, source: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < source.length; index += 1) {
    hash ^= source.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return `pp-${prefix}-${(hash >>> 0).toString(36)}`;
}

export function domSafeId(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "");
}

export function shortEntityId(value: string | null): string {
  if (!value) return "inspecting existing IDs";
  const suffix = value.split("-").at(-1) || value;
  return `orch-${suffix.slice(0, 8)}`;
}
