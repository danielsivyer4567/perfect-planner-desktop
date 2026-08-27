# Native lifecycle and interaction evidence

Date: 2026-08-27
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
- Real Windows Escape injection was not observed by the WebView DOM. The DOM Escape behavior and
  focus return pass, but physical input remains an explicit manual limitation.

Local artifact hashes:

| Artifact | SHA-256 |
|---|---|
| `artifacts/native-tauri/native-finish-line.json` | `42DBEDAF9414F34AA123196C50CF5990E1936D3E1561F3D24EAF1B7731D40FC8` |
| `artifacts/native-tauri/native-finish-line.png` | `517E380405CEB4FF279989CCB048B1D2D05478C9FA840EB937BA8A706FFFF2D7` |
| `artifacts/native-tauri/native-narrow-window.png` | `9B0711EF3CDA5BD6C73A69727E7B89DFB125A66351F769E04019D197266FC3BB` |
| `artifacts/native-tauri/native-three-times-scale.png` | `446F0A1AD89DA00BB555953F18B5C287D2CA63E4814C8AB4C61741EBE9097BF3` |

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

- A packaged run with an actual registered messaging destination and a durable routed-message log.
- An application-native launch/exit event pair. Installer logs and zero-orphan-process checks prove
  process outcomes, but they are not substitutes for product-native lifecycle events.
- Physical keyboard Escape/Tab behavior and a Windows OS-level 300% display-scale pass.
- A signed candidate installed on a separate clean Windows machine or faithful VM.
