# Perfect Planner — Finish-Line Todo

Status snapshot: 2026-08-28
Repository: `C:\repos\perfect-planner-tauri`
Branch: `feature/tauri-orchestrator-messaging-20260821-223935`
Continuation base commit: `6c0f3000614ae841913805f87efcfb2c89ce2eb3`
Continuation starting head: `bbe53bc1e285ae0f09043240dc26af06994b6a12`
Latest integrated product/security source verified: `9d28edfc22de3975e6bb69ecaf10b9586d5a77b6`
Hosted Windows CI: run `33145216487` — passed on that exact source commit

## Finish-line definition

Perfect Planner is production-ready only when every checkbox below is complete, the intended
source is committed, the working tree is clean, CI passes on the exact release commit, and the
signed Windows installer has been installed and exercised successfully.

## 1. Close the remaining product gaps

- [x] Persist the active repository and plan as an exact `repoRoot + planPath` identity.
- [x] On restart, restore that selection only when both identities still exist; otherwise show an
      honest “selection required” state instead of silently selecting another repository.
- [x] Prove repository isolation: changing plans, messages, workers, evidence, runs, or recovery
      state in one repository cannot surface under another repository.
- [x] Recheck supervisor recovery so superseded/already-applied events are not reported as a
      stopped supervisor, while genuine delivery failures remain actionable.
- [x] Recheck the primary Tauri layout: compact orchestrator header, canvas begins near the top,
      workflow canvas remains dominant, and diagnostics stay in a collapsed inspector/drawer.
- [x] Recheck explicit terminal states: every task ends completed, blocked with a reason, or
      awaiting a named human decision; no task silently disappears.
- [x] Add a prominent, persistent Parallel agents switch that defaults new runs to bounded
      four-agent admission, keeps existing runs unchanged, and enforces serial admission when off.
- [x] Add a canvas-preserving two-pane live/captured UI comparison with UI/code-evidence switching,
      capture timestamps, and an explicit comparison-grade threshold of 1280x720.
- [x] Make the Snapshot canvas read browser-proof provenance from the generated build manifest,
      show Chrome MCP as unrecorded unless evidence exists, expose partial/unavailable states, and
      prove the same manifest loads inside the packaged Tauri application under a network-closed CSP.

## 2. Reconcile the current working tree

- [x] Review all tracked modifications and classify each as intended product work, generated
      planner state, or unrelated pre-existing work.
- [x] Review all untracked evidence artifacts and bind retained proof to the exact plan item and
      source commit. Preserve evidence; do not delete or rewrite the append-only journal.
- [x] Decide which evidence is release documentation and which is local runtime output; update
      ignore rules without hiding required audit evidence.
- [x] Run `git diff --check` and review the complete diff for accidental cross-repository paths,
      secrets, unsafe process control, stale assumptions, and generated noise.
- [x] Confirm no source repository outside `C:\repos\perfect-planner-tauri` was changed by this
      finish pass. Expected machine-local changes are limited to the installed Perfect Planner
      candidate and its append-only `%APPDATA%\com.looplet.perfectplanner` runtime evidence.

## 3. Run a fresh verification matrix

- [x] Frontend typecheck and production build: `npm run build`.
- [x] Full browser/integration regression: `npm run test:e2e`.
- [x] Rust formatting: `cargo fmt --all -- --check` from `src-tauri`.
- [x] Rust tests: `cargo test --all-targets` from `src-tauri`.
- [x] Rust lint with warnings denied: `cargo clippy --all-targets -- -D warnings` from `src-tauri`.
- [x] Fresh Windows x64 package build: `npm run tauri build`.
- [x] Restart test: launch, select a repository/plan, close, relaunch, and confirm the exact same
      repository/plan is restored without cross-repository content.
- [x] Recovery test: interrupt one task, relaunch, and prove the task becomes actionable without
      duplicate claims, messages, or evidence.
- [x] Message-routing test: prove delivered, undeliverable, stale, acknowledged, and dead-letter
      states remain scoped to the correct repository, plan, and task.
- [x] CI-readiness test: prove missing verification evidence blocks readiness before CI.

## 4. Collect UI and desktop evidence

- [x] Capture a screenshot of the packaged Tauri application—not the Vite browser page—showing
      the compact header, correct repository/plan identity, dominant canvas, and collapsed inspector.
- [x] Capture restart/recovery screenshots showing the same scoped plan before and after relaunch.
- [x] Refresh and record the Tauri webview console: zero unexplained errors, failed requests, or
      security-policy violations.
- [x] Record native logs for launch, preflight, routing, interruption recovery, completion, and exit.
  - [x] Bind the real native preflight, approval, admission, heartbeat, stale-lease recovery, and
        fenced-completion events to SHA-256 hashes in `NATIVE-EVIDENCE.md`.
  - [x] Preserve installer launch/process-exit evidence and zero-orphan-process results.
  - [x] Capture a packaged routed-message lifecycle and product-native launch/exit event pair.
- [ ] Manually exercise keyboard navigation, visible focus, modal close/return-focus, narrow-window
      layout, and three-times display scaling.
  - [x] Prove DOM keyboard navigation, visible focus return, narrow-window layout, and 300% WebView
        device-scale emulation in the installed app.
  - [x] Confirm physical Windows Escape/Tab input against the installed packaged app.
  - [x] Embed and inspect a Per-Monitor V2 Windows manifest; prove the installed process is
        `PER_MONITOR_DPI_AWARE` and its WebView inherits the active monitor scale.
  - [x] Enumerate the attached physical monitors: one is 100% and two are 150%; no attached target
        can provide an honest OS-level 300% pass.
  - [ ] Confirm Windows OS-level 300% scaling on the target display.
- [x] Install both MSI and NSIS candidates on Windows x64; verify launch, upgrade/reinstall,
      uninstall, retained user data, and no orphan background processes.

## 5. Establish the real release gate

- [x] Configure and verify the exact shared-history Git remote
      `danielsivyer4567/perfect-planner-desktop`; its default branch is `main`.
- [x] Add a CI workflow that runs frontend build, browser/integration tests, Rust format/tests/lint,
      and the Windows Tauri package build.
- [x] Run a conflict-free integration against the intended base and pass the full local CI mirror.
  - [x] Fetch and verify exact remote heads: `origin/main` and the remote feature branch both point
        to `1f3a183`; product/evidence head `e20673a` is 18 commits ahead and 1 commit behind.
  - [x] Identify the only content conflict as `.gitignore`; Rust formatting and PostCSS changes
        merge automatically.
  - [x] Materialize the latest resolved candidate by retaining the local `.gitignore` superset.
        Tree `489f3000dec4b9fd4e46576eaee1f8d6fcfbc3a1` passed frontend build, all eight browser suites,
        Rust format, 308 active Rust tests, warning-denied Clippy, and both Windows installer
        builds. The only test that cannot pass in a deliberately Git-less tree export requires a
        `.git` checkout identity; its source is unchanged in the candidate and the same test passed
        separately in the real repository, bringing the environment-correct active total to 309.
  - [x] Resolve `.gitignore` by retaining the audited local superset in merge commit `4a4d50e`.
  - [x] Remove the obfuscated network-executing payload discovered in the automatically merged
        remote `postcss.config.js`; add a fail-closed pre-execution security gate.
  - [x] Rerun hosted CI on the final sanitized feature source SHA; Windows run `33145216487`
        passed security, frontend, browser/integration, Rust format/tests/lint, and MSI/NSIS build.
- [x] Commit all intended source, tests, documentation, and required evidence with a clear message.
- [x] Confirm the working tree is clean and record the exact handoff commit SHA in the final report.
- [x] Push the feature branch after broad finish authorization and require the actual CI run to pass
      on the same source SHA. No merge or push to `main` was performed.
- [ ] Confirm branch protection/review requirements and prevent release from an unverified commit.
  - [x] Query branch protection and repository rulesets on the exact private repository.
  - [ ] GitHub returned HTTP 403 because private-repository protection requires an account upgrade;
        enforce this gate after the repository plan supports it or make an explicit alternative policy.

## 6. Close the build-host security incident

- [x] Preserve the exact remote commit, payload location, exposure boundary, containment, and scan
      evidence in `SECURITY-INCIDENT-20260828.md`.
- [x] Reinstall JavaScript dependencies from the lockfile with lifecycle scripts disabled; package
      audit reports zero vulnerabilities.
- [x] Confirm no known loader process, external Node connection, or payload indicator remains live
      or in the final working tree; complete a fresh Defender quick scan with zero new detections.
- [x] Enforce the exact declarative PostCSS allowlist before local dev/build/test/preview/Tauri and
      hosted CI can load the build configuration.
- [ ] Rotate credentials that were accessible to the exposed Windows account from a known-clean
      device; local inspection cannot prove they were not disclosed.
- [ ] Rebuild, sign, and smoke-test the release on a known-clean Windows host or fresh VM.

## 7. Prepare Windows production distribution

- [ ] Obtain and configure an appropriate Windows code-signing identity.
- [ ] Sign the application executable, MSI, and NSIS installer; verify all signatures with
      `Get-AuthenticodeSignature`.
- [ ] Record SHA-256 hashes for the signed artifacts and bind them to the release commit and CI run.
- [x] Decide and document the update strategy: signed in-app updater or controlled manual releases.
- [x] Write release notes, supported Windows/architecture matrix, installation instructions,
      known limitations, support path, and rollback procedure.
- [ ] Perform a clean-machine or faithful Windows VM smoke test using the signed installer.
- [ ] Promote only the exact signed, hash-recorded artifacts that passed the clean-machine smoke.

## Release decision

- [x] No known P0 product or isolation defects remain.
- [x] Integrated sanitized source commit `9d28edf` passes the security gate, frontend build, all eight
      browser suites, Rust format, 309 active Rust tests plus 17 contract tests, warning-denied
      Clippy, MSI/NSIS packaging, and packaged/installed native smoke; hosted Windows CI run
      `33145216487` independently passes the same source and package gates.
- [ ] Required screenshots, console output, native logs, and manual interaction notes are attached.
  - [x] Attach the locally reproducible screenshot, console, geometry, scale-emulation, focus, and
        native event-hash evidence described in `NATIVE-EVIDENCE.md`.
  - [x] Attach packaged routing, product launch/exit, and physical Windows keyboard evidence.
  - [ ] Attach a Windows OS-level 300% display-scale pass on the target display.
- [x] CI passes on the exact release source commit.
- [x] The working tree is clean and every intended file is committed.
- [ ] Signed installers pass clean-machine installation and runtime smoke tests.
- [ ] Remaining limitations are documented and accepted explicitly.

Only after all seven release-decision checks are complete may the result be labelled
**production-ready**.
