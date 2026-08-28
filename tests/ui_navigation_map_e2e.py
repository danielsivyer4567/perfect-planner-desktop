import base64
from pathlib import Path
from urllib.parse import urlparse

from playwright.sync_api import expect, sync_playwright


ROOT = Path(__file__).resolve().parents[1]
APP_URL = "http://127.0.0.1:5180/"
SCREENSHOT = ROOT / "artifacts" / "ui-navigation-map.png"

BOARD = {
    "planPath": r"C:\repos\perfect-planner-tauri\.claude\scratch\perfect-plan\ui-map.json",
    "number": "PP-UI",
    "topic": "Application page system",
    "repoName": "Perfect Planner",
    "repoRoot": r"C:\repos\perfect-planner-tauri",
    "project": "Perfect Planner Desktop",
    "worktreeName": "perfect-planner-tauri",
    "branch": "feature/ui-map",
}

PLAN = {
    "title": "Application page system",
    "approved": "yes @ test",
    "meta": {
        "number": "PP-UI",
        "project": "Perfect Planner Desktop",
        "branch": "feature/ui-map",
        "topic": "Application page system",
    },
    "spine": [
        {"id": "P1", "title": "Command workspace", "summary": "Select and inspect work."},
        {"id": "P2", "title": "Evidence and release", "summary": "Prove readiness."},
    ],
    "vertebrae": [
        {
            "id": "A01",
            "spineId": "P1",
            "side": "L",
            "title": "Repository selection page",
            "status": "done",
            "files": ["src/App.tsx"],
            "checklist": [{
                "text": "Repository scope stays visible",
                "ui": True,
                "built": True,
                "tested": True,
                "proof": {
                    "at": "2026-08-28T00:00:00Z",
                    "screenshot": "PP-UI-A01.png",
                    "shotNote": "Previous repository selection screen",
                    "screenshotCheck": {"ok": True, "width": 1440, "height": 900},
                },
            }],
        },
        {
            "id": "A02",
            "spineId": "P1",
            "side": "R",
            "title": "Native lease service",
            "status": "in-progress",
            "files": ["src-tauri/src/orchestrator.rs"],
            "checklist": [{"text": "Lease stays fenced", "built": True, "tested": False}],
        },
        {
            "id": "A03",
            "spineId": "P2",
            "side": "R",
            "title": "Release evidence page",
            "status": "pending",
            "files": ["src/components/OperationalTruth.tsx"],
            "checklist": [{"text": "Release evidence is visible", "ui": True, "built": False, "tested": False}],
        },
        {
            "id": "A99",
            "spineId": "P9",
            "side": "L",
            "title": "Unassigned legacy page",
            "status": "pending",
            "files": ["src/legacy.tsx"],
            "checklist": [],
        },
    ],
}


def main():
    SCREENSHOT.parent.mkdir(parents=True, exist_ok=True)
    console_errors = []
    page_errors = []
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        page = browser.new_page(viewport={"width": 1440, "height": 900}, device_scale_factor=3)
        page.set_default_timeout(10_000)
        page.set_default_navigation_timeout(30_000)
        page.on("console", lambda message: console_errors.append(message.text) if message.type == "error" else None)
        page.on("pageerror", lambda error: page_errors.append(str(error)))

        def mock_probe(route):
            parsed = urlparse(route.request.url)
            pieces = parsed.path.strip("/").split("/")
            if len(pieces) == 4 and pieces[:3] == ["board-probe", "5230", "evidence"]:
                route.fulfill(
                    status=200,
                    json={
                        "name": pieces[3],
                        "mime": "image/png",
                        "dataBase64": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
                    },
                )
                return
            if len(pieces) != 3 or pieces[0] != "board-probe" or int(pieces[1]) != 5230:
                route.fulfill(status=200, json={"ok": False})
                return
            endpoint = pieces[2]
            if endpoint == "whoami":
                route.fulfill(status=200, json={"ok": True, **BOARD, "approved": "yes @ test", "awaiting": None, "port": 5230, "pid": 5230})
            elif endpoint == "plan":
                route.fulfill(status=200, json=PLAN)
            elif endpoint == "workers":
                route.fulfill(status=200, json={"workers": {}})
            else:
                route.fulfill(status=404, json={"ok": False})

        page.route("**/board-probe/**", mock_probe)
        page.route(
            "**/build-screenshots/manifest.json",
            lambda route: route.fulfill(status=200, json={
                "schemaVersion": 1,
                "generatedAt": "2026-08-28T00:00:00Z",
                "captures": [{
                    "id": "release",
                    "label": "Latest verified release page",
                    "planNodes": ["PP-UI:A03"],
                    "url": "/build-screenshots/files/release.png",
                    "width": 1440,
                    "height": 900,
                    "sha256": "build-shot",
                    "sourceArtifact": "artifacts/release.png",
                }],
            }),
        )
        page.route(
            "**/build-screenshots/files/release.png",
            lambda route: route.fulfill(
                status=200,
                content_type="image/png",
                body=base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="),
            ),
        )
        page.route("http://127.0.0.1:5230/", lambda route: route.fulfill(body="<html><title>board</title></html>"))
        page.goto(APP_URL, wait_until="domcontentloaded")

        mode = page.locator("#pp-btn-toggle-ui-navigation-map")
        expect(mode).to_have_attribute("aria-pressed", "false")
        mode_box = mode.bounding_box()
        assert mode_box and mode_box["x"] + mode_box["width"] >= 1420, (
            f"Snapshots pill is not attached to the right edge: {mode_box}"
        )
        expect(mode).to_have_accessible_name("Snapshots")
        mode.evaluate("element => element.focus()")
        expect(mode).to_be_focused()
        mode.evaluate("element => element.click()")
        expect(mode).to_have_attribute("aria-pressed", "true")
        expect(page.locator("#pp-frame-active-board")).to_have_count(0)

        ui_map = page.locator("#pp-region-ui-navigation-map")
        expect(ui_map).to_be_visible()
        expect(ui_map).to_have_attribute("data-plan-id", "PP-UI")
        expect(ui_map).to_contain_text("SNAPSHOT CANVAS")
        expect(page.locator('[data-proof-method="chrome-mcp"]')).to_contain_text("preferred")
        expect(page.locator('[data-proof-method="playwright-script"]')).to_contain_text("ready")
        expect(page.locator('[data-proof-method="last-run"]')).to_contain_text("Build captures")
        p1 = page.locator('.ui-map-spine-row[data-spine-id="P1"]')
        p2 = page.locator('.ui-map-spine-row[data-spine-id="P2"]')
        orphan_row = page.locator('.ui-map-spine-row[data-spine-id="unmapped"]')
        expect(p1).to_be_visible()
        expect(p2).to_be_visible()
        expect(orphan_row).to_contain_text("Outside the spine")
        expect(page.locator(".ui-map-spine-axis")).to_be_visible()

        left_toggle = page.locator("#pp-btn-ui-map-P1-left")
        right_toggle = page.locator("#pp-btn-ui-map-P1-right")
        expect(left_toggle).to_have_attribute("aria-expanded", "true")
        expect(right_toggle).to_have_attribute("aria-expanded", "true")
        expect(left_toggle).to_have_accessible_name("Collapse 1 left pages for P1")
        expect(right_toggle).to_have_accessible_name("Collapse 1 right pages for P1")

        zoom_out = page.locator("#pp-btn-ui-map-zoom-out")
        zoom_in = page.locator("#pp-btn-ui-map-zoom-in")
        fit = page.locator("#pp-btn-ui-map-fit")
        actual = page.locator("#pp-btn-ui-map-actual")
        expect(zoom_out).to_have_accessible_name("Zoom out")
        expect(zoom_in).to_have_accessible_name("Zoom in")
        expect(ui_map).to_have_attribute("data-layout-ready", "true")
        expect(zoom_in).to_be_enabled()
        zoom_in.focus()
        expect(zoom_in).to_be_focused()
        initial_zoom = int(ui_map.get_attribute("data-zoom"))
        zoom_in.evaluate("element => element.click()")
        expect(ui_map).not_to_have_attribute("data-zoom", str(initial_zoom))
        actual.evaluate("element => element.click()")
        expect(ui_map).to_have_attribute("data-zoom", "100")
        zoom_out.evaluate("element => element.click()")
        expect(ui_map).to_have_attribute("data-zoom", "90")
        fit.evaluate("element => element.click()")
        assert int(ui_map.get_attribute("data-zoom")) < 100

        left_page = page.locator("#pp-ui-page-A01")
        expect(left_page).to_be_visible()
        expect(left_page).to_have_attribute("data-spine-id", "P1")
        expect(left_page).to_have_attribute("data-page-id", "A01")
        expect(left_page).to_have_attribute("data-page-side", "L")
        expect(left_page).to_have_attribute("data-page-kind", "ui-capable")
        expect(left_page).to_have_attribute("data-snapshot-state", "attached")
        expect(left_page.locator(".ui-map-artboard-frame img")).to_have_attribute(
            "alt", "Previous repository selection screen"
        )
        assert left_page.locator(".ui-map-artboard-frame img").evaluate(
            "image => image.complete && image.naturalWidth > 0"
        ), "attached node screenshot did not decode"
        left_page.locator(".ui-map-artboard-select").evaluate("element => element.click()")
        expect(ui_map).to_have_attribute("data-focus-page", "A01")
        expect(ui_map).to_have_attribute("data-focus-side", "L")
        expect(left_page).to_have_attribute("data-selected", "true")

        support_page = page.locator("#pp-ui-page-A02")
        expect(support_page).to_be_visible()
        expect(support_page).to_have_attribute("data-page-kind", "support-work")
        expect(support_page).to_have_attribute("data-snapshot-state", "not-applicable")
        expect(support_page).to_have_attribute("data-snapshot-source", "none")
        support_page.locator(".ui-map-artboard-select").evaluate("element => element.click()")
        expect(ui_map).to_have_attribute("data-focus-page", "A02")
        expect(ui_map).to_have_attribute("data-focus-side", "R")
        expect(support_page).to_contain_text("Support node")
        expect(support_page).to_contain_text("screenshot not required")

        release_page = page.locator("#pp-ui-page-A03")
        expect(release_page).to_have_attribute("data-snapshot-state", "build-capture")
        expect(release_page).to_have_attribute("data-snapshot-source", "build")
        expect(release_page.locator(".ui-map-build-badge")).to_have_text("BUILD CAPTURE")
        expect(release_page.locator(".ui-map-artboard-frame img")).to_have_attribute(
            "src", "/build-screenshots/files/release.png"
        )

        spine_segment = p1.locator(".ui-map-spine-segment")
        left_box = left_page.bounding_box()
        spine_box = spine_segment.bounding_box()
        right_box = support_page.bounding_box()
        assert left_box and spine_box and right_box
        assert left_box["x"] + left_box["width"] < spine_box["x"], (
            f"left artboard did not branch left of the spine: page={left_box}, spine={spine_box}"
        )
        assert right_box["x"] > spine_box["x"] + spine_box["width"], (
            f"right artboard did not branch right of the spine: page={right_box}, spine={spine_box}"
        )

        p1_box = p1.bounding_box()
        p2_box = p2.bounding_box()
        assert p1_box and p2_box
        p1_spine_box = p1.locator(".ui-map-spine-segment").bounding_box()
        p2_spine_box = p2.locator(".ui-map-spine-segment").bounding_box()
        axis_box = page.locator(".ui-map-spine-axis").bounding_box()
        assert p1_spine_box and p2_spine_box and axis_box
        assert abs((p1_spine_box["x"] + p1_spine_box["width"] / 2) - (p2_spine_box["x"] + p2_spine_box["width"] / 2)) < 2, (
            f"spine segments are not vertically aligned: P1={p1_spine_box}, P2={p2_spine_box}"
        )
        assert abs((p1_spine_box["x"] + p1_spine_box["width"] / 2) - (axis_box["x"] + axis_box["width"] / 2)) < 2, (
            f"silver rail does not pass through the spine segments: rail={axis_box}, P1={p1_spine_box}"
        )
        assert p2_box["y"] > p1_box["y"] + p1_box["height"], (
            f"spine does not progress vertically: P1={p1_box}, P2={p2_box}"
        )

        left_toggle.evaluate("element => element.click()")
        expect(left_toggle).to_have_attribute("aria-expanded", "false")
        expect(left_page).to_be_hidden()
        left_toggle.evaluate("element => element.click()")
        expect(left_toggle).to_have_attribute("aria-expanded", "true")
        expect(left_page).to_be_visible()

        expect(page.locator("#pp-ui-branch-P2-left")).to_contain_text("No left pages")
        expect(page.locator(".ui-map-artboard")).to_have_count(4)

        page.screenshot(path=str(SCREENSHOT), full_page=True, animations="disabled")

        comparison = page.locator("#pp-btn-toggle-ui-comparison")
        comparison.evaluate("element => element.click()")
        expect(mode).to_have_attribute("aria-pressed", "false")
        expect(ui_map).to_have_count(0)
        expect(page.locator("#pp-frame-active-board")).to_be_visible()
        expect(page.locator("#pp-region-ui-comparison")).to_be_visible()

        assert not console_errors, f"browser console errors: {console_errors}"
        assert not page_errors, f"uncaught page errors: {page_errors}"
        browser.close()

    print("ui_navigation_map_e2e: PASS")
    print(f"screenshot: {SCREENSHOT}")


if __name__ == "__main__":
    main()
