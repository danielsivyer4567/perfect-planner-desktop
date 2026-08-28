# Perfect Planner finish-line report

Date: 2026-08-28
Repository: `C:\repos\perfect-planner-tauri`
Branch: `feature/tauri-orchestrator-messaging-20260821-223935`

## Outcome

The local Perfect Planner product lifecycle is substantially finished and regression-proven, but
the application is **not production-ready**. Sanitized merge commit `4a4d50e` integrates the exact
remote `main`, resolves `.gitignore`, and passes the complete local matrix. During the real merge,
an obfuscated network-executing payload was discovered in remote `postcss.config.js`; it is removed
from the final tree and guarded against recurrence, but an earlier simulation executed the tainted
configuration. Production promotion is therefore blocked by clean-host credential rotation/build
evidence, Windows signing, OS-level 300% scale, and clean-VM smoke. See
`SECURITY-INCIDENT-20260828.md`.

The final local UI slice adds a prominent Parallel agents switch that defaults new runs to four
native-enforced active workers and creates serial one-worker runs when off. Existing runs retain
their stored limit. The workflow canvas can split into live UI and captured evidence, switch the
evidence pane between UI and attached text/code output, and refuses to label captures below
1280x720 or without dimensions as comparison-grade.

The real integration is committed and pushed on the feature branch. Hosted Windows CI run
`33145216487` passed the exact source commit
`9d28edfc22de3975e6bb69ecaf10b9586d5a77b6`. The final tree passes the repository-security gate,
frontend build, all eight browser suites, Rust format, 309 active Rust tests plus 17 contract tests,
warning-denied Clippy, MSI/NSIS packaging, and packaged/installed native smoke. The native proof has
zero console errors, page errors, or unexpected failed requests and records physical Escape/Tab,
Per-Monitor awareness, scoped message routing, and correlated launch/exit.

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
  -> hosted CI
  -> signed distribution (external release gate still pending)
```

The most serious finding was a global recovery replay path that could mutate a discovered board in
a foreign repository. Recovery is now authorized only for the exact explicitly selected
`repositoryRoot + planPath`, matching active port identity, freshly read in-progress task, and
original worker session. A separate same-port replacement regression proves port number alone is
never identity.

## Product and test files changed

- `.github/workflows/ci.yml`
- `.gitignore`
- `src-tauri/src/app_lifecycle.rs`
- `src-tauri/src/lib.rs`
- `src/App.tsx`
- `src/components/ActionContextMenu.tsx`
- `src/services/orchestrationWorkspace.ts`
- `src/services/sessionSupervisor.ts`
- `tests/native_release_evidence_e2e.py`
- `tests/orchestrator_pipeline_e2e.py`
- `tests/orchestration_workspace_e2e.py`
- `tests/repository_rail_e2e.py`
- `tests/native_finish_line_e2e.py`
- `FINISH-LINE-TODO.md`
- `docs/handovers/PERFECT-PLANNER-FINISH-LINE-20260827/ARCHITECTURE.md`
- `docs/handovers/PERFECT-PLANNER-FINISH-LINE-20260827/NATIVE-EVIDENCE.md`
- `docs/handovers/PERFECT-PLANNER-FINISH-LINE-20260827/RECONCILIATION.md`
- `docs/handovers/PERFECT-PLANNER-FINISH-LINE-20260827/WINDOWS-DISTRIBUTION.md`
- `docs/handovers/PERFECT-PLANNER-FINISH-LINE-20260827/FINAL-REPORT.md`

Latest parallel/comparison/DPI finish slice additionally changes:

- `src-tauri/build.rs`
- `src-tauri/windows-app.manifest.xml`
- `src-tauri/tests/windows_manifest_contract.rs`
- `src-tauri/src/orchestrator/api.rs`
- `src-tauri/src/orchestrator/scheduler.rs`
- `src/components/LocalOutputWitness.tsx`
- `src/components/PipelineConsole.tsx`
- `src/index.css`
- `src/services/orchestratorPipeline.ts`
- `tests/evidence_e2e.py`
- `docs/handovers/PERFECT-PLANNER-FINISH-LINE-20260827/WINDOWS-DISTRIBUTION.md`

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
- `cargo test --all-targets`: 309 unit tests passed, 3 intentionally ignored; all 17 native
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
- Native focus and scale: a real Windows Escape event closed the inspector and returned focus to
  `Inspect`; a real Windows Tab event moved to a repository button. A 3456x2058 physical viewport
  represented at device scale factor 3 produced a
  1152x686 CSS viewport without overflowing the Perfect Planner shell; the embedded plan remains
  horizontally pannable. This is WebView emulation, not an OS-setting claim.
- Native DPI contract: the release and installed executable manifests contain Common Controls v6
  and `PerMonitorV2, PerMonitor`; the installed process reported `PER_MONITOR_DPI_AWARE`. On the
  active physical 96-DPI/100% monitor the WebView reported device pixel ratio 1. No 300% physical
  monitor claim is made.
- Parallel and comparison UI: the browser suite proved the switch defaults on, persists off/on,
  communicates “new runs x4/x1,” opens a side-by-side live/captured UI, switches to real attached
  text/code evidence, labels a recorded 1440x900 capture comparison-grade, and records no console
  or page errors.
- Native lifecycle records: real run `run-mt4oo7z4` durably records preflight, approval, admission,
  heartbeat, stale-lease recovery, and evidence-gated fenced completion. The record hashes and
  timeline are in `NATIVE-EVIDENCE.md`.
- Packaged native routing: synthetic evidence message `pp-message-1787883016705-5`, isolated under
  repository ID `pp-finish-line-native-evidence`, progressed exactly queued, claimed, delivered,
  acknowledged through Tauri IPC. Product session `pp-app-1787883015005-39004` recorded one
  correlated launch/exit pair. The packaged WebView console, page-error list, and unexpected failed
  request list were empty.
- NSIS: install, installed-app native smoke, reinstall, uninstall, retained app data, and zero
  orphan app processes passed.
- MSI: install, installed-app native smoke, reconfigure, uninstall, retained app data, and zero
  orphan app processes passed. Verbose Windows Installer logs record status 0.
- `git diff --check` passed for product source, tests, workflows, and documentation; the retained
  historical `.log` evidence contains original CRLF/trailing-space command output and is not
  rewritten. The tracked-diff secret-shape scan found no matches.

### Historical remote-main integration simulation

- Remote: `https://github.com/danielsivyer4567/perfect-planner-desktop.git`.
- Integration-simulation input head: `7c1e91c1863d7e31a72eb4dfb08252876f0cf395`.
- `origin/main` and the remote feature branch: `1f3a183`; local is 15 ahead and 1 behind.
- Automatic merge analysis found one content conflict, `.gitignore`. The resolved simulation kept
  the local superset, which already includes the remote transient-output exclusions. Rust formatting
  and PostCSS changes merged automatically.
- The materialized candidate is Git tree `b8a51cc569587af6ad1512602801c3d12b0b51be`.
- On that tree: frontend production build passed; all eight browser/integration suites passed; Rust
  formatting passed; 306 active unit tests passed with 3 intentionally ignored; all 15 contract
  tests passed; warning-denied Clippy passed; MSI and NSIS package creation passed.
- The first package attempt was terminated during a Windows restart (`0x40010004`). The retry on the
  restarted system compiled the same dependencies and product successfully without a source change.
- No merge, rebase, push, or remote mutation was performed during this earlier simulation. The
  later real integration and feature-branch push are recorded below.

### Exact hosted verification

- Feature source commit: `9d28edfc22de3975e6bb69ecaf10b9586d5a77b6`.
- GitHub Actions run: `33145216487`.
- Result: passed on Windows in 21m10s.
- Passed steps: security guard, frontend build, all browser/integration regressions, Rust format,
  all Rust tests, warning-denied Clippy, and Windows x64 MSI/NSIS package creation.
- The feature branch and `origin/feature/tauri-orchestrator-messaging-20260821-223935` both resolved
  to that exact source commit after the push; the working tree was clean.

## Screenshot and local evidence

- `artifacts/native-tauri/native-finish-line.png`
- `artifacts/native-tauri/native-narrow-window.png`
- `artifacts/native-tauri/native-three-times-scale.png`
- `artifacts/native-tauri/finish-line-selection-before-restart.png`
- `artifacts/native-tauri/finish-line-selection-after-restart.png`
- `artifacts/native-tauri/native-finish-line.json`
- `artifacts/native-tauri/native-routed-message-lifecycle.json`
- `artifacts/native-tauri/native-routed-message-lifecycle.png`
- `artifacts/native-tauri/native-physical-keyboard.png`
- `artifacts/left-rail-output.png` (split live/captured UI comparison proof)
- `artifacts/native-tauri/installed-app.manifest.xml`
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

The resolved remote-main candidate tree produced these additional **unsigned, simulation-only**
fingerprints. They prove package creation after conflict resolution; they were not installed and are
not promotable release artifacts:

- app EXE: `AE0AF1EA9F8BC13BBFC4D54510AF2D0AEBEB742D311BDF19E3E4BC95C991A50E`
- MSI: `5EA60ECFEBBDBE0E9B234F7865F7DB5FBAC2810F4238464F18067E7F8F9B3DC8`
- NSIS: `3DA3EF917FF4DF221F656B1D7474DCBB748DF173FE0E18BAF2B1EDB6B2C43893`

`Get-AuthenticodeSignature` reports `NotSigned` for all three.

The latest resolved tree `489f3000dec4b9fd4e46576eaee1f8d6fcfbc3a1`, which includes the Parallel
agents and UI-comparison slice, produced these further **unsigned, simulation-only** fingerprints:

- app EXE: `F94EA2FE5A27485B13F285E5BB19ACF15F575B2348A2C8C87D0DE916BD8E68F8`
- MSI: `5AF2000FD4958C296E93FEB34C0B2E9F190469DE7FDF8DD5A6105EEB34BA1D5E`
- NSIS: `F8A31EF4D46B4610E04EA578A398C0889F00B2089FA03D4CC3C8EAF61DA63692`

All three report `NotSigned`. The temporary exported source and generated candidates were removed
after hashes and verification were recorded.

The latest post-fix unsigned local build produced MSI
`83DE84AB0C59A0136BA5086846C92C74D2825CCFF7AFC1EA50D4F995027EC924` and NSIS installer
`580D01FAA0384107E1318E65D8FEBAD5436F8CC0872ABF34D1511DEF6B21EE9D`. The NSIS installer was
reinstalled and its installed executable
`70BABB6DB337837DA8C5DCF8DF071BD0629138A48D003498CCBFBDE0EE9DC155` passed the routed-message,
product launch/exit, physical keyboard, screenshot, and clean-console proof. These remain unsigned
local candidates, not production release artifacts.

The subsequent Parallel agents, UI comparison, and Per-Monitor V2 package produced these latest
**unsigned local** fingerprints:

- release app EXE: `D5C6C7E09E5CDC0AF7BDFA89B370DCFF07458611C4C1D6C27700D617987BABED`
- MSI: `3D1AA4DB6825F26D889A865C5A26FB64F8739F1B13BB2BDBE9D6A2C650101296`
- NSIS: `CA6C57FB43745BF9E58C7BC9EFFA8677027AA92682738062B187482112CE5C9C`
- installed NSIS executable: `9E9A9156D0DE65CC9F324CC17641D6A318F757E760A4597A8E9A63C7695A1183`

The NSIS candidate was reinstalled and passed native routing, correlated launch/exit, physical
Escape/Tab, per-monitor awareness/active-scale inheritance, screenshot, and clean-console proof.
These artifacts remain unsigned and were not tested on a clean VM.

The sanitized integrated candidate produced these latest **unsigned local-test-only** fingerprints:

- release app EXE: `02DB0D8127FFE764D561C5A6B9B9E20201C49C8C02775D79B63FD0176D89F851`
- MSI: `83BD5AD0758D6C1FAE2550C1D43AEF33CE269F16274662E14A08AE439035A4B2`
- NSIS: `ECD75ECFB6E189A1943BF5B6C87BDD92FC090B3D428D2368C5F0B9E963D3D667`
- installed NSIS executable: `C378020DBA631440F0C88116C967C3B8ADDA937DDA82496B94CE8C24AFC3D2FE`

The installed executable passed the packaged native lifecycle after silent reinstall. All four
fingerprints are unsigned and must not be promoted from this exposed build host.

## Known gaps deliberately left open

1. Remote commit `1f3a183` contains an obfuscated network-executing PostCSS payload. The final tree
   is clean and guarded, but the prior execution means credentials must be rotated and production
   artifacts rebuilt on a known-clean host. See `SECURITY-INCIDENT-20260828.md`.
2. GitHub returned HTTP 403 for branch-protection and ruleset APIs because this private repository's
   current account plan does not provide those controls.
3. No user or machine code-signing certificate is installed. Signing and signed-artifact hashes
   cannot be completed.
4. No clean Windows VM was available for final signed-installer smoke or artifact promotion.
5. Real Windows Escape and Tab input, native narrow-window behavior, and 300% WebView device-scale
   emulation are proven. The three attached physical monitors enumerate as 100%, 150%, and 150%,
   so Windows OS-level 300% display scaling still requires a different target display.
6. The current native run `run-mt52331o` is honestly `ready` with no recorded preflight, worker,
   completion, or CI result. Those states were proven in isolated browser/native test fixtures and
   Rust tests, but were not invented in the packaged app.
7. During pre-fix verification, Perfect Planner wrote a recovery marker into one external Looplet
   plan. The app was stopped, Looplet was not edited or reverted here, and the corrected package did
   not advance that external file's timestamp. Details are preserved in `RECONCILIATION.md`.

## Release decision

The verified feature source is suitable for review and has passed the real hosted Windows CI
pipeline. It must not be labelled production-ready or distributed until the open gates in
`FINISH-LINE-TODO.md` are completed on the exact signed release commit.

See `NATIVE-EVIDENCE.md` for the exact local interaction proof, durable native event timeline,
hashes, and remaining evidence boundary.
