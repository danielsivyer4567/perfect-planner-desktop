# Perfect Planner 1.0.0 — Windows distribution notes

Status: local release candidate; not production-promotable until signed artifacts pass hosted CI
and a clean-machine smoke test.

## Release notes

Perfect Planner 1.0.0 provides a repository-isolated Tauri workspace for Perfect Planning boards
and a native orchestration lifecycle. The finish pass adds exact repository/plan restoration,
fail-closed missing selection, same-port foreign-board rejection, stale recovery classification,
actionable recovery-delivery status, compact canvas-first layout verification, Windows CI, and
packaged restart/installer regression coverage.

The critical safety correction prevents a durable recovery event from being written to any board
other than the explicitly selected repository and plan held by the original worker session.

## Supported platform matrix

| Platform | Architecture | Package | Local status |
|---|---|---|---|
| Windows 11 | x64 | NSIS per-user installer | Install, launch, reinstall, uninstall, data retention, and no-orphan-process checks passed. |
| Windows 11 | x64 | MSI installer | Install, launch, reconfigure, uninstall, data retention, and no-orphan-process checks passed. |
| Windows 10 | x64 | MSI/NSIS | Expected from Tauri/WebView2 support; not tested in this finish pass. |
| Windows ARM64 | ARM64 | none | Not built or supported by this candidate. |
| macOS/Linux | any | none | Out of scope for this Windows desktop release. |

Microsoft Edge WebView2 is required. The installed app stores durable native state under
`%APPDATA%\com.looplet.perfectplanner\` and discovers only bounded loopback Perfect Planning board
ports.

## Installation

Do not distribute the current unsigned candidates. After signing and hash verification, choose one
approved x64 package:

- MSI: `Perfect Planner_1.0.0_x64_en-US.msi`
- NSIS: `Perfect Planner_1.0.0_x64-setup.exe`

Run the signed installer as the intended Windows user, launch Perfect Planner, select the required
repository and plan, close the app, and relaunch it. The exact repository and plan must return. If
the saved identity no longer exists, the app must show “Saved plan unavailable” and must not select
another repository.

## Update strategy

Version 1.0.0 uses **controlled manual releases**. No in-app updater is enabled. Each update must:

1. pass the repository CI workflow on the exact release commit;
2. be signed with the approved Windows code-signing identity;
3. have SHA-256 hashes recorded after signing;
4. pass install/reinstall/uninstall and clean-machine smoke tests;
5. be promoted without rebuilding or modifying the tested artifacts.

A signed Tauri updater may be introduced later as a separately planned security feature. Until
then, the app must never download or apply an update automatically.

## Known limitations

- The exact private GitHub remote and `main` branch are configured, but hosted CI has not run because
  no push was authorized. GitHub branch protection/rulesets are unavailable on the current private
  repository account plan.
- No Windows code-signing identity is installed; current binaries report `NotSigned`.
- A physical native Escape event was not observed through WebView2 during automation. DOM focus
  return, browser keyboard navigation, and native narrow-window layout pass, but a human keyboard
  and 300% Windows display-scale pass remains required.
- No clean Windows VM smoke test has been performed.
- Windows 10 and ARM64 are not verified.

## Support and diagnostics

Before rollout, replace this local handoff path with the real support/repository URL. For this
candidate, diagnostics are available in the orchestration inspector and under
`%APPDATA%\com.looplet.perfectplanner\`. Preserve ledgers and installer logs when reporting a
failure; do not edit them to make a state appear healthy.

## Rollback

1. Stop Perfect Planner and confirm no `perfect-planner-desktop.exe` process remains.
2. Uninstall the current version using Windows Installed Apps or its signed installer.
3. Preserve `%APPDATA%\com.looplet.perfectplanner\`; uninstall verification confirmed the durable
   user data is retained.
4. Install the last signed, hash-recorded version that passed clean-machine smoke.
5. Launch and confirm the exact saved repository/plan identity. If it is unavailable, select the
   intended plan explicitly; never copy or rewrite plan state to force a match.
6. If rollback changes the durable schema or cannot read existing state, stop and escalate rather
   than deleting app data.
