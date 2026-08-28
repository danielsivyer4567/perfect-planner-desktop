from pathlib import Path
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
APP_URL = "http://127.0.0.1:5180/"
SCREENSHOT = ROOT / "artifacts" / "repository-rail.png"


BOARDS = {
    5230: {
        "planPath": r"C:\repos\looplet-worktrees\Looplet-import-a\.claude\scratch\perfect-plan\a.json",
        "number": "PP-001",
        "topic": "Import rehearsal",
        "repoName": "Looplet CRM",
        "repoRoot": r"C:\repos\looplet-crm",
        "project": "Looplet CRM",
        "worktreeName": "Looplet-import-a",
        "branch": "feat/import-a",
    },
    # Same plan on a second presentation port: still one plan row.
    5231: {
        "planPath": r"C:\repos\looplet-worktrees\Looplet-import-a\.claude\scratch\perfect-plan\a.json",
        "number": "PP-001",
        "topic": "Import rehearsal",
        "repoName": "Looplet CRM",
        "repoRoot": r"C:\repos\looplet-crm",
        "project": "Looplet CRM",
        "worktreeName": "Looplet-import-a",
        "branch": "feat/import-a",
    },
    5232: {
        "planPath": r"C:\repos\looplet-worktrees\Looplet-browser-b\.claude\scratch\perfect-plan\b.json",
        "number": "PP-001",
        "topic": "Browser companion",
        "repoName": "Looplet CRM",
        "repoRoot": r"C:\repos\looplet-crm",
        "project": "Looplet AI",
        "worktreeName": "Looplet-browser-b",
        "branch": "feat/browser-b",
    },
    5233: {
        "planPath": r"C:\repos\perfect-planner-desktop\.claude\scratch\perfect-plan\c.json",
        "number": "PP-001",
        "topic": "Desktop shell",
        "repoName": "Perfect Planner",
        "repoRoot": r"C:\repos\perfect-planner-desktop",
        "project": "Perfect Planner",
        "worktreeName": "perfect-planner-desktop",
        "branch": "main",
    },
}


def main():
    SCREENSHOT.parent.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        page = browser.new_page(viewport={"width": 1440, "height": 900})
        page.set_default_timeout(10_000)
        probe_available = {port: True for port in BOARDS}

        def mock_probe(route):
            parsed = urlparse(route.request.url)
            pieces = parsed.path.strip("/").split("/")
            if len(pieces) != 3 or pieces[0] != "board-probe":
                route.fulfill(status=404, json={"ok": False})
                return
            port = int(pieces[1])
            board = BOARDS.get(port)
            if not board or not probe_available[port]:
                route.fulfill(status=200, json={"ok": False})
                return
            endpoint = pieces[2]
            if endpoint == "whoami":
                route.fulfill(
                    status=200,
                    json={
                        "ok": True,
                        **board,
                        "approved": "yes @ test",
                        "awaiting": None,
                        "port": port,
                        "pid": 1000 + port,
                    },
                )
            elif endpoint == "workers":
                route.fulfill(status=200, json={"workers": {}})
            elif endpoint == "plan":
                route.fulfill(status=200, json={"vertebrae": []})
            else:
                route.fulfill(status=404, json={"ok": False})

        page.route("**/board-probe/**", mock_probe)
        for port in BOARDS:
            page.route(
                f"http://127.0.0.1:{port}/",
                lambda route: route.fulfill(body="<html><title>board</title></html>"),
            )

        page.goto(APP_URL)
        page.wait_for_load_state("networkidle")
        print("repository rail: loaded", flush=True)

        repositories = page.locator("[data-repository-id]")
        expect(repositories).to_have_count(2)
        expect(page.get_by_role("heading", name="Looplet CRM")).to_be_visible()
        expect(page.get_by_role("heading", name="Perfect Planner")).to_be_visible()
        expect(page.locator(".rail-item")).to_have_count(3)
        expect(repositories.nth(0)).to_have_attribute("data-repository-call-sign", "A")
        expect(repositories.nth(1)).to_have_attribute("data-repository-call-sign", "B")
        expect(repositories.nth(0).locator(".repo-call-sign")).to_have_text("A")
        expect(repositories.nth(1).locator(".repo-call-sign")).to_have_text("B")

        looplet_cards = page.locator('[data-repository-name="Looplet CRM"] .rail-item')
        expect(looplet_cards).to_have_count(2)
        import_card = page.get_by_role("button", name="PP-001 Repository A Looplet CRM feat/import-a Import rehearsal")
        browser_card = page.get_by_role("button", name="PP-001 Repository A Looplet CRM Project Looplet AI feat/browser-b Browser companion")
        expect(import_card).to_contain_text("PP-001")
        expect(import_card).to_contain_text("Looplet CRM")
        expect(import_card).to_contain_text("feat/import-a")
        expect(browser_card).to_contain_text("feat/browser-b")
        expect(browser_card).to_contain_text("Looplet AI")

        browser_card.evaluate("element => element.click()")
        active_scope = page.locator("#pp-region-active-board-heading")
        expect(active_scope).to_contain_text("Looplet CRM")
        expect(active_scope).to_contain_text("A")
        expect(active_scope).to_contain_text("Looplet AI")
        expect(active_scope).to_contain_text("feat/browser-b")
        expect(active_scope).to_contain_text("PP-001")
        expect(active_scope).to_contain_text("Browser companion")

        planner_plan = page.get_by_role("button", name="PP-001 Repository B Perfect Planner main Desktop shell")
        planner_plan.evaluate("element => element.click()")
        print("repository rail: selected planner", flush=True)
        expect(active_scope).to_contain_text("Perfect Planner")
        expect(active_scope).to_contain_text("main")
        expect(active_scope).to_contain_text("Desktop shell")
        expect(page.locator("#pp-entity-head-orchestrator")).to_have_attribute(
            "data-repository-name", "Perfect Planner"
        )
        expect(page.locator("#pp-entity-head-orchestrator")).to_have_attribute(
            "data-repository-call-sign", "B"
        )
        assert page.evaluate(
            "JSON.parse(localStorage.getItem('perfect-planner:active-board'))"
        ) == {
            "repositoryRoot": BOARDS[5233]["repoRoot"],
            "planPath": BOARDS[5233]["planPath"],
        }

        # A port is transport, not identity. If another repository appears on the selected
        # port, fail closed until the exact saved repository + plan identity returns.
        selected_board = BOARDS[5233]
        BOARDS[5233] = {
            **selected_board,
            "planPath": r"C:\repos\foreign\.claude\scratch\perfect-plan\foreign.json",
            "topic": "Foreign replacement",
            "repoName": "Foreign Repository",
            "repoRoot": r"C:\repos\foreign",
            "project": "Foreign Repository",
        }
        page.locator("#pp-btn-rescan-boards").evaluate("element => element.click()")
        expect(page.locator("#pp-btn-rescan-boards")).to_have_text("rescan", timeout=10_000)
        expect(page.locator("#pp-region-empty-stage")).to_contain_text("Saved plan unavailable")
        expect(page.locator("#pp-frame-active-board")).to_have_count(0)
        BOARDS[5233] = selected_board
        page.locator("#pp-btn-rescan-boards").evaluate("element => element.click()")
        expect(page.locator("#pp-btn-rescan-boards")).to_have_text("rescan", timeout=10_000)
        expect(active_scope).to_contain_text("Perfect Planner")
        expect(active_scope).to_contain_text("Desktop shell")

        # A single missed discovery poll must not silently switch the selected plan to a
        # sibling board. Keep the last trusted card and active scope during the grace window.
        probe_available[5233] = False
        page.locator("#pp-btn-rescan-boards").evaluate("element => element.click()")
        expect(page.locator("#pp-btn-rescan-boards")).to_have_text("rescan", timeout=10_000)
        expect(active_scope).to_contain_text("Perfect Planner")
        expect(active_scope).to_contain_text("Desktop shell")
        expect(planner_plan).to_have_attribute("aria-pressed", "true")
        probe_available[5233] = True
        page.locator("#pp-btn-rescan-boards").evaluate("element => element.click()")
        expect(page.locator("#pp-btn-rescan-boards")).to_have_text("rescan", timeout=10_000)

        page.reload()
        page.wait_for_load_state("networkidle")
        print("repository rail: reloaded", flush=True)
        expect(page.locator('.repo-section[data-repository-name="Looplet CRM"]')).to_have_attribute(
            "data-repository-call-sign", "A"
        )
        expect(page.locator('.repo-section[data-repository-name="Perfect Planner"]')).to_have_attribute(
            "data-repository-call-sign", "B"
        )
        repository_tabs = page.locator(".repository-scope-tabs button")
        expect(repository_tabs).to_have_count(2)
        expect(repository_tabs.nth(0)).to_contain_text("Looplet CRM")
        expect(repository_tabs.nth(1)).to_contain_text("Perfect Planner")
        expect(active_scope).to_contain_text("Perfect Planner")
        expect(active_scope).to_contain_text("Desktop shell")
        expect(planner_plan).to_have_attribute("aria-pressed", "true")
        expect(page.locator("#pp-status-workspace-health")).to_contain_text("run not selected")
        expect(page.locator("#pp-status-workspace-messages")).to_contain_text("CI unknown")

        orchestrator_toggle = page.locator("#pp-btn-toggle-orchestrator")
        expect(orchestrator_toggle).to_have_attribute("aria-expanded", "false")
        frame_top_before = page.locator("#pp-frame-active-board").bounding_box()["y"]
        orchestrator_toggle.evaluate("element => element.click()")
        expect(orchestrator_toggle).to_have_attribute("aria-expanded", "true")
        print("repository rail: inspector opened", flush=True)
        inspector = page.locator("#pp-panel-orchestrator-inspector")
        expect(inspector).to_be_visible()
        expect(inspector).to_contain_text("Perfect Planner")
        print("repository rail: inspector scoped", flush=True)
        frame_top_after = page.locator("#pp-frame-active-board").bounding_box()["y"]
        assert frame_top_after == frame_top_before, (
            f"fixed inspector displaced the workflow canvas: {frame_top_before} -> {frame_top_after}"
        )
        resource_guard = page.locator("#pp-btn-resource-guard")
        expect(resource_guard).to_contain_text("RESOURCE GUARD · UNAVAILABLE")
        resource_guard.evaluate("element => element.click()")
        expect(page.locator("#pp-resource-guard")).to_contain_text(
            "Resource guard is available in the Tauri desktop app"
        )
        print("repository rail: guard opened", flush=True)
        page.locator("#pp-btn-close-orchestrator-inspector").dispatch_event(
            "keydown", {"key": "Escape"}
        )
        print("repository rail: inspector verified", flush=True)
        expect(inspector).to_be_hidden()
        expect(orchestrator_toggle).to_be_focused()

        # A saved identity that is no longer in the discovery census must fail closed. It may
        # not silently select the first plan from another repository.
        page.evaluate(
            """
            localStorage.setItem('perfect-planner:active-board', JSON.stringify({
              repositoryRoot: 'C:\\repos\\missing-repository',
              planPath: 'C:\\repos\\missing-repository\\.claude\\scratch\\perfect-plan\\missing.json'
            }))
            """
        )
        page.reload()
        page.wait_for_load_state("networkidle")
        expect(page.locator("#pp-region-empty-stage")).to_contain_text("Saved plan unavailable")
        expect(page.locator("#pp-frame-active-board")).to_have_count(0)
        expect(page.locator(".rail-item[aria-pressed='true']")).to_have_count(0)

        target_plan = page.get_by_role(
            "button", name="PP-001 Repository B Perfect Planner main Desktop shell"
        )
        target_plan.evaluate("element => element.click()")
        # The preceding selection deliberately replaces the empty stage and can rerender this
        # rail item while Playwright is completing a low-level pointer action. Dispatch the
        # browser's context-menu event directly so this test exercises the delegated handler
        # without racing actionability bookkeeping for a node React has just replaced.
        target_plan.evaluate(
            """
            element => element.dispatchEvent(new MouseEvent('contextmenu', {
              bubbles: true,
              cancelable: true,
              button: 2,
              clientX: 180,
              clientY: 400
            }))
            """
        )
        context_menu = page.locator("#pp-context-menu")
        expect(context_menu).to_be_visible()
        expect(context_menu.get_by_role("menuitem", name="Reject and delete blocked")).to_be_disabled()
        context_menu.get_by_role("menuitem", name="Remove from rail").evaluate(
            "element => element.click()"
        )
        expect(page.locator(".rail-item")).to_have_count(2)
        expect(page.locator("#pp-btn-restore-dismissed")).to_contain_text("restore 1")
        page.locator("#pp-btn-restore-dismissed").evaluate("element => element.click()")
        print("repository rail: context actions verified", flush=True)
        expect(page.locator(".rail-item")).to_have_count(3)

        console_toggle = page.locator("#pp-btn-diagnostics-toggle")
        expect(console_toggle).to_contain_text("CONSOLE")
        if console_toggle.get_attribute("aria-expanded") != "true":
            console_toggle.evaluate("element => element.click()")
        expect(page.locator("#pp-panel-diagnostics-console")).to_contain_text("MCP runtime")
        expect(page.locator("#pp-panel-diagnostics-console")).to_contain_text("NOT ATTESTED")

        page.screenshot(path=str(SCREENSHOT), full_page=True)
        print("repository rail: screenshot captured", flush=True)
        browser.close()

    print("repository_rail_e2e: PASS")
    print(f"screenshot: {SCREENSHOT}")


if __name__ == "__main__":
    main()
