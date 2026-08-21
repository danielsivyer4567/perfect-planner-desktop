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

- `ORCH-LEFT-005` · **resolved** — Independent repetition exposed intermittent Windows error 5
  (`ERROR_ACCESS_DENIED`) while concurrent writers created the `events.jsonl.append.lock` sidecar.
  Operation-level diagnostics confirmed the denial occurred before opening or writing `events.jsonl`.
  Commit `2f61566` now treats only `AlreadyExists`, Windows `ERROR_ACCESS_DENIED` and
  `ERROR_SHARING_VIOLATION` as bounded lock contention, applies capped backoff during acquisition
  and release, and preserves the original 160-event completeness and uniqueness assertions.
  Verification: the unchanged test passed 25/25 focused repetitions, then the complete Rust suite
  passed 73/73; formatting and warnings-denied Clippy also passed with only the already-recorded
  `large_enum_variant` allowance. No Tauri API command or permission surface changed.
  Packaging refresh: `npm run tauri build` completed after the fix, and the replacement MSI/NSIS
  byte sizes and SHA-256 values are recorded in `COMPLETION-REPORT.md`.
