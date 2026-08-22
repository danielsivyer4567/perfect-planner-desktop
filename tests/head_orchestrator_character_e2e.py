from pathlib import Path
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
APP_URL = "http://127.0.0.1:5180/"
PLAN_PATH = r"C:\fixtures\head-orchestrator-character.json"
SCREENSHOT = ROOT / "artifacts" / "head-orchestrator-character-3x.png"


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
                        "awaiting": None,
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
                        "B22": {"vertebra": "B22", "session": "s-actor", "state": "ACTIVE"},
                        "B15": {"vertebra": "B15", "session": "s-bridge", "state": "ACTIVE"},
                    },
                },
            )

        page.route("**/board-probe/**", mock_probe)
        page.route("http://127.0.0.1:5232/", lambda route: route.fulfill(body="<html></html>"))
        page.goto(APP_URL)
        page.wait_for_load_state("networkidle")

        head = page.locator("#pp-entity-head-orchestrator")
        actor = page.locator("#pp-entity-head-orchestrator-character")
        speech = page.locator("#pp-status-head-orchestrator-speech")
        worker_route = page.locator("#pp-list-worker-reports")
        spinner = page.locator("#pp-status-looplet-live-feed")

        expect(head).to_be_visible()
        expect(actor).to_be_visible()
        expect(actor).to_have_attribute("data-role", "head-orchestrator-character")
        expect(actor).to_have_attribute("data-orchestrator-id", head.get_attribute("data-entity-id"))
        expect(actor).to_have_attribute("aria-label", "Head orchestrator character, working")
        expect(speech).to_contain_text("HEAD ORCH → WORKERS")
        expect(speech).to_contain_text("Keep moving clockwise. 2 active workers")
        expect(worker_route).to_be_visible()
        expect(spinner).to_be_visible()

        assert actor.locator("svg").count() == 1, "the ORCH actor is not a physical character glyph"
        assert actor.locator(".orch-radio").count() == 1, "the ORCH actor lost its radio"
        assert spinner.locator("#pp-entity-head-orchestrator-character").count() == 0, (
            "the Looplet feed spinner was still being used as the head orchestrator"
        )
        assert page.locator("[data-speaking-to='pp-list-worker-reports']").count() == 1

        actor_box = actor.bounding_box()
        route_box = worker_route.bounding_box()
        assert actor_box and route_box
        assert actor_box["y"] + actor_box["height"] <= route_box["y"], (
            "the head orchestrator character is not physically above the worker route"
        )
        assert page.evaluate(
            "getComputedStyle(document.querySelector('#pp-entity-head-orchestrator-character')).animationName"
        ) == "none", "reduced-motion did not stop the actor animation"
        assert not console_errors, f"browser console errors: {console_errors}"

        page.screenshot(path=str(SCREENSHOT), full_page=False)
        image_size = SCREENSHOT.stat().st_size
        assert image_size > 100_000, f"3x screenshot is unexpectedly small: {image_size} bytes"

        context.close()
        browser.close()

    print("head orchestrator character e2e: 1 scenario passed")
    print(f"screenshot: {SCREENSHOT}")


if __name__ == "__main__":
    main()
