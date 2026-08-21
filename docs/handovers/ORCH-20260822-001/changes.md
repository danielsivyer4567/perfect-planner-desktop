# Changes — ORCH-20260822-001

| Desired change | Actual implementation | Status |
|---|---|---|
| Durable canonical audit events | Process-safe `events.jsonl`, exact event schema, bounded tail and torn-line recovery | Succeeded |
| Prevent workers touching the same files | Repository/run manifests, per-node allowed files, exclusive leases and stale-token fencing | Succeeded |
| Safe parallel worker recovery | Heartbeats, one alarm per fence, preserved evidence, reassignment and bounded retries | Succeeded |
| Evidence at every UI stage | UI profile requires before/after screenshot plus Git diff; command outputs include exit evidence | Succeeded |
| Avoid screenshot pressure for headless work | Evidence profiles reject unnecessary OCR/screenshot tax for headless nodes | Succeeded |
| Planned work versus committed reality | Exact reconciliation tags/manifests/outputs and named auditable waivers | Succeeded |
| Block unsafe release/merge | Fail-closed release state distinguishes conflicts, proof gaps, CI code failure and CI infrastructure failure | Succeeded |
| Durable gate history after restart | Atomic versioned preflight/reconciliation/release result files and strict reload | Succeeded |
| Separate repositories and completed runs | Capped, containment-checked repository catalogue and distinct active/completed shelves | Succeeded |
| Head orchestrator and worker reporting | Existing durable control plane plus pipeline identity, workers, leases, warnings and decisions | Succeeded |
| Draggable bottom audit log | Pointer/keyboard resize, double-click/maximize, collapse and exactly two audit tabs | Succeeded |
| Desired versus actual audit view | Desired left column, actual committed right column and explicit missing/unplanned states | Succeeded |
| Clean handover and disaster recovery | Completion report, changes, leftovers, archive and append-only checklist pointer | Succeeded |
| Automatic exact-process stop button | Withheld until the app owns a durable exact process identity registry | Not shipped — safety gate |
| Automatic hardware-based scaling to 100 workers | Scheduler primitives shipped; adaptive admission/load proof remains explicit follow-up | Not shipped — capacity policy |
| GitHub push/PR/CI/merge | No remote operation was authorized | Not performed |
