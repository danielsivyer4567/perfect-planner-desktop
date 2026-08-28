import React, { useEffect, useMemo, useState } from "react";
import { readBoardEvidence, type Board } from "../services/boards";
import type { ChecklistProof, EvidenceArtifact, PlanSnapshot, Vertebra } from "../types/plan";

type BranchSide = "L" | "R";

function domToken(value: string): string {
  return value.replace(/[^a-zA-Z0-9_-]/g, "-");
}

function frontendFiles(node: Vertebra): string[] {
  return (node.files || []).filter((file) => /\.(tsx?|jsx?|vue|svelte|html|css|scss)$/i.test(file));
}

function isUiPage(node: Vertebra): boolean {
  return frontendFiles(node).length > 0 || (node.checklist || []).some((item) => item.ui);
}

function progressLabel(node: Vertebra): string {
  const checklist = node.checklist || [];
  if (!checklist.length) return (node.status || "unknown").toUpperCase();
  const tested = checklist.filter((item) => item.tested).length;
  return `${tested}/${checklist.length} proven`;
}

function latestScreenshotProof(node: Vertebra): ChecklistProof | null {
  return (node.checklist || [])
    .map((item) => item.proof)
    .filter((proof): proof is ChecklistProof => Boolean(proof?.screenshot) && proof?.screenshotCheck?.ok !== false)
    .sort((left, right) => String(right.at || "").localeCompare(String(left.at || "")))[0] || null;
}

function PageCard({
  node,
  selected,
  onSelect,
  screenshot,
  screenshotProof,
}: {
  node: Vertebra;
  selected: boolean;
  onSelect: (node: Vertebra) => void;
  screenshot: EvidenceArtifact | null | undefined;
  screenshotProof: ChecklistProof | null;
}) {
  const files = frontendFiles(node);
  const uiItems = (node.checklist || []).filter((item) => item.ui);
  const uiPage = isUiPage(node);
  const screenshotLoading = Boolean(screenshotProof) && screenshot === undefined;
  return (
    <details
      className={`ui-map-page${uiPage ? " ui-capable" : " support-work"}${selected ? " selected" : ""}`}
      id={`pp-ui-page-${domToken(node.id)}`}
      data-page-id={node.id}
      data-spine-id={node.spineId}
      data-page-side={node.side || "R"}
      data-page-kind={uiPage ? "ui-capable" : "support-work"}
      data-selected={selected}
      data-snapshot-state={screenshot?.dataUrl ? "attached" : screenshotLoading ? "loading" : screenshotProof ? "unavailable" : "missing"}
    >
      <summary
        id={`pp-ui-page-summary-${domToken(node.id)}`}
        aria-current={selected ? "page" : undefined}
        onClick={() => onSelect(node)}
      >
        <span className="ui-map-page-id">{node.id}</span>
        <span className={`ui-map-page-thumb${screenshot?.dataUrl ? " attached" : " missing"}`}>
          {screenshot?.dataUrl ? (
            <img src={screenshot.dataUrl} alt={`Previous screenshot for ${node.id} ${node.title}`} />
          ) : <i aria-hidden="true">{screenshotLoading ? "…" : "—"}</i>}
        </span>
        <span className="ui-map-page-copy">
          <strong>{node.title}</strong>
          <small>{uiPage ? "UI-capable page/surface" : "Support work · no UI mapping recorded"}</small>
        </span>
        <span className="ui-map-page-progress">{progressLabel(node)}</span>
      </summary>
      <div className={`ui-map-page-snapshot${screenshot?.dataUrl ? " attached" : " missing"}`}>
        {screenshot?.dataUrl ? (
          <img
            src={screenshot.dataUrl}
            alt={screenshotProof?.shotNote || `Previous UI proof for ${node.id} ${node.title}`}
          />
        ) : (
          <span>{screenshotLoading ? "LOADING PREVIOUS CAPTURE" : screenshotProof ? "CAPTURE REFERENCED · FILE UNAVAILABLE" : "NO SNAPSHOT ATTACHED TO THIS NODE"}</span>
        )}
      </div>
      <div className="ui-map-page-body">
        <dl>
          <div><dt>Spine ID</dt><dd>{node.spineId}</dd></div>
          <div><dt>Page / node ID</dt><dd>{node.id}</dd></div>
          <div><dt>Recorded side</dt><dd>{node.side || "R (default)"}</dd></div>
          <div><dt>Status</dt><dd>{node.status || "unknown"}</dd></div>
        </dl>
        <section>
          <h3>UI files</h3>
          {files.length ? files.map((file) => <code key={file}>{file}</code>) : <p>Unknown — no frontend file is recorded for this node.</p>}
        </section>
        <section>
          <h3>UI outcomes</h3>
          {uiItems.length ? (
            <ul>{uiItems.map((item, index) => <li key={item.id || `${node.id}-${index}`}>{item.text}</li>)}</ul>
          ) : <p>Unknown — no checklist item is marked as a UI outcome.</p>}
        </section>
      </div>
    </details>
  );
}

export function UiNavigationMap({
  board,
  plan,
}: {
  board: Board;
  plan: PlanSnapshot | null | undefined;
}) {
  const [openSides, setOpenSides] = useState<Set<string>>(() => new Set());
  const [selectedPageId, setSelectedPageId] = useState<string | null>(null);
  const [screenshots, setScreenshots] = useState<Map<string, EvidenceArtifact | null>>(() => new Map());
  const phases = useMemo(() => {
    if (!plan) return [];
    return plan.spine.map((phase) => {
      const nodes = plan.vertebrae.filter((node) => node.spineId === phase.id);
      return {
        phase,
        left: nodes.filter((node) => node.side === "L"),
        right: nodes.filter((node) => node.side !== "L"),
      };
    });
  }, [plan]);
  const allNodes = plan?.vertebrae || [];
  const uiCount = allNodes.filter(isUiPage).length;
  const supportCount = allNodes.length - uiCount;
  const assignedIds = new Set((plan?.spine || []).map((phase) => phase.id));
  const orphaned = allNodes.filter((node) => !assignedIds.has(node.spineId));
  const selectedNode = allNodes.find((node) => node.id === selectedPageId) || null;

  useEffect(() => {
    if (!plan) return;
    setOpenSides(new Set(plan.spine.flatMap((phase) => [`${phase.id}:L`, `${phase.id}:R`])));
    setSelectedPageId(null);
  }, [plan]);

  useEffect(() => {
    let live = true;
    setScreenshots(new Map());
    if (!plan) return () => { live = false; };
    const captures = plan.vertebrae.flatMap((node) => {
      const proof = latestScreenshotProof(node);
      return proof?.screenshot ? [{ nodeId: node.id, fileName: proof.screenshot }] : [];
    });
    void Promise.all(captures.map(async ({ nodeId, fileName }) => ({
      nodeId,
      artifact: await readBoardEvidence(board, fileName),
    }))).then((results) => {
      if (live) setScreenshots(new Map(results.map(({ nodeId, artifact }) => [nodeId, artifact])));
    });
    return () => { live = false; };
  }, [board.planPath, board.port, plan]);

  const toggleSide = (spineId: string, side: BranchSide) => {
    const key = `${spineId}:${side}`;
    setOpenSides((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const focusPage = (node: Vertebra) => {
    setSelectedPageId(node.id);
    window.requestAnimationFrame(() => {
      document.getElementById(`pp-ui-page-${domToken(node.id)}`)?.scrollIntoView({
        block: "center",
        inline: "center",
        behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth",
      });
    });
  };

  if (!plan) {
    return (
      <section className="ui-navigation-map ui-navigation-map-empty" id="pp-region-ui-navigation-map">
        <strong>UI MAP · NOT AVAILABLE</strong>
        <p>The plan snapshot for {board.repoName} / {board.number || "unnumbered plan"} is not loaded. Perfect Planner will not invent page IDs.</p>
      </section>
    );
  }

  return (
    <section
      className="ui-navigation-map"
      id="pp-region-ui-navigation-map"
      aria-labelledby="pp-heading-ui-navigation-map"
      data-plan-id={plan.meta?.number || board.number || "unassigned"}
      data-repository-root={board.repoRoot}
      data-focus-page={selectedNode?.id || "spine"}
      data-focus-side={selectedNode?.side === "L" ? "L" : selectedNode ? "R" : "spine"}
    >
      <header className="ui-navigation-map-head">
        <div>
          <span>SEPARATE MODE · PLAN-DERIVED PAGE INVENTORY</span>
          <h2 id="pp-heading-ui-navigation-map">UI navigation spine</h2>
          <p>{board.repoName} · {plan.meta?.number || board.number || "unnumbered plan"} · {board.branch}</p>
        </div>
        <div className="ui-map-counts" aria-label="UI mapping totals">
          <span><b>{plan.spine.length}</b> spine segments</span>
          <span><b>{uiCount}</b> UI-capable</span>
          <span><b>{supportCount}</b> support</span>
          <span data-state={orphaned.length ? "warning" : "ready"}><b>{orphaned.length}</b> unmapped</span>
        </div>
      </header>
      <aside className="ui-map-disclosure" role="note">
        This view mirrors the loaded Perfect Plan. A node is called UI-capable only when its plan records a frontend file or a UI checklist item; it is not a runtime route crawl.
      </aside>

      <section className="ui-map-proof-routing" aria-labelledby="pp-heading-browser-proof-routing">
        <div>
          <span>BROWSER PROOF ROUTING</span>
          <h3 id="pp-heading-browser-proof-routing">One visual result, explicit evidence source</h3>
        </div>
        <dl>
          <div data-proof-method="chrome-mcp">
            <dt>Chrome MCP</dt><dd><b>PREFERRED</b> · host availability checked at runtime</dd>
          </div>
          <div data-proof-method="playwright-script">
            <dt>Script fallback</dt><dd><b>READY</b> · full PNG + JSON logs · Chrome/Chromium · cross-platform</dd>
          </div>
          <div data-proof-method="last-run">
            <dt>Last attached run</dt><dd><b>UNKNOWN</b> · no report is inferred from the plan</dd>
          </div>
        </dl>
      </section>

      <nav className="ui-map-path" aria-label="Selected construction path">
        <span>CONSTRUCTION PATH</span>
        <b>{selectedNode ? `${selectedNode.spineId} → ${selectedNode.id}` : "FULL SPINE"}</b>
        <small>{selectedNode ? `${selectedNode.side === "L" ? "Left" : "Right"} branch · ${selectedNode.title}` : "All recorded left and right branches are visible."}</small>
      </nav>

      <div className="ui-map-canvas">
        <div className="ui-map-axis" aria-hidden="true" />
        {phases.map(({ phase, left, right }, phaseIndex) => {
          const leftKey = `${phase.id}:L`;
          const rightKey = `${phase.id}:R`;
          const leftOpen = openSides.has(leftKey);
          const rightOpen = openSides.has(rightKey);
          const leftRegionId = `pp-ui-branch-${domToken(phase.id)}-left`;
          const rightRegionId = `pp-ui-branch-${domToken(phase.id)}-right`;
          return (
            <section
              className="ui-map-spine-row"
              key={phase.id}
              data-spine-id={phase.id}
              style={{ "--phase-index": phaseIndex } as React.CSSProperties}
            >
              <div className={`ui-map-branch left${leftOpen ? " open" : ""}`} id={leftRegionId}>
                {leftOpen ? left.map((node) => <PageCard key={node.id} node={node} selected={selectedPageId === node.id} onSelect={focusPage} screenshot={screenshots.get(node.id)} screenshotProof={latestScreenshotProof(node)} />) : null}
                {leftOpen && !left.length ? <p className="ui-map-branch-empty">No left-side page IDs are recorded.</p> : null}
              </div>

              <article className="ui-map-spine-segment">
                <button
                  id={`pp-btn-ui-map-${domToken(phase.id)}-left`}
                  type="button"
                  className="ui-map-branch-toggle left"
                  aria-expanded={leftOpen}
                  aria-controls={leftRegionId}
                  onClick={() => toggleSide(phase.id, "L")}
                  title={`Show ${left.length} left-side page${left.length === 1 ? "" : "s"} for ${phase.id}`}
                >
                  <span aria-hidden="true">‹</span><b>{left.length}</b><span className="sr-only"> left-side pages</span>
                </button>
                <span className="ui-map-spine-id">{phase.id}</span>
                <h3>{phase.title}</h3>
                <p>{phase.summary || "No phase summary is recorded."}</p>
                <button
                  id={`pp-btn-ui-map-${domToken(phase.id)}-right`}
                  type="button"
                  className="ui-map-branch-toggle right"
                  aria-expanded={rightOpen}
                  aria-controls={rightRegionId}
                  onClick={() => toggleSide(phase.id, "R")}
                  title={`Show ${right.length} right-side page${right.length === 1 ? "" : "s"} for ${phase.id}`}
                >
                  <b>{right.length}</b><span aria-hidden="true">›</span><span className="sr-only"> right-side pages</span>
                </button>
              </article>

              <div className={`ui-map-branch right${rightOpen ? " open" : ""}`} id={rightRegionId}>
                {rightOpen ? right.map((node) => <PageCard key={node.id} node={node} selected={selectedPageId === node.id} onSelect={focusPage} screenshot={screenshots.get(node.id)} screenshotProof={latestScreenshotProof(node)} />) : null}
                {rightOpen && !right.length ? <p className="ui-map-branch-empty">No right-side page IDs are recorded.</p> : null}
              </div>
            </section>
          );
        })}
      </div>

      {orphaned.length ? (
        <aside className="ui-map-orphans" role="status">
          <strong>UNMAPPED NODE IDs</strong>
          <p>{orphaned.map((node) => `${node.id} → missing spine ${node.spineId}`).join(" · ")}</p>
        </aside>
      ) : null}
    </section>
  );
}
