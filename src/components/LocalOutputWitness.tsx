import React, { useEffect, useMemo, useState } from "react";
import { Code2, ExternalLink, Images, MonitorCheck } from "lucide-react";
import { Board, readBoardEvidence } from "../services/boards";
import { EvidenceArtifact, PlanSnapshot } from "../types/plan";

interface LocalOutputWitnessProps {
  board: Board | null;
  plan: PlanSnapshot | null;
  variant?: "rail" | "comparison";
}

export const LocalOutputWitness: React.FC<LocalOutputWitnessProps> = ({ board, plan, variant = "rail" }) => {
  const visual = useMemo(() => {
    if (!plan) return { latest: null, phases: [] as Array<{ id: string; shown: number; total: number; missing: string[] }> };
    const screenshots = plan.vertebrae.flatMap((vertebra) =>
      (vertebra.checklist || []).flatMap((item) => item.proof?.screenshot ? [{ proof: item.proof, vertebra }] : [])
    ).sort((a, b) => String(b.proof.at || "").localeCompare(String(a.proof.at || "")));
    return {
      latest: screenshots[0] || null,
      phases: plan.spine.map((phase) => {
        const nodes = plan.vertebrae.filter((vertebra) => vertebra.spineId === phase.id);
        const shown = nodes.filter((vertebra) => (vertebra.checklist || []).some((item) =>
          Boolean(item.proof?.screenshot) && item.proof?.screenshotCheck?.ok !== false
        ));
        return { id: phase.id, shown: shown.length, total: nodes.length, missing: nodes.filter((node) => !shown.includes(node)).map((node) => node.id) };
      }),
    };
  }, [plan]);
  const [shot, setShot] = useState<EvidenceArtifact | null>(null);
  const [codeEvidence, setCodeEvidence] = useState<EvidenceArtifact | null>(null);
  const [view, setView] = useState<"ui" | "code">("ui");

  useEffect(() => {
    let live = true;
    setShot(null);
    setCodeEvidence(null);
    if (board && visual.latest?.proof.screenshot) {
      void readBoardEvidence(board, visual.latest.proof.screenshot).then((artifact) => live && setShot(artifact));
    }
    if (board && visual.latest?.proof.log) {
      void readBoardEvidence(board, visual.latest.proof.log).then((artifact) => live && setCodeEvidence(artifact));
    }
    return () => { live = false; };
  }, [board, visual.latest?.proof.log, visual.latest?.proof.screenshot]);

  const url = plan?.meta?.appUrl || null;
  const missing = visual.phases.flatMap((phase) => phase.missing);
  const captureCheck = visual.latest?.proof.screenshotCheck;
  const comparisonGrade = captureCheck?.ok === true
    && (captureCheck.width || 0) >= 1280
    && (captureCheck.height || 0) >= 720;
  const captureTone = comparisonGrade ? "verified" : captureCheck?.ok === false ? "failed" : captureCheck?.ok === true ? "below-standard" : "unknown";
  const captureLabel = comparisonGrade
    ? "COMPARISON-GRADE CAPTURE"
    : captureCheck?.ok === false
      ? "CAPTURE CHECK FAILED"
      : captureCheck?.ok === true
        ? "CAPTURE BELOW 1280 × 720"
        : "CAPTURE QUALITY UNKNOWN";
  return (
    <section className={`local-output ${variant}`} id={variant === "rail" ? "pp-region-local-output" : "pp-region-ui-comparison"} aria-label={variant === "rail" ? "Localhost visual output witness" : "Code and UI evidence comparison"}>
      <header><MonitorCheck size={14} /><span>{variant === "rail" ? "LOCAL OUTPUT" : "EVIDENCE PANE"}</span>{url ? <a id={variant === "rail" ? "pp-link-open-local-output" : "pp-link-open-comparison-output"} href={url} target="_blank" rel="noreferrer" aria-label={`Open local output ${url}`}><ExternalLink size={12} /></a> : null}</header>
      <code className="local-url" title={url || "No app URL declared in plan meta.appUrl"}>{url || "app URL not declared"}</code>
      {variant === "comparison" ? (
        <div className="evidence-view-switch" role="group" aria-label="Evidence view">
          <button type="button" className={view === "ui" ? "selected" : ""} aria-pressed={view === "ui"} onClick={() => setView("ui")}><Images size={13} /> UI</button>
          <button type="button" className={view === "code" ? "selected" : ""} aria-pressed={view === "code"} onClick={() => setView("code")}><Code2 size={13} /> Code</button>
        </div>
      ) : null}
      {view === "ui" ? (
        <div className="local-screen">
          {shot?.dataUrl ? <img src={shot.dataUrl} alt={visual.latest?.proof.shotNote || "Latest captured localhost output"} /> : <span>NO CAPTURED LOCAL SCREEN</span>}
        </div>
      ) : (
        <pre className="code-evidence">{codeEvidence?.text || "NO TEXT CODE EVIDENCE IS ATTACHED TO THE LATEST UI CAPTURE"}</pre>
      )}
      {variant === "comparison" ? (
        <div className={`capture-standard ${captureTone}`} role="status">
          <strong>{captureLabel}</strong>
          <span>{captureCheck?.width && captureCheck?.height ? `${captureCheck.width} × ${captureCheck.height}` : "dimensions not recorded"}</span>
        </div>
      ) : null}
      <div className="visibility-map" aria-label="Visual coverage by plan phase">
        {visual.phases.map((phase) => <span key={phase.id} className={phase.shown === phase.total && phase.total > 0 ? "covered" : "partial"}>{phase.id} {phase.shown}/{phase.total}</span>)}
      </div>
      <p className={missing.length ? "local-gap" : "local-clear"}>
        {missing.length ? `Not shown in captured UI: ${missing.join(", ")}` : visual.phases.length ? "Every planned node has visual evidence." : "No phase visibility data yet."}
      </p>
      {visual.latest?.proof.at ? <small>latest capture · {new Date(visual.latest.proof.at).toLocaleString()}</small> : <small>capture time not recorded</small>}
    </section>
  );
};
