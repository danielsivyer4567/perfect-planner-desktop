# Perfect Planner finish-line report

Date: 2026-08-27
Repository: `C:\repos\perfect-planner-tauri`
Branch: `feature/tauri-orchestrator-messaging-20260821-223935`

## Outcome

The local Perfect Planner product lifecycle is substantially finished and regression-proven, but
the application is **not production-ready**. Product, browser, native package, and both installer
candidates pass locally. Production promotion remains blocked by the absent Git remote/hosted CI,
absent Windows signing identity, untested physical keyboard/300% Windows scale, and absent clean-VM
smoke.

The Graphify architecture map influenced this review by separating the Session Recovery, Control
Plane Messaging, Collision Registry/Census, Repository Identity, Verification Evidence, and Release
Gate communities. Direct source inspection then verified the authority boundaries and persistence
paths recorded in `ARCHITECTURE.md`.

## Architecture findings and lifecycle

See `ARCHITECTURE.md` for the full audit. The central lifecycle is:

```text
bounded board identity census
  -> explicit exact repository + plan selection
  -> immutable repository-scoped native run
  -> preflight + dirty/prerequisite/conflict checks
  -> explicit approval + collision clearance
  -> authority-backed worker admission + scoped messages
  -> heartbeat / explicit interruption or failure
  -> evidence-gated completion
  -> reconciliation
  -> CI readiness
  -> hosted CI + signed distribution (not yet available)
```

The most serious finding was a global recovery replay path that could mutate a discovered board in
a foreign repository. Recovery is now authorized only for the exact explicitly selected
`repositoryRoot + planPath`, matching active port identity, freshly read in-progress task, and
original worker session. A separate same-port replacement regression proves port number alone is
never identity.

## Product and test files changed

- `.github/workflows/ci.yml`
- `.gitignore`
- `src/App.tsx`
- `src/services/orchestrationWorkspace.ts`
- `src/services/sessionSupervisor.ts`
- `tests/orchestration_workspace_e2e.py`
- `tests/repository_rail_e2e.py`
- `tests/native_finish_line_e2e.py`
- `FINISH-LINE-TODO.md`
- `docs/handovers/PERFECT-PLANNER-FINISH-LINE-20260827/ARCHITECTURE.md`
- `docs/handovers/PERFECT-PLANNER-FINISH-LINE-20260827/RECONCILIATION.md`
- `docs/handovers/PERFECT-PLANNER-FINISH-LINE-20260827/WINDOWS-DISTRIBUTION.md`
- `docs/handovers/PERFECT-PLANNER-FINISH-LINE-20260827/FINAL-REPORT.md`

## Durable planner state and evidence retained

- `.claude/scratch/ledger.md`
- `.claude/scratch/perfect-plan/evidence/journal.jsonl`
- `.claude/scratch/perfect-plan/orchestrator-messaging.json`
- `COMPLETE-CHECKLIST.md`
- 92 files under `.claude/scratch/perfect-plan/evidence/`: 61 PP-001, 29 PP-002, and 2
  combined-CI logs. Every filename is bound to PP-001, PP-002, the canonical journal, or the
  generated checklist; zero are orphaned.

The exact final committed inventory is available with `git show --name-status --stat <handoff SHA>`.

## Verification performed

- `npm run build`: passed.
- `npm run test:e2e`: all eight suites passed.
- `cargo fmt --all -- --check`: passed.
- `cargo test --all-targets`: 306 unit tests passed, 3 intentionally ignored; all 15 native
  collision-authority/census/claim contract tests passed.
- `cargo clippy --all-targets -- -D warnings`: passed.
- `npm run tauri build`: passed; x64 MSI and NSIS candidates produced.
- Packaged restart: exact `Perfect Planner / PP-002 / Cross-repository Collision Assessor`
  restored.
- Recovery durability: exactly one B20 event, `pp-reaper-1787422242997-23`; plan and reaper hashes
  were unchanged across each packaged/installed smoke.
- Native geometry: compact header about 53 CSS px; canvas begins about 131 CSS px in the primary
  window and 138 CSS px in the 900x700 window; inspector does not displace the canvas; narrow native
  window has no horizontal document overflow.
- Native console: zero console errors, page errors, unexpected request failures, or CSP violations.
  The WebView2 IPC transport's expected `ERR_ABORTED` receipt is classified separately.
- NSIS: install, installed-app native smoke, reinstall, uninstall, retained app data, and zero
  orphan app processes passed.
- MSI: install, installed-app native smoke, reconfigure, uninstall, retained app data, and zero
  orphan app processes passed. Verbose Windows Installer logs record status 0.
- `git diff --check` passed for product source, tests, workflows, and documentation; the retained
  historical `.log` evidence contains original CRLF/trailing-space command output and is not
  rewritten. The tracked-diff secret-shape scan found no matches.

## Screenshot and local evidence

- `artifacts/native-tauri/native-finish-line.png`
- `artifacts/native-tauri/native-narrow-window.png`
- `artifacts/native-tauri/finish-line-selection-before-restart.png`
- `artifacts/native-tauri/finish-line-selection-after-restart.png`
- `artifacts/native-tauri/native-finish-line.json`
- `artifacts/native-tauri/msi-install-per-user.log`
- `artifacts/native-tauri/msi-reinstall.log`
- `artifacts/native-tauri/msi-uninstall.log`

These are ignored, machine-specific runtime outputs. They are not source or promoted release
artifacts.

## Unsigned candidate fingerprints

These hashes identify the **installer-tested unsigned** candidates from the package build used for
the MSI/NSIS install, launch, reinstall, and uninstall matrix. They must not be presented as signed
release hashes:

- app EXE: `A1846196F2C7B3D7F3DE7302A0653953C5639F072F14C5D0A3B27860265576A5`
- MSI: `206412BC3D36DC98575852CE6BFD05320668F37F23B5422DE033696EFD860E2E`
- NSIS: `28D15FE2D432BBA58E56B9BD2FBD4513C8FEAEAF3105273052C3B2CCE5E4B985`

All three return `NotSigned`. Signed hashes must be generated again after signing.

The final full local CI mirror ran on exact product commit
`dff52d20df3fee7a68970368fde3b40b43c948af` and rebuilt the unsigned bundles. Bundle metadata makes
those files non-reproducible against the prior package build; their post-matrix hashes are:

- app EXE: `78E0280C0AE67DF8DE5AB1CB18C55E70F6779CEFC2729DAFA07FA7464592582D`
- MSI: `DE223FD1A82005B8BB97FD371001B96362BBB5652F5B82C8F82E032DCF763AC4`
- NSIS: `871DED5A00BEE845554C346C2B4C8B3CE93C54488DE2B744EBF96AC0CE1C4CD9`

The exact-commit rebuild passed package creation but was not reinstalled after this final metadata
change. Signing must produce the definitive artifacts, hashes, and clean-machine install evidence.

## Known gaps deliberately left open

1. No Git remote/default branch exists. Hosted CI, simulated merge against the true base, review,
   push, and branch protection cannot be inferred.
2. No user or machine code-signing certificate is installed. Signing and signed-artifact hashes
   cannot be completed.
3. No clean Windows VM was available for final signed-installer smoke or artifact promotion.
4. WebView2 did not surface an actual Windows Escape injection to the DOM. DOM focus return,
   browser keyboard navigation, and native narrow-window behavior are proven; physical keyboard and
   300% Windows display scaling require a human pass.
5. The current native run `run-mt52331o` is honestly `ready` with no recorded preflight, worker,
   completion, or CI result. Those states were proven in isolated browser/native test fixtures and
   Rust tests, but were not invented in the packaged app.
6. During pre-fix verification, Perfect Planner wrote a recovery marker into one external Looplet
   plan. The app was stopped, Looplet was not edited or reverted here, and the corrected package did
   not advance that external file's timestamp. Details are preserved in `RECONCILIATION.md`.

## Release decision

The verified local commit is suitable for review and for configuring a real remote CI/signing
pipeline. It must not be labelled production-ready or distributed until the open gates in
`FINISH-LINE-TODO.md` are completed on the exact signed release commit.
