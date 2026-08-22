import type { ResourceSnapshot } from "./orchestratorPipeline";

export interface ResourceProbeResult {
  provider: string;
  executable: string;
  sampledAtMs: number;
  resources: ResourceSnapshot;
}

const inTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const requireFinite = (value: number, field: string): number => {
  if (!Number.isFinite(value) || value < 0) throw new Error(`${field} is invalid`);
  return value;
};

const validate = (value: ResourceProbeResult): ResourceProbeResult => {
  if (!value || typeof value !== "object") throw new Error("resource probe returned no result");
  if (value.provider !== "Windows native system APIs") {
    throw new Error("resource probe returned an unknown provider");
  }
  if (!value.executable?.trim()) throw new Error("resource probe did not identify its executable");
  requireFinite(value.sampledAtMs, "sampledAtMs");
  requireFinite(value.resources.logicalCpuCount, "logicalCpuCount");
  requireFinite(value.resources.cpuUsagePercent, "cpuUsagePercent");
  requireFinite(value.resources.totalMemoryBytes, "totalMemoryBytes");
  requireFinite(value.resources.availableMemoryBytes, "availableMemoryBytes");
  requireFinite(value.resources.repositoryDiskAvailableBytes, "repositoryDiskAvailableBytes");
  return value;
};

const exactErrorMessage = (cause: unknown): string => {
  if (cause instanceof Error && cause.message.trim()) return cause.message.trim();
  if (typeof cause === "string" && cause.trim()) return cause.trim();
  if (cause && typeof cause === "object" && "message" in cause) {
    const message = String((cause as { message?: unknown }).message || "").trim();
    if (message) return message;
  }
  return "Native resource probe returned an unknown failure";
};

export async function probeResourceGuard(repositoryRoot: string): Promise<ResourceProbeResult> {
  if (!repositoryRoot.trim()) throw new Error("Select a repository to check system resources");
  if (!inTauri()) throw new Error("Resource guard is available in the Tauri desktop app");
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    const result = await invoke<ResourceProbeResult>("orchestrator_resource_probe", {
      request: { repositoryRoot },
    });
    return validate(result);
  } catch (cause) {
    throw new Error(exactErrorMessage(cause));
  }
}
