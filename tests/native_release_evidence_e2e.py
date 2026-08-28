"""Packaged Perfect Planner routing and native process-lifecycle evidence.

This opt-in Windows test launches one explicitly supplied packaged executable, drives
the real Tauri IPC boundary through its attached WebView2 instance, and records one
synthetic evidence message in a dedicated repository scope. It never edits a plan or
claims that this evidence message is live project work.
"""

from __future__ import annotations

import ctypes
import hashlib
import json
import os
import subprocess
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = ROOT / "artifacts" / "native-tauri"
EXECUTABLE = Path(os.environ["PP_NATIVE_RELEASE_EXE"]).resolve()
CDP_PORT = int(os.environ.get("PP_NATIVE_RELEASE_CDP_PORT", "9225"))
CDP_URL = f"http://127.0.0.1:{CDP_PORT}"
APP_DATA = Path(os.environ["APPDATA"]) / "com.looplet.perfectplanner"
LIFECYCLE_LEDGER = APP_DATA / "app-lifecycle.jsonl"
CONTROL_LEDGER = APP_DATA / "control-plane.jsonl"
WINDOW_TITLE = "perfect planning · boards"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest().upper()


def read_json_lines(path: Path) -> list[dict]:
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]


def wait_for_cdp(process: subprocess.Popen, timeout_seconds: float = 30.0) -> None:
    deadline = time.monotonic() + timeout_seconds
    endpoint = f"{CDP_URL}/json/list"
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise AssertionError(f"packaged app exited before WebView2 attached: {process.returncode}")
        try:
            with urllib.request.urlopen(endpoint, timeout=1.0) as response:
                if response.status == 200:
                    return
        except OSError:
            time.sleep(0.1)
    raise AssertionError(f"WebView2 CDP endpoint did not start: {endpoint}")


def native_page(browser):
    pages = [
        page
        for context in browser.contexts
        for page in context.pages
        if page.title() == WINDOW_TITLE
    ]
    assert len(pages) == 1, f"expected one packaged Perfect Planner WebView, found {len(pages)}"
    return pages[0]


def find_window_for_process(process_id: int) -> int:
    user32 = ctypes.windll.user32
    windows: list[int] = []
    callback_type = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)

    @callback_type
    def collect(window, _parameter):
        candidate_process_id = ctypes.c_ulong()
        user32.GetWindowThreadProcessId(window, ctypes.byref(candidate_process_id))
        title_length = user32.GetWindowTextLengthW(window)
        title_buffer = ctypes.create_unicode_buffer(title_length + 1)
        user32.GetWindowTextW(window, title_buffer, len(title_buffer))
        if (
            candidate_process_id.value == process_id
            and user32.IsWindowVisible(window)
            and title_buffer.value == WINDOW_TITLE
        ):
            windows.append(window)
        return True

    user32.EnumWindows(collect, 0)
    assert len(windows) == 1, (
        f"expected one visible top-level window for process {process_id}, found {windows}"
    )
    return windows[0]


def send_windows_key(process_id: int, virtual_key: int) -> None:
    """Send a real Windows keyboard event to the packaged app's foreground window."""
    user32 = ctypes.windll.user32
    kernel32 = ctypes.windll.kernel32
    window = find_window_for_process(process_id)
    target_thread = user32.GetWindowThreadProcessId(window, None)
    current_thread = kernel32.GetCurrentThreadId()
    foreground_window = user32.GetForegroundWindow()
    foreground_thread = user32.GetWindowThreadProcessId(foreground_window, None)
    attached_threads: list[int] = []
    for thread_id in {target_thread, foreground_thread}:
        if (
            thread_id
            and thread_id != current_thread
            and user32.AttachThreadInput(current_thread, thread_id, True)
        ):
            attached_threads.append(thread_id)
    try:
        user32.ShowWindow(window, 9)  # SW_RESTORE
        user32.BringWindowToTop(window)
        user32.SetForegroundWindow(window)
        user32.SetActiveWindow(window)
        time.sleep(0.2)
        assert user32.GetForegroundWindow() == window, (
            "Perfect Planner could not receive physical keyboard focus"
        )
        user32.keybd_event(virtual_key, 0, 0, 0)
        user32.keybd_event(virtual_key, 0, 0x0002, 0)  # KEYEVENTF_KEYUP
        time.sleep(0.3)
        assert user32.GetForegroundWindow() == window, (
            "Perfect Planner lost foreground focus while proving physical keyboard behavior"
        )
    finally:
        for thread_id in attached_threads:
            user32.AttachThreadInput(current_thread, thread_id, False)


def close_native_window() -> None:
    user32 = ctypes.windll.user32
    window = user32.FindWindowW(None, WINDOW_TITLE)
    assert window, "packaged Perfect Planner window was not found for graceful close"
    assert user32.PostMessageW(window, 0x0010, 0, 0), "WM_CLOSE could not be posted"


def wait_for_exit(process: subprocess.Popen, timeout_seconds: float = 20.0) -> int:
    try:
        return process.wait(timeout=timeout_seconds)
    except subprocess.TimeoutExpired as error:
        raise AssertionError("packaged Perfect Planner did not exit after WM_CLOSE") from error


def main() -> None:
    assert os.name == "nt", "native release evidence is Windows-only"
    assert EXECUTABLE.is_file(), f"packaged executable is missing: {EXECUTABLE}"
    ARTIFACTS.mkdir(parents=True, exist_ok=True)
    lifecycle_before = read_json_lines(LIFECYCLE_LEDGER)
    console_errors: list[str] = []
    page_errors: list[str] = []
    failed_requests: list[str] = []
    launch_environment = os.environ.copy()
    launch_environment["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = (
        f"--remote-debugging-port={CDP_PORT}"
    )
    process = subprocess.Popen([str(EXECUTABLE)], env=launch_environment)
    result: dict | None = None
    physical_keyboard: dict | None = None
    screenshot = ARTIFACTS / "native-routed-message-lifecycle.png"
    keyboard_screenshot = ARTIFACTS / "native-physical-keyboard.png"
    try:
        wait_for_cdp(process)
        with sync_playwright() as playwright:
            browser = playwright.chromium.connect_over_cdp(CDP_URL)
            page = native_page(browser)
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
            page.wait_for_function(
                "document.readyState === 'complete' && Boolean(window.__TAURI_INTERNALS__)"
            )
            toggle = page.locator("#pp-btn-toggle-orchestrator")
            toggle.click()
            inspector = page.locator("#pp-panel-orchestrator-inspector")
            expect(inspector).to_be_visible()
            expect(inspector.locator("[data-inspector-close]")).to_be_focused()
            page.evaluate(
                """
                window.__ppPhysicalEscapeCount = 0;
                window.addEventListener('keydown', event => {
                  if (event.key === 'Escape') window.__ppPhysicalEscapeCount += 1;
                });
                """
            )
            send_windows_key(process.pid, 0x1B)  # VK_ESCAPE
            expect(inspector).to_be_hidden()
            expect(toggle).to_be_focused()
            escape_count = page.evaluate("window.__ppPhysicalEscapeCount")
            assert escape_count == 1, f"expected one physical Escape event, received {escape_count}"
            send_windows_key(process.pid, 0x09)  # VK_TAB
            tab_focus = page.evaluate(
                """(() => {
                  const active = document.activeElement;
                  return active ? {
                    id: active.id || null,
                    tagName: active.tagName,
                    text: (active.textContent || '').trim().slice(0, 120),
                  } : null;
                })()"""
            )
            assert tab_focus and tab_focus["tagName"] not in {"BODY", "HTML"}, (
                f"physical Tab did not reach an interactive control: {tab_focus}"
            )
            assert tab_focus["id"] != "pp-btn-toggle-orchestrator", (
                f"physical Tab did not advance focus: {tab_focus}"
            )
            physical_keyboard = {
                "escapeEvents": escape_count,
                "inspectorClosed": True,
                "focusRestoredAfterEscape": True,
                "tabFocus": tab_focus,
                "screenshot": str(keyboard_screenshot),
            }
            page.screenshot(path=str(keyboard_screenshot), full_page=True)
            result = page.evaluate(
                """async ({ repositoryRoot, branch, nonce }) => {
                  const invoke = window.__TAURI_INTERNALS__.invoke;
                  const repositoryId = 'pp-finish-line-native-evidence';
                  const organizationId = 'perfect-planner-verification';
                  const planId = 'finish-line-native-evidence';
                  const consumerId = `finish-line-consumer-${nonce}`;
                  const scope = {
                    organizationId,
                    repositoryId,
                    repositoryRoot,
                    worktreePath: repositoryRoot,
                    branch,
                    planId,
                    planPath: `${repositoryRoot}/FINISH-LINE-TODO.md`,
                    nodeId: 'native-message-proof',
                    itemId: 'native-message-proof',
                    workerId: `finish-line-worker-${nonce}`,
                    orchestratorId: 'finish-line-orchestrator',
                  };
                  const queued = await invoke('post_control_message', { request: {
                    idempotencyKey: `native-finish-line:${nonce}`,
                    correlationId: `native-finish-line:${nonce}`,
                    kind: 'workerNote',
                    scope,
                    authorId: scope.workerId,
                    body: 'Synthetic packaged-app routing evidence; this is not live project work.',
                    destination: {
                      registrationId: `finish-line-worker-route:${nonce}`,
                      kind: 'worker',
                      label: 'Finish-line evidence consumer',
                      address: consumerId,
                      enabled: true,
                      requiresAcknowledgement: true,
                      maxAttempts: 1,
                      retryBaseMs: 5000,
                      registeredAtMs: Date.now(),
                      metadata: { purpose: 'packaged-native-release-evidence' },
                    },
                  }});
                  const claimedMessages = await invoke('claim_control_deliveries', { request: {
                    repositoryId,
                    organizationId,
                    consumerId,
                    destinationKinds: ['worker'],
                    limit: 1,
                    leaseMs: 15000,
                    filter: { branch, planId, nodeId: scope.nodeId, workerId: scope.workerId },
                  }});
                  if (claimedMessages.length !== 1) {
                    throw new Error(`expected one native claim, received ${claimedMessages.length}`);
                  }
                  const claimed = claimedMessages[0];
                  const attempt = claimed.attempts.find(value => value.state === 'claimed');
                  if (!attempt) throw new Error('native claim did not return an active attempt');
                  const delivered = await invoke('record_control_delivery', { request: {
                    repositoryId,
                    messageId: claimed.id,
                    attemptId: attempt.attemptId,
                    consumerId,
                    outcome: 'delivered',
                    error: null,
                    retryAfterMs: null,
                  }});
                  const acknowledged = await invoke('acknowledge_control_message', { request: {
                    repositoryId,
                    messageId: delivered.id,
                    acknowledgedBy: 'finish-line-orchestrator',
                    note: 'Packaged native routing lifecycle verified.',
                  }});
                  const snapshot = await invoke('control_plane_snapshot', { request: {
                    repositoryId,
                    organizationId,
                  }});
                  return {
                    scope,
                    consumerId,
                    messageId: acknowledged.id,
                    states: [queued.state, claimed.state, delivered.state, acknowledged.state],
                    acknowledgement: acknowledged.acknowledgement,
                    snapshotState: snapshot.messages.find(value => value.id === acknowledged.id)?.state,
                    stateCounts: snapshot.stateCounts,
                  };
                }""",
                {
                    "repositoryRoot": str(ROOT),
                    "branch": os.environ.get("PP_NATIVE_RELEASE_BRANCH", "unknown"),
                    "nonce": f"{int(time.time() * 1000)}-{process.pid}",
                },
            )
            assert result["states"] == ["queued", "claimed", "delivered", "acknowledged"]
            assert result["snapshotState"] == "acknowledged"
            assert result["acknowledgement"]["acknowledgedBy"] == "finish-line-orchestrator"
            page.screenshot(path=str(screenshot), full_page=True)
        close_native_window()
        exit_code = wait_for_exit(process)
        assert exit_code == 0, f"packaged app exited with code {exit_code}"
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=10)

    lifecycle_after = read_json_lines(LIFECYCLE_LEDGER)
    added = lifecycle_after[len(lifecycle_before) :]
    sessions = {}
    for event in added:
        sessions.setdefault(event.get("sessionId"), []).append(event)
    matching = [
        events
        for events in sessions.values()
        if {event.get("kind") for event in events} == {"LAUNCH", "EXIT"}
        and all(event.get("processId") == process.pid for event in events)
    ]
    assert len(matching) == 1, f"expected one correlated native launch/exit pair, found {matching}"
    lifecycle_pair = sorted(matching[0], key=lambda event: event["atMs"])
    assert lifecycle_pair[0]["kind"] == "LAUNCH"
    assert lifecycle_pair[1]["kind"] == "EXIT"
    assert lifecycle_pair[1]["atMs"] >= lifecycle_pair[0]["atMs"]
    unexpected_failures = [
        failure
        for failure in failed_requests
        if not (failure.startswith("POST http://ipc.localhost/") and "ERR_ABORTED" in failure)
    ]
    assert not console_errors, f"packaged WebView console errors: {console_errors}"
    assert not page_errors, f"packaged WebView page errors: {page_errors}"
    assert not unexpected_failures, f"packaged WebView request failures: {unexpected_failures}"
    proof = {
        "executable": str(EXECUTABLE),
        "executableSha256": sha256(EXECUTABLE),
        "processId": process.pid,
        "messageLifecycle": result,
        "physicalKeyboard": physical_keyboard,
        "appLifecycle": lifecycle_pair,
        "lifecycleLedger": str(LIFECYCLE_LEDGER),
        "lifecycleLedgerSha256": sha256(LIFECYCLE_LEDGER),
        "controlPlaneLedger": str(CONTROL_LEDGER),
        "controlPlaneLedgerSha256": sha256(CONTROL_LEDGER),
        "consoleErrors": console_errors,
        "pageErrors": page_errors,
        "unexpectedFailedRequests": unexpected_failures,
        "screenshot": str(screenshot),
    }
    proof_path = ARTIFACTS / "native-routed-message-lifecycle.json"
    proof_path.write_text(json.dumps(proof, indent=2), encoding="utf-8")
    print("native_release_evidence_e2e: PASS")
    print(f"message: {result['messageId']}")
    print(f"lifecycle session: {lifecycle_pair[0]['sessionId']}")
    print(f"proof: {proof_path}")


if __name__ == "__main__":
    main()
