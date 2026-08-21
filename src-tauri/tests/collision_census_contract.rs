use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const HELPER_ARG: &str = "--perfect-planner-collision-census-helper-v1";

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_perfect-planner-desktop"))
}

#[test]
fn internal_helper_rejects_extra_arguments_before_tauri_startup() {
    let output = Command::new(binary())
        .args([HELPER_ARG, "caller-selected-path"])
        .env_clear()
        .output()
        .expect("fixed test binary starts");
    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn malformed_anonymous_pipe_frame_fails_closed_without_echo_or_secret_leak() {
    let sentinel = "B04-SENTINEL-SECRET-MUST-NOT-LEAK";
    let mut child = Command::new(binary())
        .arg(HELPER_ARG)
        .env_clear()
        .env("B04_SENTINEL", sentinel)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("fixed test binary starts");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"not-a-census-frame")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(65));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(sentinel));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(sentinel));
}

#[test]
fn native_service_and_process_boundary_have_no_web_shell_or_board_fallback() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let collector =
        std::fs::read_to_string(manifest.join("src/collision_assessor/collector_process.rs"))
            .unwrap();
    for forbidden in [
        "cmd.exe",
        "powershell",
        "TcpStream",
        "reqwest",
        "http://",
        "https://",
        "discover_boards",
        "read_board_plan",
    ] {
        assert!(
            !collector.contains(forbidden),
            "forbidden authority: {forbidden}"
        );
    }
    for required in [
        ".env_clear()",
        "CREATE_NO_WINDOW",
        "KILL_ON_CLOSE",
        "ACTIVE_PROCESS",
        "QueryFullProcessImageNameW",
        "BCryptGenRandom",
    ] {
        assert!(collector.contains(required), "missing boundary: {required}");
    }

    let boards = std::fs::read_to_string(manifest.join("../src/services/boards.ts")).unwrap();
    let native_start = boards.find("issueCollisionCensusCapability").unwrap();
    let native_end = boards[native_start..].find("function toBoard").unwrap() + native_start;
    let native = &boards[native_start..native_end];
    assert!(native.contains("if (!inTauri())"));
    assert!(native.contains("@tauri-apps/api/core"));
    assert!(!native.contains("fetch("));
    assert!(!native.contains("requestJson"));
}
