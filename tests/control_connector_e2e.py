from pathlib import Path
import json
import os
import shutil
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


ROOT = Path(__file__).resolve().parents[1]
CLIENT = ROOT / "scripts" / "pp-control.mjs"


def run(arguments, environment, cwd=ROOT, expect_success=True):
    result = subprocess.run(
        ["node", str(CLIENT), *arguments],
        cwd=cwd,
        env=environment,
        text=True,
        capture_output=True,
        shell=False,
        check=False,
    )
    if expect_success and result.returncode != 0:
        raise AssertionError(f"command failed ({result.returncode}): {result.stderr}\n{result.stdout}")
    if not expect_success and result.returncode == 0:
        raise AssertionError(f"command unexpectedly passed: {result.stdout}")
    return result


def stable_entity_id(prefix, source):
    value = 0x811C9DC5
    for character in source:
        value ^= ord(character)
        value = (value * 0x01000193) & 0xFFFFFFFF
    alphabet = "0123456789abcdefghijklmnopqrstuvwxyz"
    encoded = "0" if value == 0 else ""
    while value:
        value, remainder = divmod(value, 36)
        encoded = alphabet[remainder] + encoded
    return f"pp-{prefix}-{encoded}"


def read_drops(app_data):
    inbox = app_data / "control-plane-inbox"
    assert inbox.is_dir(), "control-plane inbox was not created"
    assert not list(inbox.glob("*.tmp")), "a partial drop was exposed as a consumable file"
    assert not [path for path in inbox.iterdir() if path.name.startswith(".") and path.suffix == ".tmp"], (
        "an atomic-write temporary file was left behind"
    )
    return [json.loads(path.read_text(encoding="utf-8")) for path in sorted(inbox.glob("*.json"))]


def git(repository, *arguments):
    result = subprocess.run(
        ["git", "-C", str(repository), *arguments],
        text=True,
        capture_output=True,
        shell=False,
        check=False,
    )
    if result.returncode != 0:
        raise AssertionError(f"git {' '.join(arguments)} failed: {result.stderr}")
    return result.stdout.strip()


def windows_junction_alias(target, parent):
    alias = parent / "repository-alpha-alias"
    result = subprocess.run(
        ["cmd.exe", "/d", "/c", "mklink", "/J", str(alias), str(target)],
        text=True,
        capture_output=True,
        shell=False,
        check=False,
    )
    if result.returncode != 0:
        raise AssertionError(f"cannot create Windows junction alias: {result.stderr}\n{result.stdout}")
    return alias


def serve_board(plan_path):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path != "/whoami" or self.headers.get("Host") != f"127.0.0.1:{self.server.server_port}":
                self.send_response(404)
                self.end_headers()
                return
            body = json.dumps(
                {
                    "ok": True,
                    "planPath": str(plan_path.resolve()),
                    "pid": os.getpid(),
                    "approved": "pending",
                }
            ).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            return

    for port in range(5230, 5250):
        try:
            server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
            break
        except OSError:
            continue
    else:
        raise AssertionError("no free Perfect Planner board port for connector test")
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread


def main():
    assert shutil.which("node"), "Node.js is required"
    assert shutil.which("git"), "Git is required"
    with tempfile.TemporaryDirectory(prefix="perfect-planner-connector-") as temporary:
        temporary_path = Path(temporary)
        repository = temporary_path / "repository-alpha"
        repository.mkdir()
        git(repository, "init", "-b", "integration/test-connector")
        plan_path = repository / ".claude" / "scratch" / "perfect-plan" / "connector.json"
        plan_path.parent.mkdir(parents=True)
        plan_path.write_text(
            json.dumps(
                {
                    "meta": {"number": "PP-017", "topic": "Connector test"},
                    "vertebrae": [
                        {
                            "id": "A01",
                            "title": "Connector",
                            "checklist": [{"text": "First item"}],
                        }
                    ],
                }
            ),
            encoding="utf-8",
        )
        app_data = temporary_path / "tauri-app-data"
        environment = {
            **os.environ,
            "PP_CONTROL_APP_DATA": str(app_data),
            # A deliberately invalid executable proves register-codex never launches Codex.
            "CODEX_CLI": str(temporary_path / "must-not-be-executed"),
        }

        self_test = run(["self-test"], environment)
        self_test_result = json.loads(self_test.stdout)
        assert self_test_result == {
            "ok": True,
            "fnv": "PASS",
            "atomicDrop": "PASS",
            "codexExecuted": False,
        }

        plan_argument = plan_path
        junction = None
        if os.name == "nt":
            junction = windows_junction_alias(repository, temporary_path)
            plan_argument = junction / plan_path.relative_to(repository)
        try:
            note_result = run(
                [
                    "note",
                    "--plan",
                    str(plan_argument),
                    "--node",
                    "A01",
                    "--item",
                    "A01:0",
                    "--worker",
                    "s-worker-alpha",
                    "--body",
                    "OAuth consent is required before the worker can continue.",
                ],
                environment,
            )
        finally:
            if junction is not None:
                os.rmdir(junction)
        note_output = json.loads(note_result.stdout)
        assert note_output["ok"] is True

        server, thread = serve_board(plan_path)
        try:
            register_result = run(
                [
                    "register-codex",
                    "--plan",
                    str(plan_path),
                    "--thread-id",
                    "task-6a86a696-cac8",
                    "--launch-nonce",
                    "11111111-2222-4333-8444-555555555555",
                    "--ttl-minutes",
                    "30",
                ],
                environment,
            )
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)
        register_output = json.loads(register_result.stdout)
        assert register_output["ok"] is True
        assert register_output["type"] == "REGISTER_APPROVAL_ROUTE"
        assert register_output["boardPort"] == server.server_port
        assert register_output["boardPid"] == os.getpid()
        assert "must-not-be-executed" not in register_result.stdout

        drops = read_drops(app_data)
        assert len(drops) == 2
        assert all(drop["schemaVersion"] == 1 for drop in drops)
        note = next(drop["request"] for drop in drops if drop["type"] == "POST_MESSAGE")
        registration = next(
            drop["request"] for drop in drops if drop["type"] == "REGISTER_APPROVAL_ROUTE"
        )

        canonical_repository = str(repository.resolve())
        repository_source = canonical_repository.replace("\\", "/").lower()
        expected_repository_id = stable_entity_id("repo", repository_source)
        expected_plan_id = stable_entity_id("plan", str(plan_path.resolve()).lower())
        assert note["scope"]["repositoryId"] == expected_repository_id
        assert note["scope"]["organizationId"] == expected_repository_id
        assert note["scope"]["repositoryRoot"] == canonical_repository
        assert note["scope"]["worktreePath"] == canonical_repository
        assert note["scope"]["branchName"] == "integration/test-connector"
        assert note["scope"]["planId"] == expected_plan_id
        assert note["scope"]["planPath"] == str(plan_path.resolve())
        assert note["scope"]["nodeId"] == "A01"
        assert note["scope"]["itemId"] == "A01:0"
        assert note["scope"]["workerId"] == "s-worker-alpha"
        assert note["scope"]["orchestratorId"] is None
        assert note["sender"] == {"kind": "WORKER", "actorId": "s-worker-alpha"}
        assert note["destination"]["kind"] == "ORCHESTRATOR"
        assert note["destination"]["routeId"] == f"local-orchestrator-inbox:{expected_repository_id}"
        assert note["destination"]["requiresAcknowledgement"] is True

        thread_id = "task-6a86a696-cac8"
        expected_route = f"codex-exec:{expected_repository_id}:{thread_id}"
        assert registration["organizationId"] == expected_repository_id
        assert registration["repositoryId"] == expected_repository_id
        assert registration["planId"] == expected_plan_id
        assert registration["planPath"] == str(plan_path.resolve())
        assert registration["boardPort"] == server.server_port
        assert registration["boardPid"] == os.getpid()
        assert registration["launchNonce"] == "11111111-2222-4333-8444-555555555555"
        assert registration["connectorId"] == "codex-exec"
        assert registration["routeId"] == expected_route
        assert registration["taskId"] == thread_id
        assert registration["expiresAtMs"] - registration["createdAtMs"] == 30 * 60_000

        invalid = run(
            [
                "note",
                "--plan",
                str(plan_path),
                "--node",
                "A99",
                "--worker",
                "s-worker-alpha",
                "--body",
                "must not be written",
            ],
            environment,
            expect_success=False,
        )
        assert "is not present" in invalid.stderr
        assert len(read_drops(app_data)) == 2, "invalid input wrote a drop"

    print("control_connector_e2e: PASS")
    print("proved: FNV scope, Git isolation, atomic drops, worker notes, Codex registration, no Codex execution")


if __name__ == "__main__":
    main()
