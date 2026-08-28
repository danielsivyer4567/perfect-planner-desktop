from pathlib import Path
import re
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
SCREENSHOT = ROOT / "artifacts" / "alarm-deterministic.png"
APP_URL = "http://127.0.0.1:5180/"
PLAN_PATH = r"C:\fixtures\alarm-transition-plan.json"

AUDIO_SPY = """
window.__alarmStarts = 0;
const AudioClass = window.AudioContext || window.webkitAudioContext;
if (AudioClass) {
  const original = AudioClass.prototype.createOscillator;
  AudioClass.prototype.createOscillator = function () {
    const oscillator = original.call(this);
    const start = oscillator.start.bind(oscillator);
    oscillator.start = (...args) => {
      window.__alarmStarts += 1;
      return start(...args);
    };
    return oscillator;
  };
}
"""


def starts(page):
    return page.evaluate("window.__alarmStarts")


def click_rescan(page):
    button = page.get_by_role("button", name="rescan")
    generation = int(button.get_attribute("data-scan-generation"))
    # This deterministic harness owns many mocked loopback routes. Dispatch the semantic click
    # without Playwright's synthetic pointer action, which can wait behind those route callbacks;
    # the generation fence below remains the authoritative completion signal.
    button.dispatch_event("click")
    page.wait_for_function(
        """previous => Number(
          document.querySelector('#pp-btn-rescan-boards')?.dataset.scanGeneration
        ) > previous""",
        arg=generation,
        timeout=15_000,
    )
    expect(button).to_be_enabled(timeout=5_000)


def main():
    SCREENSHOT.parent.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(
            channel="chrome",
            headless=True,
            args=["--autoplay-policy=no-user-gesture-required"],
        )

        # Deterministic transition proof: active -> two stalls -> unchanged -> recovery ->
        # same stalls again. The page sees exactly the same API shape as a real board.
        context = browser.new_context(viewport={"width": 1440, "height": 900})
        page = context.new_page()
        page.add_init_script(AUDIO_SPY)
        phase = {"value": "active"}

        def mock_probe(route):
            parsed = urlparse(route.request.url)
            pieces = parsed.path.strip("/").split("/")
            if (
                len(pieces) != 3
                or pieces[0] != "board-probe"
                or pieces[1] not in {"5232", "5233"}
            ):
                route.fulfill(status=404, json={"ok": False})
                return
            port = int(pieces[1])
            if pieces[2] == "whoami":
                route.fulfill(
                    status=200,
                    json={
                        "ok": True,
                        "planPath": PLAN_PATH,
                        "number": "PP-TEST",
                        "topic": "Alarm transitions",
                        "approved": "yes @ test",
                        "awaiting": {
                            "kind": "user-decision",
                            "item": "A01:2",
                            "since": "2026-08-21T00:00:00.000Z",
                        },
                        "port": port,
                        "pid": 1234 if port == 5232 else 5678,
                    },
                )
                return
            state = "ACTIVE" if phase["value"] == "active" else "STALE"
            route.fulfill(
                status=200,
                json={
                    "asOf": "2026-08-21T00:00:00.000Z",
                    "activeWindowMs": 600000,
                    "workers": {
                        "A01": {"vertebra": "A01", "session": "s-one", "state": state},
                        "A02": {"vertebra": "A02", "session": "s-two", "state": state},
                    },
                },
            )

        page.route("**/board-probe/**", mock_probe)
        page.route("http://127.0.0.1:5232/", lambda route: route.fulfill(body="<html></html>"))
        page.route("http://127.0.0.1:5233/", lambda route: route.fulfill(body="<html></html>"))
        page.goto(APP_URL, wait_until="domcontentloaded")
        orchestrator_toggle = page.locator("#pp-btn-toggle-orchestrator")
        if orchestrator_toggle.get_attribute("aria-expanded") == "false":
            orchestrator_toggle.evaluate("element => element.click()")
        expect(orchestrator_toggle).to_have_attribute("aria-expanded", "true")
        expect(page.locator(".rail-num", has_text="PP-TEST")).to_be_visible()
        assert page.locator("#pp-list-boards .rail-item").count() == 1, (
            "duplicate servers for the same plan must collapse to one plan row"
        )
        expect(page.locator("#pp-frame-active-board")).to_have_attribute(
            "src", re.compile(r":5232/$")
        )
        head = page.locator("#pp-entity-head-orchestrator")
        expect(head).to_be_visible()
        expect(head).to_have_attribute("data-entity-id", re.compile(r"^pp-orchestrator-"))
        expect(page.get_by_role("button", name="DECISION · A01:2")).to_be_visible()
        assert page.evaluate(
            """
            () => {
              const ids = [...document.querySelectorAll('[id]')].map((element) => element.id);
              const controls = [...document.querySelectorAll('button, a, input')];
              return ids.length === new Set(ids).size && controls.every((control) => control.id);
            }
            """
        ), "every command-deck control must have one unique inspectable ID"
        assert page.evaluate(
            """
            () => {
              const head = document.querySelector('#pp-entity-head-orchestrator');
              const workers = [...document.querySelectorAll('[data-worker-id]')]
                .map((element) => element.getAttribute('data-worker-id'));
              return !workers.includes(head?.getAttribute('data-entity-id'));
            }
            """
        ), "head orchestrator ID collided with an observed worker ID"
        first_orchestrator_id = head.get_attribute("data-entity-id")
        second_page = context.new_page()
        second_page.route("**/board-probe/**", mock_probe)
        second_page.route(
            "http://127.0.0.1:5232/", lambda route: route.fulfill(body="<html></html>")
        )
        second_page.goto(APP_URL, wait_until="domcontentloaded")
        second_head = second_page.locator("#pp-entity-head-orchestrator")
        expect(second_head).to_have_attribute("data-entity-id", re.compile(r"^pp-orchestrator-"))
        assert second_head.get_attribute("data-entity-id") != first_orchestrator_id, (
            "parallel command decks reserved the same head ID"
        )
        second_page.close()
        assert starts(page) == 0, "active workers must not sound the alarm"

        phase["value"] = "stale"
        click_rescan(page)
        expect(page.locator(".alarm-state")).to_have_text("2 stalled")
        page.wait_for_function("window.__alarmStarts === 1")
        click_rescan(page)
        page.wait_for_timeout(750)
        assert starts(page) == 1, "an unchanged stale set replayed the alarm"

        phase["value"] = "active"
        click_rescan(page)
        expect(page.locator(".alarm-state")).to_have_text("playing")
        page.wait_for_timeout(3_100)
        expect(page.locator(".alarm-state")).to_have_text("armed")

        phase["value"] = "stale"
        click_rescan(page)
        page.wait_for_function("window.__alarmStarts === 2")
        page.wait_for_timeout(3_100)

        page.get_by_role("button", name="sound on").dispatch_event("click")
        expect(page.get_by_role("button", name="muted")).to_be_visible()
        phase["value"] = "active"
        click_rescan(page)
        phase["value"] = "stale"
        click_rescan(page)
        page.wait_for_timeout(750)
        assert starts(page) == 2, "muted automatic transition produced audio"

        slider = page.get_by_role("slider", name="Alarm volume")
        slider.fill("0.75")
        assert page.evaluate("localStorage.getItem('perfect-planner:stall-volume')") == "0.75"
        page.get_by_role("button", name="test", exact=True).dispatch_event("click")
        page.wait_for_function("window.__alarmStarts === 3")
        expect(page.get_by_role("button", name="sound on")).to_be_visible()
        page.screenshot(path=str(SCREENSHOT), full_page=True)
        context.close()
        browser.close()

    print("alarm_e2e: PASS")
    print("transition proof: grouped=1, unchanged=1, recovered-and-restalled=2, muted=2, test=3")
    print(f"screenshot: {SCREENSHOT}")


if __name__ == "__main__":
    main()
