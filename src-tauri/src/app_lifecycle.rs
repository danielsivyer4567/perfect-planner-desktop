use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const LIFECYCLE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AppLifecycleKind {
    Launch,
    Exit,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppLifecycleEvent {
    pub schema_version: u32,
    pub event_id: String,
    pub session_id: String,
    pub kind: AppLifecycleKind,
    pub at_ms: u64,
    pub process_id: u32,
    pub app_version: String,
}

pub struct AppLifecycleLog {
    path: PathBuf,
    session_id: String,
    process_id: u32,
    app_version: String,
    exit_recorded: Mutex<bool>,
}

impl AppLifecycleLog {
    pub fn open(app_data_dir: &Path, at_ms: u64) -> Result<Self, String> {
        fs::create_dir_all(app_data_dir)
            .map_err(|error| format!("cannot create app data directory: {error}"))?;
        let process_id = std::process::id();
        let session_id = format!("pp-app-{at_ms}-{process_id}");
        let log = Self {
            path: app_data_dir.join("app-lifecycle.jsonl"),
            session_id,
            process_id,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            exit_recorded: Mutex::new(false),
        };
        log.append(AppLifecycleKind::Launch, at_ms)?;
        Ok(log)
    }

    pub fn record_exit(&self, at_ms: u64) -> Result<(), String> {
        let mut recorded = self
            .exit_recorded
            .lock()
            .map_err(|_| "app lifecycle exit state is poisoned".to_string())?;
        if *recorded {
            return Ok(());
        }
        self.append(AppLifecycleKind::Exit, at_ms)?;
        *recorded = true;
        Ok(())
    }

    fn append(&self, kind: AppLifecycleKind, at_ms: u64) -> Result<(), String> {
        let kind_id = match kind {
            AppLifecycleKind::Launch => "launch",
            AppLifecycleKind::Exit => "exit",
        };
        let event = AppLifecycleEvent {
            schema_version: LIFECYCLE_SCHEMA_VERSION,
            event_id: format!("{}-{kind_id}", self.session_id),
            session_id: self.session_id.clone(),
            kind,
            at_ms,
            process_id: self.process_id,
            app_version: self.app_version.clone(),
        };
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| format!("cannot open app lifecycle ledger: {error}"))?;
        let mut encoded = serde_json::to_vec(&event)
            .map_err(|error| format!("cannot serialize app lifecycle event: {error}"))?;
        encoded.push(b'\n');
        file.write_all(&encoded)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_data())
            .map_err(|error| format!("cannot persist app lifecycle event: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    fn temporary_directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "perfect-planner-app-lifecycle-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn launch_and_exactly_one_exit_are_append_only_and_correlated() {
        let directory = temporary_directory();
        let log = AppLifecycleLog::open(&directory, 1_000).unwrap();
        log.record_exit(2_000).unwrap();
        log.record_exit(3_000).unwrap();

        let file = fs::File::open(directory.join("app-lifecycle.jsonl")).unwrap();
        let events: Vec<AppLifecycleEvent> = BufReader::new(file)
            .lines()
            .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, AppLifecycleKind::Launch);
        assert_eq!(events[1].kind, AppLifecycleKind::Exit);
        assert_eq!(events[0].session_id, events[1].session_id);
        assert_eq!(events[0].process_id, events[1].process_id);
        assert_eq!(events[0].app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(events[0].at_ms, 1_000);
        assert_eq!(events[1].at_ms, 2_000);
        assert_ne!(events[0].event_id, events[1].event_id);

        fs::remove_dir_all(directory).unwrap();
    }
}
