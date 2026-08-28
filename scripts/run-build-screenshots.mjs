import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const ROOT = process.cwd();
const CONTRACT_PATH = path.join(ROOT, "build-screenshots.json");
const PLAN_DIRECTORY = path.join(ROOT, ".claude", "scratch", "perfect-plan");
const ARTIFACT_OUTPUT = path.join(ROOT, "artifacts", "build-screenshots");
const DIST_OUTPUT = path.join(ROOT, "dist", "build-screenshots");

function fail(message) {
  throw new Error(message);
}

function repositoryPath(relativePath, prefix) {
  if (typeof relativePath !== "string" || !relativePath.startsWith(prefix)) {
    fail(`path must start with ${prefix}: ${String(relativePath)}`);
  }
  const resolved = path.resolve(ROOT, relativePath);
  const relative = path.relative(ROOT, resolved);
  if (!relative || relative.startsWith("..") || path.isAbsolute(relative)) {
    fail(`path escapes the repository: ${relativePath}`);
  }
  return resolved;
}

function pngDimensions(filePath) {
  const header = fs.readFileSync(filePath).subarray(0, 24);
  if (header.length !== 24 || header.subarray(0, 8).toString("hex") !== "89504e470d0a1a0a") {
    fail(`capture is not a PNG: ${filePath}`);
  }
  return { width: header.readUInt32BE(16), height: header.readUInt32BE(20) };
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function requiredUiNodes() {
  if (!fs.existsSync(PLAN_DIRECTORY)) return new Set();
  const required = new Set();
  for (const name of fs.readdirSync(PLAN_DIRECTORY).filter((value) => value.endsWith(".json"))) {
    const plan = readJson(path.join(PLAN_DIRECTORY, name));
    const number = plan?.meta?.number;
    if (typeof number !== "string" || !Array.isArray(plan?.vertebrae)) continue;
    for (const node of plan.vertebrae) {
      if (
        typeof node?.id === "string" &&
        Array.isArray(node.checklist) &&
        node.checklist.some((item) => item?.ui === true)
      ) {
        required.add(`${number}:${node.id}`);
      }
    }
  }
  return required;
}

function validateContract(contract) {
  if (contract?.schemaVersion !== 1 || !Array.isArray(contract?.captures) || !contract.captures.length) {
    fail("build-screenshots.json must contain a non-empty schemaVersion 1 capture list");
  }
  const ids = new Set();
  const declaredNodes = new Set();
  for (const capture of contract.captures) {
    if (!/^[a-z0-9-]+$/.test(capture?.id || "") || ids.has(capture.id)) {
      fail(`capture ID is invalid or duplicated: ${String(capture?.id)}`);
    }
    ids.add(capture.id);
    repositoryPath(capture.script, "tests/");
    repositoryPath(capture.artifact, "artifacts/");
    if (!Array.isArray(capture.planNodes) || !capture.planNodes.length) {
      fail(`capture ${capture.id} has no plan nodes`);
    }
    for (const planNode of capture.planNodes) {
      if (!/^[A-Za-z0-9-]+:[A-Za-z0-9-]+$/.test(planNode) || declaredNodes.has(planNode)) {
        fail(`plan node mapping is invalid or duplicated: ${String(planNode)}`);
      }
      declaredNodes.add(planNode);
    }
  }
  const requiredNodes = requiredUiNodes();
  const missing = [...requiredNodes].filter((value) => !declaredNodes.has(value));
  const unknown = [...declaredNodes].filter((value) => !requiredNodes.has(value));
  if (missing.length || unknown.length) {
    fail(`screenshot coverage mismatch; missing=${missing.join(",") || "none"}; unknown=${unknown.join(",") || "none"}`);
  }
}

function runPython(script) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.env.PYTHON || "python", [script], {
      cwd: ROOT,
      env: { ...process.env, PP_APP_URL: "http://127.0.0.1:5180/" },
      stdio: "inherit",
      windowsHide: true,
    });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) resolve();
      else reject(new Error(`${script} failed (${signal || `exit ${code}`})`));
    });
  });
}

const contract = readJson(CONTRACT_PATH);
validateContract(contract);

// Remove the previous manifest before capture begins. A failed build must never expose stale
// screenshots as if they belonged to the current bundle.
for (const output of [ARTIFACT_OUTPUT, DIST_OUTPUT]) {
  fs.rmSync(output, { recursive: true, force: true });
  fs.mkdirSync(path.join(output, "files"), { recursive: true });
}

const { createServer } = await import("vite");
const server = await createServer({
  root: ROOT,
  logLevel: "error",
  server: { host: "127.0.0.1", port: 5180, strictPort: true },
});

try {
  await server.listen();
  for (const script of [...new Set(contract.captures.map((capture) => capture.script))]) {
    await runPython(script);
  }
} finally {
  await server.close();
}

const generatedAt = new Date().toISOString();
const captures = contract.captures.map((capture) => {
  const source = repositoryPath(capture.artifact, "artifacts/");
  if (!fs.statSync(source, { throwIfNoEntry: false })?.isFile()) {
    fail(`required screenshot was not produced: ${capture.artifact}`);
  }
  const dimensions = pngDimensions(source);
  if (dimensions.width < 1280 || dimensions.height < 720) {
    fail(`required screenshot is below 1280x720: ${capture.artifact} (${dimensions.width}x${dimensions.height})`);
  }
  const bytes = fs.readFileSync(source);
  const sha256 = createHash("sha256").update(bytes).digest("hex");
  const fileName = `${capture.id}.png`;
  for (const output of [ARTIFACT_OUTPUT, DIST_OUTPUT]) {
    fs.copyFileSync(source, path.join(output, "files", fileName));
  }
  return {
    id: capture.id,
    label: capture.label,
    planNodes: capture.planNodes,
    url: `/build-screenshots/files/${fileName}`,
    width: dimensions.width,
    height: dimensions.height,
    sha256,
    sourceArtifact: capture.artifact,
  };
});

const manifest = { schemaVersion: 1, generatedAt, captures };
for (const output of [ARTIFACT_OUTPUT, DIST_OUTPUT]) {
  fs.writeFileSync(path.join(output, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
}

console.log(`build screenshots: PASS (${captures.length} captures / ${requiredUiNodes().size} UI nodes)`);
console.log(`manifest: ${path.join(ARTIFACT_OUTPUT, "manifest.json")}`);
