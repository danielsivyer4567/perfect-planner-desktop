from pathlib import Path
import os
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
APP_URL = os.environ.get("PP_APP_URL", "http://127.0.0.1:5180/")
SCREENSHOT = ROOT / "artifacts" / "orchestrator-messaging-after.png"

BOARDS = {
    5230: {
        "planPath": r"C:\repos\looplet-worktrees\repo-a-work\.claude\scratch\perfect-plan\a.json",
        "number": "PP-101",
        "topic": "Orchestrator messaging",
        "repoName": "Repository Alpha",
        "repoRoot": r"C:\repos\repository-alpha",
        "project": "Repository Alpha",
        "worktreeName": "repo-a-work",
        "branch": "feature/orchestrator-messaging",
        "worker": "s-worker-alpha",
    },
    5231: {
        "planPath": r"C:\repos\looplet-worktrees\repo-b-work\.claude\scratch\perfect-plan\b.json",
        "number": "PP-101",
        "topic": "Unrelated repository",
        "repoName": "Repository Beta",
        "repoRoot": r"C:\repos\repository-beta",
        "project": "Repository Beta",
        "worktreeName": "repo-b-work",
        "branch": "feature/unrelated",
        "worker": "s-worker-beta",
    },
}


def main():
    SCREENSHOT.parent.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        # The user selected Chrome for visual verification. Keep it headless so this proof
        # never steals focus or consumes a visible desktop window.
        browser = playwright.chromium.launch(channel="chrome", headless=True)
        context = browser.new_context(
            viewport={"width": 1440, "height": 900},
            device_scale_factor=2,
        )
        page = context.new_page()
        console_errors = []
        page.on(
            "console",
            lambda message: console_errors.append(message.text)
            if message.type == "error"
            else None,
        )

        def mock_probe(route):
            parsed = urlparse(route.request.url)
            parts = parsed.path.strip("/").split("/")
            if len(parts) != 3 or parts[0] != "board-probe":
                route.fulfill(status=404, json={"ok": False})
                return
            board = BOARDS.get(int(parts[1]))
            if not board:
                route.fulfill(status=200, json={"ok": False})
                return
            endpoint = parts[2]
            if endpoint == "whoami":
                route.fulfill(
                    status=200,
                    json={
                        "ok": True,
                        **{key: value for key, value in board.items() if key != "worker"},
                        "approved": "yes @ test",
                        "awaiting": None,
                        "port": int(parts[1]),
                        "pid": 9000 + int(parts[1]),
                    },
                )
            elif endpoint == "workers":
                route.fulfill(
                    status=200,
                    json={
                        "workers": {
                            "A01": {
                                "vertebra": "A01",
                                "session": board["worker"],
                                "state": "ACTIVE",
                                "model": "gpt-5",
                                "user": "test-worker",
                            }
                        }
                    },
                )
            elif endpoint == "plan":
                route.fulfill(
                    status=200,
                    json={
                        "spine": [{"id": "P1", "title": "Control plane"}],
                        "vertebrae": [
                            {
                                "id": "A01",
                                "spineId": "P1",
                                "title": "Worker notes",
                                "files": ["src/control.ts"],
                                "resources": ["runtime:control-plane"],
                                "checklist": [],
                            }
                        ],
                    },
                )
            else:
                route.fulfill(status=404, json={"ok": False})

        page.route("**/board-probe/**", mock_probe)
        for port in BOARDS:
            page.route(
                f"http://127.0.0.1:{port}/",
                lambda route: route.fulfill(body="<html><title>planner board</title></html>"),
            )

        page.goto(APP_URL, wait_until="domcontentloaded")
        alpha = page.locator(
            '.rail-item[data-repository-name="Repository Alpha"][data-board-port="5230"]'
        )
        expect(alpha).to_be_visible()
        alpha.evaluate("element => element.click()")

        orchestrator_toggle = page.locator("#pp-btn-toggle-orchestrator")
        expect(orchestrator_toggle).to_have_attribute("aria-expanded", "false")
        orchestrator_toggle.evaluate("element => element.click()")
        expect(orchestrator_toggle).to_have_attribute("aria-expanded", "true")

        messenger = page.locator("#pp-region-orchestrator-messenger")
        expect(messenger).to_be_visible()
        expect(page.locator("#pp-badge-orchestrator-inbox")).to_contain_text("0")
        expect(page.locator("#pp-badge-orchestrator-outbox")).to_contain_text("0")
        worker_button = page.locator("#pp-btn-open-worker-notes-s-worker-alpha")
        expect(worker_button).to_be_visible()
        worker_button.evaluate("element => element.click()")
        panel = page.locator("#pp-panel-worker-notes")
        expect(panel).to_be_visible()
        expect(panel).to_have_attribute("data-worker-id", "s-worker-alpha")
        expect(panel).to_have_attribute("data-node-id", "A01")

        page.locator("#pp-input-worker-note").evaluate(
            """(element, value) => {
              const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
              setter.call(element, value);
              element.dispatchEvent(new Event('input', { bubbles: true }));
            }""",
            "Provider access is blocked; the orchestrator must decide whether to use the export fallback.",
        )
        page.locator("#pp-check-escalate-chat").evaluate("element => element.click()")
        expect(page.locator("#pp-status-chat-route")).to_contain_text("No chat destination")
        page.locator("#pp-btn-send-worker-note").evaluate("element => element.click()")
        expect(page.locator("#pp-notice-worker-note")).to_contain_text("Worker note recorded")
        expect(page.locator("#pp-list-worker-notes article")).to_have_count(2)
        expect(page.locator('[data-delivery-state="DELIVERED"]')).to_have_count(1)
        expect(page.locator('[data-delivery-state="UNROUTED"]')).to_have_count(1)
        expect(page.locator("#pp-badge-orchestrator-unrouted")).to_contain_text("1")
        expect(page.locator("#pp-status-workspace-messages")).to_contain_text(
            "awaiting delivery"
        )
        expect(page.locator(".orchestrator-activity")).to_contain_text(
            "Provider access is blocked"
        )
        assert page.get_by_text("SENT", exact=True).count() == 0, (
            "the UI claimed a delivery was sent without connector proof"
        )

        # Simulate a real worker process using the same typed control-plane contract. The local
        # UI is the consumer: its next poll must claim and deliver the note before ACK appears.
        incoming_id = page.evaluate(
            """
            async () => {
              const api = await import('/src/services/controlPlane.ts');
              const stored = Object.keys(localStorage)
                .filter((key) => key.startsWith('perfect-planner:control-plane:v1:'))
                .map((key) => JSON.parse(localStorage.getItem(key)))
                .find((value) => value?.messages?.some((message) => message.scope?.workerId === 's-worker-alpha'));
              const scope = stored.messages.find((message) => message.scope.workerId === 's-worker-alpha').scope;
              const message = await api.postControlMessage({
                idempotencyKey: 'worker-alpha:handoff:1',
                correlationId: 'worker-alpha:blocker',
                kind: 'workerNote',
                scope,
                authorId: 's-worker-alpha',
                body: 'Worker note: OAuth consent is required before this node can continue.',
                destination: {
                  registrationId: `pp-local-ui:${scope.repositoryId}`,
                  kind: 'localUi',
                  label: 'Head orchestrator inbox',
                  address: 'pp-region-orchestrator-messenger',
                  enabled: true,
                  requiresAcknowledgement: true,
                  maxAttempts: 3,
                  retryBaseMs: 5000,
                  registeredAtMs: Date.now(),
                  metadata: { source: 'external-worker-test' },
                },
              });
              return message.id;
            }
            """
        )
        expect(
            page.locator("#pp-list-worker-notes").get_by_text(
                "Worker note: OAuth consent is required", exact=False
            )
        ).to_be_visible(timeout=8_000)
        incoming = page.locator(f'[data-message-id="{incoming_id}"]')
        expect(incoming).to_have_attribute("data-delivery-state", "DELIVERED")
        incoming.get_by_role("button", name="ACKNOWLEDGE").evaluate(
            "element => element.click()"
        )
        expect(incoming).to_have_attribute("data-delivery-state", "ACKNOWLEDGED")

        # Same PP number in another repository must not reveal Alpha's notes.
        page.locator("#pp-btn-close-worker-notes").evaluate("element => element.click()")
        beta = page.locator(
            '.rail-item[data-repository-name="Repository Beta"][data-board-port="5231"]'
        )
        # Selecting a different repository intentionally changes the embedded board iframe.
        # Dispatch the button's semantic click and assert scoped UI state below instead of
        # letting Playwright wait on an unrelated frame-navigation heuristic.
        beta.evaluate("element => element.click()")
        expect(page.locator("#pp-badge-orchestrator-inbox")).to_contain_text("0")
        expect(page.locator("#pp-badge-orchestrator-outbox")).to_contain_text("0")
        expect(page.locator(".orchestrator-activity")).not_to_contain_text(
            "Provider access is blocked"
        )
        page.locator("#pp-btn-open-worker-notes-s-worker-beta").evaluate(
            "element => element.click()"
        )
        expect(page.locator("#pp-list-worker-notes article")).to_have_count(0)

        page.locator("#pp-btn-close-worker-notes").evaluate("element => element.click()")
        alpha.evaluate("element => element.click()")
        page.locator("#pp-btn-open-worker-notes-s-worker-alpha").evaluate(
            "element => element.click()"
        )
        expect(page.locator("#pp-list-worker-notes article")).to_have_count(3)
        page.screenshot(path=str(SCREENSHOT), full_page=True)
        page.locator("#pp-panel-worker-notes").evaluate(
            "element => element.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, button: 2 }))"
        )
        expect(page.locator("#pp-context-menu")).to_contain_text("Worker notes")
        page.locator("#pp-context-menu").get_by_role(
            "menuitem", name="Close modal"
        ).evaluate("element => element.click()")
        expect(page.locator("#pp-panel-worker-notes")).to_have_count(0)
        assert not console_errors, f"browser console errors: {console_errors}"

        context.close()
        browser.close()

    print("control_plane_e2e: PASS")
    print("proved: scoped notes, UNROUTED truth, local delivery, worker-originated note, ack, repo isolation")
    print(f"screenshot: {SCREENSHOT}")


if __name__ == "__main__":
    main()
