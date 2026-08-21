#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(exit_code) =
        perfect_planner_desktop_lib::collision_assessor::collector_process::dispatch_internal_helper(
        )
    {
        std::process::exit(exit_code);
    }
    perfect_planner_desktop_lib::run();
}
