from pathlib import Path
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
APP_URL = "http://127.0.0.1:5180/"
SCREENSHOT = ROOT / "artifacts" / "ui-navigation-map.png"

BOARD = {
    "planPath": r"C:\repos\perfect-planner-tauri\.claude\scratch\perfect-plan\ui-map.json",
    "number": "PP-UI",
    "topic": "Application page system",
    "repoName": "Perfect Planner",
    "repoRoot": r"C:\repos\perfect-planner-tauri",
    "project": "Perfect Planner Desktop",
    "worktreeName": "perfect-planner-tauri",
    "branch": "feature/ui-map",
}

PLAN = {
    "title": "Application page system",
    "approved": "yes @ test",
    "meta": {
        "number": "PP-UI",
        "project": "Perfect Planner Desktop",
        "branch": "feature/ui-map",
        "topic": "Application page system",
    },
    "spine": [
        {"id": "P1", "title": "Command workspace", "summary": "Select and inspect work."},
        {"id": "P2", "title": "Evidence and release", "summary": "Prove readiness."},
    ],
    "vertebrae": [
        {
            "id": "A01",
            "spineId": "P1",
            "side": "L",
            "title": "Repository selection page",
            "status": "done",
            "files": ["src/App.tsx"],
            "checklist": [{"text": "Repository scope stays visible", "ui": True, "built": True, "tested": True}],
        },
        {
            "id": "A02",
            "spineId": "P1",
            "side": "R",
            "title": "Native lease service",
            "status": "in-progress",
            "files": ["src-tauri/src/orchestrator.rs"],
            "checklist": [{"text": "Lease stays fenced", "built": True, "tested": False}],
        },
        {
            "id": "A03",
            "spineId": "P2",
            "side": "R",
            "title": "Release evidence page",
            "status": "pending",
            "files": ["src/components/OperationalTruth.tsx"],
            "checklist": [{"text": "Release evidence is visible", "ui": True, "built": False, "tested": False}],
        },
        {
            "id": "A99",
            "spineId": "P9",
            "side": "L",
            "title": "Unassigned legacy page",
            "status": "pending",
            "files": ["src/legacy.tsx"],
            "checklist": [],
        },
    ],
}


def main():
    SCREENSHOT.parent.mkdir(parents=True, exist_ok=True)
    console_errors = []
    page_errors = []
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        page = browser.new_page(viewport={"width": 1440, "height": 900}, device_scale_factor=3)
        page.set_default_timeout(10_000)
        page.on("console", lambda message: console_errors.append(message.text) if message.type == "error" else None)
        page.on("pageerror", lambda error: page_errors.append(str(error)))

        def mock_probe(route):
            parsed = urlparse(route.request.url)
            pieces = parsed.path.strip("/").split("/")
            if len(pieces) != 3 or pieces[0] != "board-probe" or int(pieces[1]) != 5230:
                route.fulfill(status=200, json={"ok": False})
                return
            endpoint = pieces[2]
            if endpoint == "whoami":
                route.fulfill(status=200, json={"ok": True, **BOARD, "approved": "yes @ test", "awaiting": None, "port": 5230, "pid": 5230})
            elif endpoint == "plan":
                route.fulfill(status=200, json=PLAN)
            elif endpoint == "workers":
                route.fulfill(status=200, json={"workers": {}})
            else:
                route.fulfill(status=404, json={"ok": False})

        page.route("**/board-probe/**", mock_probe)
        page.route("http://127.0.0.1:5230/", lambda route: route.fulfill(body="<html><title>board</title></html>"))
        page.goto(APP_URL, wait_until="networkidle")

        mode = page.locator("#pp-btn-toggle-ui-navigation-map")
        expect(mode).to_have_attribute("aria-pressed", "false")
        mode.evaluate("element => element.focus()")
        expect(mode).to_be_focused()
        mode.evaluate("element => element.click()")
        expect(mode).to_have_attribute("aria-pressed", "true")
        expect(page.locator("#pp-frame-active-board")).to_have_count(0)

        ui_map = page.locator("#pp-region-ui-navigation-map")
        expect(ui_map).to_be_visible()
        expect(ui_map).to_have_attribute("data-plan-id", "PP-UI")
        expect(ui_map).to_contain_text("PLAN-DERIVED PAGE INVENTORY")
        expect(ui_map).to_contain_text("not a runtime route crawl")
        expect(page.locator(".ui-map-spine-row")).to_have_count(2)
        expect(page.locator(".ui-map-orphans")).to_contain_text("A99 → missing spine P9")

        left = page.locator("#pp-btn-ui-map-P1-left")
        right = page.locator("#pp-btn-ui-map-P1-right")
        expect(left).to_have_attribute("aria-expanded", "false")
        expect(right).to_have_attribute("aria-expanded", "false")
        left.evaluate("element => element.focus()")
        expect(left).to_be_focused()
        left.evaluate("element => element.click()")
        expect(left).to_have_attribute("aria-expanded", "true")
        left_page = page.locator("#pp-ui-page-A01")
        expect(left_page).to_be_visible()
        expect(left_page).to_have_attribute("data-spine-id", "P1")
        expect(left_page).to_have_attribute("data-page-id", "A01")
        expect(left_page).to_have_attribute("data-page-side", "L")
        expect(left_page).to_have_attribute("data-page-kind", "ui-capable")
        expect(page.locator("#pp-ui-page-A02")).to_be_hidden()

        right.evaluate("element => element.click()")
        expect(right).to_have_attribute("aria-expanded", "true")
        support_page = page.locator("#pp-ui-page-A02")
        expect(support_page).to_be_visible()
        expect(support_page).to_have_attribute("data-page-kind", "support-work")
        support_page.locator("summary").evaluate("element => element.click()")
        expect(support_page).to_contain_text("Unknown — no frontend file is recorded")

        empty_left = page.locator("#pp-btn-ui-map-P2-left")
        empty_left.evaluate("element => element.click()")
        expect(page.locator("#pp-ui-branch-P2-left")).to_contain_text("No left-side page IDs are recorded")

        page.screenshot(path=str(SCREENSHOT), full_page=True, animations="disabled")

        comparison = page.locator("#pp-btn-toggle-ui-comparison")
        comparison.evaluate("element => element.click()")
        expect(mode).to_have_attribute("aria-pressed", "false")
        expect(ui_map).to_have_count(0)
        expect(page.locator("#pp-frame-active-board")).to_be_visible()
        expect(page.locator("#pp-region-ui-comparison")).to_be_visible()

        assert not console_errors, f"browser console errors: {console_errors}"
        assert not page_errors, f"uncaught page errors: {page_errors}"
        browser.close()

    print("ui_navigation_map_e2e: PASS")
    print(f"screenshot: {SCREENSHOT}")


if __name__ == "__main__":
    main()
