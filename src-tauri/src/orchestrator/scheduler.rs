use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAX_ATTEMPTS: u32 = 3;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeLease {
    pub node_id: String,
    pub worker_id: String,
    pub token: String,
    pub fence: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NodeStatus {
    Ready,
    Running,
    Done,
    Blocked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledNode {
    pub id: String,
    pub wave: u32,
    pub depends_on: Vec<String>,
    pub attempts: u32,
    pub status: NodeStatus,
    pub lease: Option<NodeLease>,
    pub stall_alarm_fence: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerState {
    pub next_fence: u64,
    pub nodes: BTreeMap<String, ScheduledNode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReapAction {
    Reassigned {
        node_id: String,
        worker_id: String,
        preserved_evidence: Option<PathBuf>,
    },
    Blocked {
        node_id: String,
        worker_id: String,
    },
}

#[derive(Clone)]
pub struct SchedulerStore {
    state_path: PathBuf,
    run_dir: PathBuf,
    inner: Arc<Mutex<SchedulerState>>,
}

impl SchedulerStore {
    pub fn open(
        state_path: PathBuf,
        run_dir: PathBuf,
        nodes: Vec<ScheduledNode>,
    ) -> Result<Self, String> {
        let state = if state_path.exists() {
            let bytes = fs::read(&state_path)
                .map_err(|error| format!("cannot read scheduler state: {error}"))?;
            serde_json::from_slice(&bytes)
                .map_err(|error| format!("cannot parse scheduler state: {error}"))?
        } else {
            let state = SchedulerState {
                next_fence: 1,
                nodes: nodes
                    .into_iter()
                    .map(|node| (node.id.clone(), node))
                    .collect(),
            };
            persist(&state_path, &state)?;
            state
        };
        validate_state(&state)?;
        Ok(Self {
            state_path,
            run_dir,
            inner: Arc::new(Mutex::new(state)),
        })
    }

    pub fn snapshot(&self) -> Result<SchedulerState, String> {
        self.inner
            .lock()
            .map_err(|_| "scheduler lock is poisoned".to_string())
            .map(|state| state.clone())
    }

    pub fn claim(
        &self,
        node_id: &str,
        worker_id: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<NodeLease, String> {
        if worker_id.trim().is_empty() || lease_ms < 1_000 {
            return Err("claim requires a worker and lease of at least one second".to_string());
        }
        self.mutate(|state| {
            let dependencies_ready = {
                let node = state
                    .nodes
                    .get(node_id)
                    .ok_or_else(|| format!("unknown node {node_id}"))?;
                node.depends_on.iter().all(|dependency| {
                    state
                        .nodes
                        .get(dependency)
                        .is_some_and(|value| value.status == NodeStatus::Done)
                })
            };
            if !dependencies_ready {
                return Err(format!("node {node_id} has incomplete dependencies"));
            }
            let fence = state.next_fence.max(1);
            state.next_fence = fence.saturating_add(1);
            let node = state
                .nodes
                .get_mut(node_id)
                .ok_or_else(|| format!("unknown node {node_id}"))?;
            if node.status != NodeStatus::Ready || node.lease.is_some() {
                return Err(format!("node {node_id} is not claimable"));
            }
            node.attempts = node.attempts.saturating_add(1);
            let token = lease_token(node_id, worker_id, fence, now_ms);
            let lease = NodeLease {
                node_id: node_id.to_string(),
                worker_id: worker_id.to_string(),
                token,
                fence,
                expires_at_ms: now_ms.saturating_add(lease_ms),
            };
            node.status = NodeStatus::Running;
            node.lease = Some(lease.clone());
            node.stall_alarm_fence = None;
            Ok(lease)
        })
    }

    pub fn renew(
        &self,
        node_id: &str,
        token: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<NodeLease, String> {
        self.mutate(|state| {
            let node = state
                .nodes
                .get_mut(node_id)
                .ok_or_else(|| format!("unknown node {node_id}"))?;
            let lease = node
                .lease
                .as_mut()
                .ok_or_else(|| format!("node {node_id} has no lease"))?;
            if lease.token != token || lease.expires_at_ms <= now_ms {
                return Err("lease token is stale or expired".to_string());
            }
            lease.expires_at_ms = now_ms.saturating_add(lease_ms);
            Ok(lease.clone())
        })
    }

    /// The commit path must call this immediately before `git commit`.
    pub fn authorize_commit(&self, node_id: &str, token: &str, now_ms: u64) -> Result<(), String> {
        let state = self
            .inner
            .lock()
            .map_err(|_| "scheduler lock is poisoned".to_string())?;
        let node = state
            .nodes
            .get(node_id)
            .ok_or_else(|| format!("unknown node {node_id}"))?;
        let lease = node
            .lease
            .as_ref()
            .ok_or_else(|| format!("node {node_id} has no live lease"))?;
        if node.status != NodeStatus::Running
            || lease.token != token
            || lease.expires_at_ms <= now_ms
        {
            return Err("commit fence rejected a stale worker".to_string());
        }
        Ok(())
    }

    pub fn complete(&self, node_id: &str, token: &str, now_ms: u64) -> Result<(), String> {
        self.authorize_commit(node_id, token, now_ms)?;
        self.mutate(|state| {
            let node = state
                .nodes
                .get_mut(node_id)
                .ok_or_else(|| format!("unknown node {node_id}"))?;
            if node.lease.as_ref().map(|lease| lease.token.as_str()) != Some(token) {
                return Err("completion fence changed before persistence".to_string());
            }
            node.status = NodeStatus::Done;
            node.lease = None;
            Ok(())
        })
    }

    pub fn fail(&self, node_id: &str, token: &str) -> Result<NodeStatus, String> {
        self.mutate(|state| {
            let node = state
                .nodes
                .get_mut(node_id)
                .ok_or_else(|| format!("unknown node {node_id}"))?;
            if node.lease.as_ref().map(|lease| lease.token.as_str()) != Some(token) {
                return Err("failure report came from a stale worker".to_string());
            }
            node.lease = None;
            node.status = if node.attempts >= MAX_ATTEMPTS {
                NodeStatus::Blocked
            } else {
                NodeStatus::Ready
            };
            Ok(node.status.clone())
        })
    }

    pub fn reap_expired(&self, now_ms: u64) -> Result<Vec<ReapAction>, String> {
        let mut expired = Vec::new();
        {
            let state = self
                .inner
                .lock()
                .map_err(|_| "scheduler lock is poisoned".to_string())?;
            for node in state.nodes.values() {
                if let Some(lease) = &node.lease {
                    if lease.expires_at_ms <= now_ms {
                        expired.push((node.id.clone(), lease.clone(), node.attempts));
                    }
                }
            }
        }
        let mut actions = Vec::new();
        for (node_id, lease, attempts) in expired {
            let preserved =
                preserve_evidence(&self.run_dir, &node_id, &lease.worker_id, lease.fence)?;
            let action = self.mutate(|state| {
                let node = state
                    .nodes
                    .get_mut(&node_id)
                    .ok_or_else(|| format!("unknown node {node_id}"))?;
                if node.lease.as_ref().map(|value| value.token.as_str())
                    != Some(lease.token.as_str())
                {
                    return Err("lease changed while stalled evidence was preserved".to_string());
                }
                node.lease = None;
                node.stall_alarm_fence = Some(lease.fence);
                if attempts >= MAX_ATTEMPTS {
                    node.status = NodeStatus::Blocked;
                    Ok(ReapAction::Blocked {
                        node_id: node_id.clone(),
                        worker_id: lease.worker_id.clone(),
                    })
                } else {
                    node.status = NodeStatus::Ready;
                    Ok(ReapAction::Reassigned {
                        node_id: node_id.clone(),
                        worker_id: lease.worker_id.clone(),
                        preserved_evidence: preserved.clone(),
                    })
                }
            })?;
            actions.push(action);
        }
        Ok(actions)
    }

    fn mutate<T>(
        &self,
        operation: impl FnOnce(&mut SchedulerState) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| "scheduler lock is poisoned".to_string())?;
        let before = state.clone();
        let result = operation(&mut state);
        match result {
            Ok(value) => {
                if let Err(error) =
                    validate_state(&state).and_then(|_| persist(&self.state_path, &state))
                {
                    *state = before;
                    return Err(error);
                }
                Ok(value)
            }
            Err(error) => {
                *state = before;
                Err(error)
            }
        }
    }
}

fn validate_state(state: &SchedulerState) -> Result<(), String> {
    for (id, node) in &state.nodes {
        if id != &node.id || id.trim().is_empty() {
            return Err("scheduler node identity mismatch".to_string());
        }
        if node.status == NodeStatus::Running && node.lease.is_none() {
            return Err(format!("running node {id} has no lease"));
        }
        if node.status != NodeStatus::Running && node.lease.is_some() {
            return Err(format!("non-running node {id} retains a lease"));
        }
        for dependency in &node.depends_on {
            if dependency == id || !state.nodes.contains_key(dependency) {
                return Err(format!("node {id} has an invalid dependency"));
            }
        }
    }
    Ok(())
}

fn lease_token(node_id: &str, worker_id: &str, fence: u64, now_ms: u64) -> String {
    let mut digest = Sha256::new();
    digest.update(node_id.as_bytes());
    digest.update([0]);
    digest.update(worker_id.as_bytes());
    digest.update([0]);
    digest.update(fence.to_le_bytes());
    digest.update(now_ms.to_le_bytes());
    digest.update(std::process::id().to_le_bytes());
    format!("{:x}", digest.finalize())
}

fn persist(path: &Path, state: &SchedulerState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create scheduler directory: {error}"))?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("cannot encode scheduler state: {error}"))?;
    fs::write(&temporary, bytes)
        .map_err(|error| format!("cannot write scheduler state: {error}"))?;
    if path.exists() {
        fs::remove_file(path)
            .map_err(|error| format!("cannot replace scheduler state: {error}"))?;
    }
    fs::rename(&temporary, path).map_err(|error| format!("cannot publish scheduler state: {error}"))
}

fn preserve_evidence(
    run_dir: &Path,
    node_id: &str,
    worker_id: &str,
    fence: u64,
) -> Result<Option<PathBuf>, String> {
    let source = run_dir.join("evidence").join(node_id).join(worker_id);
    if !source.exists() {
        return Ok(None);
    }
    let destination = run_dir
        .join("evidence-preserved")
        .join(format!("{node_id}-{worker_id}-f{fence}"));
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create preserved evidence directory: {error}"))?;
    }
    fs::rename(&source, &destination)
        .map_err(|error| format!("cannot preserve expired worker evidence: {error}"))?;
    Ok(Some(destination))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("pp-scheduler-{label}-{id}"));
        fs::create_dir_all(&path).expect("temp dir");
        path
    }

    fn node() -> ScheduledNode {
        ScheduledNode {
            id: "B01".to_string(),
            wave: 1,
            depends_on: Vec::new(),
            attempts: 0,
            status: NodeStatus::Ready,
            lease: None,
            stall_alarm_fence: None,
        }
    }

    #[test]
    fn exactly_one_claim_wins_and_stale_commit_is_fenced() {
        let root = temp_dir("claim");
        let store = SchedulerStore::open(root.join("leases.json"), root.clone(), vec![node()])
            .expect("store");
        let lease = store.claim("B01", "worker-a", 100, 1_000).expect("claim");
        assert!(store.claim("B01", "worker-b", 101, 1_000).is_err());
        assert!(store.authorize_commit("B01", &lease.token, 999).is_ok());
        assert!(store.authorize_commit("B01", "stale", 999).is_err());
        assert!(store.authorize_commit("B01", &lease.token, 1_100).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn expiry_preserves_evidence_and_third_attempt_blocks() {
        let root = temp_dir("reap");
        let store = SchedulerStore::open(root.join("leases.json"), root.clone(), vec![node()])
            .expect("store");
        for attempt in 1..=3 {
            let lease = store
                .claim("B01", &format!("worker-{attempt}"), attempt * 10, 1_000)
                .expect("claim");
            let evidence = root
                .join("evidence")
                .join("B01")
                .join(format!("worker-{attempt}"));
            fs::create_dir_all(&evidence).expect("evidence dir");
            fs::write(evidence.join("proof.txt"), b"proof").expect("proof");
            let actions = store
                .reap_expired(lease.expires_at_ms)
                .expect("reap expired");
            assert_eq!(actions.len(), 1);
            if attempt < 3 {
                assert!(matches!(actions[0], ReapAction::Reassigned { .. }));
            } else {
                assert!(matches!(actions[0], ReapAction::Blocked { .. }));
            }
        }
        assert_eq!(
            store.snapshot().expect("snapshot").nodes["B01"].status,
            NodeStatus::Blocked
        );
        let _ = fs::remove_dir_all(root);
    }
}
