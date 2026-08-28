import React, { useMemo } from "react";
import type { Board } from "../services/boards";
import type { ControlPlaneSnapshot } from "../services/controlPlane";
import type { OrchestratorSnapshot } from "../services/orchestratorPipeline";
import { deriveOperationalTruth, type EvidenceCell } from "../services/operationalTruth";

function timeLabel(value: number | null): string {
  if (!value) return "not checked this session";
  return new Date(value).toLocaleString();
}

function EvidenceState({ value }: { value: EvidenceCell }) {
  return (
    <span className="truth-evidence-state" data-state={value.tone} title={value.detail}>
      {value.label}
    </span>
  );
}

export function OperationalTruth({
  board,
  pipeline,
  controlPlane,
  parallelAgents,
}: {
  board: Board | null;
  pipeline: OrchestratorSnapshot | null;
  controlPlane: ControlPlaneSnapshot | null;
  parallelAgents: boolean;
}) {
  const truth = useMemo(
    () => deriveOperationalTruth({ board, pipeline, controlPlane, parallelAgents }),
    [board, controlPlane, parallelAgents, pipeline],
  );

  return (
    <section className="operational-truth" id="pp-panel-operational-truth" aria-labelledby="pp-heading-operational-truth">
      <header className="operational-truth-head">
        <div>
          <span>OPERATIONAL TRUTH · {truth.scopeLabel}</span>
          <h2 id="pp-heading-operational-truth">What is proven, blocked, and still unknown</h2>
        </div>
        <strong
          id="pp-inspector-release-verdict"
          className="truth-release-verdict"
          data-state={truth.release.tone}
        >
          RELEASE · {truth.release.label}
        </strong>
      </header>

      <div className="truth-summary-grid">
        <article className="truth-summary-card" data-state={truth.collision.tone}>
          <span>Collision admission</span>
          <strong id="pp-status-collision-truth">{truth.collision.label}</strong>
          <p>{truth.collision.impact}</p>
          <dl>
            <div><dt>Source</dt><dd>{truth.collision.source}</dd></div>
            <div><dt>Checked</dt><dd>{timeLabel(truth.collision.checkedAtMs)}</dd></div>
            <div><dt>Safe next action</dt><dd>{truth.collision.nextAction}</dd></div>
          </dl>
        </article>
        <article className="truth-summary-card" data-state={truth.release.tone}>
          <span>Release decision</span>
          <strong>RELEASE · {truth.release.label}</strong>
          {truth.release.blockers.length ? (
            <ul id="pp-list-release-blockers">
              {truth.release.blockers.map((blocker, index) => <li key={`${index}-${blocker}`}>{blocker}</li>)}
            </ul>
          ) : <p>No blocking issue is recorded by the loaded native release receipt.</p>}
          <dl>
            <div><dt>Source</dt><dd>{truth.release.source}</dd></div>
            <div><dt>Checked</dt><dd>{timeLabel(truth.release.checkedAtMs)}</dd></div>
          </dl>
        </article>
        <article className="truth-summary-card" data-state={truth.currentRunParallelLimit === null ? "unknown" : "active"}>
          <span>Parallel authority</span>
          <strong id="pp-status-parallel-truth">
            CURRENT RUN · {truth.currentRunParallelLimit === null ? "UNKNOWN" : `×${truth.currentRunParallelLimit}`}
          </strong>
          <p>New runs begin at ×{truth.futureRunParallelLimit}. Changing the header toggle does not rewrite an active run.</p>
          <dl>
            <div><dt>Current source</dt><dd>{truth.currentRunParallelLimit === null ? "No selected native scheduler" : "Native scheduler receipt"}</dd></div>
            <div><dt>Future source</dt><dd>Local new-run preference</dd></div>
          </dl>
        </article>
        <article className="truth-summary-card" data-state={truth.checkedAtMs ? "active" : "unknown"}>
          <span>Freshness and provenance</span>
          <strong>{truth.checkedAtMs ? "OBSERVED STATE" : "UNKNOWN · NOT LOADED"}</strong>
          <p>{truth.provenance}</p>
          <dl>
            <div><dt>Last checked</dt><dd>{timeLabel(truth.checkedAtMs)}</dd></div>
            <div><dt>Scope</dt><dd>{truth.scopeLabel}</dd></div>
          </dl>
        </article>
      </div>

      <details className="truth-detail" open>
        <summary id="pp-summary-collision-scope">Collision scope and affected work · {truth.collision.affected.length}</summary>
        <div className="truth-detail-body">
          {truth.collision.affected.map((item) => <code key={item}>{item}</code>)}
        </div>
      </details>

      <details className="truth-detail" open>
        <summary id="pp-summary-task-evidence">Task verification evidence · {truth.evidence.length || "none loaded"}</summary>
        {truth.evidence.length ? (
          <div className="truth-evidence-wrap">
            <table className="truth-evidence-table">
              <thead><tr><th>Task</th><th>State</th><th>Tests</th><th>Build / typecheck</th><th>Runtime / browser</th><th>Artifacts</th><th>Unresolved risk</th></tr></thead>
              <tbody>
                {truth.evidence.map((row) => (
                  <tr key={row.nodeId}>
                    <th scope="row"><b>{row.nodeId}</b><span>{row.title}</span></th>
                    <td>{row.status}</td>
                    <td><EvidenceState value={row.tests} /></td>
                    <td><EvidenceState value={row.build} /></td>
                    <td><EvidenceState value={row.runtime} /></td>
                    <td><EvidenceState value={row.artifacts} /></td>
                    <td>{row.risks.length ? row.risks.join(" · ") : "None recorded"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <p className="truth-empty">No exact native run is selected, so task verification evidence is unknown.</p>
        )}
      </details>

      <details className="truth-detail">
        <summary id="pp-summary-operational-activity">Chronological activity · {truth.activity.length || "none loaded"}</summary>
        {truth.activity.length ? (
          <ol className="truth-activity-feed" id="pp-list-operational-activity">
            {truth.activity.map((entry) => (
              <li key={entry.id}>
                <time dateTime={new Date(entry.atMs).toISOString()}>{timeLabel(entry.atMs)}</time>
                <strong>{entry.actor} · {entry.action}</strong>
                <p>{entry.result}</p>
                <small>{entry.scope} · source: {entry.source}</small>
              </li>
            ))}
          </ol>
        ) : (
          <p className="truth-empty">No repository-and-plan-scoped run event or message has been loaded.</p>
        )}
      </details>
    </section>
  );
}
