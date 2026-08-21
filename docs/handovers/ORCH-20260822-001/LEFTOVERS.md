# Leftovers — ORCH-20260822-001

- `ORCH-LEFT-001` · **external** — Push the branch, open a PR, run GitHub CI and merge only after
  user authorization. Location: repository remote. Suggested next action: review this handover and
  then run the normal authorized GitHub delivery workflow.
- `ORCH-LEFT-002` · **safety-design** — One-click process stopping remains intentionally unavailable
  in the public Tauri API because the app does not yet own a durable PID/start-time/executable
  registry for launched helpers. Location: preflight process decisions. Suggested next action: add
  an app-owned process registry and require exact identity revalidation plus an explicit user action
  before exposing a stop button; never accept an arbitrary PID or command.
- `ORCH-LEFT-003` · **capacity-policy** — The scheduler supports dependency-safe parallel claims and
  fencing, but it does not yet calculate an adaptive worker admission limit from live CPU, RAM and
  disk telemetry. Location: claim admission. Suggested next action: design a persisted capacity
  policy with freshness bounds and prove it under load before advertising automatic scaling to 100
  simultaneous workers.
- `ORCH-LEFT-004` · **pre-existing-lint** — Full Clippy warnings-denied requires an explicit allowance
  for the existing large `ControlPlaneEventKind` enum. Location: `src-tauri/src/control_plane.rs`.
  Suggested next action: benchmark and safely box the large variant in its own manifest-bounded task.

No hidden code, evidence, reconciliation or packaging defect is being deferred by this list.
