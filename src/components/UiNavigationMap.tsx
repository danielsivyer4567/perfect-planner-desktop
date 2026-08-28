import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { readBoardEvidence, type Board } from "../services/boards";
import type { ChecklistProof, EvidenceArtifact, PlanSnapshot, SpinePhase, Vertebra } from "../types/plan";

type BranchSide = "L" | "R";

const MIN_ZOOM = 0.18;
const MAX_ZOOM = 1.4;
const ZOOM_STEP = 0.1;

function clampZoom(value: number): number {
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, Math.round(value * 100) / 100));
}

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

function Artboard({
  node,
  phase,
  selected,
  screenshot,
  screenshotProof,
  onSelect,
}: {
  node: Vertebra;
  phase: SpinePhase | null;
  selected: boolean;
  screenshot: EvidenceArtifact | null | undefined;
  screenshotProof: ChecklistProof | null;
  onSelect: (node: Vertebra) => void;
}) {
  const screenshotLoading = Boolean(screenshotProof) && screenshot === undefined;
  const snapshotState = screenshot?.dataUrl
    ? "attached"
    : screenshotLoading
      ? "loading"
      : screenshotProof
        ? "unavailable"
        : "missing";
  const width = screenshotProof?.screenshotCheck?.width || 1440;
  const height = screenshotProof?.screenshotCheck?.height || 1000;
  const pageKind = isUiPage(node) ? "ui-capable" : "support-work";

  return (
    <article
      className={`ui-map-artboard ${pageKind}${selected ? " selected" : ""}`}
      id={`pp-ui-page-${domToken(node.id)}`}
      data-page-id={node.id}
      data-spine-id={node.spineId}
      data-page-side={node.side || "R"}
      data-page-kind={pageKind}
      data-selected={selected}
      data-snapshot-state={snapshotState}
      style={{ "--artboard-ratio": `${width} / ${height}` } as React.CSSProperties}
    >
      <button
        type="button"
        className="ui-map-artboard-select"
        aria-current={selected ? "page" : undefined}
        aria-label={`Focus ${node.id}, ${node.title}`}
        onClick={() => onSelect(node)}
      >
        <span className="ui-map-artboard-frame">
          <span className="ui-map-artboard-index">{node.id}</span>
          {screenshot?.dataUrl ? (
            <img
              src={screenshot.dataUrl}
              alt={screenshotProof?.shotNote || `Previous UI proof for ${node.id} ${node.title}`}
            />
          ) : (
            <span className="ui-map-artboard-empty">
              <i aria-hidden="true">{screenshotLoading ? "···" : "□"}</i>
              <b>{screenshotLoading ? "Loading capture" : screenshotProof ? "Capture unavailable" : "No screenshot yet"}</b>
              <small>{screenshotProof?.screenshot || "This node has no visual proof attached."}</small>
            </span>
          )}
        </span>
        <span className="ui-map-artboard-caption">
          <span>
            <b>{node.title}</b>
            <small>{phase?.title || `Missing spine ${node.spineId}`}</small>
          </span>
          <span className="ui-map-artboard-meta">
            <i>{node.side === "L" ? "←" : "→"}</i>
            <em>{progressLabel(node)}</em>
          </span>
        </span>
      </button>
    </article>
  );
}

export function UiNavigationMap({
  board,
  plan,
}: {
  board: Board;
  plan: PlanSnapshot | null | undefined;
}) {
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const worldRef = useRef<HTMLDivElement | null>(null);
  const panRef = useRef({ active: false, pointerId: 0, x: 0, y: 0, left: 0, top: 0 });
  const [zoom, setZoom] = useState(0.5);
  const [panning, setPanning] = useState(false);
  const [layoutReady, setLayoutReady] = useState(false);
  const [openSides, setOpenSides] = useState<Set<string>>(() => new Set());
  const [selectedPageId, setSelectedPageId] = useState<string | null>(null);
  const [screenshots, setScreenshots] = useState<Map<string, EvidenceArtifact | null>>(() => new Map());

  const phases = useMemo(() => {
    if (!plan) return [];
    return plan.spine.map((phase) => ({
      phase,
      left: plan.vertebrae.filter((node) => node.spineId === phase.id && node.side === "L"),
      right: plan.vertebrae.filter((node) => node.spineId === phase.id && node.side !== "L"),
    }));
  }, [plan]);
  const allNodes = plan?.vertebrae || [];
  const assignedIds = new Set((plan?.spine || []).map((phase) => phase.id));
  const orphaned = allNodes.filter((node) => !assignedIds.has(node.spineId));
  const uiCount = allNodes.filter(isUiPage).length;
  const selectedNode = allNodes.find((node) => node.id === selectedPageId) || null;
  const maxBranchSlots = Math.max(
    1,
    ...phases.flatMap(({ left, right }) => [left.length, right.length]),
    orphaned.filter((node) => node.side === "L").length,
    orphaned.filter((node) => node.side !== "L").length,
  );
  const branchWidth = maxBranchSlots * 520 + (maxBranchSlots - 1) * 32;

  const changeZoom = useCallback((requested: number) => {
    const next = clampZoom(requested);
    const viewport = viewportRef.current;
    if (!viewport) {
      setZoom(next);
      return;
    }
    const contentX = (viewport.scrollLeft + viewport.clientWidth / 2) / zoom;
    const contentY = (viewport.scrollTop + viewport.clientHeight / 2) / zoom;
    setZoom(next);
    window.requestAnimationFrame(() => {
      viewport.scrollLeft = Math.max(0, contentX * next - viewport.clientWidth / 2);
      viewport.scrollTop = Math.max(0, contentY * next - viewport.clientHeight / 2);
    });
  }, [zoom]);

  const fitToView = useCallback(() => {
    const viewport = viewportRef.current;
    const world = worldRef.current;
    if (!viewport || !world) return;
    const baseWidth = world.scrollWidth;
    const baseHeight = world.scrollHeight;
    if (!baseWidth || !baseHeight) return;
    const next = clampZoom(Math.min(
      (viewport.clientWidth - 72) / baseWidth,
      (viewport.clientHeight - 72) / baseHeight,
      1,
    ));
    setZoom(next);
    window.requestAnimationFrame(() => viewport.scrollTo({ left: 0, top: 0, behavior: "auto" }));
  }, []);

  useEffect(() => {
    setSelectedPageId(null);
    setZoom(0.5);
    setLayoutReady(false);
    if (plan) setOpenSides(new Set(plan.spine.flatMap((phase) => [`${phase.id}:L`, `${phase.id}:R`])));
  }, [plan]);

  useEffect(() => {
    if (!plan) return;
    const allBranchesExpanded = plan.spine.every(
      (phase) => openSides.has(`${phase.id}:L`) && openSides.has(`${phase.id}:R`),
    );
    if (!allBranchesExpanded || layoutReady) return;
    const fitFrame = window.requestAnimationFrame(() => {
      fitToView();
      window.requestAnimationFrame(() => setLayoutReady(true));
    });
    return () => window.cancelAnimationFrame(fitFrame);
  }, [fitToView, layoutReady, openSides, plan]);

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

  const toggleSide = (spineId: string, side: BranchSide) => {
    const key = `${spineId}:${side}`;
    setOpenSides((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const beginPan = (event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || (event.target as HTMLElement).closest("button, input, .ui-map-artboard")) return;
    const viewport = viewportRef.current;
    if (!viewport) return;
    panRef.current = {
      active: true,
      pointerId: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      left: viewport.scrollLeft,
      top: viewport.scrollTop,
    };
    viewport.setPointerCapture(event.pointerId);
    setPanning(true);
  };

  const movePan = (event: React.PointerEvent<HTMLDivElement>) => {
    const pan = panRef.current;
    const viewport = viewportRef.current;
    if (!pan.active || pan.pointerId !== event.pointerId || !viewport) return;
    viewport.scrollLeft = pan.left - (event.clientX - pan.x);
    viewport.scrollTop = pan.top - (event.clientY - pan.y);
  };

  const endPan = (event: React.PointerEvent<HTMLDivElement>) => {
    if (panRef.current.pointerId !== event.pointerId) return;
    panRef.current.active = false;
    setPanning(false);
  };

  if (!plan) {
    return (
      <section className="ui-navigation-map ui-navigation-map-empty" id="pp-region-ui-navigation-map">
        <strong>SNAPSHOT CANVAS · NOT AVAILABLE</strong>
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
      data-focus-page={selectedNode?.id || "canvas"}
      data-focus-side={selectedNode?.side === "L" ? "L" : selectedNode ? "R" : "canvas"}
      data-zoom={Math.round(zoom * 100)}
      data-layout-ready={layoutReady ? "true" : "false"}
    >
      <header className="ui-map-toolbar">
        <div className="ui-map-toolbar-title">
          <span>SNAPSHOT CANVAS</span>
          <h2 id="pp-heading-ui-navigation-map">{board.repoName}</h2>
          <p>{plan.meta?.number || board.number || "unnumbered plan"} · {selectedNode ? `${selectedNode.spineId} / ${selectedNode.id} / ${selectedNode.title}` : `${plan.spine.length} phases · ${allNodes.length} pages`}</p>
        </div>
        <div className="ui-map-proof-state" aria-label="Browser proof routing">
          <span data-proof-method="chrome-mcp"><b>Chrome MCP</b> preferred</span>
          <span data-proof-method="playwright-script"><b>Script proof</b> ready</span>
          <span data-proof-method="last-run"><b>Attached run</b> unknown</span>
        </div>
        <div className="ui-map-zoom-controls" role="group" aria-label="Snapshot canvas zoom">
          <button id="pp-btn-ui-map-zoom-out" type="button" aria-label="Zoom out" disabled={!layoutReady} onClick={() => changeZoom(zoom - ZOOM_STEP)}>−</button>
          <label>
            <span className="sr-only">Canvas zoom percentage</span>
            <input
              id="pp-input-ui-map-zoom"
              type="range"
              min={MIN_ZOOM * 100}
              max={MAX_ZOOM * 100}
              step="1"
              value={Math.round(zoom * 100)}
              disabled={!layoutReady}
              onChange={(event) => changeZoom(Number(event.target.value) / 100)}
            />
          </label>
          <output htmlFor="pp-input-ui-map-zoom">{Math.round(zoom * 100)}%</output>
          <button id="pp-btn-ui-map-zoom-in" type="button" aria-label="Zoom in" disabled={!layoutReady} onClick={() => changeZoom(zoom + ZOOM_STEP)}>+</button>
          <button id="pp-btn-ui-map-fit" type="button" disabled={!layoutReady} onClick={fitToView}>Fit</button>
          <button id="pp-btn-ui-map-actual" type="button" disabled={!layoutReady} onClick={() => changeZoom(1)}>100%</button>
        </div>
      </header>

      <div
        ref={viewportRef}
        className={`ui-map-viewport${panning ? " panning" : ""}`}
        tabIndex={0}
        aria-label="Scrollable snapshot canvas. Hold Control and use the mouse wheel to zoom. Drag empty canvas space to pan."
        onPointerDown={beginPan}
        onPointerMove={movePan}
        onPointerUp={endPan}
        onPointerCancel={endPan}
        onWheel={(event) => {
          if (!event.ctrlKey && !event.metaKey) return;
          event.preventDefault();
          changeZoom(zoom + (event.deltaY < 0 ? ZOOM_STEP : -ZOOM_STEP));
        }}
      >
        <div
          className="ui-map-world"
          ref={worldRef}
          style={{ zoom, "--branch-width": `${branchWidth}px` } as React.CSSProperties}
        >
          <div className="ui-map-spine-axis" aria-hidden="true" />
          {phases.map(({ phase, left, right }, phaseIndex) => {
            const leftOpen = openSides.has(`${phase.id}:L`);
            const rightOpen = openSides.has(`${phase.id}:R`);
            const leftRegionId = `pp-ui-branch-${domToken(phase.id)}-left`;
            const rightRegionId = `pp-ui-branch-${domToken(phase.id)}-right`;
            return (
              <section className="ui-map-spine-row" key={phase.id} data-spine-id={phase.id}>
                <div className={`ui-map-artboards ui-map-artboards-left${leftOpen ? " open" : ""}`} id={leftRegionId}>
                  {leftOpen ? left.map((node) => (
                    <Artboard key={node.id} node={node} phase={phase} selected={selectedPageId === node.id} screenshot={screenshots.get(node.id)} screenshotProof={latestScreenshotProof(node)} onSelect={focusPage} />
                  )) : null}
                  {leftOpen && !left.length ? <span className="ui-map-empty-branch">No left pages</span> : null}
                </div>

                <article className="ui-map-spine-segment">
                  <button
                    id={`pp-btn-ui-map-${domToken(phase.id)}-left`}
                    type="button"
                    className="ui-map-branch-toggle left"
                    aria-expanded={leftOpen}
                    aria-controls={leftRegionId}
                    onClick={() => toggleSide(phase.id, "L")}
                    aria-label={`${leftOpen ? "Collapse" : "Expand"} ${left.length} left pages for ${phase.id}`}
                  >
                    <span aria-hidden="true">←</span><b>{left.length}</b>
                  </button>
                  <span>{String(phaseIndex + 1).padStart(2, "0")} · {phase.id}</span>
                  <h3>{phase.title}</h3>
                  <p>{phase.summary || "No phase summary is recorded."}</p>
                  <button
                    id={`pp-btn-ui-map-${domToken(phase.id)}-right`}
                    type="button"
                    className="ui-map-branch-toggle right"
                    aria-expanded={rightOpen}
                    aria-controls={rightRegionId}
                    onClick={() => toggleSide(phase.id, "R")}
                    aria-label={`${rightOpen ? "Collapse" : "Expand"} ${right.length} right pages for ${phase.id}`}
                  >
                    <b>{right.length}</b><span aria-hidden="true">→</span>
                  </button>
                </article>

                <div className={`ui-map-artboards ui-map-artboards-right${rightOpen ? " open" : ""}`} id={rightRegionId}>
                  {rightOpen ? right.map((node) => (
                    <Artboard key={node.id} node={node} phase={phase} selected={selectedPageId === node.id} screenshot={screenshots.get(node.id)} screenshotProof={latestScreenshotProof(node)} onSelect={focusPage} />
                  )) : null}
                  {rightOpen && !right.length ? <span className="ui-map-empty-branch">No right pages</span> : null}
                </div>
              </section>
            );
          })}

          {orphaned.length ? (
            <section className="ui-map-spine-row ui-map-orphan-row" data-spine-id="unmapped">
              <div className="ui-map-artboards ui-map-artboards-left open">
                {orphaned.filter((node) => node.side === "L").map((node) => (
                  <Artboard key={node.id} node={node} phase={null} selected={selectedPageId === node.id} screenshot={screenshots.get(node.id)} screenshotProof={latestScreenshotProof(node)} onSelect={focusPage} />
                ))}
              </div>
              <article className="ui-map-spine-segment ui-map-orphan-segment"><span>UNMAPPED</span><h3>Outside the spine</h3><p>These nodes reference a phase missing from the loaded plan.</p></article>
              <div className="ui-map-artboards ui-map-artboards-right open">
                {orphaned.filter((node) => node.side !== "L").map((node) => (
                  <Artboard key={node.id} node={node} phase={null} selected={selectedPageId === node.id} screenshot={screenshots.get(node.id)} screenshotProof={latestScreenshotProof(node)} onSelect={focusPage} />
                ))}
              </div>
            </section>
          ) : null}
        </div>
      </div>

      <footer className="ui-map-statusbar">
        <span><b>{uiCount}</b> UI pages</span>
        <span><b>{allNodes.length - uiCount}</b> support nodes</span>
        <span data-state={orphaned.length ? "warning" : "ready"}><b>{orphaned.length}</b> unmapped</span>
        <span>Drag to pan · Ctrl/⌘ + wheel to zoom</span>
      </footer>
    </section>
  );
}
