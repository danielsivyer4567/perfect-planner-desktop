import React from "react";
import type { ResourceProbeResult } from "../services/resourceGuard";

export type ResourceGuardState =
  | { status: "checking"; result: null; error: null }
  | { status: "active"; result: ResourceProbeResult; error: null }
  | { status: "unavailable"; result: null; error: string };

const formatBytes = (bytes: number): string => {
  if (!Number.isFinite(bytes) || bytes < 0) return "unknown";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit < 2 ? 0 : 1)} ${units[unit]}`;
};

export function ResourceGuard({ state, onRefresh }: {
  state: ResourceGuardState;
  onRefresh: () => void;
}) {
  const active = state.status === "active";
  const label = state.status === "checking"
    ? "RESOURCE GUARD · CHECKING"
    : active
      ? "RESOURCE GUARD · WINDOWS NATIVE"
      : "RESOURCE GUARD · UNAVAILABLE";
  const remedy = state.status === "unavailable"
    ? state.error.includes("Git worktree") || state.error.includes("repository root")
        ? "Select a discovered plan whose repository path still exists, then rescan."
        : "Open the bottom-right console for the full native error, then check again after correcting that exact cause."
    : null;

  return (
    <details
      className={`resource-guard resource-guard-${state.status}`}
      id="pp-resource-guard"
      data-resource-provider={state.result?.provider || "unavailable"}
      data-context-kind="expandable"
      data-context-label="Windows resource guard"
    >
      <summary id="pp-btn-resource-guard" aria-label={label}>
        <span className="resource-guard-light" aria-hidden="true" />
        <span>{label}</span>
      </summary>
      <div className="resource-guard-panel" role="status">
        {active ? (
          <>
            <header>
              <strong>Windows system probe is on</strong>
              <span>{state.result.provider}</span>
            </header>
            <dl>
              <div><dt>CPU</dt><dd>{state.result.resources.cpuUsagePercent.toFixed(1)}% · {state.result.resources.logicalCpuCount} logical</dd></div>
              <div><dt>Available RAM</dt><dd>{formatBytes(state.result.resources.availableMemoryBytes)}</dd></div>
              <div><dt>Repository disk</dt><dd>{formatBytes(state.result.resources.repositoryDiskAvailableBytes)}</dd></div>
              <div><dt>Native executable</dt><dd title={state.result.executable}>{state.result.executable}</dd></div>
            </dl>
          </>
        ) : state.status === "checking" ? (
          <p>Checking bounded Windows-native telemetry…</p>
        ) : (
          <dl className="resource-guard-failure">
            <div><dt>Problem</dt><dd>{state.error}</dd></div>
            <div><dt>Where</dt><dd>Windows-native resource probe for the selected repository</dd></div>
            <div><dt>Remedy</dt><dd>{remedy}</dd></div>
          </dl>
        )}
        <button id="pp-btn-refresh-resource-guard" type="button" className="resource-guard-refresh" onClick={onRefresh}>
          Check now
        </button>
      </div>
    </details>
  );
}
