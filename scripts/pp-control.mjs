#!/usr/bin/env node

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawnSync } from "node:child_process";

const APP_IDENTIFIER = "com.looplet.perfectplanner";
const INBOX_DIRECTORY = "control-plane-inbox";
const REPOSITORY_SENTINEL = "__repository__";
const ORCHESTRATOR_TARGET = "__orchestrator__";
const MAX_BODY_BYTES = 256 * 1024;
let dropSequence = 0;

function fail(message) {
  throw new Error(message);
}

function required(value, label) {
  const normalized = typeof value === "string" ? value.trim() : "";
  if (!normalized) fail(`${label} is required`);
  if (normalized.includes("\0")) fail(`${label} cannot contain a null character`);
  return normalized;
}

function stableEntityId(prefix, source) {
  let hash = 0x811c9dc5;
  for (let index = 0; index < source.length; index += 1) {
    hash ^= source.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return `pp-${prefix}-${(hash >>> 0).toString(36)}`;
}

function parseArguments(argv) {
  const [command = "help", ...tokens] = argv;
  const options = new Map();
  for (let index = 0; index < tokens.length; index += 1) {
    const token = tokens[index];
    if (!token.startsWith("--")) fail(`unexpected positional argument: ${token}`);
    const equals = token.indexOf("=");
    const name = (equals >= 0 ? token.slice(2, equals) : token.slice(2)).trim();
    if (!name) fail("empty option name");
    if (options.has(name)) fail(`option --${name} was supplied more than once`);
    if (equals >= 0) {
      options.set(name, token.slice(equals + 1));
      continue;
    }
    const value = tokens[index + 1];
    if (value === undefined || value.startsWith("--")) fail(`option --${name} needs a value`);
    options.set(name, value);
    index += 1;
  }
  return { command, options };
}

function option(options, name, environmentName, fallback = undefined) {
  if (options.has(name)) return options.get(name);
  if (environmentName && process.env[environmentName]) return process.env[environmentName];
  return fallback;
}

function rejectUnknownOptions(options, allowed) {
  for (const name of options.keys()) {
    if (!allowed.has(name)) fail(`unsupported option: --${name}`);
  }
}

/** Run Git directly, never through cmd.exe, PowerShell, or a POSIX shell. */
function git(cwd, ...args) {
  const result = spawnSync("git", ["-C", cwd, ...args], {
    encoding: "utf8",
    windowsHide: true,
    shell: false,
  });
  if (result.error) fail(`cannot run Git: ${result.error.message}`);
  if (result.status !== 0) {
    fail(`Git ${args.join(" ")} failed: ${(result.stderr || result.stdout || "unknown error").trim()}`);
  }
  return result.stdout.trim();
}

function canonicalRepository(worktreePath) {
  const common = git(worktreePath, "rev-parse", "--git-common-dir");
  const commonDirectory = path.resolve(worktreePath, common);
  return path.basename(commonDirectory).toLowerCase() === ".git"
    ? path.dirname(commonDirectory)
    : worktreePath;
}

function readPlanScope(planArgument) {
  const planPath = path.resolve(required(planArgument, "--plan"));
  let plan;
  try {
    plan = JSON.parse(fs.readFileSync(planPath, "utf8"));
  } catch (error) {
    fail(`cannot read plan JSON ${planPath}: ${error.message}`);
  }
  if (!plan || typeof plan !== "object" || Array.isArray(plan)) fail("plan must be a JSON object");
  if (!Array.isArray(plan.vertebrae)) fail("plan.vertebrae must be an array");

  const worktreePath = path.resolve(git(path.dirname(planPath), "rev-parse", "--show-toplevel"));
  const relativePlan = path.relative(worktreePath, planPath);
  if (relativePlan.startsWith("..") || path.isAbsolute(relativePlan)) {
    fail("plan must be inside its Git worktree");
  }
  const repositoryRoot = canonicalRepository(worktreePath);
  const repositorySource = repositoryRoot.replace(/\\/g, "/").toLocaleLowerCase();
  const repositoryId = stableEntityId("repo", repositorySource);
  let branchName = git(worktreePath, "branch", "--show-current");
  if (!branchName) branchName = `detached@${git(worktreePath, "rev-parse", "--short=12", "HEAD")}`;

  return {
    plan,
    planPath,
    worktreePath,
    repositoryRoot,
    repositoryId,
    organizationId: repositoryId,
    branchName,
    planId: stableEntityId("plan", planPath.toLocaleLowerCase()),
    planNumber:
      typeof plan.meta?.number === "string" && plan.meta.number.trim()
        ? plan.meta.number.trim()
        : "unassigned-plan",
  };
}

function tauriAppDataDirectory(override) {
  if (override) return path.resolve(override);
  if (process.platform === "win32") {
    return path.join(required(process.env.APPDATA, "APPDATA"), APP_IDENTIFIER);
  }
  if (process.platform === "darwin") {
    return path.join(required(os.homedir(), "home directory"), "Library", "Application Support", APP_IDENTIFIER);
  }
  const dataRoot = process.env.XDG_DATA_HOME
    ? path.resolve(process.env.XDG_DATA_HOME)
    : path.join(required(os.homedir(), "home directory"), ".local", "share");
  return path.join(dataRoot, APP_IDENTIFIER);
}

function validateEnvelope(envelope) {
  if (envelope.schemaVersion !== 1 || envelope.type !== "POST_MESSAGE") {
    fail("invalid control-plane drop envelope");
  }
  if (!Number.isSafeInteger(envelope.createdAtMs) || envelope.createdAtMs <= 0) {
    fail("drop createdAtMs must be a positive integer");
  }
  const bodyBytes = Buffer.byteLength(required(envelope.request?.body, "request.body"), "utf8");
  if (bodyBytes > MAX_BODY_BYTES) fail(`request.body exceeds ${MAX_BODY_BYTES} bytes`);
  required(envelope.request?.scope?.repositoryId, "request.scope.repositoryId");
  required(envelope.request?.scope?.planId, "request.scope.planId");
  required(envelope.request?.idempotencyKey, "request.idempotencyKey");
}

/**
 * A producer writes and flushes a private file, then renames it inside the same directory. The
 * Tauri ingester must read only `.json` files, so a crash can expose a stale `.tmp` but never a
 * partial message. Every producer writes its own file; there is no contended shared append.
 */
function writeDrop(envelope, appDataOverride) {
  validateEnvelope(envelope);
  const inbox = path.join(tauriAppDataDirectory(appDataOverride), INBOX_DIRECTORY);
  fs.mkdirSync(inbox, { recursive: true });
  const keyHash = stableEntityId("drop", envelope.request.idempotencyKey).slice("pp-drop-".length);
  for (let attempt = 0; attempt < 32; attempt += 1) {
    dropSequence += 1;
    const stem = `${envelope.createdAtMs}-${process.pid}-${dropSequence}-${keyHash}`;
    const temporaryPath = path.join(inbox, `.${stem}.tmp`);
    const finalPath = path.join(inbox, `${stem}.json`);
    let handle;
    try {
      handle = fs.openSync(temporaryPath, "wx", 0o600);
      fs.writeFileSync(handle, `${JSON.stringify(envelope)}\n`, "utf8");
      fs.fsyncSync(handle);
      fs.closeSync(handle);
      handle = undefined;
      fs.renameSync(temporaryPath, finalPath);
      if (process.platform !== "win32") {
        const directoryHandle = fs.openSync(inbox, "r");
        try {
          fs.fsyncSync(directoryHandle);
        } finally {
          fs.closeSync(directoryHandle);
        }
      }
      return finalPath;
    } catch (error) {
      if (handle !== undefined) fs.closeSync(handle);
      try {
        fs.unlinkSync(temporaryPath);
      } catch {
        // A missing temp file means rename already succeeded or another cleanup won the race.
      }
      if (error?.code !== "EEXIST") throw error;
    }
  }
  fail("could not allocate a unique atomic drop file after 32 attempts");
}

function baseScope(scope, nodeId, itemId, workerId) {
  return {
    organizationId: scope.organizationId,
    repositoryId: scope.repositoryId,
    repositoryRoot: scope.repositoryRoot,
    worktreePath: scope.worktreePath,
    branchName: scope.branchName,
    planId: scope.planId,
    planPath: scope.planPath,
    nodeId,
    itemId,
    workerId,
    // A filesystem-drop producer cannot truthfully know the current UI lease holder.
    orchestratorId: null,
  };
}

function noteEnvelope(options, nowMs = Date.now()) {
  rejectUnknownOptions(
    options,
    new Set(["plan", "node", "item", "worker", "body", "subject", "idempotency-key", "correlation-id", "app-data"]),
  );
  const scope = readPlanScope(option(options, "plan", "PP_PLAN"));
  const nodeId = required(option(options, "node", "PP_VERTEBRA"), "--node");
  const vertebra = scope.plan.vertebrae.find((candidate) => candidate?.id === nodeId);
  if (!vertebra) fail(`node ${nodeId} is not present in ${scope.planPath}`);
  const itemId = option(options, "item", "PP_ITEM", null);
  if (itemId) {
    const itemIds = new Set(
      (Array.isArray(vertebra.checklist) ? vertebra.checklist : []).flatMap((item, index) => [
        `${nodeId}:${index}`,
        typeof item?.id === "string" ? item.id : "",
      ]),
    );
    if (!itemIds.has(itemId)) fail(`item ${itemId} is not present on node ${nodeId}`);
  }
  const workerId = required(option(options, "worker", "PP_SESSION"), "--worker");
  const body = required(option(options, "body", "PP_NOTE"), "--body");
  const subject = option(options, "subject", null, `Worker note · ${nodeId}`).trim();
  const fingerprint = [scope.repositoryId, scope.planId, nodeId, itemId || "", workerId, subject, body].join("\0");
  const idempotencyKey = option(
    options,
    "idempotency-key",
    null,
    `worker-note:${stableEntityId("event", fingerprint)}`,
  );
  return {
    schemaVersion: 1,
    type: "POST_MESSAGE",
    createdAtMs: nowMs,
    request: {
      scope: baseScope(scope, nodeId, itemId || null, workerId),
      kind: "WORKER_NOTE",
      sender: { kind: "WORKER", actorId: workerId },
      destination: {
        kind: "ORCHESTRATOR",
        targetId: ORCHESTRATOR_TARGET,
        connectorId: "pp-control-drop",
        routeId: `local-orchestrator-inbox:${scope.repositoryId}`,
        label: "Perfect Planner head orchestrator",
        requiresAcknowledgement: true,
        retryBaseMs: 5_000,
        registeredAtMs: null,
        metadata: {
          source: "pp-control",
          planNumber: scope.planNumber,
          nodeId,
        },
      },
      subject,
      body,
      idempotencyKey: required(idempotencyKey, "--idempotency-key"),
      correlationId: option(
        options,
        "correlation-id",
        null,
        `worker-note:${scope.planId}:${nodeId}:${workerId}`,
      ),
      replyToMessageId: null,
      maxDeliveryAttempts: 3,
    },
  };
}

function codexRegistrationEnvelope(options, nowMs = Date.now()) {
  rejectUnknownOptions(options, new Set(["plan", "thread", "thread-id", "label", "app-data"]));
  const scope = readPlanScope(option(options, "plan", "PP_PLAN"));
  const threadId = required(
    option(options, "thread-id", "CODEX_THREAD_ID", option(options, "thread", null)),
    "--thread-id",
  );
  const routeId = `codex-exec:${scope.repositoryId}:${threadId}`;
  const label = option(options, "label", null, `Codex task ${threadId}`).trim();
  return {
    schemaVersion: 1,
    type: "POST_MESSAGE",
    createdAtMs: nowMs,
    request: {
      scope: baseScope(scope, REPOSITORY_SENTINEL, null, REPOSITORY_SENTINEL),
      kind: "STATUS",
      sender: { kind: "CONNECTOR", actorId: "codex-exec" },
      destination: {
        kind: "CHAT",
        targetId: threadId,
        connectorId: "codex-exec",
        routeId,
        label: required(label, "--label"),
        requiresAcknowledgement: true,
        retryBaseMs: 5_000,
        registeredAtMs: null,
        metadata: {
          connector: "codex-exec",
          registration: "repository",
          planNumber: scope.planNumber,
        },
      },
      subject: "Codex chat route registered",
      body: `Registered codex-exec route ${routeId} for repository ${scope.repositoryId}.`,
      idempotencyKey: `connector-registration:${routeId}`,
      correlationId: `connector-registration:${scope.repositoryId}`,
      replyToMessageId: null,
      maxDeliveryAttempts: 3,
    },
  };
}

function selfTest() {
  if (stableEntityId("repo", "c:/repos/example") !== "pp-repo-lmz2af") {
    fail("FNV repository identity vector failed");
  }
  if (
    stableEntityId("plan", "c:\\repos\\example\\.claude\\scratch\\perfect-plan\\plan.json") !==
    "pp-plan-1sig83t"
  ) {
    fail("FNV plan identity vector failed");
  }
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "perfect-planner-control-self-test-"));
  try {
    const envelope = {
      schemaVersion: 1,
      type: "POST_MESSAGE",
      createdAtMs: 1,
      request: {
        scope: { repositoryId: "pp-repo-self-test", planId: "pp-plan-self-test" },
        body: "atomic self-test",
        idempotencyKey: "self-test",
      },
    };
    const written = writeDrop(envelope, temporary);
    const parsed = JSON.parse(fs.readFileSync(written, "utf8"));
    if (parsed.request.idempotencyKey !== "self-test") fail("atomic drop round-trip failed");
    const names = fs.readdirSync(path.dirname(written));
    if (names.some((name) => name.endsWith(".tmp")) || names.filter((name) => name.endsWith(".json")).length !== 1) {
      fail("atomic drop exposed a partial or missing file");
    }
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }
  return { ok: true, fnv: "PASS", atomicDrop: "PASS", codexExecuted: false };
}

function printUsage() {
  process.stdout.write(`Perfect Planner control-plane drop client\n\n`);
  process.stdout.write(`  pp-control note --plan <plan.json> --node <A01> --worker <session> --body <text> [--item <A01:0>]\n`);
  process.stdout.write(`  pp-control register-codex --plan <plan.json> --thread-id <task-id> [--label <label>]\n`);
  process.stdout.write(`  pp-control self-test\n\n`);
  process.stdout.write(`Drops are written atomically to the Tauri app-data control-plane-inbox. No chat CLI is executed.\n`);
}

function main() {
  const { command, options } = parseArguments(process.argv.slice(2));
  if (command === "help" || command === "--help" || command === "-h") {
    printUsage();
    return;
  }
  if (command === "self-test") {
    rejectUnknownOptions(options, new Set());
    process.stdout.write(`${JSON.stringify(selfTest())}\n`);
    return;
  }
  let envelope;
  if (command === "note") envelope = noteEnvelope(options);
  else if (command === "register-codex") envelope = codexRegistrationEnvelope(options);
  else fail(`unknown command: ${command}`);
  const file = writeDrop(envelope, option(options, "app-data", "PP_CONTROL_APP_DATA"));
  process.stdout.write(
    `${JSON.stringify({ ok: true, file, repositoryId: envelope.request.scope.repositoryId, planId: envelope.request.scope.planId, routeId: envelope.request.destination.routeId })}\n`,
  );
}

try {
  main();
} catch (error) {
  process.stderr.write(`pp-control: ${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
}
