from __future__ import annotations

import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CONTRACT = ROOT / "build-screenshots.json"


def main() -> None:
    package = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))
    assert "build:screenshots" in package["scripts"]
    assert "build:screenshots" in package["scripts"]["build"]

    contract = json.loads(CONTRACT.read_text(encoding="utf-8"))
    assert contract["schemaVersion"] == 1
    captures = contract["captures"]
    assert captures

    declared: set[str] = set()
    for capture in captures:
        assert capture["id"]
        assert (ROOT / capture["script"]).is_file()
        assert capture["artifact"].startswith("artifacts/")
        assert capture["planNodes"]
        for plan_node in capture["planNodes"]:
            assert plan_node not in declared, f"duplicate screenshot mapping: {plan_node}"
            declared.add(plan_node)

    required: set[str] = set()
    for plan_path in (ROOT / ".claude" / "scratch" / "perfect-plan").glob("*.json"):
        plan = json.loads(plan_path.read_text(encoding="utf-8"))
        number = plan.get("meta", {}).get("number")
        if not number:
            continue
        for node in plan.get("vertebrae", []):
            if any(item.get("ui") is True for item in node.get("checklist", [])):
                required.add(f"{number}:{node['id']}")

    assert declared == required, (
        f"build screenshot contract mismatch; missing={sorted(required - declared)}, "
        f"unknown={sorted(declared - required)}"
    )
    runner_source = (ROOT / "scripts" / "run-build-screenshots.mjs").read_text(encoding="utf-8")
    assert 'runner: "playwright-script"' in runner_source
    assert 'result: "passed"' in runner_source
    assert "requiredUiNodeCount" in runner_source
    assert "requiredUiNodes:" in runner_source
    print("build_screenshot_pipeline_e2e: PASS")
    print(f"proved: {len(required)} UI nodes have mandatory build captures")


if __name__ == "__main__":
    main()
