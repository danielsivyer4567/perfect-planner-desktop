import { defineConfig, Plugin } from "vite";
import react from "@vitejs/plugin-react";
import http from "node:http";
import fs from "node:fs";
import path from "node:path";

interface ProbeReply {
  status: number;
  body: unknown;
}

interface EvidenceReply {
  status: number;
  contentType: string;
  body: Buffer;
}

function readBoard(
  port: number,
  path: "/whoami" | "/workers" | "/plan"
): Promise<ProbeReply> {
  const responseLimit = path === "/plan" ? 8 * 1024 * 1024 : 512 * 1024;
  return new Promise((resolve, reject) => {
    const upstream = http.get(
      {
        host: "127.0.0.1",
        port,
        path,
        headers: { Host: `127.0.0.1:${port}`, Connection: "close" },
        timeout: 700,
      },
      (response) => {
        const chunks: Buffer[] = [];
        let size = 0;
        response.on("data", (chunk: Buffer) => {
          size += chunk.length;
          if (size > responseLimit) {
            response.destroy(new Error(`board ${path} response exceeded ${responseLimit} bytes`));
            return;
          }
          chunks.push(chunk);
        });
        response.on("end", () => {
          try {
            resolve({
              status: response.statusCode || 500,
              body: JSON.parse(Buffer.concat(chunks).toString("utf8")),
            });
          } catch {
            reject(new Error("board returned malformed JSON"));
          }
        });
      }
    );
    upstream.on("timeout", () => upstream.destroy(new Error("board probe timed out")));
    upstream.on("error", reject);
  });
}

function readEvidence(port: number, fileName: string): Promise<EvidenceReply> {
  return new Promise((resolve, reject) => {
    const upstream = http.get(
      {
        host: "127.0.0.1",
        port,
        path: `/evidence/${encodeURIComponent(fileName)}`,
        headers: { Host: `127.0.0.1:${port}`, Connection: "close" },
        timeout: 1200,
      },
      (response) => {
        const chunks: Buffer[] = [];
        let size = 0;
        response.on("data", (chunk: Buffer) => {
          size += chunk.length;
          if (size > 16 * 1024 * 1024) {
            response.destroy(new Error("evidence artifact exceeded 16 MiB"));
            return;
          }
          chunks.push(chunk);
        });
        response.on("end", () =>
          resolve({
            status: response.statusCode || 500,
            contentType: String(response.headers["content-type"] || "application/octet-stream"),
            body: Buffer.concat(chunks),
          })
        );
      }
    );
    upstream.on("timeout", () => upstream.destroy(new Error("evidence read timed out")));
    upstream.on("error", reject);
  });
}

/**
 * Board discovery for `npm run dev`.
 *
 * perfect-plan-server.cjs 403s any request carrying a foreign `Origin`, so the page cannot
 * probe `/whoami` itself. This middleware does it from the dev server instead — Node sends
 * no Origin, and sets the exact `Host` the board requires. Read-only, loopback only, and
 * strictly inside the port window the app scans; it can reach nothing else. `/workers`
 * additionally re-checks `/whoami` so a reused port cannot leak another plan's state.
 */
function boardProbe(start = 5230, end = 5249): Plugin {
  return {
    name: "perfect-planner-board-probe",
    configureServer(server) {
      server.middlewares.use("/board-probe", async (req, res) => {
        const requestUrl = new URL(req.url || "/", "http://127.0.0.1");
        const match = /^\/(\d{2,5})\/(whoami|workers|plan)\/?$/.exec(requestUrl.pathname);
        const evidenceMatch = /^\/(\d{2,5})\/evidence\/([^/]+)$/.exec(requestUrl.pathname);
        const port = Number(match?.[1] || evidenceMatch?.[1] || NaN);
        const endpoint = match?.[2] as "whoami" | "workers" | "plan" | undefined;

        const fail = (code: number, error: string) => {
          res.statusCode = code;
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ ok: false, error }));
        };

        if ((!match && !evidenceMatch) || !Number.isInteger(port) || port < start || port > end)
          return fail(400, "probe is limited to board reads in the configured port window");

        try {
          if (endpoint === "workers" || endpoint === "plan" || evidenceMatch) {
            const expectedPlanPath = requestUrl.searchParams.get("planPath");
            if (!expectedPlanPath) return fail(400, "board read requires plan identity");
            const identity = await readBoard(port, "/whoami");
            const body = identity.body as { ok?: boolean; planPath?: string };
            if (
              identity.status !== 200 ||
              body?.ok !== true ||
              body.planPath !== expectedPlanPath
            ) {
              return fail(200, "board identity changed");
            }
          }

          if (evidenceMatch) {
            const fileName = decodeURIComponent(evidenceMatch[2]);
            if (!/^[A-Za-z0-9_.-]+$/.test(fileName) || fileName.includes(".."))
              return fail(400, "invalid evidence file name");
            const reply = await readEvidence(port, fileName);
            if (reply.status !== 200) return fail(404, "no evidence");
            const textual = /^(?:text\/|application\/(?:json|javascript))/.test(reply.contentType);
            res.statusCode = 200;
            res.setHeader("Content-Type", "application/json");
            res.setHeader("Cache-Control", "no-store");
            res.end(JSON.stringify({
              name: fileName,
              mime: reply.contentType.split(";")[0],
              ...(textual
                ? { text: reply.body.toString("utf8") }
                : { dataBase64: reply.body.toString("base64") }),
            }));
            return;
          }

          const reply = await readBoard(port, `/${endpoint!}`);
          if (reply.status !== 200) return fail(200, "no board");
          res.statusCode = 200;
          res.setHeader("Content-Type", "application/json");
          res.setHeader("Cache-Control", "no-store");
          res.end(JSON.stringify(reply.body));
        } catch {
          fail(200, "no board");
        }
      });
    },
  };
}

function buildScreenshotArtifacts(): Plugin {
  const artifactRoot = path.resolve(process.cwd(), "artifacts", "build-screenshots");
  return {
    name: "perfect-planner-build-screenshots",
    configureServer(server) {
      server.middlewares.use("/build-screenshots", (req, res) => {
        const requestPath = new URL(req.url || "/", "http://127.0.0.1").pathname.replace(/^\//, "");
        if (!/^(?:manifest\.json|files\/[a-z0-9-]+\.png)$/.test(requestPath)) {
          res.statusCode = 404;
          return res.end("not found");
        }
        const filePath = path.resolve(artifactRoot, requestPath);
        if (!filePath.startsWith(`${artifactRoot}${path.sep}`) && filePath !== path.join(artifactRoot, "manifest.json")) {
          res.statusCode = 400;
          return res.end("invalid path");
        }
        fs.readFile(filePath, (error, body) => {
          if (error) {
            res.statusCode = 404;
            return res.end("not found");
          }
          res.statusCode = 200;
          res.setHeader("Content-Type", filePath.endsWith(".png") ? "image/png" : "application/json");
          res.setHeader("Cache-Control", "no-store");
          res.end(body);
        });
      });
    },
  };
}

export default defineConfig({
  plugins: [react(), boardProbe(), buildScreenshotArtifacts()],
  clearScreen: false,
  server: {
    port: 5180,
    strictPort: true,
    host: "127.0.0.1",
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
