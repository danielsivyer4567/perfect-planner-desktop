import json
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "artifacts" / "browser-proof"
REPORT = OUTPUT / "ui-snapshots-fallback-report.json"


def main() -> None:
    command = [
        sys.executable,
        str(ROOT / "scripts" / "browser-proof.py"),
        "--url",
        "http://127.0.0.1:5180/",
        "--out",
        str(OUTPUT),
        "--name",
        "ui-snapshots-fallback",
        "--click",
        "#pp-btn-toggle-ui-navigation-map",
        "--expect",
        "#pp-region-ui-navigation-map",
        "--expect",
        ".ui-map-spine-row",
        "--expand-scroll",
        ".stage-workspace.mapping",
    ]
    completed = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, timeout=90)
    assert completed.returncode == 0, f"{completed.stdout}\n{completed.stderr}"
    report = json.loads(REPORT.read_text(encoding="utf-8"))
    assert report["controller"]["id"] == "playwright-script"
    assert report["controller"]["chromeMcpClaimed"] is False
    assert report["result"]["passed"] is True
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
