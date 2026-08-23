from pathlib import Path
import os
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
APP_URL = os.environ.get("PP_APP_URL", "http://127.0.0.1:5180/")
BEFORE = ROOT / "artifacts" / "approval-chat-bridge-before.png"
AFTER = ROOT / "artifacts" / "approval-chat-bridge-after.png"
EXACT_TASK = "task-exact-originator"


def bridge(plan_path, state, task_id=None):
    delivered = state == "DELIVERED"
    repository_id = "pp-repo-alpha"
    return {
        "planPath": plan_path,
        "registrationId": "approval-route-exact" if task_id else None,
        "routeId": f"codex-exec:{repository_id}:{task_id}" if task_id else None,
        "taskId": task_id,
        "messageId": "approval-message-exact" if delivered else None,
        "state": state,
        "admissionReleased": delivered,
        "deliveryReceipt": "connector-receipt-exact" if delivered else None,
        "lastError": None,
        "routeExpiresAtMs": 9_999_999_999_999 if task_id else None,
    }


def main():
    BEFORE.parent.mkdir(parents=True, exist_ok=True)
    plan_a = r"C:\repos\alpha-work\.claude\scratch\perfect-plan\a.json"
    plan_b = r"C:\repos\beta-work\.claude\scratch\perfect-plan\b.json"
    state = {"approved": False}
    boards = {
        5230: {
            "planPath": plan_a,
            "number": "PP-201",
            "topic": "Exact approval route",
            "repoName": "Repository Alpha",
            "repoRoot": r"C:\repos\repository-alpha",
            "worktreeName": "alpha-work",
            "branch": "feature/exact-approval",
        },
        5231: {
            "planPath": plan_b,
            "number": "PP-202",
            "topic": "Unregistered route remains blocked",
            "repoName": "Repository Beta",
            "repoRoot": r"C:\repos\repository-beta",
            "worktreeName": "beta-work",
            "branch": "feature/unregistered",
        },
    }

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(channel="chrome", headless=True)
        context = browser.new_context(
            viewport={"width": 1440, "height": 900},
            device_scale_factor=3,
        )
        page = context.new_page()
        console_errors = []
        page_errors = []
        page.on(
            "console",
            lambda message: console_errors.append(message.text)
            if message.type == "error"
            else None,
        )
        page.on("pageerror", lambda error: page_errors.append(str(error)))

        def mock_probe(route):
            parsed = urlparse(route.request.url)
            pieces = parsed.path.strip("/").split("/")
            if len(pieces) != 3 or pieces[0] != "board-probe":
                route.fulfill(status=404, json={"ok": False})
                return
            port = int(pieces[1])
            board = boards.get(port)
            if not board:
                route.fulfill(status=200, json={"ok": False})
                return
            endpoint = pieces[2]
            if endpoint == "whoami":
                if port == 5230:
                    approval = "yes @ test" if state["approved"] else "pending"
                    approval_bridge = bridge(
                        plan_a,
                        "DELIVERED" if state["approved"] else "PENDING",
                        EXACT_TASK,
                    )
                else:
                    approval = "yes @ test"
                    approval_bridge = bridge(plan_b, "UNREGISTERED")
                route.fulfill(
                    status=200,
                    json={
                        "ok": True,
                        **board,
                        "approved": approval,
                        "approvalBridge": approval_bridge,
                        "awaiting": None,
                        "port": port,
                        "pid": 10_000 + port,
                        "project": board["repoName"],
                    },
                )
            elif endpoint == "workers":
                route.fulfill(status=200, json={"workers": {}})
            elif endpoint == "plan":
                route.fulfill(
                    status=200,
                    json={
                        "spine": [{"id": "P1", "title": "Approval"}],
                        "vertebrae": [
                            {
                                "id": "B15",
                                "spineId": "P1",
                                "title": "Approval bridge",
                                "files": [],
                                "resources": [],
                                "checklist": [{"text": "Deliver exact approval", "built": False, "tested": False}],
                            }
                        ],
                    },
                )
            else:
                route.fulfill(status=404, json={"ok": False})

        def mock_board(route):
            parsed = urlparse(route.request.url)
            if parsed.path == "/approve" and route.request.method == "POST":
                state["approved"] = True
                route.fulfill(status=200, json={"ok": True})
                return
            route.fulfill(
                status=200,
                content_type="text/html",
                body="""
<!doctype html><html><body style="font-family:system-ui;padding:32px">
<h1>Exact approval route fixture</h1>
<button id="approve" onclick="fetch('/approve',{method:'POST'}).then(()=>document.body.dataset.approved='yes')">APPROVE</button>
</body></html>
""",
            )

        page.route("**/board-probe/**", mock_probe)
        context.route("http://127.0.0.1:5230/**", mock_board)
        context.route("http://127.0.0.1:5231/**", mock_board)
        page.goto(APP_URL, wait_until="networkidle")

        orchestrator_toggle = page.locator("#pp-btn-toggle-orchestrator")
        expect(orchestrator_toggle).to_have_attribute("aria-expanded", "false")
        # The embedded board may still be settling a routed navigation. Dispatch the
        # semantic button click and assert the resulting state instead of coupling this
        # shell control to Playwright's unrelated iframe-navigation wait.
        orchestrator_toggle.evaluate("element => element.click()")
        expect(orchestrator_toggle).to_have_attribute("aria-expanded", "true")

        exact_row = page.locator('[data-board-port="5230"]')
        blocked_row = page.locator('[data-board-port="5231"]')
        expect(exact_row.locator('[data-approval-bridge-state="UNVERIFIED_BOARD_CLAIM"]')).to_be_visible()
        expect(blocked_row.locator('[data-approval-bridge-state="UNVERIFIED_BOARD_CLAIM"]')).to_be_visible()
        live_mark = page.locator("#pp-status-looplet-live-feed")
        expect(live_mark).to_be_visible()
        expect(live_mark).to_have_attribute("data-feed-state", "live")
        first_frame = live_mark.locator("pre").text_content()
        page.wait_for_timeout(350)
        second_frame = live_mark.locator("pre").text_content()
        assert first_frame and second_frame and first_frame != second_frame, "Looplet live mark did not advance"
        page.screenshot(path=str(BEFORE), full_page=True)

        frame = page.frame(url=lambda url: url.startswith("http://127.0.0.1:5230/"))
        if frame is None:
            raise AssertionError("exact board iframe did not load")
        frame.locator("#approve").click()
        page.reload(wait_until="networkidle")
        orchestrator_toggle = page.locator("#pp-btn-toggle-orchestrator")
        expect(orchestrator_toggle).to_have_attribute("aria-expanded", "false")
        orchestrator_toggle.evaluate("element => element.click()")
        expect(orchestrator_toggle).to_have_attribute("aria-expanded", "true")

        exact_row = page.locator('[data-board-port="5230"]')
        blocked_row = page.locator('[data-board-port="5231"]')
        unverified = exact_row.locator('[data-approval-bridge-state="UNVERIFIED_BOARD_CLAIM"]')
        expect(unverified).to_be_visible()
        expect(unverified).to_contain_text("chat · unverified board claim")
        assert state["approved"], "fixture approval was not recorded"
        expect(blocked_row.locator('[data-approval-bridge-state="UNVERIFIED_BOARD_CLAIM"]')).to_be_visible()
        expect(page.locator("#pp-status-looplet-live-feed")).to_be_visible()
        page.screenshot(path=str(AFTER), full_page=True)
        assert not console_errors, f"browser console errors: {console_errors}"
        assert not page_errors, f"browser page errors: {page_errors}"
        browser.close()

    print("approval_chat_bridge_e2e: PASS")
    print(f"before: {BEFORE}")
    print(f"after: {AFTER}")
    print("proved: browser-reported approval routes remain visibly unverified before and after approval")


if __name__ == "__main__":
    main()
