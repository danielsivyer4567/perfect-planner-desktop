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
            "collision_assessor_issue_discovery_capability",
            "collision_assessor_collect_census",
            "collision_assessor_revoke_discovery_capability",
            "orchestrator_create_run",
            "orchestrator_preflight_inspect",
            "orchestrator_pipeline_snapshot",
            "orchestrator_run_catalog",
            "orchestrator_authorize_fenced_completion",
            "orchestrator_record_failure",
            "orchestrator_reap_expired",
            "orchestrator_validate_worker_submission",
            "orchestrator_reconcile",
            "orchestrator_evaluate_release",
            "orchestrator_deliver",
        ]),
    ))
    .expect("failed to build Perfect Planner Desktop");
}
