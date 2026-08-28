import React, { useMemo, useState } from "react";
import type { Board } from "../services/boards";
import type { OrchestratorSnapshot } from "../services/orchestratorPipeline";
import type { PlanSnapshot, Vertebra } from "../types/plan";
import type { ResourceGuardState } from "./ResourceGuard";

export interface DiagnosticEntry {
  id: string;
  at: number;
  level: "info" | "warning" | "error";
  source: string;
  message: string;
}

export function DiagnosticsConsole({
  activeBoard,
  activePlan,
  resourceGuard,
  pipelineSnapshot,
  entries,
  onClear,
}: {
  activeBoard: Board | null;
  activePlan?: PlanSnapshot | null;
  resourceGuard: ResourceGuardState;
  pipelineSnapshot?: OrchestratorSnapshot | null;
  entries: DiagnosticEntry[];
  onClear: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [maximized, setMaximized] = useState(false);
  const [tab, setTab] = useState<"status" | "gaps" | "logs">("status");
  const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  const issueCount = useMemo(
    () => entries.filter((entry) => entry.level !== "info").length,
    [entries]
  );
  const diagnosticGroups = useMemo(() => {
    const counts = { application: 0, messaging: 0, plan: 0 };
    entries.forEach((entry) => {
      const source = entry.source.toLocaleLowerCase();
      if (/(message|control|connector|recovery|delivery)/.test(source)) counts.messaging += 1;
      else if (/(plan|pipeline|context|board)/.test(source)) counts.plan += 1;
      else counts.application += 1;
    });
    return counts;
  }, [entries]);
  const incompleteNodes = useMemo(() => {
    if (!activePlan) return [];
    return activePlan.vertebrae
      .filter((node) => {
        const checklist = node.checklist || [];
        return checklist.length
          ? checklist.some((item) => !item.built || !item.tested)
          : node.status !== "done";
      })
      .sort((left, right) => left.id.localeCompare(right.id, undefined, { numeric: true }));
  }, [activePlan]);
  const collisionGuarantee = useMemo(() => {
    if (!pipelineSnapshot) {
      return {
        state: "unknown",
        code: "not-initialized",
        label: "NOT ACTIVE · NO VERIFIED RUN",
        detail: "Select or create an exact native run; a plan checkbox is not an admission receipt.",
      } as const;
    }
    const nodes = Object.values(pipelineSnapshot.scheduler.nodes);
    const safelyCompleted = nodes.length > 0 && nodes.every((node) => {
      const completion = pipelineSnapshot.scheduler.completions?.[node.id];
      return node.status === "DONE" &&
        Boolean(completion?.gate.passed) &&
        Boolean(completion?.artifacts.length) &&
        completion?.verification.every((result) => result.exitCode === 0);
    });
    if (safelyCompleted) {
      return {
        state: "ready",
        code: "completed",
        label: "ENFORCED · COMPLETED",
        detail: `${nodes.length} node${nodes.length === 1 ? "" : "s"} completed under immutable manifests, fenced authority, validation, and hashed evidence.`,
      } as const;
    }
    const running = nodes.filter((node) => node.status === "RUNNING");
    const runningAuthorityComplete = running.length > 0 && running.every(
      (node) => node.lease?.authorityEpoch !== null && Boolean(node.lease?.authorizationId)
    );
    if (pipelineSnapshot.runApproval && runningAuthorityComplete) {
      return {
        state: "ready",
        code: "lease-active",
        label: "ENFORCED · LEASE ACTIVE",
        detail: `${running.length} worker lease${running.length === 1 ? "" : "s"} bound to approval ${pipelineSnapshot.runApproval.approvalDigest.slice(0, 12)}…`,
      } as const;
    }
    if (running.length) {
      return {
        state: "warning",
        code: "authority-missing",
        label: "BLOCKED · AUTHORITY INCOMPLETE",
        detail: "A running node lacks its native approval, authorization ID, or authority epoch; treat the claim as untrusted.",
      } as const;
    }
    if (!pipelineSnapshot.preflightFresh) {
      return {
        state: "warning",
        code: "preflight-expired",
        label: "BLOCKED · PREFLIGHT EXPIRED",
        detail: "Refresh native preflight and explicitly re-approve before another admission.",
      } as const;
    }
    if (!pipelineSnapshot.runApproval) {
      return {
        state: "warning",
        code: "approval-required",
        label: "BLOCKED · APPROVAL REQUIRED",
        detail: "No native approval receipt is attached to this exact run and manifest.",
      } as const;
    }
    return {
      state: "ready",
      code: "approved",
      label: "ENFORCED · APPROVED",
      detail: `Native collision census approved ${pipelineSnapshot.runApproval.collisionAssessments.length} node${pipelineSnapshot.runApproval.collisionAssessments.length === 1 ? "" : "s"}; admission remains native-only.`,
    } as const;
  }, [pipelineSnapshot]);

  return (
    <section
      className={`diagnostics-console${open ? " open" : ""}${maximized ? " maximized" : ""}`}
      id="pp-diagnostics-console"
      data-context-kind="modal"
      data-context-label="Diagnostics and connectivity console"
      data-context-close="#pp-btn-diagnostics-toggle"
    >
      <button
        type="button"
        id="pp-btn-diagnostics-toggle"
        className="diagnostics-console-toggle"
        aria-expanded={open}
        aria-controls="pp-panel-diagnostics-console"
        onClick={() => setOpen((current) => !current)}
      >
        CONSOLE <span>{issueCount ? `${issueCount} issue${issueCount === 1 ? "" : "s"}` : "clear"}</span>
      </button>
      {open ? (
        <div id="pp-panel-diagnostics-console" className="diagnostics-console-panel">
          <header onDoubleClick={() => setMaximized((current) => !current)}>
            <div><span>SYSTEM TRUTH</span><strong>Session diagnostics</strong></div>
            <button type="button" id="pp-btn-diagnostics-maximize" onClick={() => setMaximized((current) => !current)}>
              {maximized ? "Restore" : "Maximize"}
            </button>
          </header>
          <nav role="tablist" aria-label="Console views">
            <button id="pp-tab-diagnostics-status" type="button" role="tab" aria-selected={tab === "status"} onClick={() => setTab("status")}>STATUS</button>
            <button id="pp-tab-diagnostics-gaps" type="button" role="tab" aria-selected={tab === "gaps"} onClick={() => setTab("gaps")}>PLAN GAPS · {incompleteNodes.length}</button>
            <button id="pp-tab-diagnostics-logs" type="button" role="tab" aria-selected={tab === "logs"} onClick={() => setTab("logs")}>DIAGNOSTIC LOGS · {entries.length}</button>
          </nav>
          {tab === "status" ? (
            <div className="diagnostics-status-grid" role="tabpanel">
              <article data-state={activeBoard ? "ready" : "unknown"}><span>Local board</span><strong>{activeBoard ? `LIVE · :${activeBoard.port}` : "NOT DISCOVERED"}</strong><small>{activeBoard?.planPath || "No active plan"}</small></article>
              <article data-state={isTauri && activeBoard?.approvalBridge?.registrationId ? "ready" : "warning"}><span>Chat route</span><strong>{isTauri ? activeBoard?.approvalBridge?.state || "UNREGISTERED" : "UNVERIFIED BOARD CLAIM"}</strong><small>{isTauri ? activeBoard?.approvalBridge?.routeId || "No registered native route receipt" : "Browser HTTP state is display-only and cannot attest a chat route."}</small></article>
              <article data-state={isTauri && resourceGuard.status === "active" ? "ready" : "warning"}><span>Tauri native</span><strong>{isTauri ? resourceGuard.status.toUpperCase() : "BROWSER ONLY"}</strong><small>{resourceGuard.status === "unavailable" ? resourceGuard.error : resourceGuard.result?.provider || "Native bridge not available"}</small></article>
              <article data-state={pipelineSnapshot ? "ready" : "unknown"}><span>Orchestrator run</span><strong>{pipelineSnapshot ? `VERIFIED · ${pipelineSnapshot.run.runId}` : "NOT INITIALIZED"}</strong><small>A Perfect Plan ID is not automatically an orchestrator run ID.</small></article>
              <article data-state="unknown"><span>MCP runtime</span><strong>NOT ATTESTED</strong><small>No MCP handshake receipt is present. Worker access is not assumed.</small></article>
              <article data-state={collisionGuarantee.state} data-collision-state={collisionGuarantee.code}><span>Collision guarantee</span><strong>{collisionGuarantee.label}</strong><small>{collisionGuarantee.detail}</small></article>
            </div>
          ) : tab === "gaps" ? (
            <PlanGaps activePlan={activePlan || null} nodes={incompleteNodes} collisionGuarantee={collisionGuarantee} />
          ) : (
            <div className="diagnostics-log" role="tabpanel" aria-live="polite">
              <div className="diagnostics-source-summary" aria-label="Diagnostic sources">
                <span>APPLICATION · {diagnosticGroups.application}</span>
                <span>MESSAGING · {diagnosticGroups.messaging}</span>
                <span>PLAN / RUN · {diagnosticGroups.plan}</span>
                <span data-state="unknown">BROWSER CONSOLE · NOT COLLECTED HERE</span>
              </div>
              <button type="button" id="pp-btn-diagnostics-clear" onClick={onClear}>Clear display</button>
              {entries.length ? entries.slice().reverse().map((entry) => (
                <article key={entry.id} data-level={entry.level}>
                  <time dateTime={new Date(entry.at).toISOString()}>{new Date(entry.at).toLocaleTimeString()}</time>
                  <strong>{entry.source}</strong>
                  <p>{entry.message}</p>
                </article>
              )) : <p>No local diagnostic events have been recorded in this session.</p>}
            </div>
          )}
        </div>
      ) : null}
    </section>
  );
}

function PlanGaps({
  activePlan,
  nodes,
  collisionGuarantee,
}: {
  activePlan: PlanSnapshot | null;
  nodes: Vertebra[];
  collisionGuarantee: { label: string; detail: string; state: string };
}) {
  if (!activePlan) {
    return (
      <div className="diagnostics-gaps-empty" role="tabpanel">
        <strong>NO PLAN SNAPSHOT</strong>
        <p>The active plan could not be read, so this console will not guess which work is incomplete.</p>
      </div>
    );
  }

  if (!nodes.length) {
    return (
      <div className="diagnostics-gaps-empty" role="tabpanel">
        <strong>NO INCOMPLETE NODES FOUND</strong>
        <p>This only reflects the loaded plan file. Release proof and external CI still require their own verification.</p>
      </div>
    );
  }

  return (
    <div className="diagnostics-gaps" role="tabpanel" aria-label="Incomplete plan nodes">
      <aside className="diagnostics-guarantee-warning" role="status" data-state={collisionGuarantee.state}>
        <strong>{collisionGuarantee.label}</strong>
        <span>{collisionGuarantee.detail}</span>
      </aside>
      <header>
        <div>
          <span>LOADED PLAN TRUTH</span>
          <strong>{activePlan.meta?.number || "UNNUMBERED"} · {activePlan.title || activePlan.meta?.topic || "Untitled plan"}</strong>
        </div>
        <p>{nodes.length} node{nodes.length === 1 ? "" : "s"} still contain unbuilt or untested work. Nothing in this view changes plan state.</p>
      </header>
      {nodes.map((node, nodeIndex) => {
        const checklist = node.checklist || [];
        const incomplete = checklist
          .map((item, index) => ({ item, index }))
          .filter(({ item }) => !item.built || !item.tested);
        const builtCount = checklist.filter((item) => item.built).length;
        const testedCount = checklist.filter((item) => item.tested).length;
        return (
          <details
            className="diagnostics-gap-node"
            key={node.id}
            open={nodeIndex === 0}
            data-context-kind="node"
            data-context-label={`${node.id} ${node.title}`}
          >
            <summary>
              <span className="diagnostics-gap-id">{node.id}</span>
              <span className="diagnostics-gap-title">{node.title}</span>
              <span className="diagnostics-gap-progress">{builtCount}/{checklist.length} built · {testedCount}/{checklist.length} tested</span>
            </summary>
            <div className="diagnostics-gap-body">
              <div className="diagnostics-gap-meta">
                <span>STATUS · {(node.status || "unknown").toUpperCase()}</span>
                <span>SPINE · {node.spineId}</span>
                {node.dependsOn?.length ? <span>DEPENDS ON · {node.dependsOn.join(", ")}</span> : null}
              </div>
              <section>
                <h3>FULL NOTES</h3>
                <p>{node.notes || "No notes were recorded for this node."}</p>
              </section>
              <section>
                <h3>INCOMPLETE CHECKLIST · {incomplete.length}</h3>
                {incomplete.length ? (
                  <ol>
                    {incomplete.map(({ item, index }) => (
                      <li key={item.id || `${node.id}-${index}`}>
                        <p>{item.text}</p>
                        <div>
                          <span data-state={item.built ? "ready" : "missing"}>{item.built ? "BUILT" : "NOT BUILT"}</span>
                          <span data-state={item.tested ? "ready" : "missing"}>{item.tested ? "TESTED" : "NOT TESTED"}</span>
                        </div>
                        {item.verify ? <code>{item.verify}</code> : null}
                      </li>
                    ))}
                  </ol>
                ) : <p>No checklist entries are incomplete; the node status itself is not done.</p>}
              </section>
            </div>
          </details>
        );
      })}
    </div>
  );
}
