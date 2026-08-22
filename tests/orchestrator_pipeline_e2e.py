"""Chrome proof contract for the Perfect Orchestrator pipeline console.

The UI consumes the recursively frozen ``window.__ORCHESTRATOR_PIPELINE__`` fixture before
React starts. The contract intentionally uses durable IDs for primary controls and data
attributes for repeated records:

* ``#pp-orch-pipeline-console`` owns the selected repository via
  ``data-repository-id``.
* run cards use ``[data-run-id][data-plan-id][data-repository-id]`` inside
  ``#pp-orch-shelf-active`` or ``#pp-orch-shelf-completed``.
* node, evidence, reconciliation, release, and audit records expose their stable IDs/kinds
  through the data attributes asserted below.
* the audit drawer uses ``#pp-orch-region-audit-drawer`` and
  ``#pp-orch-control-audit-resize``; dragging the
  handle changes its height and double-clicking sets ``data-state=\"maximized\"``.

If the production UI uses a different selector, align it to this explicit contract instead of
weakening the proof with text-only or positional selectors.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

from playwright.sync_api import Page, expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
APP_URL = os.environ.get("PP_APP_URL", "http://127.0.0.1:5180/")
SCREENSHOT = (
    ROOT
    / "artifacts"
    / "orchestrator-pipeline"
    / "orchestrator-pipeline-chrome.png"
)

PRIMARY_REPOSITORY = "repo-looplet-crm"
SECONDARY_REPOSITORY = "repo-looplet-accounting"
ACTIVE_RUN = "ORCH-20260822-ACTIVE"
COMPLETED_RUN = "ORCH-20260821-COMPLETE"
ACTIVE_PLAN = "PP-TO"
COMPLETED_PLAN = "PP-ARCHIVE"


PIPELINE_FIXTURE = {
    "schemaVersion": 1,
    "generatedAt": "2026-08-22T01:30:00Z",
    "selectedRepositoryId": PRIMARY_REPOSITORY,
    "repositories": [
        {
            "organizationId": "org-looplet",
            "repositoryId": PRIMARY_REPOSITORY,
            "repositoryCallSign": "A",
            "repositoryName": "Looplet CRM",
            "repositoryRoot": r"C:\repos\looplet-crm",
            "branch": "feature/perfect-orchestrator-pipeline",
        },
        {
            "organizationId": "org-looplet",
            "repositoryId": SECONDARY_REPOSITORY,
            "repositoryCallSign": "B",
            "repositoryName": "Looplet Accounting",
            "repositoryRoot": r"C:\repos\looplet-accounting",
            "branch": "main",
        },
    ],
    "runs": [
        {
            "repositoryId": PRIMARY_REPOSITORY,
            "runId": ACTIVE_RUN,
            "planId": ACTIVE_PLAN,
            "topic": "Perfect Orchestrator standalone pipeline",
            "branch": "feature/perfect-orchestrator-pipeline",
            "status": "decision-required",
            "createdAt": "2026-08-22T00:00:00Z",
            "updatedAt": "2026-08-22T01:29:00Z",
            "preflight": {
                "disposition": "decision-required",
                "reasons": [
                    "Port 5180 is owned by an allowlisted Vite process; explicit stop decision required."
                ],
                "conflicts": [
                    {
                        "port": 5180,
                        "address": "127.0.0.1",
                        "process": {
                            "pid": 14200,
                            "executablePath": r"C:\Program Files\nodejs\node.exe",
                            "startedAtEpochMs": 1787348700000,
                            "commandLine": "node vite --port 5180",
                        },
                    }
                ],
                "unknownConflicts": [],
                "resources": {
                    "logicalCpuCount": 16,
                    "cpuUsagePercent": 22.5,
                    "totalMemoryBytes": 34359738368,
                    "availableMemoryBytes": 21474836480,
                    "repositoryDiskAvailableBytes": 536870912000,
                },
            },
            "decision": {
                "decisionId": "decision-preflight-5180",
                "kind": "preflight-process-conflict",
                "status": "pending",
                "message": "Choose whether to stop the exact allowlisted Vite process on port 5180.",
                "requiredAt": "2026-08-22T00:01:00Z",
            },
            "nodes": [
                {
                    "nodeId": "TO-02",
                    "title": "Preflight and isolated run scope",
                    "status": "running",
                    "worker": "worker-A-02",
                    "attempts": 2,
                    "dependsOn": ["TO-01"],
                    "allowedFiles": [
                        "src-tauri/src/orchestrator/preflight.rs",
                        "src-tauri/src/orchestrator/run_scope.rs",
                    ],
                    "lease": {
                        "nodeId": "TO-02",
                        "worker": "worker-A-02",
                        "token": "lease-TO-02-fence-0002",
                        "expiresAt": "2026-08-22T01:35:00Z",
                    },
                    "evidence": [
                        {
                            "evidenceId": "ev-TO-02-before",
                            "nodeId": "TO-02",
                            "kind": "screenshot",
                            "phase": "before",
                            "path": "evidence/TO-02/before.png",
                            "sha256": "a" * 64,
                            "capturedAt": "2026-08-22T00:02:00Z",
                        },
                        {
                            "evidenceId": "ev-TO-02-diff",
                            "nodeId": "TO-02",
                            "kind": "git-diff",
                            "phase": "verification",
                            "path": "evidence/TO-02/change.diff",
                            "sha256": "b" * 64,
                            "capturedAt": "2026-08-22T00:28:00Z",
                        },
                        {
                            "evidenceId": "ev-TO-02-test",
                            "nodeId": "TO-02",
                            "kind": "command-output",
                            "phase": "verification",
                            "path": "evidence/TO-02/cargo-test.log",
                            "sha256": "c" * 64,
                            "capturedAt": "2026-08-22T00:29:00Z",
                            "command": "cargo test orchestrator::preflight",
                            "exitCode": 0,
                        },
                    ],
                },
                {
                    "nodeId": "TO-05",
                    "title": "Exact reconciliation gate",
                    "status": "gate-failed",
                    "worker": "worker-A-05",
                    "attempts": 1,
                    "allowedFiles": ["src-tauri/src/orchestrator/reconcile.rs"],
                    "dependsOn": ["TO-01"],
                    "lease": {
                        "nodeId": "TO-05",
                        "worker": "worker-A-05",
                        "token": "lease-TO-05-fence-0001",
                        "expiresAt": "2026-08-22T01:36:00Z",
                    },
                    "evidence": [
                        {
                            "evidenceId": "ev-TO-05-test",
                            "nodeId": "TO-05",
                            "kind": "command-output",
                            "phase": "verification",
                            "path": "evidence/TO-05/reconcile-tests.log",
                            "sha256": "d" * 64,
                            "capturedAt": "2026-08-22T01:20:00Z",
                            "command": "cargo test reconcile",
                            "exitCode": 0,
                        }
                    ],
                },
            ],
        },
        {
            "repositoryId": PRIMARY_REPOSITORY,
            "runId": COMPLETED_RUN,
            "planId": COMPLETED_PLAN,
            "topic": "Durable control-plane baseline",
            "branch": "feature/tauri-orchestrator-messaging-20260821-223935",
            "status": "completed",
            "createdAt": "2026-08-21T02:00:00Z",
            "updatedAt": "2026-08-21T05:00:00Z",
            "preflight": {"disposition": "ready", "reasons": [], "conflicts": []},
            "decision": None,
            "nodes": [
                {
                    "nodeId": "A01",
                    "title": "Durable orchestrator control plane",
                    "status": "done",
                    "worker": "worker-A-legacy",
                    "attempts": 1,
                    "lease": None,
                    "allowedFiles": ["src-tauri/src/control_plane.rs"],
                    "dependsOn": [],
                    "evidence": [
                        {
                            "evidenceId": "ev-A01-after",
                            "nodeId": "A01",
                            "kind": "screenshot",
                            "phase": "after",
                            "path": "evidence/A01/after.png",
                            "sha256": "e" * 64,
                            "capturedAt": "2026-08-21T04:45:00Z",
                        }
                    ],
                }
            ],
        },
    ],
    "reconciliations": [
        {
            "reconciliationId": "reconcile-active-001",
            "repositoryId": PRIMARY_REPOSITORY,
            "runId": ACTIVE_RUN,
            "planId": ACTIVE_PLAN,
            "passed": False,
            "rows": [
                {
                    "rowId": "reconcile-missing-TO-03",
                    "status": "missing",
                    "category": "unproven",
                    "violationId": "UNPROVEN:NO_TAGGED_COMMIT:TO-03",
                    "desired": {
                        "nodeId": "TO-03",
                        "change": "Lease scheduler with bounded parallel waves",
                        "manifest": ["src-tauri/src/orchestrator/scheduler.rs"],
                    },
                    "actual": None,
                    "waivedBy": [],
                },
                {
                    "rowId": "reconcile-unplanned-app-css",
                    "status": "unplanned",
                    "category": "unplanned",
                    "violationId": "UNPLANNED:OUTSIDE_MANIFEST:f00baa:TO-05:src_index.css",
                    "desired": None,
                    "actual": {
                        "commitId": "f00baa7",
                        "tag": "[PP-TO/TO-05]",
                        "file": "src/index.css",
                        "change": "Unmanifested visual adjustment",
                    },
                    "waivedBy": [],
                },
            ],
        }
    ],
    "releaseGates": [
        {
            "releaseGateId": "release-active-001",
            "repositoryId": PRIMARY_REPOSITORY,
            "runId": ACTIVE_RUN,
            "planId": ACTIVE_PLAN,
            "readyForPr": False,
            "readyToMerge": False,
            "merged": False,
            "issues": [
                {
                    "issueId": "release-infra-001",
                    "kind": "CI_INFRASTRUCTURE_FAILURE",
                    "message": "CI infrastructure failure - decision required",
                    "decisionRequired": True,
                }
            ],
        }
    ],
    "auditEvents": [
        {
            "eventId": "audit-preflight-001",
            "ts": "2026-08-22T00:01:00Z",
            "repositoryId": PRIMARY_REPOSITORY,
            "planId": ACTIVE_PLAN,
            "runId": ACTIVE_RUN,
            "nodeId": None,
            "worker": "head-orchestrator-A",
            "type": "decision-required",
            "msg": "Preflight process decision required for port 5180",
        },
        {
            "eventId": "audit-claim-TO-02",
            "ts": "2026-08-22T00:02:00Z",
            "repositoryId": PRIMARY_REPOSITORY,
            "planId": ACTIVE_PLAN,
            "runId": ACTIVE_RUN,
            "nodeId": "TO-02",
            "worker": "worker-A-02",
            "type": "claim",
            "msg": "TO-02 lease claimed with fence 0002",
        },
        {
            "eventId": "audit-evidence-TO-02",
            "ts": "2026-08-22T00:29:00Z",
            "repositoryId": PRIMARY_REPOSITORY,
            "planId": ACTIVE_PLAN,
            "runId": ACTIVE_RUN,
            "nodeId": "TO-02",
            "worker": "worker-A-02",
            "type": "evidence",
            "msg": "Cargo test output captured with exit code 0",
        },
        {
            "eventId": "audit-release-warning-001",
            "ts": "2026-08-22T01:25:00Z",
            "repositoryId": PRIMARY_REPOSITORY,
            "planId": ACTIVE_PLAN,
            "runId": ACTIVE_RUN,
            "nodeId": None,
            "worker": "head-orchestrator-A",
            "type": "warning",
            "msg": "CI infrastructure failure - decision required",
        },
        {
            "eventId": "audit-run-done-archive",
            "ts": "2026-08-21T05:00:00Z",
            "repositoryId": PRIMARY_REPOSITORY,
            "planId": COMPLETED_PLAN,
            "runId": COMPLETED_RUN,
            "nodeId": None,
            "worker": "head-orchestrator-A",
            "type": "run-done",
            "msg": "Durable control-plane baseline completed",
        },
    ],
}


def pipeline_snapshot() -> dict:
    """Adapt the readable source facts to the typed orchestrator_snapshot response."""

    active = PIPELINE_FIXTURE["runs"][0]
    completed = PIPELINE_FIXTURE["runs"][1]

    def summary(run: dict) -> dict:
        nodes = run["nodes"]
        return {
            "organizationId": "org-looplet",
            "repositoryId": run["repositoryId"],
            "repositoryRoot": r"C:\repos\looplet-crm",
            "worktreePath": (
                r"C:\repos\looplet-worktrees\perfect-orchestrator-pipeline"
            ),
            "branch": run["branch"],
            "runId": run["runId"],
            "planId": run["planId"],
            "title": run["topic"],
            "status": run["status"],
            "completedNodes": sum(node["status"] == "done" for node in nodes),
            "totalNodes": len(nodes),
            "updatedAt": run["updatedAt"],
        }

    active_summary = summary(active)
    completed_summary = summary(completed)
    scheduled_nodes = {}
    for node in active["nodes"]:
        evidence = []
        for artifact in node["evidence"]:
            kind = artifact["kind"]
            if kind == "screenshot":
                kind = "before-screenshot"
            evidence.append(
                {
                    "kind": kind,
                    "path": artifact["path"],
                    "sha256": artifact["sha256"],
                    "bytes": 8192,
                }
            )
        lease = node["lease"]
        scheduled_nodes[node["nodeId"]] = {
            "id": node["nodeId"],
            "title": node["title"],
            "wave": 2 if node["nodeId"] == "TO-02" else 5,
            "dependsOn": node["dependsOn"],
            "attempts": node["attempts"],
            "status": "RUNNING" if node["status"] == "running" else "BLOCKED",
            "lease": {
                "nodeId": lease["nodeId"],
                "workerId": lease["worker"],
                "token": lease["token"],
                "fence": node["attempts"],
                "expiresAtMs": 1787355300000,
            },
            "stallAlarmFence": None,
            "profile": "ui" if node["nodeId"] == "TO-02" else "headless",
            "evidence": evidence,
            "allowedFiles": node["allowedFiles"],
            "verification": [
                {
                    "commandId": f"verify-{node['nodeId']}",
                    "exitCode": 0,
                    "outputArtifact": f"evidence/{node['nodeId']}/test.log",
                }
            ],
        }

    source_reconciliation = PIPELINE_FIXTURE["reconciliations"][0]
    violations = {"unplanned": [], "unproven": [], "orphaned": [], "fatal": []}
    changes = []
    for row in source_reconciliation["rows"]:
        category = row["category"]
        desired = row["desired"]
        actual = row["actual"]
        violation = {
            "violationId": row["violationId"],
            "category": category.upper(),
            "summary": (
                f"Desired change is missing for {desired['nodeId']}"
                if desired
                else f"Actual change {actual['file']} was not planned"
            ),
            "planId": ACTIVE_PLAN,
            "nodeId": desired["nodeId"] if desired else None,
            "commitId": actual["commitId"] if actual else None,
            "file": actual["file"] if actual else desired["manifest"][0],
            "waivedBy": row["waivedBy"],
        }
        violations[category].append(violation)
        changes.append(
            {
                "id": row["rowId"],
                "nodeId": desired["nodeId"] if desired else None,
                "desired": desired["change"] if desired else "Not planned",
                "actualCommit": actual["commitId"] if actual else None,
                "status": row["status"],
                "details": (
                    desired["manifest"]
                    if desired
                    else [actual["file"], actual["change"]]
                ),
            }
        )

    release = PIPELINE_FIXTURE["releaseGates"][0]
    release_issues = [
        {
            "kind": issue["kind"],
            "message": issue["message"],
            "decisionRequired": issue["decisionRequired"],
        }
        for issue in release["issues"]
    ]
    audit_events = []
    for event in PIPELINE_FIXTURE["auditEvents"]:
        audit_events.append(
            {
                "ts": event["ts"],
                "runId": event["runId"],
                "nodeId": event["nodeId"],
                "worker": event["worker"],
                "type": event["type"],
                "msg": event["msg"],
                "data": {
                    "eventId": event["eventId"],
                    "repositoryId": event["repositoryId"],
                    "planId": event["planId"],
                },
            }
        )

    return {
        "nowMs": 1787355000000,
        "run": active_summary,
        "stages": [
            {
                "id": "preflight",
                "label": "System preflight",
                "status": "blocked",
                "summary": "Exact process decision required",
            },
            {
                "id": "execution",
                "label": "Parallel execution",
                "status": "running",
                "summary": "Two leased nodes retain evidence",
            },
            {
                "id": "reconciliation",
                "label": "Exact reconciliation",
                "status": "failed",
                "summary": "One missing and one unplanned change",
            },
            {
                "id": "release",
                "label": "Release gate",
                "status": "blocked",
                "summary": "Infrastructure decision required",
            },
        ],
        "preflight": {
            "disposition": "decisionRequired",
            "baseline": {
                "repositoryRoot": r"C:\repos\looplet-crm",
                "gitStatusPorcelainV2": "",
                "portBindings": active["preflight"]["conflicts"],
                "resources": active["preflight"]["resources"],
            },
            "conflicts": active["preflight"]["conflicts"],
            "unknownConflicts": [],
            "stoppedProcesses": [],
            "reasons": active["preflight"]["reasons"],
        },
        "scheduler": {"nextFence": 3, "nodes": scheduled_nodes},
        "reconciliation": {
            "passed": False,
            **violations,
            "waivers": [],
        },
        "changes": changes,
        "release": {
            "readyForPr": release["readyForPr"],
            "readyToMerge": release["readyToMerge"],
            "merged": release["merged"],
            "issues": release_issues,
        },
        "delivery": None,
        "warnings": [
            {
                "id": "decision-preflight-5180",
                "severity": "critical",
                "message": active["decision"]["message"],
                "decisionRequired": True,
                "nodeId": None,
                "createdAt": active["decision"]["requiredAt"],
            },
            {
                "id": "release-infra-001",
                "severity": "warning",
                "message": release_issues[0]["message"],
                "decisionRequired": True,
                "nodeId": None,
                "createdAt": "2026-08-22T01:25:00Z",
            },
        ],
        "events": audit_events,
        "activeRuns": [active_summary],
        "completedRuns": [completed_summary],
    }


def frozen_fixture_script() -> str:
    client_fixture = {
        "version": 1,
        "responses": {
            "orchestrator_snapshot": {"ok": True, "value": pipeline_snapshot()}
        },
    }
    payload = json.dumps(client_fixture, separators=(",", ":"))
    return f"""
(() => {{
  const deepFreeze = (value) => {{
    if (!value || typeof value !== "object" || Object.isFrozen(value)) return value;
    for (const child of Object.values(value)) deepFreeze(child);
    return Object.freeze(value);
  }};
  Object.defineProperty(window, "__ORCHESTRATOR_PIPELINE__", {{
    value: deepFreeze({payload}),
    writable: false,
    configurable: false,
    enumerable: true,
  }});
}})();
"""


def assert_unique_control_ids(page: Page) -> None:
    control_audit = page.locator(
        "button, a[href], input, select, textarea, summary, [role='button']"
    ).evaluate_all(
        """controls => ({
          total: controls.length,
          missing: controls
            .filter(control => !control.id)
            .map(control => control.outerHTML.slice(0, 180)),
          duplicateIds: [...new Set(controls.map(control => control.id).filter(Boolean))]
            .filter(id => document.querySelectorAll(`[id="${CSS.escape(id)}"]`).length !== 1),
        })"""
    )
    assert control_audit["total"] > 0, "pipeline console rendered no interactive controls"
    assert not control_audit["missing"], (
        f"every interactive control needs a stable ID: {control_audit['missing']}"
    )
    assert not control_audit["duplicateIds"], (
        f"control IDs must be document-unique: {control_audit['duplicateIds']}"
    )


def assert_repository_and_plan_scope(page: Page) -> None:
    root = page.locator("#pp-orch-pipeline-console")
    expect(root).to_have_attribute("data-repository-id", PRIMARY_REPOSITORY)

    active_section = page.locator("#pp-orch-shelf-active")
    completed_section = page.locator("#pp-orch-shelf-completed")
    active = active_section.locator(f"[data-run-id='{ACTIVE_RUN}']")
    completed = completed_section.locator(f"[data-run-id='{COMPLETED_RUN}']")
    expect(active).to_have_attribute("data-plan-id", ACTIVE_PLAN)
    expect(active).to_have_attribute("data-repository-id", PRIMARY_REPOSITORY)
    expect(active).to_have_attribute("data-run-status", "decision-required")
    expect(completed).to_have_attribute("data-plan-id", COMPLETED_PLAN)
    expect(completed).to_have_attribute("data-repository-id", PRIMARY_REPOSITORY)
    expect(completed).to_have_attribute("data-run-status", "completed")
    assert active.locator(f"[data-run-id='{COMPLETED_RUN}']").count() == 0
    assert completed.locator(f"[data-run-id='{ACTIVE_RUN}']").count() == 0

    expect(active).to_be_visible()
    expect(completed).to_be_visible()

    # Every run rendered under this selected repository must carry the same explicit scope;
    # matching plan numbers in another repository cannot leak into this list.
    for run in page.locator("[data-run-id]").all():
        expect(run).to_have_attribute("data-repository-id", PRIMARY_REPOSITORY)


def assert_node_evidence_and_persistent_warnings(page: Page) -> None:
    decision = page.locator("#pp-orch-warning-decision-preflight-5180")
    expect(decision).to_be_visible()
    expect(decision).to_have_attribute("data-decision-status", "pending")
    expect(decision).to_contain_text("port 5180")

    node = page.locator("#pp-orch-node-TO-02")
    expect(node).to_be_visible()
    expect(node).to_have_attribute("data-worker-id", "worker-A-02")
    expect(node).to_have_attribute("data-attempts", "2")
    page.locator("#pp-orch-btn-toggle-node-TO-02").click()
    expect(node).to_have_attribute("data-lease-fence", "2")
    expect(node.locator("[data-lease-token]")).to_have_count(0)
    expect(node).to_contain_text("worker-A-02")
    expect(node.locator("[data-evidence-id]")).to_have_count(3)
    expect(node.locator("[data-evidence-kind='before-screenshot']")).to_have_count(1)
    expect(node.locator("[data-evidence-kind='git-diff']")).to_have_count(1)
    expect(node.locator("[data-evidence-kind='command-output']")).to_have_count(1)

    node.click(button="right")
    context_menu = page.locator("#pp-context-menu")
    expect(context_menu).to_be_visible()
    expect(context_menu).to_contain_text("TO-02")
    # The action intentionally closes the context-menu portal and mutates the owning
    # <details> in the same React update. Playwright's high-level click waits for the
    # clicked menu item to remain stable after dispatch, so it can time out even though
    # the intended action completed. Dispatching the trusted test event directly proves
    # the menu wiring without imposing that incompatible post-click stability contract.
    context_menu.get_by_role(
        "menuitem", name="Expand / collapse"
    ).dispatch_event("click")
    expect(node).not_to_have_attribute("open", "")

    node.click(button="right")
    page.locator("#pp-context-menu").get_by_role(
        "menuitem", name="Expand / collapse"
    ).dispatch_event("click")
    expect(node).to_have_attribute("open", "")
    node.locator("[data-evidence-id]").first.click(button="right")
    expect(page.locator("#pp-context-menu")).to_contain_text("evidence")
    expect(page.locator("#pp-context-menu")).to_contain_text("Copy evidence identity")
    page.keyboard.press("Escape")

    release_warning = page.locator("#pp-orch-warning-release-infra-001")
    expect(release_warning).to_be_visible()
    expect(release_warning).to_have_attribute(
        "data-issue-kind", "CI_INFRASTRUCTURE_FAILURE"
    )
    expect(release_warning).to_contain_text("infrastructure failure")
    expect(release_warning).to_contain_text("decision required")

    expect(decision).to_be_visible()
    expect(decision).to_have_attribute("data-decision-status", "pending")


def assert_audit_drawer(page: Page) -> None:
    drawer = page.locator("#pp-orch-region-audit-drawer")
    handle = page.locator("#pp-orch-control-audit-resize")
    logs_tab = page.locator("#pp-orch-tab-audit-logs")
    changes_tab = page.locator("#pp-orch-tab-audit-changes")
    expect(drawer).to_be_visible()
    expect(handle).to_be_visible()
    expect(logs_tab).to_have_text("LOGS")
    expect(changes_tab).to_have_text("CHANGES / SUCCEEDED")

    logs_tab.click()
    expect(logs_tab).to_have_attribute("aria-selected", "true")
    logs = page.locator("#pp-orch-panel-audit-active")
    expect(logs).to_be_visible()
    expect(logs.locator("[data-audit-event-id]")).to_have_count(5)
    expect(logs.locator("[data-audit-event-id='audit-claim-TO-02']")).to_contain_text(
        "worker-A-02"
    )
    for event in logs.locator("[data-audit-event-id]").all():
        expect(event).to_have_attribute("data-repository-id", PRIMARY_REPOSITORY)

    before = drawer.bounding_box()
    handle_box = handle.bounding_box()
    assert before and handle_box, "audit drawer and drag handle need measurable bounds"
    page.mouse.move(
        handle_box["x"] + handle_box["width"] / 2,
        handle_box["y"] + handle_box["height"] / 2,
    )
    page.mouse.down()
    page.mouse.move(
        handle_box["x"] + handle_box["width"] / 2,
        handle_box["y"] - 220,
        steps=12,
    )
    page.mouse.up()
    page.wait_for_function(
        """expected => {
          const drawer = document.querySelector('#pp-orch-region-audit-drawer');
          return drawer && drawer.getBoundingClientRect().height >= expected;
        }""",
        arg=before["height"] + 140,
    )

    changes_tab.click()
    expect(changes_tab).to_have_attribute("aria-selected", "true")
    changes = page.locator("#pp-orch-panel-audit-active")
    expect(changes).to_be_visible()
    missing = changes.locator(
        "[data-reconciliation-row-id='reconcile-missing-TO-03']"
    )
    unplanned = changes.locator(
        "[data-reconciliation-row-id='reconcile-unplanned-app-css']"
    )
    expect(missing).to_have_attribute("data-reconciliation-status", "missing")
    expect(unplanned).to_have_attribute("data-reconciliation-status", "unplanned")
    expect(missing.locator("[data-reconciliation-column='desired']")).to_contain_text(
        "Lease scheduler"
    )
    expect(missing.locator("[data-reconciliation-column='actual']")).to_contain_text(
        "Missing"
    )
    expect(unplanned.locator("[data-reconciliation-column='desired']")).to_contain_text(
        "Not planned"
    )
    expect(unplanned.locator("[data-reconciliation-column='actual']")).to_contain_text(
        "src/index.css"
    )
    expect(unplanned.locator("[data-reconciliation-column='actual']")).to_contain_text(
        "f00baa7"
    )

    handle.dblclick()
    expect(drawer).to_have_attribute("data-state", "maximized")
    page.wait_for_function(
        """() => {
          const drawer = document.querySelector('#pp-orch-region-audit-drawer');
          return drawer && drawer.getBoundingClientRect().height >= window.innerHeight * 0.78;
        }"""
    )

    # Tab and drawer interaction must not consume the unresolved preflight decision.
    decision = page.locator("#pp-orch-warning-decision-preflight-5180")
    expect(decision).to_have_attribute("data-decision-status", "pending")


def main() -> None:
    SCREENSHOT.parent.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        # The requested proof browser is installed Google Chrome. Headless mode avoids
        # stealing the user's desktop while device_scale_factor=3 preserves fine UI detail.
        browser = playwright.chromium.launch(
            channel="chrome",
            headless=True,
            args=["--hide-scrollbars"],
        )
        context = browser.new_context(
            viewport={"width": 1920, "height": 1080},
            device_scale_factor=3,
            color_scheme="light",
            reduced_motion="reduce",
        )
        page = context.new_page()
        page.add_init_script(frozen_fixture_script())

        console_errors: list[str] = []
        page_errors: list[str] = []
        failed_requests: list[str] = []
        bad_responses: list[str] = []
        page.on(
            "console",
            lambda message: console_errors.append(message.text)
            if message.type == "error"
            else None,
        )
        page.on("pageerror", lambda error: page_errors.append(str(error)))
        page.on(
            "requestfailed",
            lambda request: failed_requests.append(
                f"{request.method} {request.url}: {request.failure}"
            ),
        )
        page.on(
            "response",
            lambda response: bad_responses.append(
                f"{response.status} {response.request.method} {response.url}"
            )
            if response.status >= 400
            else None,
        )

        # Older shell discovery may still probe local boards while the frozen pipeline is
        # active. Return a clean, deterministic miss instead of touching live local services.
        page.route(
            "**/board-probe/**",
            lambda route: route.fulfill(status=200, json={"ok": False}),
        )

        page.goto(APP_URL, wait_until="networkidle")
        root = page.locator("#pp-orch-pipeline-console")
        expect(root).to_be_visible(timeout=10_000)
        assert page.evaluate(
            """() => Object.isFrozen(window.__ORCHESTRATOR_PIPELINE__)
              && Object.isFrozen(window.__ORCHESTRATOR_PIPELINE__.responses)
              && Object.isFrozen(window.__ORCHESTRATOR_PIPELINE__.responses.orchestrator_snapshot)
              && Object.isFrozen(window.__ORCHESTRATOR_PIPELINE__.responses.orchestrator_snapshot.value)
              && Object.isFrozen(window.__ORCHESTRATOR_PIPELINE__.responses.orchestrator_snapshot.value.events)"""
        ), "the browser pipeline fixture must stay recursively frozen"

        assert_repository_and_plan_scope(page)
        assert_node_evidence_and_persistent_warnings(page)
        assert_audit_drawer(page)
        assert_unique_control_ids(page)

        page.screenshot(
            path=str(SCREENSHOT),
            full_page=True,
            animations="disabled",
        )

        assert not console_errors, f"browser console errors: {console_errors}"
        assert not page_errors, f"uncaught page errors: {page_errors}"
        assert not failed_requests, f"failed browser requests: {failed_requests}"
        assert not bad_responses, f"HTTP error responses: {bad_responses}"

        context.close()
        browser.close()

    print("orchestrator_pipeline_e2e: PASS")
    print(
        "proved: repo/plan scope, active/completed split, persistent decisions, "
        "node leases/evidence, reconciliation, release warning, audit drag/maximize"
    )
    print(f"screenshot: {SCREENSHOT} (1920x1080 CSS viewport at 3x scale)")


if __name__ == "__main__":
    main()
