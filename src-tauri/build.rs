fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "discover_boards",
            "read_board_workers",
            "read_board_plan",
            "read_board_evidence",
            "reconcile_session_leases",
            "supervisor_snapshot",
            "recover_board_session",
        ]),
    ))
    .expect("failed to build Perfect Planner Desktop");
}
