use super::model::{Validate, ValidationErrors, ValidationIssue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const STALE_LOCK_AGE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "preflight")]
    Preflight,
    #[serde(rename = "claim")]
    Claim,
    #[serde(rename = "heartbeat")]
    Heartbeat,
    #[serde(rename = "progress")]
    Progress,
    #[serde(rename = "evidence")]
    Evidence,
    #[serde(rename = "gate-pass")]
    GatePass,
    #[serde(rename = "gate-fail")]
    GateFail,
    #[serde(rename = "decision-required")]
    DecisionRequired,
    #[serde(rename = "node-done")]
    NodeDone,
    #[serde(rename = "reassign")]
    Reassign,
    #[serde(rename = "warning")]
    Warning,
    #[serde(rename = "run-done")]
    RunDone,
}

impl EventType {
    pub const ALL: [Self; 12] = [
        Self::Preflight,
        Self::Claim,
        Self::Heartbeat,
        Self::Progress,
        Self::Evidence,
        Self::GatePass,
        Self::GateFail,
        Self::DecisionRequired,
        Self::NodeDone,
        Self::Reassign,
        Self::Warning,
        Self::RunDone,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
            Self::Claim => "claim",
            Self::Heartbeat => "heartbeat",
            Self::Progress => "progress",
            Self::Evidence => "evidence",
            Self::GatePass => "gate-pass",
            Self::GateFail => "gate-fail",
            Self::DecisionRequired => "decision-required",
            Self::NodeDone => "node-done",
            Self::Reassign => "reassign",
            Self::Warning => "warning",
            Self::RunDone => "run-done",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEvent {
    pub ts: String,
    #[serde(rename = "runId")]
    pub run_id: String,
    #[serde(rename = "nodeId")]
    pub node_id: Option<String>,
    pub worker: String,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub msg: String,
    #[serde(default)]
    pub data: Value,
}

impl Validate for RunEvent {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut issues = Vec::new();
        for (field, value) in [
            ("ts", self.ts.as_str()),
            ("runId", self.run_id.as_str()),
            ("worker", self.worker.as_str()),
            ("msg", self.msg.as_str()),
        ] {
            if value.trim().is_empty() {
                issues.push(ValidationIssue {
                    field: field.into(),
                    message: "must not be empty".into(),
                });
            }
        }
        if self
            .node_id
            .as_deref()
            .is_some_and(|node_id| node_id.trim().is_empty())
        {
            issues.push(ValidationIssue {
                field: "nodeId".into(),
                message: "must not be empty when set".into(),
            });
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ValidationErrors::new(issues))
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventBus {
    path: PathBuf,
    lock_timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendReceipt {
    pub start_offset: u64,
    pub end_offset: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TailBatch {
    pub events: Vec<RunEvent>,
    pub next_offset: u64,
    pub skipped_lines: usize,
    pub trailing_partial: bool,
}

#[derive(Debug)]
pub enum EventBusError {
    Io(io::Error),
    Json(serde_json::Error),
    Validation(ValidationErrors),
    LockTimeout(PathBuf),
    InvalidOffset { offset: u64, file_len: u64 },
}

impl fmt::Display for EventBusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "event bus I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "event serialization failed: {error}"),
            Self::Validation(error) => write!(formatter, "event validation failed: {error}"),
            Self::LockTimeout(path) => {
                write!(
                    formatter,
                    "timed out waiting for event append lock: {}",
                    path.display()
                )
            }
            Self::InvalidOffset { offset, file_len } => write!(
                formatter,
                "tail offset {offset} is beyond events file length {file_len}"
            ),
        }
    }
}

impl Error for EventBusError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Validation(error) => Some(error),
            Self::LockTimeout(_) | Self::InvalidOffset { .. } => None,
        }
    }
}

impl From<io::Error> for EventBusError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for EventBusError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<ValidationErrors> for EventBusError {
    fn from(error: ValidationErrors) -> Self {
        Self::Validation(error)
    }
}

impl EventBus {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
        }
    }

    pub fn with_lock_timeout(path: impl Into<PathBuf>, lock_timeout: Duration) -> Self {
        Self {
            path: path.into(),
            lock_timeout,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, event: &RunEvent) -> Result<AppendReceipt, EventBusError> {
        event.validate()?;
        let mut encoded = serde_json::to_vec(event)?;
        encoded.push(b'\n');

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let _lock = AppendLock::acquire(&self.path, self.lock_timeout)?;
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)?;

        let mut start_offset = file.metadata()?.len();
        if start_offset > 0 {
            file.seek(SeekFrom::End(-1))?;
            let mut last_byte = [0_u8; 1];
            file.read_exact(&mut last_byte)?;
            if last_byte[0] != b'\n' {
                file.write_all(b"\n")?;
                start_offset += 1;
            }
        }

        file.write_all(&encoded)?;
        file.sync_data()?;
        Ok(AppendReceipt {
            start_offset,
            end_offset: start_offset + encoded.len() as u64,
        })
    }

    /// Reads newline-terminated events from `offset`.
    ///
    /// Malformed complete lines are skipped and consumed. A final line without a newline is
    /// treated as torn: it is not parsed and `next_offset` remains at that line's first byte.
    pub fn tail_from(&self, offset: u64) -> Result<TailBatch, EventBusError> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(TailBatch {
                    events: Vec::new(),
                    next_offset: offset,
                    skipped_lines: 0,
                    trailing_partial: false,
                });
            }
            Err(error) => return Err(error.into()),
        };

        let file_len = file.metadata()?.len();
        if offset > file_len {
            return Err(EventBusError::InvalidOffset { offset, file_len });
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        let trailing_partial = bytes.last().is_some_and(|byte| *byte != b'\n');
        let complete_len = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        let complete = &bytes[..complete_len];
        let mut events = Vec::new();
        let mut skipped_lines = 0;

        for raw_line in complete.split(|byte| *byte == b'\n') {
            if raw_line.is_empty() {
                continue;
            }
            let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
            let event = match serde_json::from_slice::<RunEvent>(line) {
                Ok(event) if event.validate().is_ok() => event,
                Ok(_) | Err(_) => {
                    skipped_lines += 1;
                    continue;
                }
            };
            events.push(event);
        }

        Ok(TailBatch {
            events,
            next_offset: offset + complete_len as u64,
            skipped_lines,
            trailing_partial,
        })
    }
}

struct AppendLock {
    path: PathBuf,
}

impl AppendLock {
    fn acquire(events_path: &Path, timeout: Duration) -> Result<Self, EventBusError> {
        let lock_path = append_lock_path(events_path);
        let started = Instant::now();
        let mut retry_attempt = 0_usize;
        loop {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    file.sync_data()?;
                    return Ok(Self { path: lock_path });
                }
                Err(error) if is_retryable_lock_contention(&error) => {
                    if error.kind() == io::ErrorKind::AlreadyExists && lock_is_stale(&lock_path) {
                        match fs::remove_file(&lock_path) {
                            Ok(()) => continue,
                            Err(remove_error) if remove_error.kind() == io::ErrorKind::NotFound => {
                                continue;
                            }
                            Err(_) => {}
                        }
                    }
                    if started.elapsed() >= timeout {
                        return Err(EventBusError::LockTimeout(lock_path));
                    }
                    thread::sleep(lock_retry_delay(retry_attempt));
                    retry_attempt = retry_attempt.saturating_add(1);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for AppendLock {
    fn drop(&mut self) {
        for retry_attempt in 0..5 {
            match fs::remove_file(&self.path) {
                Ok(()) => return,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return,
                Err(error) if is_retryable_lock_contention(&error) => {
                    thread::sleep(lock_retry_delay(retry_attempt));
                }
                Err(_) => return,
            }
        }
    }
}

fn is_retryable_lock_contention(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::AlreadyExists {
        return true;
    }

    #[cfg(windows)]
    {
        // Windows can leave a successfully removed lock file briefly delete-pending. During
        // that window, a new CREATE_NEW receives ERROR_ACCESS_DENIED or
        // ERROR_SHARING_VIOLATION instead of ERROR_FILE_EXISTS.
        matches!(error.raw_os_error(), Some(5 | 32))
    }

    #[cfg(not(windows))]
    {
        false
    }
}

fn lock_retry_delay(attempt: usize) -> Duration {
    const BACKOFF_MS: [u64; 5] = [2, 4, 8, 16, 25];
    Duration::from_millis(BACKOFF_MS[attempt.min(BACKOFF_MS.len() - 1)])
}

fn append_lock_path(events_path: &Path) -> PathBuf {
    let mut file_name = events_path
        .file_name()
        .map_or_else(|| OsString::from("events.jsonl"), OsString::from);
    file_name.push(".append.lock");
    events_path.with_file_name(file_name)
}

fn lock_is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age >= STALE_LOCK_AGE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn temp_events(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "perfect-orchestrator-{name}-{}-{nonce}/events.jsonl",
            std::process::id()
        ))
    }

    fn event(message: impl Into<String>) -> RunEvent {
        RunEvent {
            ts: "2026-08-22T00:00:00Z".into(),
            run_id: "ORCH-20260822-001".into(),
            node_id: Some("TO-01".into()),
            worker: "worker-a".into(),
            event_type: EventType::Progress,
            msg: message.into(),
            data: serde_json::json!({"percent": 50}),
        }
    }

    fn cleanup(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn event_types_are_exact_and_stable() {
        let names = EventType::ALL.map(EventType::as_str);
        assert_eq!(
            names,
            [
                "preflight",
                "claim",
                "heartbeat",
                "progress",
                "evidence",
                "gate-pass",
                "gate-fail",
                "decision-required",
                "node-done",
                "reassign",
                "warning",
                "run-done",
            ]
        );

        let json = serde_json::to_value(event("working")).expect("serialize event");
        assert_eq!(json["runId"], "ORCH-20260822-001");
        assert_eq!(json["nodeId"], "TO-01");
        assert_eq!(json["type"], "progress");
        assert!(json.get("run_id").is_none());

        let unknown = serde_json::json!({
            "ts": "2026-08-22T00:00:00Z",
            "runId": "ORCH-20260822-001",
            "nodeId": null,
            "worker": "orchestrator",
            "type": "made-up",
            "msg": "invalid",
            "data": null
        });
        assert!(serde_json::from_value::<RunEvent>(unknown).is_err());
    }

    #[test]
    fn appends_and_tails_from_byte_offsets() {
        let path = temp_events("offsets");
        let bus = EventBus::with_lock_timeout(&path, Duration::from_secs(1));
        assert_eq!(bus.path(), path);
        let first = bus.append(&event("first")).expect("append first");
        let second = bus.append(&event("second")).expect("append second");
        assert_eq!(first.start_offset, 0);
        assert_eq!(first.end_offset, second.start_offset);

        let first_batch = bus.tail_from(0).expect("tail all");
        assert_eq!(first_batch.events.len(), 2);
        assert_eq!(first_batch.next_offset, second.end_offset);
        assert!(!first_batch.trailing_partial);

        let second_batch = bus.tail_from(first.end_offset).expect("tail second");
        assert_eq!(second_batch.events, vec![event("second")]);
        assert_eq!(second_batch.next_offset, second.end_offset);
        cleanup(&path);
    }

    #[test]
    fn skips_malformed_complete_lines_but_retains_torn_final_line() {
        let path = temp_events("malformed");
        fs::create_dir_all(path.parent().expect("parent")).expect("create temp dir");
        let valid = serde_json::to_string(&event("valid")).expect("serialize event");
        fs::write(&path, format!("not-json\n{valid}\n{{\"ts\":")).expect("write fixture");

        let bus = EventBus::new(&path);
        let batch = bus.tail_from(0).expect("tail fixture");
        assert_eq!(batch.events, vec![event("valid")]);
        assert_eq!(batch.skipped_lines, 1);
        assert!(batch.trailing_partial);
        assert_eq!(
            batch.next_offset as usize,
            "not-json\n".len() + valid.len() + 1
        );

        let same_partial = bus.tail_from(batch.next_offset).expect("retry torn line");
        assert!(same_partial.events.is_empty());
        assert_eq!(same_partial.next_offset, batch.next_offset);
        assert!(same_partial.trailing_partial);
        cleanup(&path);
    }

    #[test]
    fn append_quarantines_a_preexisting_torn_line() {
        let path = temp_events("repair");
        fs::create_dir_all(path.parent().expect("parent")).expect("create temp dir");
        fs::write(&path, b"{\"torn\":true").expect("write torn line");

        let bus = EventBus::new(&path);
        bus.append(&event("after crash"))
            .expect("append after torn line");
        let batch = bus.tail_from(0).expect("tail repaired file");
        assert_eq!(batch.skipped_lines, 1);
        assert_eq!(batch.events, vec![event("after crash")]);
        assert!(!batch.trailing_partial);
        cleanup(&path);
    }

    #[test]
    fn concurrent_appenders_produce_complete_unique_lines() {
        let path = temp_events("concurrent");
        let bus = EventBus::new(&path);
        let handles = (0..8)
            .map(|worker| {
                let bus = bus.clone();
                thread::spawn(move || {
                    for item in 0..20 {
                        bus.append(&event(format!("{worker}:{item}")))
                            .expect("concurrent append");
                    }
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().expect("writer thread");
        }

        let batch = bus.tail_from(0).expect("tail concurrent events");
        assert_eq!(batch.skipped_lines, 0);
        assert_eq!(batch.events.len(), 160);
        let messages = batch
            .events
            .iter()
            .map(|event| event.msg.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(messages.len(), 160);
        cleanup(&path);
    }
}
