# Ledger: Perfect Planner Tauri — fail-closed orchestration pipeline

## Architecture deep dive and finish — 2026-08-23

Approved: yes (the user explicitly directed implementation, verification and a local commit)
Repository: `C:\repos\perfect-planner-tauri`
Worktree: `C:\repos\perfect-planner-tauri`
Branch: `feature/tauri-orchestrator-messaging-20260821-223935`
Baseline commit: `77abb60cbbba9f964e04461e68e237b0255a9fd1`
Remote delivery: forbidden; create one verified local commit only

### Current lifecycle

```text
board discovery -> selected port -> plan JSON -> native run manifest
       |                 |              |              |
 legacy reports      iframe canvas   approval bridge  preflight -> approve
       |                                |              -> admit/lease
 session reaper -> exact-board clear    |              -> evidence/complete
                                        +-> control-plane messages

native durable truth: app-data JSONL + run-scoped manifest/scheduler/evidence/events
UI truth today: App + PipelineConsole + Messenger poll and present those streams separately
CI/release truth: persisted model can be displayed, but the renderer cannot invent or run it
```

### Required lifecycle

```text
explicit repository tab -> scoped plan -> native preflight -> safe recommendation
          |                   |               |
          +-- visual fence ---+        conflict / dirty / prerequisite truth
                                                  |
task registry + routed activity <---- admit/lease/heartbeat/result/recovery
          |                                       |
          +---- evidence gate (tests/build/smoke/risks) ----> ready for CI
                                                               |
                                             CI result -> complete / blocked / decision
```

### Prioritised gaps found before implementation

1. **P0 — fragmented operational truth:** run nodes, legacy workers, approval routing and messages
   are separately polled and cannot be read as one scoped lifecycle in the command surface.
2. **P0 — canvas displacement:** expanding orchestration inserts large pipeline and messaging panels
   into document flow, moving the active plan hundreds of pixels down the window.
3. **P0 — weak scope visibility:** repository identity exists in the left rail and breadcrumb but there
   is no compact repository tab/fence beside the active plan and health state.
4. **P0 — preflight blind spot:** the UI sends an empty required-port list, so declared port conflicts
   cannot become a decision before admission.
5. **P0 — CI readiness ambiguity:** native evidence/reconciliation/release state exists, but the primary
   surface does not distinguish unknown CI, blocked local evidence and genuinely ready-for-CI state.
6. **P1 — persistence/ownership:** the selected recorded run is renderer memory, while the catalog is
   durable; reload can require explicit re-selection. This pass will surface that honestly, not guess.
7. **P1 — actionable failure language:** several warnings name a subsystem but not the selected
   repository/plan and do not consistently give one safe next action.
8. **P1 — test gaps:** there is no single focused suite proving repository visual separation, declared
   port conflict derivation, interrupted recovery, message routing state, lifecycle transitions and
   CI-readiness presentation together.

### Execution slices

#### AF-01 — Scoped lifecycle projection [completed]

- Ownership: `src/services/orchestrationWorkspace.ts`, focused unit/browser fixtures.
- Depends on: architecture audit above.
- Accept: exact repository/plan inputs derive health, activity, task and CI-readiness labels without
  treating absent data as success; no cross-repository item may enter the projection.
- Stop: any projection requires fabricated worker, message, run or CI state.

#### AF-02 — Manifest-derived preflight conflicts [completed]

- Ownership: native run/preflight API, frontend pipeline service/console, focused Rust and UI tests.
- Depends on: AF-01.
- Accept: explicit plan port resources are derived from the immutable native manifest and checked by
  preflight; undeclared or malformed resources remain non-authoritative and visible as unknown/absent.
- Stop: renderer input can broaden native scope or stop an unowned process.

#### AF-03 — Command surface and non-displacing inspector [completed]

- Ownership: `src/App.tsx`, `src/components/OrchestratorMessenger.tsx`, `src/index.css`, UI regressions.
- Depends on: AF-01.
- Accept: compact full-width header shows repo, plan, health, orchestration, messaging and CI state;
  repository tabs visibly fence scope; diagnostics/pipeline/message detail lives in an accessible fixed
  inspector; collapsed canvas starts directly below the header.
- Stop: inspector traps focus incorrectly, obscures the canvas without a close path, or duplicates polls.

#### AF-04 — Verification, evidence and local commit [completed]

- Ownership: focused tests, existing verification scripts, screenshots, final diff and this ledger.
- Depends on: AF-01 through AF-03.
- Accept: formatting/typecheck, Rust tests, focused integration/browser tests, production build, fresh
  packaged Tauri smoke, screenshot plus refreshed console/native logs, and an exact staged-file review.
- Stop: any failing check, unexplained cross-repository assumption, or missing UI evidence.

### Design direction

- Palette: preserve the existing planner parchment and dark evergreen control plane; use semantic
  mint/amber/coral only for proven healthy/waiting/blocked state.
- Type: preserve the product's existing editorial heading and compact mono operations labels.
- Layout: the plan is the workspace; controls form a shallow scope strip and low-frequency detail slides
  over the right edge as an inspector rather than becoming another page section.
- Signature: an always-visible repository scope rail that reads like a physical switchboard and makes
  cross-repository context changes explicit.

## Live finish pass — 2026-08-23 (supersedes the completion claim below)

Approved: yes (user-directed execution in this task)
Plan: PP-002 — Cross-repository Collision Assessor
Session: s-049651b9
Repository: C:\repos\perfect-planner-tauri
Worktree: C:\repos\perfect-planner-tauri
Branch: feature/tauri-orchestrator-messaging-20260821-223935
Baseline commit: 177208f653bdfe622f6c292b212c02417996346b
Remote delivery: forbidden; local commits only after a coherent verified slice

The earlier TO-01..TO-09 record is retained below as historical evidence, but it is not proof for
this repository. It binds a different worktree and cites release/UI artifacts that are absent here.
Every historical checkbox is therefore reopened unless current-root source inspection, automated
tests and native/manual evidence re-establish it.

### Live dependency spine

#### LF-01 — Audit and evidence reconciliation [completed]

- Ownership: `.claude/scratch/ledger.md`, PP-002 proof records, audit-only generated reports.
- Depends on: none.
- Accept: repository/branch/worktree/remotes/status, native commands, frontend call paths, scripts,
  tests, plan state and prior artifacts are proved from `C:\repos\perfect-planner-tauri`.
- Evidence: captured command output, current-root plan integrity, focused baseline build/test/lint.
- Stop: any sibling plan/file collision, unexpected remote/worktree, or pre-existing product-file diff.
- Result: exact root/branch/worktree/remotes/status, command surfaces, call paths, scripts, plan proof
  integrity and prior artifact claims revalidated; stale old-worktree evidence is not being reused.

#### LF-02 — Exact run scope, plan approval and manifest binding [completed]

- Ownership: `src-tauri/src/orchestrator/run_scope.rs`, the create/load boundary in
  `src-tauri/src/orchestrator/api.rs`, `model.rs`, and focused Rust integration tests.
- Depends on: LF-01.
- Accept: create-or-load is idempotent only for the same physical worktree, branch, plan identity,
  plan digest, approval receipt and canonical allowed-file manifest; stale/cross-scope input fails closed.
- Evidence: negative-path Rust tests plus persisted manifest/hot-resume inspection after restart.
- Stop: Git identity cannot be derived natively or an existing run cannot be migrated fail-closed.
- Result: schema-v2 run manifests derive the physical Git worktree/common directories, live branch,
  baseline commit, stable approved plan contract, approval receipt and canonical plan-wide file union.
  Exact create is idempotent, every scoped command revalidates live binding, and drift/tamper fails closed.

#### LF-03 — Native authority admission and secret lease ownership [in progress]

- Ownership: `src-tauri/src/collision_assessor/{authority,clearance,registry,snapshot}.rs`,
  `src-tauri/src/orchestrator/{preclaim_store,scheduler,api}.rs`, `src-tauri/src/lib.rs`, permissions
  and command-contract tests.
- Depends on: LF-02 and PP-002 B20.
- Accept: reserve -> publish -> census -> CLEAR -> single-use signed grant -> claim is one native flow;
  renderer/worker never receives issuer keys or raw lease secrets; duplicate/stale/replayed grants fail.
- Evidence: race/replay/restart tests, command allowlist tests and durable audit events.
- Stop: incomplete census coverage, unknown registry state, lock/epoch loss, or scope digest drift.

#### LF-04 — Native heartbeat, evidence, recovery and atomic completion [pending]

- Ownership: `src-tauri/src/orchestrator/{worker,evidence,event_bus,reconcile,release,delivery,api,run_scope,scheduler}.rs`
  and focused Rust tests.
- Depends on: LF-03.
- Accept: native-held lease renews truthfully; actual Git changes are checked against the manifest;
  completion validates evidence and gates before an idempotent durable terminal receipt/hot-resume update;
  restart recovery resolves prepared/committed state without duplicate completion.
- Evidence: manifest-escape, dirty-state, evidence-gap, crash-window and idempotency tests plus run files.
- Stop: actual repository changes cannot be attributed safely or any partial terminal write can claim done.

#### LF-05 — Real Tauri lifecycle controls [pending]

- Ownership: `src/services/orchestratorPipeline.ts`, `src/components/PipelineConsole.tsx`, `src/App.tsx`,
  `src/index.css`, browser/native UI tests.
- Depends on: LF-02 through LF-04.
- Accept: create/load, preflight, approve, admit, observe/heartbeat, validate/complete and recover use
  registered native commands; unavailable actions are disabled with the exact fail-closed reason.
- Evidence: TypeScript build, browser regression, real packaged-app interactions, screenshots and clean logs.
- Stop: any UI control reports success without a native durable state transition.

#### LF-06 — Certification, packaging and evidence matrix [pending]

- Ownership: test scripts, `design-qa.md`, handover/evidence documents and generated release artifacts.
- Depends on: LF-01 through LF-05.
- Accept: formatting, full Rust tests, warnings-denied Clippy, frontend build, browser E2E, hazard suite,
  fresh Tauri release, real app launch and one safe bounded lifecycle demonstration all pass.
- Evidence: command logs, artifact hashes, screenshots, refreshed console/native logs, manual interaction notes,
  changed-file summary and requirement-to-proof matrix.
- Stop: any lifecycle requirement lacks both implementation evidence and an honest result/limitation.

### Global edit protocol

Before every product edit: rerun PP-002 collision scan, verify the target is owned by the current live
slice, inspect its diff and direct callers, and record dependency impact. No reset, stash, discard,
branch switch, worktree removal, network sync, merge, rebase, push, PR or remote mutation is allowed.
No slice is complete on compilation alone; focused tests precede full regression and evidence capture.

## Historical ledger (untrusted until revalidated)

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
