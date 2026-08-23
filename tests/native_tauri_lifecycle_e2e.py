"""Opt-in E2E driver for a running Tauri WebView2 development instance.

Start the native app with WebView2 remote debugging enabled, then provide the exact
pre-created run identity through PP_NATIVE_RUN_ID. The driver never fabricates worker
identity, lease data, changed files, verification results, or clocks; every lifecycle
transition is invoked through the visible production UI and native Tauri commands. The
optional ``full`` phase simulates one worker by writing only the fixture plan's single
allowed file after native admission, then completes before the renewed lease expires.
"""

from __future__ import annotations

import os
from pathlib import Path

from playwright.sync_api import expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = ROOT / "artifacts" / "native-tauri"
CDP_URL = os.environ.get("PP_NATIVE_CDP_URL", "http://127.0.0.1:9223")
RUN_ID = os.environ["PP_NATIVE_RUN_ID"]
BOARD_PORT = os.environ.get("PP_NATIVE_DEMO_PORT", "5232")
PHASE = os.environ.get("PP_NATIVE_PHASE", "admit").lower()


def selected_native_page(browser):
    pages = [
        page
        for context in browser.contexts
        for page in context.pages
        if page.title() == "perfect planning · boards"
    ]
    assert len(pages) == 1, f"expected one native Perfect Planner WebView, found {len(pages)}"
    return pages[0]


def select_run(page) -> None:
    card = page.locator(f'[data-board-port="{BOARD_PORT}"]')
    expect(card).to_be_visible(timeout=15_000)
    card.dispatch_event("click")
    expect(page.locator("#pp-orch-btn-load-runs")).to_be_visible(timeout=10_000)
    load = page.locator("#pp-orch-btn-load-runs")
    expect(load).to_be_enabled(timeout=15_000)
    load.dispatch_event("click")
    saved = page.locator(f'[data-run-id="{RUN_ID}"]')
    try:
        expect(saved).to_be_visible(timeout=15_000)
    except AssertionError:
        catalog = page.evaluate(
            """async repositoryRoot => {
              const api = await import('/src/services/orchestratorPipeline.ts');
              return api.orchestratorRunCatalog({ repositoryRoot });
            }""",
            str(ROOT / "src-tauri" / "target" / "native-demo-repo"),
        )
        print(f"native catalog response: {catalog}")
        error = page.locator("#pp-orch-error-pipeline")
        if error.count():
            print(f"native catalog error: {error.text_content()}")
        raise
    saved.dispatch_event("click")
    expect(page.locator("#pp-orch-heading-preflight")).to_be_visible(timeout=15_000)
    assert page.evaluate("Boolean(window.__TAURI_INTERNALS__)") is True


def expand_node(page) -> None:
    node = page.locator("#pp-orch-node-A01")
    expect(node).to_be_visible(timeout=10_000)
    if node.get_attribute("open") is None:
        page.locator("#pp-orch-btn-toggle-node-A01").click()
    expect(page.locator("#pp-orch-btn-admit-A01")).to_be_visible()


def admit(page) -> None:
    if page.locator("#pp-orch-status-preflight-expired").count():
        page.locator("#pp-orch-btn-run-preflight").click()
        expect(page.locator("#pp-orch-status-run-awaiting-approval")).to_be_visible(
            timeout=30_000
        )
    if page.locator("#pp-orch-status-run-approved").count() == 0:
        if page.locator("#pp-orch-status-run-awaiting-approval").count() == 0:
            page.locator("#pp-orch-btn-run-preflight").dispatch_event("click")
            expect(page.locator("#pp-orch-status-run-awaiting-approval")).to_be_visible(
                timeout=30_000
            )
        approve = page.locator("#pp-orch-btn-approve-run")
        expect(approve).to_be_enabled(timeout=10_000)
        approve.dispatch_event("click")
        expect(page.locator("#pp-orch-status-run-approved")).to_be_visible(timeout=120_000)
    expand_node(page)
    node = page.locator("#pp-orch-node-A01")
    if (node.get_attribute("data-node-status") or "").lower() == "ready":
        admit_button = page.locator("#pp-orch-btn-admit-A01")
        expect(admit_button).to_be_enabled(timeout=10_000)
        admit_button.dispatch_event("click")
    expect(node).to_have_attribute("data-node-status", "RUNNING", timeout=30_000)
    expect(node).not_to_have_attribute("data-worker-id", "unclaimed")
    expect(node).not_to_have_attribute("data-lease-fence", "none")
    expect(page.locator("#pp-orch-btn-heartbeat-A01")).to_be_enabled()
    expect(page.locator("#pp-orch-btn-complete-A01")).to_be_enabled()
    page.locator("#pp-orch-btn-heartbeat-A01").dispatch_event("click")


def complete(page) -> None:
    expand_node(page)
    node = page.locator("#pp-orch-node-A01")
    if (node.get_attribute("data-node-status") or "").lower() == "ready":
        # The two opt-in phases may be run separately. If the short native lease
        # expired between them, prove recovery by reacquiring through the same UI.
        admit(page)
    expect(node).to_have_attribute("data-node-status", "RUNNING", timeout=30_000)
    page.locator("#pp-orch-btn-heartbeat-A01").click()
    expect(page.locator("#pp-orch-btn-complete-A01")).to_be_enabled(timeout=10_000)
    page.locator("#pp-orch-btn-complete-A01").click()
    expect(node).to_have_attribute("data-node-status", "DONE", timeout=60_000)
    expect(page.locator("#pp-orch-pipeline-console")).to_contain_text(
        "git status --short", timeout=15_000
    )
    expect(page.locator("#pp-orch-pipeline-console")).to_contain_text(
        "Document Diff", timeout=15_000
    )


def main() -> None:
    assert PHASE in {"admit", "complete", "full"}, f"unsupported phase {PHASE}"
    ARTIFACTS.mkdir(parents=True, exist_ok=True)
    console_errors: list[str] = []
    page_errors: list[str] = []
    with sync_playwright() as playwright:
        browser = playwright.chromium.connect_over_cdp(CDP_URL)
        page = selected_native_page(browser)
        page.on(
            "console",
            lambda message: console_errors.append(message.text)
            if message.type == "error"
            else None,
        )
        page.on("pageerror", lambda error: page_errors.append(str(error)))
        page.reload(wait_until="load")
        page.wait_for_function(
            "document.readyState === 'complete' && Boolean(window.__TAURI_INTERNALS__)"
        )
        page.wait_for_timeout(1_500)
        select_run(page)
        if PHASE == "admit":
            admit(page)
        elif PHASE == "complete":
            complete(page)
        else:
            admit(page)
            demo_file = ROOT / "src-tauri" / "target" / "native-demo-repo" / "demo-two.txt"
            assert not demo_file.exists(), "full-phase fixture target must start absent"
            demo_file.write_text(
                "Perfect Planner native lifecycle proof two.\n"
                "This bounded edit was created only after A01 received native authority.\n",
                encoding="utf-8",
            )
            complete(page)
        screenshot = ARTIFACTS / f"native-{PHASE}.png"
        page.screenshot(path=str(screenshot), full_page=True)
        assert not page_errors, f"native WebView page errors: {page_errors}"
        assert not console_errors, f"native WebView console errors: {console_errors}"
        # This is an attached native WebView2 instance, not a browser launched by
        # Playwright. Closing it here would terminate the desktop application.

    print(f"native_tauri_lifecycle_e2e ({PHASE}): PASS")
    print(f"run: {RUN_ID}")
    print(f"screenshot: {screenshot}")


if __name__ == "__main__":
    main()
