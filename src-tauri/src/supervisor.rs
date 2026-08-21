use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const REAPER_INTERVAL_MS: u64 = 5_000;
pub const RECOVERY_GRACE_MS: u64 = 120_000;
const TOMBSTONE_RETENTION_MS: u64 = 24 * 60 * 60 * 1_000;
const MAX_RECENT_EVENTS: usize = 200;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceState {
    Active,
    Stale,
    Gone,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionObservation {
    pub organization_id: String,
    pub plan_path: String,
    pub vertebra: String,
    pub session_id: String,
    pub source_state: SourceState,
    #[serde(default)]
    pub last_heartbeat: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub resources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LeaseDisposition {
    Live,
    Grace,
    Cleared,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLease {
    pub key: String,
    pub organization_id: String,
    pub plan_path: String,
    pub vertebra: String,
    pub session_id: String,
    pub source_state: SourceState,
    pub disposition: LeaseDisposition,
    pub fence: u64,
    pub first_stale_at_ms: Option<u64>,
    pub cleared_at_ms: Option<u64>,
    pub last_observed_at_ms: u64,
    pub last_heartbeat: Option<String>,
    pub files: Vec<String>,
    pub resources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaperEvent {
    pub id: String,
    pub kind: String,
    pub at_ms: u64,
    pub organization_id: String,
    pub plan_path: String,
    pub vertebra: String,
    pub session_id: String,
    pub fence: u64,
    pub reason: String,
    pub files: Vec<String>,
    pub resources: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisorSnapshot {
    pub now_ms: u64,
    pub reaper_interval_ms: u64,
    pub recovery_grace_ms: u64,
    pub leases: Vec<SessionLease>,
    pub events: Vec<ReaperEvent>,
    pub live_count: usize,
    pub grace_count: usize,
    pub cleared_count: usize,
}

#[derive(Default)]
struct SupervisorInner {
    leases: BTreeMap<String, SessionLease>,
    recent_events: VecDeque<ReaperEvent>,
    event_counter: u64,
}

#[derive(Clone)]
pub struct SupervisorStore {
    inner: Arc<Mutex<SupervisorInner>>,
    ledger_path: Arc<PathBuf>,
}

impl SupervisorStore {
    pub fn open(ledger_path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = ledger_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create supervisor data directory: {error}"))?;
        }
        let mut inner = SupervisorInner::default();
        load_ledger(&ledger_path, &mut inner)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
            ledger_path: Arc::new(ledger_path),
        })
    }

    #[cfg(test)]
    fn memory_for_test(ledger_path: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SupervisorInner::default())),
            ledger_path: Arc::new(ledger_path),
        }
    }

    pub fn observe(
        &self,
        observations: Vec<SessionObservation>,
        now_ms: u64,
    ) -> Result<SupervisorSnapshot, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "supervisor state lock is poisoned".to_string())?;
        remove_expired_tombstones(&mut inner, now_ms);

        for observation in observations {
            if observation.organization_id.trim().is_empty()
                || observation.plan_path.trim().is_empty()
                || observation.vertebra.trim().is_empty()
                || observation.session_id.trim().is_empty()
            {
                continue;
            }
            let key = lease_key(&observation);
            let assignment_fence = inner
                .leases
                .values()
                .filter(|lease| {
                    lease.organization_id == observation.organization_id
                        && lease.plan_path == observation.plan_path
                        && lease.vertebra == observation.vertebra
                })
                .map(|lease| lease.fence)
                .max()
                .unwrap_or(0);

            if let Some(existing) = inner.leases.get_mut(&key) {
                existing.last_observed_at_ms = now_ms;
                existing.last_heartbeat = observation.last_heartbeat.clone();
                existing.files = observation.files.clone();
                existing.resources = observation.resources.clone();
                existing.source_state = observation.source_state.clone();
                match observation.source_state {
                    SourceState::Active if existing.disposition != LeaseDisposition::Cleared => {
                        existing.disposition = LeaseDisposition::Live;
                        existing.first_stale_at_ms = None;
                    }
                    SourceState::Stale | SourceState::Gone
                        if existing.disposition == LeaseDisposition::Live =>
                    {
                        existing.disposition = LeaseDisposition::Grace;
                        existing.first_stale_at_ms = Some(now_ms);
                    }
                    _ => {}
                }
            } else {
                let stale = observation.source_state != SourceState::Active;
                inner.leases.insert(
                    key.clone(),
                    SessionLease {
                        key,
                        organization_id: observation.organization_id,
                        plan_path: observation.plan_path,
                        vertebra: observation.vertebra,
                        session_id: observation.session_id,
                        source_state: observation.source_state,
                        disposition: if stale {
                            LeaseDisposition::Grace
                        } else {
                            LeaseDisposition::Live
                        },
                        fence: assignment_fence.saturating_add(1),
                        first_stale_at_ms: stale.then_some(now_ms),
                        cleared_at_ms: None,
                        last_observed_at_ms: now_ms,
                        last_heartbeat: observation.last_heartbeat,
                        files: observation.files,
                        resources: observation.resources,
                    },
                );
            }
        }

        reap_locked(&mut inner, &self.ledger_path, now_ms)?;
        Ok(snapshot_locked(&inner, now_ms))
    }

    pub fn snapshot(&self, now_ms: u64) -> Result<SupervisorSnapshot, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "supervisor state lock is poisoned".to_string())?;
        remove_expired_tombstones(&mut inner, now_ms);
        reap_locked(&mut inner, &self.ledger_path, now_ms)?;
        Ok(snapshot_locked(&inner, now_ms))
    }

    pub fn spawn_reaper(&self) -> Result<(), String> {
        let store = self.clone();
        std::thread::Builder::new()
            .name("perfect-planner-session-reaper".to_string())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(REAPER_INTERVAL_MS));
                let _ = store.snapshot(unix_ms());
            })
            .map(|_| ())
            .map_err(|error| format!("cannot start session reaper: {error}"))
    }
}

fn lease_key(observation: &SessionObservation) -> String {
    format!(
        "{}\u{0}{}\u{0}{}\u{0}{}",
        observation.organization_id,
        observation.plan_path.to_lowercase(),
        observation.vertebra,
        observation.session_id
    )
}

fn remove_expired_tombstones(inner: &mut SupervisorInner, now_ms: u64) {
    inner.leases.retain(|_, lease| {
        lease.disposition != LeaseDisposition::Cleared
            || lease
                .cleared_at_ms
                .is_some_and(|cleared| now_ms.saturating_sub(cleared) <= TOMBSTONE_RETENTION_MS)
    });
}

fn reap_locked(inner: &mut SupervisorInner, ledger_path: &Path, now_ms: u64) -> Result<(), String> {
    let candidates: Vec<(String, ReaperEvent)> = inner
        .leases
        .iter()
        .filter_map(|(key, lease)| {
            let stale_at = lease.first_stale_at_ms?;
            if lease.disposition != LeaseDisposition::Grace
                || now_ms.saturating_sub(stale_at) < RECOVERY_GRACE_MS
            {
                return None;
            }
            let fence = lease.fence.saturating_add(1);
            Some((
                key.clone(),
                ReaperEvent {
                    id: String::new(),
                    kind: "SESSION_CLEARED".to_string(),
                    at_ms: now_ms,
                    organization_id: lease.organization_id.clone(),
                    plan_path: lease.plan_path.clone(),
                    vertebra: lease.vertebra.clone(),
                    session_id: lease.session_id.clone(),
                    fence,
                    reason: format!(
                        "source remained {:?} for the full recovery grace",
                        lease.source_state
                    ),
                    files: lease.files.clone(),
                    resources: lease.resources.clone(),
                },
            ))
        })
        .collect();

    if candidates.is_empty() {
        return Ok(());
    }

    let mut events = Vec::with_capacity(candidates.len());
    for (_, mut event) in candidates.iter().cloned() {
        inner.event_counter = inner.event_counter.saturating_add(1);
        event.id = format!("pp-reaper-{now_ms}-{}", inner.event_counter);
        events.push(event);
    }
    append_events(ledger_path, &events)?;

    for ((key, _), event) in candidates.into_iter().zip(events.iter()) {
        if let Some(lease) = inner.leases.get_mut(&key) {
            lease.disposition = LeaseDisposition::Cleared;
            lease.cleared_at_ms = Some(now_ms);
            lease.fence = event.fence;
            lease.files.clear();
            lease.resources.clear();
        }
    }
    for event in events {
        inner.recent_events.push_back(event);
        while inner.recent_events.len() > MAX_RECENT_EVENTS {
            inner.recent_events.pop_front();
        }
    }
    Ok(())
}

fn append_events(path: &Path, events: &[ReaperEvent]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("cannot open session reaper ledger: {error}"))?;
    for event in events {
        let line = serde_json::to_string(event)
            .map_err(|error| format!("cannot serialize session reaper event: {error}"))?;
        writeln!(file, "{line}")
            .map_err(|error| format!("cannot append session reaper event: {error}"))?;
    }
    file.sync_data()
        .map_err(|error| format!("cannot flush session reaper ledger: {error}"))
}

fn load_ledger(path: &Path, inner: &mut SupervisorInner) -> Result<(), String> {
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot read session reaper ledger: {error}")),
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(event) = serde_json::from_str::<ReaperEvent>(&line) else {
            continue;
        };
        inner.event_counter = inner.event_counter.saturating_add(1);
        let observation = SessionObservation {
            organization_id: event.organization_id.clone(),
            plan_path: event.plan_path.clone(),
            vertebra: event.vertebra.clone(),
            session_id: event.session_id.clone(),
            source_state: SourceState::Gone,
            last_heartbeat: None,
            files: Vec::new(),
            resources: Vec::new(),
        };
        let key = lease_key(&observation);
        inner.leases.insert(
            key.clone(),
            SessionLease {
                key,
                organization_id: observation.organization_id,
                plan_path: observation.plan_path,
                vertebra: observation.vertebra,
                session_id: observation.session_id,
                source_state: SourceState::Gone,
                disposition: LeaseDisposition::Cleared,
                fence: event.fence,
                first_stale_at_ms: None,
                cleared_at_ms: Some(event.at_ms),
                last_observed_at_ms: event.at_ms,
                last_heartbeat: None,
                files: Vec::new(),
                resources: Vec::new(),
            },
        );
        inner.recent_events.push_back(event);
        while inner.recent_events.len() > MAX_RECENT_EVENTS {
            inner.recent_events.pop_front();
        }
    }
    Ok(())
}

fn snapshot_locked(inner: &SupervisorInner, now_ms: u64) -> SupervisorSnapshot {
    let leases: Vec<_> = inner.leases.values().cloned().collect();
    SupervisorSnapshot {
        now_ms,
        reaper_interval_ms: REAPER_INTERVAL_MS,
        recovery_grace_ms: RECOVERY_GRACE_MS,
        live_count: leases
            .iter()
            .filter(|lease| lease.disposition == LeaseDisposition::Live)
            .count(),
        grace_count: leases
            .iter()
            .filter(|lease| lease.disposition == LeaseDisposition::Grace)
            .count(),
        cleared_count: leases
            .iter()
            .filter(|lease| lease.disposition == LeaseDisposition::Cleared)
            .count(),
        leases,
        events: inner.recent_events.iter().cloned().collect(),
    }
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(session: &str, state: SourceState) -> SessionObservation {
        SessionObservation {
            organization_id: "pp-org-test".to_string(),
            plan_path: "C:/worktree/.claude/scratch/perfect-plan/plan.json".to_string(),
            vertebra: "A01".to_string(),
            session_id: session.to_string(),
            source_state: state,
            last_heartbeat: None,
            files: vec!["src/a.ts".to_string()],
            resources: vec!["db:test".to_string()],
        }
    }

    fn test_store(name: &str) -> SupervisorStore {
        let path = std::env::temp_dir().join(format!(
            "perfect-planner-{name}-{}-{}.jsonl",
            std::process::id(),
            unix_ms()
        ));
        SupervisorStore::memory_for_test(path)
    }

    #[test]
    fn stale_requires_the_full_grace_before_claims_are_cleared() {
        let store = test_store("grace");
        let first = store
            .observe(vec![observation("s-old", SourceState::Stale)], 1_000)
            .unwrap();
        assert_eq!(first.grace_count, 1);
        assert_eq!(first.cleared_count, 0);

        let before = store.snapshot(1_000 + RECOVERY_GRACE_MS - 1).unwrap();
        assert_eq!(before.grace_count, 1);

        let after = store.snapshot(1_000 + RECOVERY_GRACE_MS).unwrap();
        assert_eq!(after.cleared_count, 1);
        assert!(after.leases[0].files.is_empty());
        assert!(after.leases[0].resources.is_empty());
        assert_eq!(after.events.len(), 1);
    }

    #[test]
    fn cleared_session_cannot_revive_and_new_session_gets_a_higher_fence() {
        let store = test_store("fence");
        store
            .observe(vec![observation("s-old", SourceState::Stale)], 10)
            .unwrap();
        let cleared = store.snapshot(10 + RECOVERY_GRACE_MS).unwrap();
        let old_fence = cleared.leases[0].fence;

        let zombie = store
            .observe(
                vec![observation("s-old", SourceState::Active)],
                20 + RECOVERY_GRACE_MS,
            )
            .unwrap();
        assert_eq!(zombie.cleared_count, 1);
        assert_eq!(zombie.live_count, 0);

        let replacement = store
            .observe(
                vec![observation("s-new", SourceState::Active)],
                30 + RECOVERY_GRACE_MS,
            )
            .unwrap();
        let new_lease = replacement
            .leases
            .iter()
            .find(|lease| lease.session_id == "s-new")
            .unwrap();
        assert_eq!(new_lease.disposition, LeaseDisposition::Live);
        assert!(new_lease.fence > old_fence);
    }

    #[test]
    fn organizations_do_not_share_assignment_fences() {
        let store = test_store("organizations");
        let mut other = observation("s-two", SourceState::Active);
        other.organization_id = "pp-org-other".to_string();
        let snapshot = store
            .observe(vec![observation("s-one", SourceState::Active), other], 50)
            .unwrap();
        assert_eq!(snapshot.live_count, 2);
        assert!(snapshot.leases.iter().all(|lease| lease.fence == 1));
    }
}
