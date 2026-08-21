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

        def mock_probe(route):
            parsed = urlparse(route.request.url)
            pieces = parsed.path.strip("/").split("/")
            if len(pieces) != 3 or pieces[0] != "board-probe":
                route.fulfill(status=404, json={"ok": False})
                return
            port = int(pieces[1])
            board = BOARDS.get(port)
            if not board:
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

        browser_card.click()
        active_scope = page.locator("#pp-region-active-board-heading")
        expect(active_scope).to_contain_text("Looplet CRM")
        expect(active_scope).to_contain_text("A")
        expect(active_scope).to_contain_text("Looplet AI")
        expect(active_scope).to_contain_text("feat/browser-b")
        expect(active_scope).to_contain_text("PP-001")
        expect(active_scope).to_contain_text("Browser companion")

        planner_plan = page.get_by_role("button", name="PP-001 Repository B Perfect Planner main Desktop shell")
        planner_plan.click()
        expect(active_scope).to_contain_text("Perfect Planner")
        expect(active_scope).to_contain_text("main")
        expect(active_scope).to_contain_text("Desktop shell")
        expect(page.locator("#pp-entity-head-orchestrator")).to_have_attribute(
            "data-repository-name", "Perfect Planner"
        )
        expect(page.locator("#pp-entity-head-orchestrator")).to_have_attribute(
            "data-repository-call-sign", "B"
        )

        page.reload()
        page.wait_for_load_state("networkidle")
        expect(page.locator('.repo-section[data-repository-name="Looplet CRM"]')).to_have_attribute(
            "data-repository-call-sign", "A"
        )
        expect(page.locator('.repo-section[data-repository-name="Perfect Planner"]')).to_have_attribute(
            "data-repository-call-sign", "B"
        )

        page.screenshot(path=str(SCREENSHOT), full_page=True)
        browser.close()

    print("repository_rail_e2e: PASS")
    print(f"screenshot: {SCREENSHOT}")


if __name__ == "__main__":
    main()
