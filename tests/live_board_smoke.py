from pathlib import Path
import os
import re

from playwright.sync_api import expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
APP_URL = os.environ.get("PP_APP_URL", "http://127.0.0.1:5180/")
SCREENSHOT = ROOT / "artifacts" / "live-board-smoke.png"


def main():
    """Read-only smoke proof against whichever real Planner boards are currently running."""
    SCREENSHOT.parent.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(channel="chrome", headless=True)
        context = browser.new_context(viewport={"width": 1440, "height": 900})
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

        page.goto(APP_URL, wait_until="domcontentloaded")
        rows = page.locator("#pp-list-boards .rail-item")
        expect(rows.first).to_be_visible(timeout=8_000)

        target = rows.nth(1) if rows.count() > 1 else rows.first
        target_port = target.get_attribute("data-board-port")
        assert target_port and target_port.isdigit(), "selected live row has no board port"
        target.click()
        expect(target).to_have_class(re.compile(r"\bon\b"))
        orchestrator_toggle = page.locator("#pp-btn-toggle-orchestrator")
        if orchestrator_toggle.get_attribute("aria-expanded") == "false":
            orchestrator_toggle.evaluate("element => element.click()")
        expect(orchestrator_toggle).to_have_attribute("aria-expanded", "true")
        expect(page.locator("iframe.board")).to_have_attribute("src", re.compile(rf":{target_port}/$"))
        target_topic = target.locator(".rail-topic").inner_text()
        expect(page.locator(".crumb")).to_contain_text(target_topic)
        expect(page.locator("#pp-region-active-board-heading")).to_contain_text(target_topic)

        jump = page.locator("#pp-btn-show-stalled")
        if jump.count():
            jump.click()
            selected = page.locator("#pp-list-boards .rail-item.on").first
            expect(selected).to_be_visible()
            selected_port = selected.get_attribute("data-board-port")
            assert selected_port and selected_port.isdigit(), "stalled jump selected no board"
            expect(page.locator("iframe.board")).to_have_attribute("src", re.compile(rf":{selected_port}/$"))
        else:
            expect(page.locator(".alarm-state")).to_have_text("armed")

        expect(page.locator("#pp-status-looplet-live-feed")).to_be_visible()
        page.screenshot(path=str(SCREENSHOT), full_page=True)
        assert not console_errors, f"browser console errors: {console_errors}"
        assert not page_errors, f"browser page errors: {page_errors}"
        context.close()
        browser.close()

    print("live_board_smoke: PASS")
    print(f"screenshot: {SCREENSHOT}")


if __name__ == "__main__":
    main()
