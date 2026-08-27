# Perfect Planner architecture and lifecycle audit

Date: 2026-08-27
Scope: `C:\repos\perfect-planner-tauri` only

## System boundary

Perfect Planner is a Tauri desktop orchestration shell. It does not own the visual Perfect
Planning board implementation served on ports 5230-5249. It discovers those loopback board
processes, verifies their identities, renders the explicitly selected board in an iframe, and
adds native persistence and orchestration authority around them.

The desktop has two related but separate lifecycle surfaces:

1. **Legacy board observation and recovery** — repository/plan census, worker heartbeat display,
   approval observation, and stale-session recovery for already-running Perfect Planning boards.
2. **Native orchestration runs** — immutable repository-scoped run manifests, preflight, collision
   assessment, approval, worker admission, leases, evidence-gated completion, reconciliation, and
   release evaluation.

Unknown or unavailable state remains unknown. A discovered board, browser approval string, missing
run, or missing CI result is never converted into invented worker, message, run, or CI state.

## Actual data and control flow

### Repositories, worktrees, plans, and visual selection

`src/services/boards.ts` probes the bounded board-port window. Native discovery in
`src-tauri/src/lib.rs` accepts only `/whoami`, `/workers`, and `/plan`, bounds response sizes, and
rechecks `planPath` immediately before worker or plan reads. The renderer groups boards by stable
repository identity and assigns display call signs only after discovery.

`src/App.tsx` owns the active visual scope. The persistent selection is the exact normalized pair
`repositoryRoot + planPath`; the port is transport metadata only. On restart, the pair must still
exist in the census. A missing pair or a different board reusing the same port produces “Saved plan
unavailable,” no active iframe, no worker projection, and no recovery-write authority.

Worktree, branch, repository root, plan ID/path, and board process metadata originate in `/whoami`.
Native runs independently re-derive and canonicalize Git repository/worktree identity in
`src-tauri/src/orchestrator/run_scope.rs`; renderer-supplied identity does not create authority.

### Runs, tasks, workers, and evidence

`orchestrator_create_run` creates an immutable run scope under
`<repository>/.claude/scratch/orchestrator/<run-id>/`. The scope binds the canonical repository,
Git worktree/common directory, branch, HEAD, plan path and digest, allowed files/resources, nodes,
verification commands, and approval receipt digest. Reopening a run re-derives the live binding and
refuses mismatches.

`orchestrator_preflight_inspect` records a read-only native baseline: repository identity, HEAD,
dirty targets, declared ports/resources, and process/resource observations. Approval is separate:
`orchestrator_approve_run` requires a recent READY preflight, rechecks dirty target files, performs
a whole-plan machine-wide collision census, and persists a digest-bound receipt.

`orchestrator_admit_worker` is the assignment boundary. It requires the exact run and node, current
preflight, explicit approval, unchanged collision registry/census, registered approval delivery,
clean target files, and an authority-backed preclaim. The scheduler issues a bounded lease and the
worker can only heartbeat, submit evidence, complete, or fail through that lease.

Completion passes through `orchestrator_complete_worker`. `worker.rs` and `evidence.rs` validate the
manifest, claimed outputs, required test/build/runtime evidence, unresolved risks, Git baseline, and
lease authority before the scheduler records completion. Failure is explicit through
`orchestrator_fail_worker`; expiration is explicit through `orchestrator_recover_workers`. Hot-resume
state and append-only run audit/event ledgers preserve interrupted work.

### Messages, tasks/chats, and approvals

`src-tauri/src/control_plane.rs` is an append-only scoped message store. A message names its
organization, repository root/ID, worktree, branch, plan, run/node/task, actor, and destination.
Derived delivery states are queued, claimed, delivered, acknowledged, retrying, undeliverable, and
dead-letter; idempotency and claim leases prevent duplicate delivery from becoming new state.

`src-tauri/src/connectors.rs` ingests bounded atomic drop envelopes from the app-data inbox, polls
registered approval routes, claims the next Codex delivery, runs only the configured connector, and
records stdout/stderr receipts or actionable failure. Invalid drops are quarantined and connector
errors are appended to a dedicated log.

`src-tauri/src/approval_bridge.rs` binds a browser-observed approval to one registered originating
task route, board PID, port, launch nonce, plan, repository, and expiry. Approval queues a wake
message; admission remains blocked until delivery is recorded. An unregistered, expired, revoked,
or identity-mismatched route remains visibly blocked.

### Stale and interrupted work

For legacy board workers, `src-tauri/src/supervisor.rs` persists lease/reaper events in app data.
The renderer classifies each durable event against a freshly read plan. Completed/superseded and
already-applied events settle without replay. A write is attempted only when all of these match:

- the person explicitly selected a repository and plan;
- the current board has that exact `repositoryRoot + planPath`;
- the event names that exact plan;
- the active port still proves the same board identity;
- the task is still in progress and held by the cleared session.

For native workers, the scheduler reaps expired leases into retryable or blocked outcomes, records
the action in the run audit/event ledger, and updates hot-resume state. No task quietly disappears.

### Completion and CI

`orchestrator_reconcile` compares planned files/nodes with actual commits and outputs and reports
unplanned, unproven, orphaned, or waived violations. `release.rs` then treats dirty worktree,
conflicts, missing evidence, reconciliation findings, CI state, push state, and review state as
separate gates. CI is therefore a final confirmation of already-recorded evidence, not the first
place missing prerequisites are discovered.

The repository now contains `.github/workflows/ci.yml`, which runs the frontend build, full browser
regression, Rust format/tests/lint, and Windows MSI/NSIS package build on the checked-out commit.
There is currently no configured Git remote, so no hosted run exists yet.

## Current versus required lifecycle

### Before the finish pass

```text
bounded port census
  -> implicit/port-anchored visual selection
  -> aggregate board and worker state
  -> global durable recovery-event loop
  -> matching planPath/port POST
  -> possible foreign-repository recovery write
```

### Implemented product lifecycle

```text
bounded identity census
  -> explicit exact repository + plan selection
  -> identity-fenced reads and isolated visual projections
  -> create immutable repository-scoped run
  -> native preflight + dirty/prerequisite/conflict checks
  -> explicit whole-run approval + collision clearance
  -> authority-backed worker admission and scoped messaging
  -> heartbeat / actionable interruption / explicit failure
  -> evidence-gated completion
  -> reconciliation
  -> CI readiness
  -> hosted CI + signed distribution (external release gates still pending)
```

## Prioritized findings

### P0 — fixed and regression-proven

1. Recovery events were previously scanned globally and could be mirrored into a discovered board
   outside the selected repository. Exact selection authorization, fresh task/session
   classification, and port-identity checks now fail closed.
2. Active scope previously depended on a transient port and could silently change after restart or
   port reuse. Persistence now uses exact repository and plan identity; missing identity produces an
   honest empty state.
3. Recovery delivery errors previously appeared as a stopped supervisor. Supervisor availability
   and exact-task delivery failure are now separate, actionable states.

### P1 — release blockers outside the completed product changes

1. No Git remote or intended default branch is configured; hosted CI, review, branch protection,
   and a simulated merge against the true base cannot be proven.
2. The application executable, MSI, and NSIS installer are unsigned and no Windows signing identity
   is configured.
3. MSI/NSIS install, upgrade/reinstall, uninstall, retained-data, orphan-process, and clean-machine
   smoke tests have not been completed.
4. Packaged keyboard injection through WebView2/CDP produced no physical Escape event. DOM focus
   restoration is proven, and browser keyboard paths pass, but a human/native keyboard pass at
   narrow width and 300% display scaling remains required.

### P2 — deliberately deferred production operations

1. Decide between a signed in-app updater and controlled manual signed releases.
2. Add release notes, supported Windows/architecture matrix, support/rollback instructions, and
   hash binding after signed artifacts exist.
3. Promote only the exact signed artifacts that pass hosted CI and a clean Windows machine/VM.

## Test coverage map

- Repository separation, restart identity, missing identity, missed polls, and port reuse:
  `tests/repository_rail_e2e.py`
- Recovery classification, foreign-scope rejection, lifecycle state, routing/dead-letter, and CI
  readiness: `tests/orchestration_workspace_e2e.py`
- Scoped message delivery/acknowledgement and repository isolation:
  `tests/control_plane_e2e.py`
- Run scope, preflight, approval, worker leases/evidence, reconciliation, and release warnings:
  `tests/orchestrator_pipeline_e2e.py` plus native Rust unit/integration tests
- Packaged Tauri selection/restart, recovery durability, geometry, focus, console, and screenshot:
  `tests/native_finish_line_e2e.py`
