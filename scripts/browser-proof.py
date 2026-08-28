"""Cross-platform browser proof fallback for Perfect Planner.

Chrome MCP remains the preferred interactive controller. This script is the
deterministic fallback for CI, non-visual models, and hosts where MCP browser
control is unavailable. It never claims to have used MCP: every report records
the controller and browser engine that actually ran.
"""

from __future__ import annotations

import argparse
import json
import struct
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from playwright.sync_api import Error as PlaywrightError
from playwright.sync_api import TimeoutError as PlaywrightTimeoutError
from playwright.sync_api import sync_playwright


ROOT = Path(__file__).resolve().parents[1]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Capture a full-page browser proof with machine-readable logs."
    )
    parser.add_argument("--url", required=True, help="Exact page URL to test.")
    parser.add_argument(
        "--out",
        default=str(ROOT / "artifacts" / "browser-proof"),
        help="Artifact directory (default: artifacts/browser-proof).",
    )
    parser.add_argument("--name", default="browser-proof", help="Artifact name prefix.")
    parser.add_argument(
        "--browser",
        choices=("auto", "chrome", "chromium"),
        default="auto",
        help="Browser engine. auto tries installed Chrome, then bundled Chromium.",
    )
    parser.add_argument("--width", type=int, default=1440)
    parser.add_argument("--height", type=int, default=900)
    parser.add_argument("--scale", type=float, default=2.0)
    parser.add_argument("--timeout-ms", type=int, default=15_000)
    parser.add_argument("--click", action="append", default=[], help="Selector to click, in order.")
    parser.add_argument(
        "--expand-scroll",
        action="append",
        default=[],
        help="Scrollable selector to expand before the full-page capture.",
    )
    parser.add_argument(
        "--expect", action="append", default=[], help="Selector that must be visible after clicks."
    )
    return parser.parse_args()


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def png_dimensions(path: Path) -> dict[str, int]:
    with path.open("rb") as handle:
        signature = handle.read(24)
    if len(signature) != 24 or signature[:8] != b"\x89PNG\r\n\x1a\n":
        raise AssertionError(f"screenshot is not a valid PNG: {path}")
    width, height = struct.unpack(">II", signature[16:24])
    return {"width": width, "height": height}


def write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False), encoding="utf-8")


def main() -> int:
    args = parse_args()
    if args.width < 320 or args.height < 240 or not 1 <= args.scale <= 4:
        raise SystemExit("viewport must be at least 320x240 and scale must be between 1 and 4")

    output = Path(args.out).expanduser().resolve()
    output.mkdir(parents=True, exist_ok=True)
    screenshot_path = output / f"{args.name}-full.png"
    events_path = output / f"{args.name}-events.jsonl"
    report_path = output / f"{args.name}-report.json"
    started_at = utc_now()
    started_clock = time.monotonic()
    console_events: list[dict[str, Any]] = []
    page_errors: list[str] = []
    failed_requests: list[dict[str, Any]] = []
    http_errors: list[dict[str, Any]] = []
    steps: list[dict[str, Any]] = []
    fallback_reason: str | None = None
    browser_engine = "unknown"
    final_url = args.url
    title = ""
    fatal_error: str | None = None
    response_status: int | None = None

    try:
        with sync_playwright() as playwright:
            browser = None
            if args.browser in {"auto", "chrome"}:
                try:
                    browser = playwright.chromium.launch(channel="chrome", headless=True)
                    browser_engine = "chrome"
                except PlaywrightError as error:
                    if args.browser == "chrome":
                        raise
                    fallback_reason = f"installed Chrome unavailable: {error}"
            if browser is None:
                browser = playwright.chromium.launch(headless=True)
                browser_engine = "chromium"

            context = browser.new_context(
                viewport={"width": args.width, "height": args.height},
                device_scale_factor=args.scale,
                reduced_motion="reduce",
            )
            page = context.new_page()
            page.set_default_timeout(args.timeout_ms)
            page.on(
                "console",
                lambda message: console_events.append(
                    {
                        "type": message.type,
                        "text": message.text,
                        "location": message.location,
                    }
                ),
            )
            page.on("pageerror", lambda error: page_errors.append(str(error)))
            page.on(
                "requestfailed",
                lambda request: failed_requests.append(
                    {
                        "method": request.method,
                        "url": request.url,
                        "failure": request.failure,
                    }
                ),
            )
            page.on(
                "response",
                lambda response: http_errors.append(
                    {"status": response.status, "url": response.url}
                )
                if response.status >= 400
                else None,
            )

            response = page.goto(args.url, wait_until="load")
            response_status = response.status if response else None
            title = page.title()
            steps.append({"action": "navigate", "url": args.url, "status": response_status, "ok": True})

            for selector in args.click:
                locator = page.locator(selector)
                locator.wait_for(state="visible")
                locator.evaluate("element => element.click()")
                steps.append(
                    {"action": "click", "selector": selector, "method": "dom-click", "ok": True}
                )

            for selector in args.expect:
                locator = page.locator(selector)
                locator.first.wait_for(state="visible")
                steps.append(
                    {
                        "action": "expectVisible",
                        "selector": selector,
                        "matches": locator.count(),
                        "ok": True,
                    }
                )

            for selector in args.expand_scroll:
                locator = page.locator(selector).first
                locator.wait_for(state="visible")
                dimensions = locator.evaluate(
                    """element => {
                      const dimensions = {
                        scrollWidth: element.scrollWidth,
                        scrollHeight: element.scrollHeight,
                        clientWidth: element.clientWidth,
                        clientHeight: element.clientHeight,
                      };
                      element.style.width = `${Math.max(element.clientWidth, element.scrollWidth)}px`;
                      element.style.height = `${element.scrollHeight}px`;
                      element.style.maxHeight = 'none';
                      element.style.overflow = 'visible';
                      let parent = element.parentElement;
                      while (parent) {
                        parent.style.height = 'auto';
                        parent.style.maxHeight = 'none';
                        parent.style.overflow = 'visible';
                        parent = parent.parentElement;
                      }
                      document.documentElement.style.height = 'auto';
                      document.body.style.height = 'auto';
                      return dimensions;
                    }"""
                )
                steps.append(
                    {
                        "action": "expandScroll",
                        "selector": selector,
                        "dimensions": dimensions,
                        "ok": True,
                    }
                )

            page.screenshot(path=str(screenshot_path), full_page=True, animations="disabled")
            final_url = page.url
            context.close()
            browser.close()
    except (AssertionError, PlaywrightError, PlaywrightTimeoutError, OSError) as error:
        fatal_error = f"{type(error).__name__}: {error}"

    screenshot = None
    if screenshot_path.is_file():
        screenshot = {
            "path": str(screenshot_path),
            "fullPage": True,
            **png_dimensions(screenshot_path),
        }

    console_errors = [event for event in console_events if event["type"] in {"error", "assert"}]
    aborted_requests = [
        event for event in failed_requests if str(event.get("failure", "")) == "net::ERR_ABORTED"
    ]
    unexpected_failed_requests = [
        event for event in failed_requests if event not in aborted_requests
    ]
    passed = not any(
        (
            fatal_error,
            page_errors,
            unexpected_failed_requests,
            console_errors,
            response_status is not None and response_status >= 400,
            screenshot is None,
        )
    )
    finished_at = utc_now()
    report = {
        "schemaVersion": 1,
        "controller": {
            "id": "playwright-script",
            "role": "fallback",
            "requestedBrowser": args.browser,
            "browserEngine": browser_engine,
            "fallbackReason": fallback_reason,
            "chromeMcpClaimed": False,
        },
        "request": {
            "url": args.url,
            "viewport": {"width": args.width, "height": args.height, "deviceScaleFactor": args.scale},
            "clicks": args.click,
            "expandScroll": args.expand_scroll,
            "expectVisible": args.expect,
        },
        "result": {
            "passed": passed,
            "startedAt": started_at,
            "finishedAt": finished_at,
            "durationMs": round((time.monotonic() - started_clock) * 1000),
            "responseStatus": response_status,
            "finalUrl": final_url,
            "title": title,
            "fatalError": fatal_error,
            "steps": steps,
        },
        "artifacts": {"screenshot": screenshot, "events": str(events_path)},
        "diagnostics": {
            "console": console_events,
            "consoleErrors": console_errors,
            "pageErrors": page_errors,
            "failedRequests": failed_requests,
            "abortedRequests": aborted_requests,
            "unexpectedFailedRequests": unexpected_failed_requests,
            "httpErrors": http_errors,
        },
    }
    write_json(report_path, report)
    events = [
        {"source": "console", **event} for event in console_events
    ] + [
        {"source": "page", "type": "error", "text": error} for error in page_errors
    ] + [
        {"source": "request", "type": "failed", **event} for event in failed_requests
    ] + [
        {"source": "response", "type": "http-error", **event} for event in http_errors
    ]
    events_path.write_text(
        "".join(f"{json.dumps(event, ensure_ascii=False)}\n" for event in events),
        encoding="utf-8",
    )
    print(f"browser-proof: {'PASS' if passed else 'FAIL'}")
    print(f"controller: playwright-script / {browser_engine}")
    print(f"report: {report_path}")
    print(f"screenshot: {screenshot_path if screenshot else 'not captured'}")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
