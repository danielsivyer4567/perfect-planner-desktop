# Perfect Planner — Finish-Line Todo

Status snapshot: 2026-08-27
Repository: `C:\repos\perfect-planner-tauri`
Branch: `feature/tauri-orchestrator-messaging-20260821-223935`
Current commit: `aa1ae3365304a4778411775b2f85611bd53c6e0b`

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

## 2. Reconcile the current working tree

- [x] Review all tracked modifications and classify each as intended product work, generated
      planner state, or unrelated pre-existing work.
- [x] Review all untracked evidence artifacts and bind retained proof to the exact plan item and
      source commit. Preserve evidence; do not delete or rewrite the append-only journal.
- [x] Decide which evidence is release documentation and which is local runtime output; update
      ignore rules without hiding required audit evidence.
- [x] Run `git diff --check` and review the complete diff for accidental cross-repository paths,
      secrets, unsafe process control, stale assumptions, and generated noise.
- [ ] Confirm no file outside `C:\repos\perfect-planner-tauri` was changed by this finish pass.

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
- [ ] Record native logs for launch, preflight, routing, interruption recovery, completion, and exit.
- [ ] Manually exercise keyboard navigation, visible focus, modal close/return-focus, narrow-window
      layout, and three-times display scaling.
- [x] Install both MSI and NSIS candidates on Windows x64; verify launch, upgrade/reinstall,
      uninstall, retained user data, and no orphan background processes.

## 5. Establish the real release gate

- [ ] Configure and verify the intended Git remote and default branch; do not guess either.
- [x] Add a CI workflow that runs frontend build, browser/integration tests, Rust format/tests/lint,
      and the Windows Tauri package build.
- [ ] Run a clean simulated merge against the intended base and pass the full local CI mirror.
- [x] Commit all intended source, tests, documentation, and required evidence with a clear message.
- [x] Confirm the working tree is clean and record the exact handoff commit SHA in the final report.
- [ ] Push only when explicitly authorized, then require the actual CI run to pass on that same SHA.
- [ ] Confirm branch protection/review requirements and prevent release from an unverified commit.

## 6. Prepare Windows production distribution

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
- [x] All local verification commands pass on the exact handoff commit.
- [ ] Required screenshots, console output, native logs, and manual interaction notes are attached.
- [ ] CI passes on the exact release commit.
- [x] The working tree is clean and every intended file is committed.
- [ ] Signed installers pass clean-machine installation and runtime smoke tests.
- [ ] Remaining limitations are documented and accepted explicitly.

Only after all seven release-decision checks are complete may the result be labelled
**production-ready**.
