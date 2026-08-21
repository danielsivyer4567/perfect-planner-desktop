import React, { useEffect, useMemo, useState } from "react";
import { ExternalLink, MonitorCheck } from "lucide-react";
import { Board, readBoardEvidence } from "../services/boards";
import { EvidenceArtifact, PlanSnapshot } from "../types/plan";

interface LocalOutputWitnessProps {
  board: Board | null;
  plan: PlanSnapshot | null;
}

export const LocalOutputWitness: React.FC<LocalOutputWitnessProps> = ({ board, plan }) => {
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

  useEffect(() => {
    let live = true;
    setShot(null);
    if (board && visual.latest?.proof.screenshot) {
      void readBoardEvidence(board, visual.latest.proof.screenshot).then((artifact) => live && setShot(artifact));
    }
    return () => { live = false; };
  }, [board, visual.latest?.proof.screenshot]);

  const url = plan?.meta?.appUrl || null;
  const missing = visual.phases.flatMap((phase) => phase.missing);
  return (
    <section className="local-output" id="pp-region-local-output" aria-label="Localhost visual output witness">
      <header><MonitorCheck size={14} /><span>LOCAL OUTPUT</span>{url ? <a id="pp-link-open-local-output" href={url} target="_blank" rel="noreferrer" aria-label={`Open local output ${url}`}><ExternalLink size={12} /></a> : null}</header>
      <code className="local-url" title={url || "No app URL declared in plan meta.appUrl"}>{url || "app URL not declared"}</code>
      <div className="local-screen">
        {shot?.dataUrl ? <img src={shot.dataUrl} alt={visual.latest?.proof.shotNote || "Latest captured localhost output"} /> : <span>NO CAPTURED LOCAL SCREEN</span>}
      </div>
      <div className="visibility-map" aria-label="Visual coverage by plan phase">
        {visual.phases.map((phase) => <span key={phase.id} className={phase.shown === phase.total && phase.total > 0 ? "covered" : "partial"}>{phase.id} {phase.shown}/{phase.total}</span>)}
      </div>
      <p className={missing.length ? "local-gap" : "local-clear"}>
        {missing.length ? `Not shown in captured UI: ${missing.join(", ")}` : visual.phases.length ? "Every planned node has visual evidence." : "No phase visibility data yet."}
      </p>
      {visual.latest?.proof.at ? <small>latest capture · {new Date(visual.latest.proof.at).toLocaleString()}</small> : null}
    </section>
  );
};
