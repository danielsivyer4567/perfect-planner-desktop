import json
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit


ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "artifacts" / "browser-proof"
REPORT = OUTPUT / "ui-snapshots-fallback-report.json"
BOARD_PORT_START = 5230
BOARD_PORT_END = 5249
BOARD_PLAN_PATH = str(ROOT / ".test-data" / "browser-proof-plan.json")

BOARD_IDENTITY = {
    "planPath": BOARD_PLAN_PATH,
    "number": "PP-PROOF",
    "topic": "Deterministic browser proof",
    "repoName": "Perfect Planner",
    "repoRoot": str(ROOT),
    "project": "Perfect Planner Desktop",
    "worktreeName": ROOT.name,
    "branch": "test/browser-proof",
}

BROWSER_PROOF_PLAN = {
    "title": "Deterministic browser proof",
    "approved": "yes @ browser-proof",
    "meta": {
        "number": "PP-PROOF",
        "project": "Perfect Planner Desktop",
        "branch": "test/browser-proof",
        "topic": "Deterministic browser proof",
    },
    "spine": [
        {"id": "P1", "title": "Repository scope", "summary": "Select the bounded repository."},
        {"id": "P2", "title": "Orchestration", "summary": "Coordinate visible work safely."},
        {"id": "P3", "title": "Verification", "summary": "Capture evidence before release."},
    ],
    "vertebrae": [
        {
            "id": f"A0{index + 1}",
            "spineId": phase_id,
            "side": side,
            "title": title,
            "status": "done" if index < 4 else "in-progress",
            "files": ["src/App.tsx"],
            "checklist": [{"text": title, "ui": True, "built": True, "tested": index < 4}],
        }
        for index, (phase_id, side, title) in enumerate(
            [
                ("P1", "L", "Repository selection"),
                ("P1", "R", "Plan command header"),
                ("P2", "L", "Worker activity"),
                ("P2", "R", "Conflict decisions"),
                ("P3", "L", "Snapshot evidence"),
                ("P3", "R", "Release readiness"),
            ]
        )
    ],
}


class BrowserProofBoardHandler(BaseHTTPRequestHandler):
    def send_json(self, payload: object, status: int = 200) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        path = urlsplit(self.path).path
        if path == "/whoami":
            self.send_json(
                {
                    "ok": True,
                    **BOARD_IDENTITY,
                    "approved": "yes @ browser-proof",
                    "awaiting": None,
                    "port": self.server.server_port,
                    "pid": self.server.server_port,
                }
            )
        elif path == "/plan":
            self.send_json(BROWSER_PROOF_PLAN)
        elif path == "/workers":
            self.send_json({"workers": {}})
        elif path == "/":
            body = b"<!doctype html><html><head><title>proof board</title></head><body></body></html>"
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_json({"ok": False, "error": "not found"}, status=404)

    def log_message(self, _format: str, *_args: object) -> None:
        return


def start_board_fixture() -> tuple[ThreadingHTTPServer, threading.Thread]:
    last_error: OSError | None = None
    server: ThreadingHTTPServer | None = None
    for port in range(BOARD_PORT_START, BOARD_PORT_END + 1):
        try:
            server = ThreadingHTTPServer(("127.0.0.1", port), BrowserProofBoardHandler)
            break
        except OSError as error:
            last_error = error
    if server is None:
        raise AssertionError(
            f"browser proof fixture requires one unused loopback port in "
            f"{BOARD_PORT_START}-{BOARD_PORT_END}: {last_error}"
        ) from last_error
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread


def main() -> None:
    selected_board = json.dumps(
        {"repositoryRoot": BOARD_IDENTITY["repoRoot"], "planPath": BOARD_PLAN_PATH},
        separators=(",", ":"),
    )
    command = [
        sys.executable,
        str(ROOT / "scripts" / "browser-proof.py"),
        "--url",
        "http://127.0.0.1:5180/",
        "--out",
        str(OUTPUT),
        "--name",
        "ui-snapshots-fallback",
        "--height",
        "1000",
        "--storage",
        f"perfect-planner:active-board={selected_board}",
        "--click",
        "#pp-btn-toggle-ui-navigation-map",
        "--click",
        "#pp-btn-ui-map-actual",
        "--expect",
        "#pp-region-ui-navigation-map",
        "--expect",
        ".ui-map-artboard",
        "--expand-scroll",
        ".ui-map-viewport",
    ]
    server, thread = start_board_fixture()
    try:
        completed = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, timeout=90)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    if completed.returncode != 0:
        report_text = REPORT.read_text(encoding="utf-8") if REPORT.is_file() else "report missing"
        raise AssertionError(
            f"{completed.stdout}\n{completed.stderr}\n--- browser proof report ---\n{report_text}"
        )
    report = json.loads(REPORT.read_text(encoding="utf-8"))
    assert report["controller"]["id"] == "playwright-script"
    assert report["controller"]["chromeMcpClaimed"] is False
    assert report["result"]["passed"] is True
    assert report["request"]["storageKeys"] == ["perfect-planner:active-board"]
    assert report["artifacts"]["screenshot"]["fullPage"] is True
    assert report["artifacts"]["screenshot"]["width"] >= 2880
    assert report["artifacts"]["screenshot"]["height"] > 1800
    assert report["diagnostics"]["consoleErrors"] == []
    assert report["diagnostics"]["pageErrors"] == []
    assert report["diagnostics"]["unexpectedFailedRequests"] == []
    print("browser_proof_harness_e2e: PASS")
    print(f"report: {REPORT}")
    print(f"screenshot: {report['artifacts']['screenshot']['path']}")


if __name__ == "__main__":
    main()
