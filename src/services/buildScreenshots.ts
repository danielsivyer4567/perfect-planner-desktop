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

interface BuildScreenshotManifest {
  schemaVersion: number;
  generatedAt: string;
  captures: BuildScreenshotCapture[];
}

export async function readBuildScreenshots(
  planNumber: string,
): Promise<Map<string, BuildScreenshotCapture>> {
  try {
    const response = await fetch("/build-screenshots/manifest.json", { cache: "no-store" });
    if (!response.ok) return new Map();
    const manifest = await response.json() as Partial<BuildScreenshotManifest>;
    if (manifest.schemaVersion !== 1 || !Array.isArray(manifest.captures)) return new Map();
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
    return captures;
  } catch {
    return new Map();
  }
}
