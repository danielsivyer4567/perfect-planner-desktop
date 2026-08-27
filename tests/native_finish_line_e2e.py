"""Read-only finish-line proof for a running packaged Perfect Planner WebView2 app.

Launch the packaged app with WebView2 remote debugging enabled, select the intended
repository and plan once, then restart the app before running this script. The script
reloads only the renderer, observes native state, and never creates a run, worker,
message, approval, completion result, or CI result.
"""

from __future__ import annotations

import json
import hashlib
import os
import time
import ctypes
from pathlib import Path

from playwright.sync_api import expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = ROOT / "artifacts" / "native-tauri"
CDP_URL = os.environ.get("PP_NATIVE_CDP_URL", "http://127.0.0.1:9223")
EXPECTED_REPOSITORY = os.environ.get("PP_NATIVE_EXPECT_REPOSITORY", "Perfect Planner")
EXPECTED_PLAN_PATH = os.environ.get(
    "PP_NATIVE_EXPECT_PLAN_PATH",
    str(ROOT / ".claude" / "scratch" / "perfect-plan" / "cross-repository-collision-assessor.json"),
)
EXPECTED_RECOVERY_MARKER = os.environ.get(
    "PP_NATIVE_EXPECT_RECOVERY", "recovery: B20 preserved output"
)
EXPECTED_RECOVERY_ITEM = os.environ.get("PP_NATIVE_EXPECT_RECOVERY_ITEM", "B20")
REAPER_LEDGER = Path(
    os.environ.get(
        "PP_NATIVE_REAPER_LEDGER",
        str(Path(os.environ["APPDATA"]) / "com.looplet.perfectplanner" / "session-reaper.jsonl"),
    )
)
EXPECTED_PLAN_SHA256 = os.environ.get("PP_NATIVE_EXPECT_PLAN_SHA256")
EXPECTED_REAPER_SHA256 = os.environ.get("PP_NATIVE_EXPECT_REAPER_SHA256")
EXPECTED_RECOVERY_EVENT_IDS = {
    value
    for value in os.environ.get("PP_NATIVE_EXPECT_RECOVERY_EVENT_IDS", "").split(",")
    if value
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest().upper()


def selected_native_page(browser):
    pages = [
        page
        for context in browser.contexts
        for page in context.pages
        if page.title() == "perfect planning · boards"
    ]
    assert len(pages) == 1, f"expected one native Perfect Planner WebView, found {len(pages)}"
    return pages[0]


def send_windows_escape() -> bool:
    """Send Escape through the actual foreground Tauri window, not the CDP keyboard shim."""
    if os.name != "nt":
        return False
    user32 = ctypes.windll.user32
    kernel32 = ctypes.windll.kernel32
    window = user32.FindWindowW(None, "perfect planning · boards")
    if not window:
        return False
    target_thread = user32.GetWindowThreadProcessId(window, None)
    current_thread = kernel32.GetCurrentThreadId()
    attached = bool(
        target_thread
        and target_thread != current_thread
        and user32.AttachThreadInput(current_thread, target_thread, True)
    )
    user32.ShowWindow(window, 9)  # SW_RESTORE
    user32.BringWindowToTop(window)
    user32.SetForegroundWindow(window)
    user32.SetFocus(window)
    try:
        time.sleep(0.2)
        user32.keybd_event(0x1B, 0, 0, 0)  # VK_ESCAPE down
        user32.keybd_event(0x1B, 0, 0x0002, 0)  # KEYEVENTF_KEYUP
        time.sleep(0.3)
        return user32.GetForegroundWindow() == window
    finally:
        if attached:
            user32.AttachThreadInput(current_thread, target_thread, False)


def resize_windows_app(width: int, height: int) -> tuple[int, int, int, int] | None:
    if os.name != "nt":
        return None

    class Rect(ctypes.Structure):
        _fields_ = [
            ("left", ctypes.c_long),
            ("top", ctypes.c_long),
            ("right", ctypes.c_long),
            ("bottom", ctypes.c_long),
        ]

    user32 = ctypes.windll.user32
    window = user32.FindWindowW(None, "perfect planning · boards")
    bounds = Rect()
    if not window or not user32.GetWindowRect(window, ctypes.byref(bounds)):
        return None
    original = (
        bounds.left,
        bounds.top,
        bounds.right - bounds.left,
        bounds.bottom - bounds.top,
    )
    if not user32.MoveWindow(window, bounds.left, bounds.top, width, height, True):
        return None
    return original


def restore_windows_app(bounds: tuple[int, int, int, int] | None) -> None:
    if os.name != "nt" or bounds is None:
        return
    window = ctypes.windll.user32.FindWindowW(None, "perfect planning · boards")
    if window:
        ctypes.windll.user32.MoveWindow(window, *bounds, True)


def main() -> None:
    ARTIFACTS.mkdir(parents=True, exist_ok=True)
    console_errors: list[str] = []
    page_errors: list[str] = []
    failed_requests: list[str] = []

    with sync_playwright() as playwright:
        browser = playwright.chromium.connect_over_cdp(CDP_URL)
        page = selected_native_page(browser)
        page.on(
            "console",
            lambda message: console_errors.append(message.text)
            if message.type in {"error", "assert"}
            else None,
        )
        page.on("pageerror", lambda error: page_errors.append(str(error)))
        page.on(
            "requestfailed",
            lambda request: failed_requests.append(
                f"{request.method} {request.url}: {request.failure}"
            ),
        )

        page.reload(wait_until="load")
        page.wait_for_function(
            "document.readyState === 'complete' && Boolean(window.__TAURI_INTERNALS__)"
        )
        active = page.locator("#pp-region-active-board-heading")
        expect(active).to_have_attribute("data-repository-name", EXPECTED_REPOSITORY, timeout=20_000)
        expect(active).to_have_attribute("data-plan-path", EXPECTED_PLAN_PATH)
        expect(page.locator("#pp-status-head-lease")).to_contain_text(
            "LEGACY LEASE + REAPER ACTIVE", timeout=20_000
        )
        active_board_body = page.frame_locator("#pp-frame-active-board").locator("body")
        expect(active_board_body).to_contain_text(EXPECTED_RECOVERY_MARKER, timeout=20_000)
        plan_data = json.loads(Path(EXPECTED_PLAN_PATH).read_text(encoding="utf-8"))
        assert plan_data.get("awaiting", {}).get("item") == EXPECTED_RECOVERY_ITEM
        recovery_nodes = [
            node
            for node in plan_data.get("vertebrae", [])
            if node.get("id") == EXPECTED_RECOVERY_ITEM and node.get("status") == "recovery"
        ]
        assert len(recovery_nodes) == 1, "durable recovery node is missing or duplicated"
        reaper_events = [
            json.loads(line)
            for line in REAPER_LEDGER.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        scoped_recovery_event_ids = {
            event.get("id")
            for event in reaper_events
            if event.get("planPath", "").replace("/", "\\").lower()
            == EXPECTED_PLAN_PATH.replace("/", "\\").lower()
            and event.get("vertebra") == EXPECTED_RECOVERY_ITEM
        }
        assert len(scoped_recovery_event_ids) == 1, (
            f"expected one durable recovery event, found {scoped_recovery_event_ids}"
        )

        header = page.locator("#pp-entity-head-orchestrator").bounding_box()
        tabs = page.locator(".repository-scope-tabs").bounding_box()
        stage_bar = active.bounding_box()
        canvas = page.locator("#pp-frame-active-board").bounding_box()
        assert header and tabs and stage_bar and canvas, "primary workspace geometry is unavailable"
        assert header["height"] <= 90, f"orchestrator header is too tall: {header}"
        assert canvas["y"] <= 180, f"workflow canvas starts too low: {canvas}"
        assert abs(canvas["y"] - (stage_bar["y"] + stage_bar["height"])) <= 2, (
            f"unexpected gap before workflow canvas: stage={stage_bar}, canvas={canvas}"
        )

        frame_top_before = canvas["y"]
        toggle = page.locator("#pp-btn-toggle-orchestrator")
        expect(toggle).to_have_attribute("aria-expanded", "false")
        toggle.click()
        inspector = page.locator("#pp-panel-orchestrator-inspector")
        expect(inspector).to_be_visible()
        expect(inspector.locator("[data-inspector-close]")).to_be_focused()
        frame_top_after = page.locator("#pp-frame-active-board").bounding_box()["y"]
        assert frame_top_after == frame_top_before, (
            f"fixed inspector displaced the canvas: {frame_top_before} -> {frame_top_after}"
        )
        page.evaluate(
            """
            window.__ppEscapeProof = 0;
            window.addEventListener('keydown', event => {
              if (event.key === 'Escape') window.__ppEscapeProof += 1;
            }, { once: true });
            """
        )
        page.keyboard.press("Escape")
        injected_escape_count = page.evaluate("window.__ppEscapeProof")
        if injected_escape_count == 0 and send_windows_escape():
            injected_escape_count = page.evaluate("window.__ppEscapeProof")
        print(f"native injected Escape events: {injected_escape_count}")
        if injected_escape_count == 0:
            page.evaluate(
                "window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))"
            )
        expect(inspector).to_be_hidden()
        expect(toggle).to_be_focused()
        escape_restored_focus = toggle.evaluate(
            "element => element === document.activeElement"
        )

        page.keyboard.press("Tab")
        keyboard_focus_after_tab = page.evaluate(
            """(() => {
              const active = document.activeElement;
              return active ? {
                id: active.id || null,
                tagName: active.tagName,
                text: (active.textContent || '').trim().slice(0, 120),
              } : null;
            })()"""
        )
        assert keyboard_focus_after_tab, "keyboard Tab did not leave a visible active element"
        assert keyboard_focus_after_tab["tagName"] not in {"BODY", "HTML"}, (
            f"keyboard Tab fell back to the document root: {keyboard_focus_after_tab}"
        )

        page.wait_for_timeout(1_500)
        expected_ipc_aborts = [
            failure
            for failure in failed_requests
            if failure.startswith("POST http://ipc.localhost/") and "ERR_ABORTED" in failure
        ]
        unexpected_failed_requests = [
            failure for failure in failed_requests if failure not in expected_ipc_aborts
        ]
        screenshot = ARTIFACTS / "native-finish-line.png"
        page.screenshot(path=str(screenshot), full_page=True)
        original_window = resize_windows_app(900, 700)
        narrow_screenshot = ARTIFACTS / "native-narrow-window.png"
        narrow_proof = None
        if original_window:
            page.wait_for_timeout(500)
            narrow_header = page.locator("#pp-entity-head-orchestrator").bounding_box()
            narrow_canvas = page.locator("#pp-frame-active-board").bounding_box()
            narrow_overflow = page.evaluate(
                "document.documentElement.scrollWidth > window.innerWidth"
            )
            assert narrow_header and narrow_header["height"] <= 90
            assert narrow_canvas and narrow_canvas["y"] <= 180
            assert not narrow_overflow, "native narrow window has horizontal document overflow"
            page.screenshot(path=str(narrow_screenshot), full_page=True)
            narrow_proof = {
                "window": {"width": 900, "height": 700},
                "header": narrow_header,
                "canvas": narrow_canvas,
                "horizontalDocumentOverflow": narrow_overflow,
                "screenshot": str(narrow_screenshot),
            }
            restore_windows_app(original_window)
            page.wait_for_timeout(300)

        scale_screenshot = ARTIFACTS / "native-three-times-scale.png"
        cdp = page.context.new_cdp_session(page)
        try:
            cdp.send(
                "Emulation.setDeviceMetricsOverride",
                {
                    "width": 1152,
                    "height": 686,
                    "deviceScaleFactor": 3,
                    "mobile": False,
                },
            )
            page.wait_for_timeout(500)
            scale_header = page.locator("#pp-entity-head-orchestrator").bounding_box()
            scale_canvas = page.locator("#pp-frame-active-board").bounding_box()
            scale_shell_overflow = page.evaluate(
                "document.documentElement.scrollWidth > window.innerWidth"
            )
            scale_plan_overflow = active_board_body.evaluate(
                "element => element.scrollWidth > element.clientWidth"
            )
            assert scale_header and scale_header["height"] <= 90
            assert scale_canvas and scale_canvas["y"] <= 180
            assert not scale_shell_overflow, (
                "300% WebView scale emulation overflows the Perfect Planner shell"
            )
            page.screenshot(path=str(scale_screenshot), full_page=True)
            scale_proof = {
                "physicalViewport": {"width": 3456, "height": 2058},
                "cssViewport": {"width": 1152, "height": 686},
                "deviceScaleFactor": 3,
                "header": scale_header,
                "canvas": scale_canvas,
                "horizontalShellOverflow": scale_shell_overflow,
                "embeddedPlanHorizontalOverflow": scale_plan_overflow,
                "screenshot": str(scale_screenshot),
            }
        finally:
            cdp.send("Emulation.clearDeviceMetricsOverride")
            cdp.detach()
            page.wait_for_timeout(300)
        proof = {
            "native": page.evaluate("Boolean(window.__TAURI_INTERNALS__)"),
            "repository": active.get_attribute("data-repository-name"),
            "planPath": active.get_attribute("data-plan-path"),
            "recoveryMarker": EXPECTED_RECOVERY_MARKER,
            "recoveryMarkerVisible": True,
            "recoveryItem": EXPECTED_RECOVERY_ITEM,
            "recoveryEventIds": sorted(scoped_recovery_event_ids),
            "planSha256": sha256(Path(EXPECTED_PLAN_PATH)),
            "reaperLedger": str(REAPER_LEDGER),
            "reaperSha256": sha256(REAPER_LEDGER),
            "header": header,
            "tabs": tabs,
            "stageBar": stage_bar,
            "canvas": canvas,
            "inspectorDidNotDisplaceCanvas": frame_top_after == frame_top_before,
            "escapeRestoredFocus": escape_restored_focus,
            "nativeKeyboardInjectionObserved": injected_escape_count > 0,
            "keyboardFocusAfterTab": keyboard_focus_after_tab,
            "narrowWindow": narrow_proof,
            "threeTimesScaleEmulation": scale_proof,
            "consoleErrors": console_errors,
            "pageErrors": page_errors,
            "expectedTauriIpcAborts": expected_ipc_aborts,
            "unexpectedFailedRequests": unexpected_failed_requests,
            "screenshot": str(screenshot),
        }
        proof_path = ARTIFACTS / "native-finish-line.json"
        proof_path.write_text(json.dumps(proof, indent=2), encoding="utf-8")

        assert proof["native"] is True
        if EXPECTED_PLAN_SHA256:
            assert proof["planSha256"] == EXPECTED_PLAN_SHA256.upper(), (
                "selected plan changed during packaged restart"
            )
        if EXPECTED_REAPER_SHA256:
            assert proof["reaperSha256"] == EXPECTED_REAPER_SHA256.upper(), (
                "session reaper ledger changed during packaged restart"
            )
        if EXPECTED_RECOVERY_EVENT_IDS:
            assert scoped_recovery_event_ids == EXPECTED_RECOVERY_EVENT_IDS, (
                "exact recovery event identity changed or duplicated during packaged restart"
            )
        assert not console_errors, f"native WebView console errors: {console_errors}"
        assert not page_errors, f"native WebView page errors: {page_errors}"
        assert not unexpected_failed_requests, (
            f"native WebView unexpected failed requests: {unexpected_failed_requests}"
        )

    print("native_finish_line_e2e: PASS")
    print(f"proof: {proof_path}")
    print(f"screenshot: {screenshot}")


if __name__ == "__main__":
    main()
