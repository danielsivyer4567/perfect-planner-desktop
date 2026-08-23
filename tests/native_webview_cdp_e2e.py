"""Raw-CDP proof driver for the running Perfect Planner Tauri WebView.

Unlike Playwright's Browser object, this client owns only one DevTools websocket and
never sends Browser.close. Lifecycle transitions still come from semantic clicks on
the production UI; no worker identity, token, fence, clock, file list, or result is
supplied by the test.
"""

from __future__ import annotations

import base64
import json
import os
import time
import urllib.request
from pathlib import Path

import websocket


ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = ROOT / "artifacts" / "native-tauri"
CDP_HTTP = os.environ.get("PP_NATIVE_CDP_URL", "http://127.0.0.1:9223").rstrip("/")
RUN_ID = os.environ["PP_NATIVE_RUN_ID"]
BOARD_PORT = os.environ.get("PP_NATIVE_DEMO_PORT", "5232")
PHASE = os.environ.get("PP_NATIVE_PHASE", "admit").lower()


class Cdp:
    def __init__(self, url: str) -> None:
        self.ws = websocket.create_connection(url, timeout=10, origin="http://127.0.0.1:9223")
        self.sequence = 0
        self.errors: list[str] = []

    def call(self, method: str, params: dict | None = None) -> dict:
        self.sequence += 1
        request_id = self.sequence
        self.ws.send(json.dumps({"id": request_id, "method": method, "params": params or {}}))
        while True:
            message = json.loads(self.ws.recv())
            event = message.get("method")
            if event == "Runtime.exceptionThrown":
                details = message.get("params", {}).get("exceptionDetails", {})
                self.errors.append(details.get("text", "unhandled page exception"))
            elif event == "Runtime.consoleAPICalled":
                payload = message.get("params", {})
                if payload.get("type") in {"error", "assert"}:
                    values = [arg.get("value", arg.get("description", "")) for arg in payload.get("args", [])]
                    self.errors.append(" ".join(str(value) for value in values))
            if message.get("id") != request_id:
                continue
            if "error" in message:
                raise AssertionError(f"CDP {method} failed: {message['error']}")
            return message.get("result", {})

    def evaluate(self, expression: str):
        result = self.call(
            "Runtime.evaluate",
            {
                "expression": expression,
                "awaitPromise": True,
                "returnByValue": True,
                "userGesture": True,
            },
        ).get("result", {})
        if result.get("subtype") == "error":
            raise AssertionError(result.get("description", "JavaScript evaluation failed"))
        return result.get("value")

    def close(self) -> None:
        self.ws.close()


def target_websocket() -> str:
    with urllib.request.urlopen(f"{CDP_HTTP}/json", timeout=5) as response:
        targets = json.load(response)
    pages = [target for target in targets if target.get("title") == "perfect planning · boards"]
    assert len(pages) == 1, f"expected one Perfect Planner WebView, found {len(pages)}"
    return pages[0]["webSocketDebuggerUrl"]


def js_selector(selector: str) -> str:
    return json.dumps(selector)


def exists(cdp: Cdp, selector: str) -> bool:
    return bool(cdp.evaluate(f"Boolean(document.querySelector({js_selector(selector)}))"))


def text(cdp: Cdp, selector: str) -> str:
    return str(
        cdp.evaluate(
            f"document.querySelector({js_selector(selector)})?.textContent || ''"
        )
    )


def attribute(cdp: Cdp, selector: str, name: str) -> str | None:
    return cdp.evaluate(
        f"document.querySelector({js_selector(selector)})?.getAttribute({json.dumps(name)}) ?? null"
    )


def wait_until(cdp: Cdp, predicate, label: str, timeout: float = 30) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.25)
    body = str(cdp.evaluate("document.body?.innerText || ''"))[-4_000:]
    raise AssertionError(f"timed out waiting for {label}\n--- visible tail ---\n{body}")


def click(cdp: Cdp, selector: str) -> None:
    clicked = cdp.evaluate(
        """(() => {
          const element = document.querySelector(%s);
          if (!element || element.disabled) return false;
          element.click();
          return true;
        })()""" % js_selector(selector)
    )
    assert clicked, f"control is absent or disabled: {selector}"


def wait_enabled(cdp: Cdp, selector: str, label: str, timeout: float = 30) -> None:
    wait_until(
        cdp,
        lambda: exists(cdp, selector)
        and not bool(cdp.evaluate(f"document.querySelector({js_selector(selector)})?.disabled")),
        label,
        timeout,
    )


def fail_if_pipeline_error(cdp: Cdp) -> None:
    if exists(cdp, "#pp-orch-error-pipeline"):
        raise AssertionError(text(cdp, "#pp-orch-error-pipeline"))


def select_run(cdp: Cdp) -> None:
    board = f'[data-board-port="{BOARD_PORT}"]'
    wait_until(cdp, lambda: exists(cdp, board), f"board on port {BOARD_PORT}", 20)
    click(cdp, board)
    wait_until(
        cdp,
        lambda: attribute(cdp, board, "aria-pressed") == "true",
        "exact board selection",
    )
    if attribute(cdp, "#pp-btn-toggle-orchestrator", "aria-expanded") == "false":
        click(cdp, "#pp-btn-toggle-orchestrator")
    wait_until(
        cdp,
        lambda: attribute(cdp, "#pp-btn-toggle-orchestrator", "aria-expanded") == "true",
        "expanded orchestrator header",
    )
    if RUN_ID in text(cdp, "#pp-orch-pipeline-console"):
        assert cdp.evaluate("Boolean(window.__TAURI_INTERNALS__)") is True
        return
    wait_until(cdp, lambda: exists(cdp, "#pp-orch-btn-load-runs"), "saved-run control")
    wait_until(
        cdp,
        lambda: not bool(cdp.evaluate("document.querySelector('#pp-orch-btn-load-runs')?.disabled")),
        "enabled saved-run control",
        30,
    )
    click(cdp, "#pp-orch-btn-load-runs")
    saved = f'[data-run-id="{RUN_ID}"]'
    wait_until(cdp, lambda: exists(cdp, saved), f"saved run {RUN_ID}", 30)
    click(cdp, saved)
    wait_until(
        cdp,
        lambda: RUN_ID in text(cdp, "#pp-orch-pipeline-console"),
        "exact native run snapshot",
        30,
    )
    assert cdp.evaluate("Boolean(window.__TAURI_INTERNALS__)") is True


def approve_and_admit(cdp: Cdp) -> None:
    if exists(cdp, "#pp-orch-status-preflight-expired"):
        wait_enabled(cdp, "#pp-orch-btn-run-preflight", "enabled preflight refresh")
        click(cdp, "#pp-orch-btn-run-preflight")
        wait_until(
            cdp,
            lambda: exists(cdp, "#pp-orch-status-run-awaiting-approval"),
            "fresh preflight awaiting approval",
            60,
        )
    if not exists(cdp, "#pp-orch-status-run-approved"):
        if not exists(cdp, "#pp-orch-status-run-awaiting-approval"):
            wait_enabled(cdp, "#pp-orch-btn-run-preflight", "enabled preflight")
            click(cdp, "#pp-orch-btn-run-preflight")
            wait_until(
                cdp,
                lambda: exists(cdp, "#pp-orch-status-run-awaiting-approval"),
                "preflight awaiting approval",
                60,
            )
        wait_until(
            cdp,
            lambda: not bool(cdp.evaluate("document.querySelector('#pp-orch-btn-approve-run')?.disabled")),
            "enabled explicit approval",
        )
        click(cdp, "#pp-orch-btn-approve-run")
        wait_until(
            cdp,
            lambda: exists(cdp, "#pp-orch-status-run-approved")
            or exists(cdp, "#pp-orch-error-pipeline"),
            "native approval receipt",
            120,
        )
        fail_if_pipeline_error(cdp)

    cdp.evaluate("document.querySelector('#pp-orch-node-A01')?.setAttribute('open', '')")
    node = "#pp-orch-node-A01"
    if (attribute(cdp, node, "data-node-status") or "").lower() == "ready":
        click(cdp, "#pp-orch-btn-admit-A01")
    wait_until(
        cdp,
        lambda: attribute(cdp, node, "data-node-status") == "RUNNING"
        or exists(cdp, "#pp-orch-error-pipeline"),
        "authority-backed running lease",
        60,
    )
    fail_if_pipeline_error(cdp)
    assert attribute(cdp, node, "data-worker-id") not in {None, "unclaimed"}
    assert attribute(cdp, node, "data-lease-fence") not in {None, "none"}
    wait_until(
        cdp,
        lambda: "NATIVE LEASE ACTIVE" in text(cdp, "#pp-status-head-lease"),
        "pipeline-backed head status",
    )
    assert attribute(cdp, '[data-pipeline-node-id="A01"]', "data-worker-id") == attribute(
        cdp, node, "data-worker-id"
    )


def complete(cdp: Cdp) -> None:
    node = "#pp-orch-node-A01"
    cdp.evaluate("document.querySelector('#pp-orch-node-A01')?.setAttribute('open', '')")
    wait_until(
        cdp,
        lambda: attribute(cdp, node, "data-node-status") in {"RUNNING", "DONE"}
        or exists(cdp, "#pp-orch-error-pipeline"),
        "stable running or completed node snapshot",
    )
    fail_if_pipeline_error(cdp)
    if attribute(cdp, node, "data-node-status") == "RUNNING":
        click(cdp, "#pp-orch-btn-heartbeat-A01")
        wait_enabled(
            cdp,
            "#pp-orch-btn-complete-A01",
            "completion control after heartbeat",
        )
        click(cdp, "#pp-orch-btn-complete-A01")
        wait_until(
            cdp,
            lambda: attribute(cdp, node, "data-node-status") == "DONE"
            or exists(cdp, "#pp-orch-error-pipeline"),
            "atomic native completion",
            120,
        )
    fail_if_pipeline_error(cdp)
    assert attribute(cdp, node, "data-node-status") == "DONE"
    assert bool(cdp.evaluate("document.querySelector('#pp-orch-btn-complete-A01')?.disabled"))
    for evidence_kind in ("document-diff", "command-output", "exit-code"):
        assert exists(cdp, f'[data-evidence-kind="{evidence_kind}"]')
    wait_until(
        cdp,
        lambda: "PIPELINE COMPLETE" in text(cdp, "#pp-status-head-lease"),
        "completed pipeline-backed head status",
    )
    assert "1 nodes" in text(cdp, "#pp-stat-worker-reports")
    assert "1 done" in text(cdp, "#pp-stat-completed")
    assert attribute(cdp, '[data-pipeline-node-id="A01"]', "data-worker-id") not in {
        None,
        "unclaimed",
    }
    if not exists(cdp, "#pp-panel-diagnostics-console"):
        click(cdp, "#pp-btn-diagnostics-toggle")
    wait_until(
        cdp,
        lambda: attribute(cdp, '[data-collision-state="completed"]', "data-state") == "ready",
        "completed collision guarantee diagnostic",
    )


def screenshot(cdp: Cdp, phase: str) -> Path:
    ARTIFACTS.mkdir(parents=True, exist_ok=True)
    target = ARTIFACTS / f"native-raw-{phase}.png"
    result = cdp.call(
        "Page.captureScreenshot",
        {"format": "png", "captureBeyondViewport": True, "fromSurface": True},
    )
    target.write_bytes(base64.b64decode(result["data"]))
    return target


def main() -> None:
    assert PHASE in {"admit", "complete"}, f"unsupported phase {PHASE}"
    cdp = Cdp(target_websocket())
    try:
        cdp.call("Runtime.enable")
        cdp.call("Page.enable")
        select_run(cdp)
        if PHASE == "admit":
            approve_and_admit(cdp)
        else:
            complete(cdp)
        proof = screenshot(cdp, PHASE)
        assert not cdp.errors, f"native WebView errors: {cdp.errors}"
    finally:
        cdp.close()

    print(f"native_webview_cdp_e2e ({PHASE}): PASS")
    print(f"run: {RUN_ID}")
    print(f"screenshot: {proof}")


if __name__ == "__main__":
    main()
