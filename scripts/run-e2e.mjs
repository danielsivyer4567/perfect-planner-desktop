import { spawn } from "node:child_process";
import process from "node:process";
import { createServer } from "vite";

const tests = [
  "tests/control_connector_e2e.py",
  "tests/approval_chat_bridge_e2e.py",
  "tests/alarm_e2e.py",
  "tests/repository_rail_e2e.py",
  "tests/evidence_e2e.py",
  "tests/control_plane_e2e.py",
  "tests/orchestrator_pipeline_e2e.py",
];

function runPython(test) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.env.PYTHON || "python", [test], {
      cwd: process.cwd(),
      env: {
        ...process.env,
        PP_APP_URL: "http://127.0.0.1:5180/",
      },
      stdio: "inherit",
      windowsHide: true,
    });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) resolve();
      else reject(new Error(`${test} failed (${signal || `exit ${code}`})`));
    });
  });
}

const server = await createServer({
  root: process.cwd(),
  logLevel: "error",
  server: {
    host: "127.0.0.1",
    port: 5180,
    strictPort: true,
  },
});

try {
  await server.listen();
  for (const test of tests) await runPython(test);
} finally {
  await server.close();
}
