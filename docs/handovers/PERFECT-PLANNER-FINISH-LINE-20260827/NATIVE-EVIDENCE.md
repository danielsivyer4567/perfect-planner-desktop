# Native lifecycle and interaction evidence

Date: 2026-08-28
Repository: `C:\repos\perfect-planner-tauri`
Target: installed Perfect Planner 1.0.0, Windows 11 x64, WebView2

## Packaged interaction proof

`python tests/native_finish_line_e2e.py` passed against the installed Tauri WebView. The proof is
machine-local and is intentionally ignored by Git because it contains absolute paths and runtime
geometry.

- Exact selection: `Perfect Planner / PP-002 / Cross-repository Collision Assessor`.
- Plan SHA-256 stayed
  `B82946943403BE379FC93306DCBFF166D33830AC46FE8A6FB91A8DE07286F7D1`.
- Session-reaper SHA-256 stayed
  `61672959B6B04F4BA7E5F6556FD17FE7AA46EAAD115BB40B0207385810618211`.
- Recovery remained exactly one event: `pp-reaper-1787422242997-23` for B20.
- Header height was about 53 CSS px and the workflow canvas began at about 130 CSS px.
- The 900x700 native window had no horizontal document overflow.
- A 3456x2058 physical viewport represented at device scale factor 3 produced a 1152x686 CSS
  viewport with no horizontal overflow in the Perfect Planner shell. The embedded plan remains
  horizontally pannable at that scale. This is a WebView device-scale emulation, not a claim that
  Windows display settings were physically changed.
- Closing the inspector returned focus to `Inspect`; the following Tab moved focus to the first
  repository tab (`pp-btn-repository-tab-pp-repository-tab-1ezujbo`).
- Console errors, page errors, unexpected request failures, and CSP violations were empty. The
  expected WebView2 IPC `ERR_ABORTED` receipt remained separately classified.
- A later installed-candidate run sent real Windows Escape and Tab events to the foreground Tauri
  window. Escape closed the inspector and returned focus to `Inspect`; Tab then advanced to a
  repository button. The external board was not clicked or mutated.

Local artifact hashes:

| Artifact | SHA-256 |
|---|---|
| `artifacts/native-tauri/native-finish-line.json` | `42DBEDAF9414F34AA123196C50CF5990E1936D3E1561F3D24EAF1B7731D40FC8` |
| `artifacts/native-tauri/native-finish-line.png` | `517E380405CEB4FF279989CCB048B1D2D05478C9FA840EB937BA8A706FFFF2D7` |
| `artifacts/native-tauri/native-narrow-window.png` | `9B0711EF3CDA5BD6C73A69727E7B89DFB125A66351F769E04019D197266FC3BB` |
| `artifacts/native-tauri/native-three-times-scale.png` | `446F0A1AD89DA00BB555953F18B5C287D2CA63E4814C8AB4C61741EBE9097BF3` |

## Packaged routing, process lifecycle, and physical keyboard proof

`python tests/native_release_evidence_e2e.py` passed against the installed NSIS candidate at
`C:\Users\danie\AppData\Local\Perfect Planner\perfect-planner-desktop.exe`.

- Installed executable SHA-256:
  `9E9A9156D0DE65CC9F324CC17641D6A318F757E760A4597A8E9A63C7695A1183`.
- Synthetic proof message `pp-message-1787883016705-5` used the dedicated repository ID
  `pp-finish-line-native-evidence`; it progressed exactly
  `queued -> claimed -> delivered -> acknowledged` through the native Tauri IPC boundary.
- The message was labelled synthetic evidence and did not represent or mutate live project work.
- Process `39004` recorded correlated session `pp-app-1787883015005-39004` with exactly one
  `LAUNCH` and one later `EXIT` event in the product-owned append-only lifecycle ledger.
- The physical keyboard proof observed exactly one Windows Escape event, closed the inspector,
  restored focus, and advanced focus with Windows Tab to a repository button.
- The installed executable manifest contains `PerMonitorV2, PerMonitor`; the running process
  reported `PER_MONITOR_DPI_AWARE`. The active monitor was physically 96 DPI/100%, and the WebView
  reported device pixel ratio 1. A 288 DPI/300% target-monitor pass remains unclaimed.
- Console errors, page errors, and unexpected request failures were all empty.

| Artifact or ledger | SHA-256 |
|---|---|
| `artifacts/native-tauri/native-routed-message-lifecycle.json` | `91A48DA9D6E4E61033793FC1B039DBA5178E79C31F52C74E1BC1493761CD51F2` |
| `artifacts/native-tauri/native-routed-message-lifecycle.png` | `A638BF71B732986BB5B796C45F5F040E064DA2457776BD2FEC96E63982125108` |
| `artifacts/native-tauri/native-physical-keyboard.png` | `A638BF71B732986BB5B796C45F5F040E064DA2457776BD2FEC96E63982125108` |
| `artifacts/native-tauri/installed-app.manifest.xml` | `8D0777C126B337DE5F030315E3C4B0A189781099EF54FF155C0EFA2D7B432CE9` |
| `%APPDATA%/com.looplet.perfectplanner/app-lifecycle.jsonl` after proof | `37F9CABFDDF56B74A08D7FC76896872BD24BCD2CF085360583F023D941BF8687` |
| `%APPDATA%/com.looplet.perfectplanner/control-plane.jsonl` after proof | `5A303635806DADB823EC8FC178058027C240438ABAC3DB55F266237086CAF3FC` |

The screenshot hashes are identical because the routing operation does not invent visible live-work
state and the second capture retained the same focused, inspector-closed workspace.

## Native lifecycle event proof

The real native demonstration run `run-mt4oo7z4` contains the following durable event sequence:

```text
2026-08-23 04:33:43 +10:00  gate-pass   explicit whole-plan approval
2026-08-23 04:33:45 +10:00  claim       A01 authority-backed worker admitted
2026-08-23 04:34:14 +10:00  reassign    A01 stale or expired lease recovered
2026-08-23 05:10:16 +10:00  preflight   host inspected without process termination
2026-08-23 05:10:17 +10:00  gate-pass   explicit whole-plan approval
2026-08-23 05:10:18 +10:00  claim       A01 authority-backed worker admitted
2026-08-23 05:10:20 +10:00  heartbeat   A01 authority-backed worker heartbeat
2026-08-23 05:10:20 +10:00  heartbeat   native completion verification window opened
2026-08-23 05:10:22 +10:00  node-done   A01 fenced completion persisted
```

The completed hot-resume record names A01 as the last completed step and has no next actions. The
scheduler contains the evidence-gated completion receipt and document-diff, command-output, and
exit-code artifacts.

| Native record | SHA-256 |
|---|---|
| `run-mt4oo7z4/events.jsonl` | `CF198FB1EF19DD5AB3EC6EB8EAED5EB659DE24DC5403414A558F88C4D7A8A60A` |
| `run-mt4oo7z4/hot-resume.json` | `65E273355A878772011E0E641EA7F60045D4BC3A09F6BA7EE4BB3993FF7C524E` |
| `run-mt4oo7z4/scheduler.json` | `EEE7C85FC1D10E2ECE457BD93D755321579A41598E76D246C8C12879A160258D` |

These records live under the ignored native-demo target and are local runtime proof, not promoted
release artifacts.

## Evidence still missing

- A Windows OS-level 300% display-scale pass on the target display; WebView 300% emulation is proven
  but is not a substitute for changing the operating-system setting.
- A signed candidate installed on a separate clean Windows machine or faithful VM.
