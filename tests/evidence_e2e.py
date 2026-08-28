import base64
import re
from pathlib import Path
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
APP_URL = "http://127.0.0.1:5180/"
SCREENSHOT = ROOT / "artifacts" / "left-rail-output.png"
PLAN_PATH = r"C:\repos\fixture\.claude\scratch\perfect-plan\evidence.json"
COMPLETE_PATH = r"C:\repos\fixture\.claude\scratch\perfect-plan\complete.json"
PNG = base64.b64encode(base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
)).decode("ascii")


def proof_item(text, *, screenshot=True):
    return {
        "text": text,
        "built": True,
        "tested": True,
        "ui": screenshot,
        "verify": "npm run test:e2e",
        "proof": {
            "by": "prove",
            "at": "2026-08-21T05:15:00.000Z",
            "note": "browser check passed",
            "log": "PP-EV-A01-0.log",
            "sha256": "abc123",
            "exit": 0,
            "durationMs": 42,
            "cwd": r"C:\repos\fixture",
            "verify": "npm run test:e2e",
            "git": {"sha": "123456789abcdef", "branch": "main", "dirty": False},
            **({
                "screenshot": "PP-EV-A01-0.png",
                "screenshotSha256": "shot123",
                "shotNote": "dashboard showing the completed card",
                "screenshotCheck": {"ok": True, "width": 1440, "height": 900},
            } if screenshot else {}),
        },
    }


def plan(complete=False):
    nodes = [{
        "id": "A01", "spineId": "P1", "side": "L", "title": "Visible dashboard",
        "status": "complete", "files": ["src/dashboard.tsx"], "resources": [],
        "checklist": [proof_item("Dashboard renders")],
    }]
    if not complete:
        nodes.append({
            "id": "A02", "spineId": "P1", "side": "R", "title": "Not visible yet",
            "status": "pending", "files": ["src/pending.tsx"], "resources": [],
            "checklist": [{"text": "Pending panel renders", "built": True, "tested": False, "ui": True}],
        })
    return {
        "title": "Truthful UI evidence", "goal": "Show files and local output as different facts.",
        "approved": "yes @ test",
        "meta": {"number": "PP-EV", "topic": "Evidence lab", "appUrl": "http://127.0.0.1:4173/"},
        "spine": [{"id": "P1", "title": "Visible product", "summary": "Rendered output"}],
        "vertebrae": nodes,
    }


def main():
    SCREENSHOT.parent.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        page = browser.new_page(viewport={"width": 1600, "height": 1000})
        console_errors = []
        page_errors = []
        page.on("console", lambda message: console_errors.append(message.text) if message.type == "error" else None)
        page.on("pageerror", lambda error: page_errors.append(str(error)))

        def route_probe(route):
            parsed = urlparse(route.request.url)
            parts = parsed.path.strip("/").split("/")
            if len(parts) < 3 or parts[0] != "board-probe" or parts[1] not in {"5230", "5231"}:
                route.fulfill(status=200, json={"ok": False})
                return
            port = int(parts[1])
            path = PLAN_PATH if port == 5230 else COMPLETE_PATH
            if parts[2] == "whoami":
                route.fulfill(status=200, json={
                    "ok": True, "planPath": path, "number": "PP-EV" if port == 5230 else "PP-DONE",
                    "topic": "Evidence lab" if port == 5230 else "Completed plan", "approved": "yes @ test",
                    "awaiting": None, "port": port, "pid": 9000 + port, "project": "Fixture",
                    "repoName": "Fixture", "repoRoot": r"C:\repos\fixture", "worktreeName": "fixture",
                    "branch": "main",
                })
            elif parts[2] == "workers":
                route.fulfill(status=200, json={"workers": {}})
            elif parts[2] == "plan":
                route.fulfill(status=200, json=plan(complete=port == 5231))
            elif parts[2] == "evidence" and len(parts) == 4:
                name = parts[3]
                if name.endswith(".log"):
                    route.fulfill(status=200, json={
                        "name": name, "mime": "text/plain",
                        "text": "# perfect-planning proof artifact\nexit: 0\nduration_ms: 42\n\n## stdout\nALL UI TESTS PASSED\n## stderr\n(empty)",
                    })
                else:
                    route.fulfill(status=200, json={"name": name, "mime": "image/png", "dataBase64": PNG})
            else:
                route.fulfill(status=404, json={"ok": False})

        page.route("**/board-probe/**", route_probe)
        page.route("http://127.0.0.1:5230/", lambda route: route.fulfill(body="<title>board</title>"))
        page.route("http://127.0.0.1:5231/", lambda route: route.fulfill(body="<title>board</title>"))
        page.goto(APP_URL)
        page.wait_for_load_state("networkidle")

        expect(page.locator("#pp-frame-active-board")).to_have_attribute("src", "http://127.0.0.1:5230/")
        expect(page.locator("#pp-region-local-output")).to_contain_text("http://127.0.0.1:4173/")
        expect(page.locator("#pp-region-local-output")).to_contain_text("P1 1/2")
        expect(page.locator("#pp-region-local-output")).to_contain_text("Not shown in captured UI: A02")
        expect(page.locator("#pp-region-local-output img")).to_be_visible()

        parallel = page.locator("#pp-btn-toggle-parallel-agents")
        expect(parallel).to_have_attribute("role", "switch")
        expect(parallel).to_have_attribute("aria-checked", "true")
        expect(parallel).to_contain_text("ON · NEW RUNS ×4")
        parallel.evaluate("element => element.click()")
        expect(parallel).to_have_attribute("aria-checked", "false")
        assert page.evaluate("localStorage.getItem('perfect-planner:parallel-agents')") == "false"
        page.reload(wait_until="networkidle")
        parallel = page.locator("#pp-btn-toggle-parallel-agents")
        expect(parallel).to_have_attribute("aria-checked", "false")
        parallel.evaluate("element => element.click()")
        expect(parallel).to_have_attribute("aria-checked", "true")

        comparison_toggle = page.locator("#pp-btn-toggle-ui-comparison")
        comparison_toggle.evaluate("element => element.click()")
        expect(comparison_toggle).to_have_attribute("aria-expanded", "true")
        comparison = page.locator("#pp-region-ui-comparison")
        expect(comparison).to_be_visible()
        expect(comparison.locator(".capture-standard")).to_contain_text("COMPARISON-GRADE CAPTURE")
        expect(comparison.locator(".capture-standard")).to_contain_text("1440 × 900")
        expect(comparison.locator("img")).to_be_visible()
        comparison.get_by_role("button", name="Code").evaluate("element => element.click()")
        expect(comparison.locator(".code-evidence")).to_contain_text("ALL UI TESTS PASSED")
        comparison.get_by_role("button", name="UI").evaluate("element => element.click()")
        expect(comparison.locator("img")).to_be_visible()

        active_row = page.locator('.rail-item[data-board-port="5230"]')
        expect(page.locator('[data-plan-status="progress"] .plan-status-divider')).to_contain_text("IN PROGRESS")
        expect(page.locator('[data-plan-status="complete"] .plan-status-divider')).to_contain_text("COMPLETED")
        assert page.locator('[data-plan-status="progress"]').evaluate("el => el.compareDocumentPosition(document.querySelector('[data-plan-status=complete]')) & Node.DOCUMENT_POSITION_FOLLOWING")
        expect(active_row).not_to_have_class(re.compile(r"(?:^|\s)complete(?:\s|$)"))
        expect(active_row.locator(".complete-flag")).to_have_count(0)
        completed_row = page.locator('.rail-item[data-board-port="5231"]')
        expect(completed_row).to_have_class(re.compile(r"(?:^|\s)complete(?:\s|$)"))
        expect(completed_row.locator(".complete-flag")).to_have_text("✓ COMPLETE")

        page.screenshot(path=str(SCREENSHOT), full_page=True)
        assert not console_errors, f"browser console errors: {console_errors}"
        assert not page_errors, f"browser page errors: {page_errors}"
        browser.close()

    print("evidence_e2e: PASS")
    print(f"screenshot: {SCREENSHOT}")
    print("proved: persistent parallel default plus split live/captured UI and code-evidence switching")


if __name__ == "__main__":
    main()
