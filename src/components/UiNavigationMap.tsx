import React, { useMemo, useState } from "react";
import type { Board } from "../services/boards";
import type { PlanSnapshot, Vertebra } from "../types/plan";

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

function PageCard({ node }: { node: Vertebra }) {
  const files = frontendFiles(node);
  const uiItems = (node.checklist || []).filter((item) => item.ui);
  const uiPage = isUiPage(node);
  return (
    <details
      className={`ui-map-page${uiPage ? " ui-capable" : " support-work"}`}
      id={`pp-ui-page-${domToken(node.id)}`}
      data-page-id={node.id}
      data-spine-id={node.spineId}
      data-page-side={node.side || "R"}
      data-page-kind={uiPage ? "ui-capable" : "support-work"}
    >
      <summary id={`pp-ui-page-summary-${domToken(node.id)}`}>
        <span className="ui-map-page-id">{node.id}</span>
        <span className="ui-map-page-copy">
          <strong>{node.title}</strong>
          <small>{uiPage ? "UI-capable page/surface" : "Support work · no UI mapping recorded"}</small>
        </span>
        <span className="ui-map-page-progress">{progressLabel(node)}</span>
      </summary>
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

  const toggleSide = (spineId: string, side: BranchSide) => {
    const key = `${spineId}:${side}`;
    setOpenSides((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
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
                {leftOpen ? left.map((node) => <PageCard key={node.id} node={node} />) : null}
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
                {rightOpen ? right.map((node) => <PageCard key={node.id} node={node} />) : null}
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
