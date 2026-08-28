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
        "--height",
        "1000",
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
    completed = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, timeout=90)
    if completed.returncode != 0:
        report_text = REPORT.read_text(encoding="utf-8") if REPORT.is_file() else "report missing"
        raise AssertionError(
            f"{completed.stdout}\n{completed.stderr}\n--- browser proof report ---\n{report_text}"
        )
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
