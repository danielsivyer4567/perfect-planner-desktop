export interface BuildScreenshotCapture {
  id: string;
  label: string;
  planNodes: string[];
  url: string;
  width: number;
  height: number;
  sha256: string;
  sourceArtifact: string;
}

interface BrowserProofRecord {
  runner: "playwright-script";
  result: "passed";
  command: string;
  captureCount: number;
  requiredUiNodeCount: number;
  requiredUiNodes: string[];
}

interface BuildScreenshotManifest {
  schemaVersion: number;
  generatedAt: string;
  captures: BuildScreenshotCapture[];
  browserProof: BrowserProofRecord;
}

export interface BuildScreenshotState {
  status: "loading" | "ready" | "partial" | "unavailable" | "not-required";
  generatedAt: string | null;
  captures: Map<string, BuildScreenshotCapture>;
  proof: BrowserProofRecord | null;
  requiredForPlan: number;
  message: string;
}

export async function readBuildScreenshotState(
  planNumber: string,
  uiNodeCount: number,
): Promise<BuildScreenshotState> {
  try {
    const response = await fetch("/build-screenshots/manifest.json", { cache: "no-store" });
    if (!response.ok) {
      return {
        status: "unavailable",
        generatedAt: null,
        captures: new Map(),
        proof: null,
        requiredForPlan: 0,
        message: `Build screenshot manifest returned HTTP ${response.status}.`,
      };
    }
    const manifest = await response.json() as Partial<BuildScreenshotManifest>;
    if (
      manifest.schemaVersion !== 1 ||
      typeof manifest.generatedAt !== "string" ||
      !Array.isArray(manifest.captures) ||
      manifest.browserProof?.runner !== "playwright-script" ||
      manifest.browserProof.result !== "passed" ||
      typeof manifest.browserProof.captureCount !== "number" ||
      typeof manifest.browserProof.requiredUiNodeCount !== "number" ||
      !Array.isArray(manifest.browserProof.requiredUiNodes) ||
      manifest.browserProof.captureCount !== manifest.captures.length ||
      manifest.browserProof.requiredUiNodeCount !== manifest.browserProof.requiredUiNodes.length
    ) {
      return {
        status: "unavailable",
        generatedAt: null,
        captures: new Map(),
        proof: null,
        requiredForPlan: 0,
        message: "Build screenshot manifest is missing verified browser-proof provenance.",
      };
    }
    const captures = new Map<string, BuildScreenshotCapture>();
    for (const capture of manifest.captures) {
      if (
        !capture ||
        typeof capture.url !== "string" ||
        typeof capture.width !== "number" ||
        typeof capture.height !== "number" ||
        !Array.isArray(capture.planNodes)
      ) continue;
      for (const planNode of capture.planNodes) {
        const [number, nodeId] = String(planNode).split(":", 2);
        if (number === planNumber && nodeId) captures.set(nodeId, capture);
      }
    }
    const requiredForPlan = manifest.browserProof.requiredUiNodes.filter((node) => node.startsWith(`${planNumber}:`)).length;
    const status = requiredForPlan === 0
      ? uiNodeCount === 0 ? "not-required" : "unavailable"
      : captures.size >= requiredForPlan ? "ready" : "partial";
    return {
      status,
      generatedAt: manifest.generatedAt,
      captures,
      proof: manifest.browserProof,
      requiredForPlan,
      message: status === "ready"
        ? "Scripted browser proof passed for every mapped UI node."
        : status === "not-required"
          ? "This plan has no UI nodes requiring build screenshots."
          : requiredForPlan === 0
            ? "This plan has UI pages but is absent from the build screenshot manifest."
            : `Scripted browser proof passed, but this plan maps ${captures.size} of ${requiredForPlan} required UI nodes.`,
    };
  } catch {
    return {
      status: "unavailable",
      generatedAt: null,
      captures: new Map(),
      proof: null,
      requiredForPlan: 0,
      message: "Build screenshot manifest could not be read.",
    };
  }
}
