# Ledger: Perfect Planner Tauri — fail-closed orchestration pipeline
Approved: yes @ 2026-08-22 (chat)
Graph: existing graph queried; direct source verification required before each edit
Run ID: ORCH-20260822-001
Repository: C:\repos\looplet-worktrees\perfect-planner-desktop-orchestrator-messaging-20260821-223935
Branch: feature/tauri-orchestrator-messaging-20260821-223935

## Execution boundary

- Work only in the repository and branch named above.
- Do not edit the AutoPro or Perfect Planning skill repositories.
- Keep the existing durable control-plane, connector, supervisor, identity, alarm and evidence behavior.
- Every slice has exclusive file ownership. Shared integration files belong to TO-09 only.
- Fail closed on invalid identity, unknown profiles, stale lease tokens, unknown process conflicts,
  manifest escapes, incomplete evidence, reconciliation violations or release uncertainty.
- No production writes, pushes, merges, branch deletion, remote process killing or unknown-process
  termination are authorized by this ledger.

## TO-01 — Event bus and run-state schemas  [completed]
DONE (machine): Rust tests prove validated event types, append/tail offsets, malformed-line handling,
  stable run/node IDs and durable reload.
DONE (human): none.
Files: src-tauri/src/orchestrator/event_bus.rs, src-tauri/src/orchestrator/model.rs
Commit: 55b10e9

## TO-02 — Fail-closed system preflight  [completed]
DONE (machine): Rust tests prove clean baseline capture, allowlisted stop through a mocked adapter,
  and unknown conflict -> decision-required with no kill.
DONE (human): conflict reasons are understandable without opening logs.
Files: src-tauri/src/orchestrator/preflight.rs, src-tauri/src/orchestrator/run_scope.rs
Commit: e7daaec

## TO-03 — Scheduler leases, heartbeats and fencing  [completed]
DONE (machine): Rust tests prove atomic claim, token renewal, stale-token commit refusal, one warning
  per stall, evidence-preserving reassignment and two-retry blocking.
DONE (human): none.
Files: src-tauri/src/orchestrator/scheduler.rs
Depends on: TO-01
Commits: ed38b74, b13d67c

## TO-04 — Worker manifest and evidence engine  [completed]
DONE (machine): Rust tests prove manifest escape refusal, profile validation, BEFORE/AFTER UI evidence,
  headless command/exit evidence without OCR, capture hashes and gate-pass/gate-fail events.
DONE (human): UI profile evidence reads as a before/after comparison rather than a raw file list.
Files: src-tauri/src/orchestrator/evidence.rs, src-tauri/src/orchestrator/worker.rs
Depends on: TO-01, TO-03
Commit: 1539c18

## TO-05 — Planned-versus-actual reconciliation  [completed]
DONE (machine): Rust tests detect unplanned hunks, missing outputs and orphaned nodes; exact named
  waivers suppress only their target and are retained in the audit result.
DONE (human): violation output names plan, node, commit and file clearly.
Files: src-tauri/src/orchestrator/reconcile.rs
Depends on: TO-01
Commit: 811bbde

## TO-06 — Release gate and GitHub state model  [completed]
DONE (machine): Rust tests prove merge conflict, CI infrastructure failure, missing evidence and dirty
  reconciliation each refuse ready-for-PR; only a fully green model advances.
DONE (human): none.
Files: src-tauri/src/orchestrator/release.rs
Depends on: TO-03, TO-04, TO-05
Commit: 127f641

## TO-07 — Delivery, archival and clean finish  [completed]
DONE (machine): Rust tests generate COMPLETION-REPORT.md, changes.md and LEFTOVERS.md, copy durable
  handover files, append without rewriting checklist history, archive the run and emit run-done last.
DONE (human): final card says where the handover is and nothing remains marked running.
Files: src-tauri/src/orchestrator/delivery.rs
Depends on: TO-06
Commits: af19fa1, b13d67c

## TO-08 — Tauri pipeline console  [completed]
DONE (machine): TypeScript build and Chrome-headless test cover run status, preflight blockers, leases,
  evidence comparison, reconciliation, draggable audit surface and Completed shelf.
DONE (human): high-quality 3x screenshots show active and completed states with clean console output.
Files: src/services/orchestratorPipeline.ts, src/components/PipelineConsole.tsx,
  tests/orchestrator_pipeline_e2e.py
Depends on: TO-01, TO-02, TO-03, TO-04, TO-05
Commits: 2dc2f65, dcb8c33, 824935f, d15ac00

## TO-09 — Tauri command integration and end-to-end delivery  [completed]
DONE (machine): cargo check, cargo test, npm build and all Python E2E tests pass; package build succeeds;
  command permissions are least-privilege and a toy run completes end-to-end.
DONE (human): screenshots inspected; before/after evidence, warnings, completion and handover are visible.
Files: src-tauri/src/orchestrator/mod.rs, src-tauri/src/orchestrator/api.rs,
  src-tauri/src/lib.rs, src-tauri/Cargo.toml, src/App.tsx, src/index.css,
  src-tauri/capabilities/main-read-only.json, src-tauri/permissions/orchestrator-pipeline.toml,
  package.json, design-qa.md, docs/handovers/ORCH-20260822-001/COMPLETION-REPORT.md,
  docs/handovers/ORCH-20260822-001/LEFTOVERS.md
Depends on: TO-01, TO-02, TO-03, TO-04, TO-05, TO-06, TO-07, TO-08
Commits: b005529, 93da861, 9d7add3, 3b22248, 5cb0368, 8da9892, efc8ab1

## Verification record

- Rust: 73/73 tests passed, including a continuous public-command toy run through archive/catalog.
- Rust lint: warnings denied with only the unrelated pre-existing
  `clippy::large_enum_variant` in `control_plane.rs` explicitly allowed.
- Frontend: `npm run build` passed (TypeScript + Vite).
- Browser: all six Python/Chrome regressions passed against the isolated worktree server.
- Pipeline proof: 1920 × 1080 CSS viewport at 3× density; screenshot visually inspected;
  browser console, page errors, failed requests and HTTP errors were clean.
- Packaging: MSI and NSIS bundles built successfully; final hashes are recorded in the handover.
- Remote delivery: no push, PR, CI run or merge was performed by this local implementation run.
