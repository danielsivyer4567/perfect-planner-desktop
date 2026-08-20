use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct PlanSummary {
    pub id: String,
    pub path: String,
    pub content: serde_json::Value,
}

#[tauri::command]
fn get_plans() -> Vec<serde_json::Value> {
    let mut plans = Vec::new();
    let plans_dir = Path::new(r"C:\repos\plans");

    if plans_dir.exists() {
        if let Ok(entries) = fs::read_dir(plans_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&content) {
                            if let Some(obj) = val.as_object_mut() {
                                obj.insert("filePath".to_string(), serde_json::Value::String(path.to_string_lossy().to_string()));
                            }
                            plans.push(val);
                        }
                    }
                }
            }
        }
    }

    plans
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![get_plans])
        .run(tauri::generate_context!())
        .expect("error while running perfect planner desktop");
}
