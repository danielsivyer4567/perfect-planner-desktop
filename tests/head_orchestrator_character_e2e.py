from pathlib import Path
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
APP_URL = "http://127.0.0.1:5180/"
PLAN_PATH = r"C:\fixtures\head-orchestrator-character.json"
SCREENSHOT = ROOT / "artifacts" / "head-orchestrator-character-3x.png"
MINIMIZED_SCREENSHOT = ROOT / "artifacts" / "head-orchestrator-minimized-3x.png"


def main() -> None:
    SCREENSHOT.parent.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(channel="chrome", headless=True)
        context = browser.new_context(
            viewport={"width": 1440, "height": 900},
            device_scale_factor=3,
            reduced_motion="reduce",
        )
        page = context.new_page()
        console_errors: list[str] = []
        page.on(
            "console",
            lambda message: console_errors.append(message.text)
            if message.type == "error"
            else None,
        )
        phase = {"value": "active"}

        def mock_probe(route) -> None:
            parsed = urlparse(route.request.url)
            pieces = parsed.path.strip("/").split("/")
            if len(pieces) != 3 or pieces[0] != "board-probe" or pieces[1] != "5232":
                route.fulfill(status=200, json={"ok": False})
                return
            if pieces[2] == "whoami":
                route.fulfill(
                    status=200,
                    json={
                        "ok": True,
                        "planPath": PLAN_PATH,
                        "number": "PP-ORCH",
                        "topic": "Physical head orchestrator",
                        "project": "Perfect Planner Desktop",
                        "repoName": "Perfect Planner Desktop",
                        "repoRoot": str(ROOT),
                        "branch": "feature/head-orchestrator-character",
                        "approved": "yes @ test",
                        "awaiting": {
                            "kind": "lifecycle-boundary",
                            "item": "release-gate",
                            "problem": "The combined workflow is not safe to treat as one automated lifecycle.",
                            "where": "Perfect Planner Desktop / release lifecycle / $start → Planner → $finish → $cleanup",
                            "remedy": "Planner may invoke $finish, but $finish remains the authoritative release gate; run $cleanup explicitly and last.",
                            "since": "2026-08-22T05:50:00.000Z",
                        }
                        if phase["value"] == "lifecycle"
                        else None,
                        "port": 5232,
                        "pid": 22022,
                    },
                )
                return
            route.fulfill(
                status=200,
                json={
                    "asOf": "2026-08-22T05:50:00.000Z",
                    "activeWindowMs": 600000,
                    "workers": {
                        "B22": {
                            "vertebra": "B22",
                            "session": "s-actor",
                            "state": "STALE" if phase["value"] == "stale" else "ACTIVE",
                        },
                        "B15": {
                            "vertebra": "B15",
                            "session": "s-bridge",
                            "state": "STALE" if phase["value"] == "stale" else "ACTIVE",
                        },
                    },
                },
            )

        page.route("**/board-probe/**", mock_probe)
        page.route("http://127.0.0.1:5232/", lambda route: route.fulfill(body="<html></html>"))
        page.goto(APP_URL, wait_until="domcontentloaded")

        head = page.locator("#pp-entity-head-orchestrator")
        actor = page.locator("#pp-entity-head-orchestrator-character")
        speech = page.locator("#pp-status-head-orchestrator-speech")
        worker_route = page.locator("#pp-list-worker-reports")
        spinner = page.locator("#pp-status-looplet-live-feed")
        minimize_toggle = page.locator("#pp-btn-toggle-orchestrator")
        orchestrator_details = page.locator("#pp-panel-orchestrator-details")
        pipeline_details = page.locator("#pp-panel-orchestrator-pipeline")
        board_frame = page.locator("#pp-frame-active-board")

        expect(head).to_be_visible()
        expect(page.locator("#pp-panel-diagnostics-console")).to_be_hidden()
        expect(minimize_toggle).to_be_visible()
        expect(minimize_toggle).to_have_attribute("aria-expanded", "false")
        expect(minimize_toggle).to_have_accessible_name("Open orchestration inspector")
        expect(head).to_have_attribute("data-minimized", "true")
        expect(orchestrator_details).to_be_hidden()
        expect(pipeline_details).to_be_hidden()
        expect(board_frame).to_be_visible()
        minimized_frame_box = board_frame.bounding_box()
        assert minimized_frame_box, "the minimized planner frame has no layout box"

        minimize_toggle.evaluate("element => element.click()")
        expect(minimize_toggle).to_have_attribute("aria-expanded", "true")
        expect(minimize_toggle).to_have_accessible_name("Close orchestration inspector")
        expect(head).to_have_attribute("data-minimized", "false")
        expect(orchestrator_details).to_be_visible()
        expect(pipeline_details).to_be_visible()
        expect(page.locator("#pp-btn-close-orchestrator-inspector")).to_be_focused()
        expanded_frame_box = board_frame.bounding_box()
        assert expanded_frame_box, "the expanded planner frame has no layout box"
        assert expanded_frame_box["y"] == minimized_frame_box["y"], (
            "the fixed orchestration inspector displaced the planner viewport"
        )

        # Inspection is deliberately temporary: every fresh load must return the
        # planner canvas to the high, compact starting position.
        page.reload(wait_until="domcontentloaded")
        expect(minimize_toggle).to_have_attribute("aria-expanded", "false")
        expect(head).to_have_attribute("data-minimized", "true")
        page.screenshot(path=str(MINIMIZED_SCREENSHOT), full_page=False)
        minimized_image_size = MINIMIZED_SCREENSHOT.stat().st_size
        assert minimized_image_size > 100_000, (
            f"minimized 3x screenshot is unexpectedly small: {minimized_image_size} bytes"
        )
        minimize_toggle.evaluate("element => element.click()")

        expect(actor).to_be_hidden()
        expect(actor).to_have_attribute("data-role", "head-orchestrator-character")
        expect(actor).to_have_attribute("data-orchestrator-id", head.get_attribute("data-entity-id"))
        expect(actor).to_have_attribute("aria-label", "Head orchestrator character, working")
        expect(speech).to_contain_text("HEAD ORCH → WORKERS")
        expect(speech).to_contain_text("Problem")
        expect(speech).to_contain_text("No blocking issue detected.")
        expect(speech).to_contain_text("Where")
        expect(speech).to_contain_text("Remedy")
        expect(speech.locator("dt")).to_have_count(3)
        expect(speech).to_have_attribute("aria-live", "polite")
        expect(worker_route).to_be_visible()
        expect(spinner).to_be_visible()

        assert actor.locator("svg").count() == 1, "the ORCH actor is not a physical character glyph"
        assert actor.locator(".orch-radio").count() == 1, "the ORCH actor lost its radio"
        assert spinner.locator("#pp-entity-head-orchestrator-character").count() == 0, (
            "the Looplet feed spinner was still being used as the head orchestrator"
        )
        assert page.locator("[data-speaking-to='pp-list-worker-reports']").count() == 1

        assert page.evaluate(
            "getComputedStyle(document.querySelector('#pp-entity-head-orchestrator-character')).animationName"
        ) == "none", "reduced-motion did not stop the actor animation"

        phase["value"] = "lifecycle"
        page.get_by_role("button", name="rescan").evaluate("element => element.click()")
        expect(page.get_by_role("button", name="rescan")).to_be_enabled(timeout=5_000)
        expect(actor).to_have_attribute("aria-label", "Head orchestrator character, holding")
        expect(speech).to_contain_text(
            "The combined workflow is not safe to treat as one automated lifecycle."
        )
        expect(speech).to_contain_text(
            "Perfect Planner Desktop / release lifecycle / $start → Planner → $finish → $cleanup"
        )
        expect(speech).to_contain_text(
            "Planner may invoke $finish, but $finish remains the authoritative release gate; "
            "run $cleanup explicitly and last."
        )
        expect(speech).to_contain_text("recommended action only")

        page.screenshot(path=str(SCREENSHOT), full_page=False)
        image_size = SCREENSHOT.stat().st_size
        assert image_size > 100_000, f"3x screenshot is unexpectedly small: {image_size} bytes"

        phase["value"] = "stale"
        page.get_by_role("button", name="rescan").evaluate("element => element.click()")
        expect(page.get_by_role("button", name="rescan")).to_be_enabled(timeout=5_000)
        expect(speech).to_contain_text("STALE worker heartbeat")
        expect(speech).to_contain_text("Perfect Planner Desktop / PP-ORCH / B22 / worker s-actor")
        expect(speech).to_contain_text("Pause new claims, check the worker heartbeat")
        assert not console_errors, f"browser console errors: {console_errors}"

        context.close()
        browser.close()

    print("head orchestrator character e2e: 1 scenario passed")
    print(f"screenshot: {SCREENSHOT}")
    print(f"minimized screenshot: {MINIMIZED_SCREENSHOT}")


if __name__ == "__main__":
    main()
