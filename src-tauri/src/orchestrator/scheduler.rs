use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::authority_runtime::AuthorizedLeaseGrant;
use super::evidence::{EvidenceArtifact, VerificationResult};
use super::worker::WorkerGateResult;

const MAX_ATTEMPTS: u32 = 3;
const MAX_PARALLEL_WORKERS: u16 = 4;
const AUTHORITY_SCHEMA_VERSION: u32 = 1;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeLease {
    pub node_id: String,
    pub worker_id: String,
    pub token: String,
    pub fence: u64,
    pub expires_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) git_baseline: Option<AdmissionGitBaseline>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AdmissionGitBaseline {
    pub(crate) head_commit: String,
    pub(crate) outside_manifest_digest: String,
}

/// Renderer-safe lease state. The bearer token never crosses the native boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicNodeLease {
    pub node_id: String,
    pub worker_id: String,
    pub fence: u64,
    pub expires_at_ms: u64,
    pub authority_epoch: Option<u64>,
    pub authorization_id: Option<String>,
}

impl From<&NodeLease> for PublicNodeLease {
    fn from(lease: &NodeLease) -> Self {
        Self {
            node_id: lease.node_id.clone(),
            worker_id: lease.worker_id.clone(),
            fence: lease.fence,
            expires_at_ms: lease.expires_at_ms,
            authority_epoch: lease.authority_epoch,
            authorization_id: lease.authorization_id.clone(),
        }
    }
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyLeaseRevocation {
    pub node_id: String,
    pub worker_id: String,
    pub fence: u64,
    pub previous_expires_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerState {
    #[serde(default)]
    pub authority_schema_version: u32,
    #[serde(default)]
    pub pending_legacy_revocations: Vec<LegacyLeaseRevocation>,
    #[serde(default)]
    pub consumed_authorization_ids: BTreeSet<String>,
    #[serde(default)]
    pub completions: BTreeMap<String, NodeCompletion>,
    #[serde(default = "serial_worker_limit")]
    pub max_parallel_workers: u16,
    pub next_fence: u64,
    pub nodes: BTreeMap<String, ScheduledNode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicSchedulerState {
    pub authority_schema_version: u32,
    pub pending_legacy_revocations: Vec<LegacyLeaseRevocation>,
    pub completions: BTreeMap<String, NodeCompletion>,
    pub max_parallel_workers: u16,
    pub next_fence: u64,
    pub nodes: BTreeMap<String, PublicScheduledNode>,
}

impl Default for SchedulerState {
    fn default() -> Self {
        Self {
            authority_schema_version: 0,
            pending_legacy_revocations: Vec::new(),
            consumed_authorization_ids: BTreeSet::new(),
            completions: BTreeMap::new(),
            max_parallel_workers: serial_worker_limit(),
            next_fence: 1,
            nodes: BTreeMap::new(),
        }
    }
}

impl Default for PublicSchedulerState {
    fn default() -> Self {
        Self {
            authority_schema_version: 0,
            pending_legacy_revocations: Vec::new(),
            completions: BTreeMap::new(),
            max_parallel_workers: serial_worker_limit(),
            next_fence: 1,
            nodes: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicScheduledNode {
    pub id: String,
    pub wave: u32,
    pub depends_on: Vec<String>,
    pub attempts: u32,
    pub status: NodeStatus,
    pub lease: Option<PublicNodeLease>,
    pub stall_alarm_fence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeCompletion {
    pub receipt_id: String,
    pub node_id: String,
    pub worker_id: String,
    pub fence: u64,
    pub authority_epoch: u64,
    pub authorization_id: String,
    pub completed_at_ms: u64,
    pub changed_files: Vec<String>,
    pub artifacts: Vec<EvidenceArtifact>,
    pub verification: Vec<VerificationResult>,
    pub gate: WorkerGateResult,
}

impl From<&SchedulerState> for PublicSchedulerState {
    fn from(state: &SchedulerState) -> Self {
        Self {
            authority_schema_version: state.authority_schema_version,
            pending_legacy_revocations: state.pending_legacy_revocations.clone(),
            completions: state.completions.clone(),
            max_parallel_workers: state.max_parallel_workers,
            next_fence: state.next_fence,
            nodes: state
                .nodes
                .iter()
                .map(|(id, node)| {
                    (
                        id.clone(),
                        PublicScheduledNode {
                            id: node.id.clone(),
                            wave: node.wave,
                            depends_on: node.depends_on.clone(),
                            attempts: node.attempts,
                            status: node.status.clone(),
                            lease: node.lease.as_ref().map(PublicNodeLease::from),
                            stall_alarm_fence: node.stall_alarm_fence,
                        },
                    )
                })
                .collect(),
        }
    }
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
        Self::open_with_limit(state_path, run_dir, nodes, serial_worker_limit())
    }

    pub fn open_with_limit(
        state_path: PathBuf,
        run_dir: PathBuf,
        nodes: Vec<ScheduledNode>,
        max_parallel_workers: u16,
    ) -> Result<Self, String> {
        if !(1..=MAX_PARALLEL_WORKERS).contains(&max_parallel_workers) {
            return Err(format!(
                "parallel worker limit must be between 1 and {MAX_PARALLEL_WORKERS}"
            ));
        }
        let mut state = if state_path.exists() {
            let bytes = fs::read(&state_path)
                .map_err(|error| format!("cannot read scheduler state: {error}"))?;
            serde_json::from_slice(&bytes)
                .map_err(|error| format!("cannot parse scheduler state: {error}"))?
        } else {
            let state = SchedulerState {
                authority_schema_version: AUTHORITY_SCHEMA_VERSION,
                pending_legacy_revocations: Vec::new(),
                consumed_authorization_ids: BTreeSet::new(),
                completions: BTreeMap::new(),
                max_parallel_workers,
                next_fence: 1,
                nodes: nodes
                    .into_iter()
                    .map(|node| (node.id.clone(), node))
                    .collect(),
            };
            persist(&state_path, &state)?;
            state
        };
        if state.authority_schema_version < AUTHORITY_SCHEMA_VERSION {
            let mut revoked = Vec::new();
            for node in state.nodes.values_mut() {
                let Some(lease) = node.lease.take() else {
                    continue;
                };
                revoked.push(LegacyLeaseRevocation {
                    node_id: node.id.clone(),
                    worker_id: lease.worker_id,
                    fence: lease.fence,
                    previous_expires_at_ms: lease.expires_at_ms,
                });
                node.stall_alarm_fence = Some(lease.fence);
                node.status = if node.attempts >= MAX_ATTEMPTS {
                    NodeStatus::Blocked
                } else {
                    NodeStatus::Ready
                };
            }
            state.pending_legacy_revocations.extend(revoked);
            state.authority_schema_version = AUTHORITY_SCHEMA_VERSION;
            validate_state(&state)?;
            persist(&state_path, &state)?;
        }
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

    pub fn public_snapshot(&self) -> Result<PublicSchedulerState, String> {
        self.snapshot()
            .map(|state| PublicSchedulerState::from(&state))
    }

    pub fn pending_legacy_revocations(&self) -> Result<Vec<LegacyLeaseRevocation>, String> {
        self.inner
            .lock()
            .map_err(|_| "scheduler lock is poisoned".to_string())
            .map(|state| state.pending_legacy_revocations.clone())
    }

    pub fn acknowledge_legacy_revocations(
        &self,
        acknowledged: &[LegacyLeaseRevocation],
    ) -> Result<(), String> {
        self.mutate(|state| {
            if state.pending_legacy_revocations != acknowledged {
                return Err(
                    "legacy revocation audit queue changed before acknowledgement".to_string(),
                );
            }
            state.pending_legacy_revocations.clear();
            Ok(())
        })
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
            enforce_parallel_limit(state)?;
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
            let token = lease_token()?;
            let lease = NodeLease {
                node_id: node_id.to_string(),
                worker_id: worker_id.to_string(),
                token,
                fence,
                expires_at_ms: now_ms.saturating_add(lease_ms),
                authority_epoch: None,
                authorization_id: None,
                git_baseline: None,
            };
            node.status = NodeStatus::Running;
            node.lease = Some(lease.clone());
            node.stall_alarm_fence = None;
            Ok(lease)
        })
    }

    /// The only B20 path from a consumed, signed CLEAR result to a persisted worker lease.
    /// The authorization ID and authority epoch are committed in the same scheduler state write
    /// as the lease, so a crash cannot persist write authority without also consuming replay.
    pub(crate) fn claim_authorized(
        &self,
        grant: &AuthorizedLeaseGrant,
        git_baseline: AdmissionGitBaseline,
        now_ms: u64,
    ) -> Result<NodeLease, String> {
        grant.verify(now_ms)?;
        let authorization = grant.authorization();
        if authorization.issued_at_ms != now_ms || authorization.expires_at_ms <= now_ms {
            return Err("authorized lease grant has a stale or non-exact issue time".to_string());
        }
        let node_id = authorization.binding.node_id.as_str();
        let worker_id = authorization.worker_id.as_str();
        self.mutate(|state| {
            enforce_parallel_limit(state)?;
            if state
                .consumed_authorization_ids
                .contains(&authorization.authorization_id)
            {
                return Err("authorized lease grant was already consumed".to_string());
            }
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
            if authorization.binding.fence != fence
                || authorization.binding.epoch == 0
                || authorization.binding.generation == 0
                || grant.scope_id().trim().is_empty()
            {
                return Err(
                    "authorized lease grant drifted from scheduler fence or scope".to_string(),
                );
            }
            let node = state
                .nodes
                .get_mut(node_id)
                .ok_or_else(|| format!("unknown node {node_id}"))?;
            if node.status != NodeStatus::Ready || node.lease.is_some() {
                return Err(format!("node {node_id} is not claimable"));
            }
            node.attempts = node.attempts.saturating_add(1);
            let token = lease_token()?;
            let lease = NodeLease {
                node_id: node_id.to_string(),
                worker_id: worker_id.to_string(),
                token,
                fence,
                expires_at_ms: authorization.expires_at_ms,
                authority_epoch: Some(authorization.binding.epoch),
                authorization_id: Some(authorization.authorization_id.clone()),
                git_baseline: Some(git_baseline.clone()),
            };
            node.status = NodeStatus::Running;
            node.lease = Some(lease.clone());
            node.stall_alarm_fence = None;
            state.next_fence = fence.saturating_add(1);
            state
                .consumed_authorization_ids
                .insert(authorization.authorization_id.clone());
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

    pub(crate) fn complete_authorized(
        &self,
        node_id: &str,
        token: &str,
        now_ms: u64,
        completion: NodeCompletion,
    ) -> Result<NodeCompletion, String> {
        if completion.node_id != node_id
            || completion.receipt_id.trim().is_empty()
            || completion.worker_id.trim().is_empty()
            || completion.authorization_id.trim().is_empty()
            || completion.completed_at_ms != now_ms
            || completion.changed_files.is_empty()
            || completion.artifacts.is_empty()
            || completion.verification.is_empty()
            || !completion.gate.passed
        {
            return Err("authorized completion receipt is incomplete".to_string());
        }
        if let Some(existing) = self.snapshot()?.completions.get(node_id).cloned() {
            return if existing == completion {
                Ok(existing)
            } else {
                Err("node already has a different completion receipt".to_string())
            };
        }
        self.authorize_commit(node_id, token, now_ms)?;
        self.mutate(|state| {
            let node = state
                .nodes
                .get_mut(node_id)
                .ok_or_else(|| format!("unknown node {node_id}"))?;
            let lease = node
                .lease
                .as_ref()
                .ok_or_else(|| format!("node {node_id} has no live lease"))?;
            if lease.token != token
                || lease.worker_id != completion.worker_id
                || lease.fence != completion.fence
                || lease.authority_epoch != Some(completion.authority_epoch)
                || lease.authorization_id.as_deref() != Some(completion.authorization_id.as_str())
            {
                return Err("completion receipt drifted from the live native lease".to_string());
            }
            node.status = NodeStatus::Done;
            node.lease = None;
            state
                .completions
                .insert(node_id.to_string(), completion.clone());
            Ok(completion.clone())
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
        self.recover_matching_leases(|lease| lease.expires_at_ms <= now_ms)
    }

    /// Recover leases that cannot be trusted by the current native authority owner. This is
    /// intentionally broader than expiry: an app restart rotates the issuer epoch immediately,
    /// so a still-unexpired lease from the previous process must stop appearing RUNNING.
    pub(crate) fn recover_stale_authority(
        &self,
        current_epoch: u64,
        now_ms: u64,
    ) -> Result<Vec<ReapAction>, String> {
        if current_epoch == 0 {
            return Err("scheduler authority epoch must be non-zero".to_string());
        }
        self.recover_matching_leases(|lease| {
            lease.expires_at_ms <= now_ms
                || lease.authority_epoch != Some(current_epoch)
                || lease.authorization_id.is_none()
                || lease.git_baseline.is_none()
        })
    }

    fn recover_matching_leases(
        &self,
        should_recover: impl Fn(&NodeLease) -> bool,
    ) -> Result<Vec<ReapAction>, String> {
        let mut expired = Vec::new();
        {
            let state = self
                .inner
                .lock()
                .map_err(|_| "scheduler lock is poisoned".to_string())?;
            for node in state.nodes.values() {
                if let Some(lease) = &node.lease {
                    if should_recover(lease) {
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

fn serial_worker_limit() -> u16 {
    1
}

fn enforce_parallel_limit(state: &SchedulerState) -> Result<(), String> {
    let active = state
        .nodes
        .values()
        .filter(|node| node.status == NodeStatus::Running && node.lease.is_some())
        .count();
    if active >= usize::from(state.max_parallel_workers) {
        return Err(format!(
            "parallel worker limit reached ({}/{})",
            active, state.max_parallel_workers
        ));
    }
    Ok(())
}

fn validate_state(state: &SchedulerState) -> Result<(), String> {
    if !(1..=MAX_PARALLEL_WORKERS).contains(&state.max_parallel_workers) {
        return Err(format!(
            "scheduler parallel worker limit must be between 1 and {MAX_PARALLEL_WORKERS}"
        ));
    }
    let active = state
        .nodes
        .values()
        .filter(|node| node.status == NodeStatus::Running && node.lease.is_some())
        .count();
    if active > usize::from(state.max_parallel_workers) {
        return Err("scheduler state exceeds its parallel worker limit".to_string());
    }
    if state.completions.iter().any(|(node_id, completion)| {
        completion.node_id != *node_id
            || !state
                .nodes
                .get(node_id)
                .is_some_and(|node| node.status == NodeStatus::Done && node.lease.is_none())
            || completion.receipt_id.trim().is_empty()
            || completion.authorization_id.trim().is_empty()
            || completion.changed_files.is_empty()
            || completion.artifacts.is_empty()
            || completion.verification.is_empty()
            || !completion.gate.passed
    }) {
        return Err("scheduler contains an invalid completion receipt".to_string());
    }
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
        if let Some(lease) = &node.lease {
            if lease.authority_epoch.is_some()
                && (lease.authorization_id.is_none()
                    || lease.git_baseline.as_ref().is_none_or(|baseline| {
                        !is_hex_digest(&baseline.outside_manifest_digest)
                            || baseline.head_commit.len() != 40
                            || !baseline
                                .head_commit
                                .bytes()
                                .all(|byte| byte.is_ascii_hexdigit())
                    }))
            {
                return Err(format!(
                    "authority-backed lease for node {id} lacks a valid Git baseline"
                ));
            }
        }
        for dependency in &node.depends_on {
            if dependency == id || !state.nodes.contains_key(dependency) {
                return Err(format!("node {id} has an invalid dependency"));
            }
        }
    }
    Ok(())
}

fn is_hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn lease_token() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    fill_os_random(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(windows)]
fn fill_os_random(bytes: &mut [u8]) -> Result<(), String> {
    use std::ptr;
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> i32;
    }
    let length = u32::try_from(bytes.len()).map_err(|_| "lease entropy request is too large")?;
    let status =
        unsafe { BCryptGenRandom(ptr::null_mut(), bytes.as_mut_ptr(), length, 0x0000_0002) };
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "operating-system lease entropy failed: NTSTATUS {status:#x}"
        ))
    }
}

#[cfg(not(windows))]
fn fill_os_random(bytes: &mut [u8]) -> Result<(), String> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(bytes))
        .map_err(|error| format!("operating-system lease entropy failed: {error}"))
}

fn persist(path: &Path, state: &SchedulerState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create scheduler directory: {error}"))?;
    }
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "scheduler state path has an invalid file name".to_string())?;
    let temporary = path.with_file_name(format!(
        ".{file_name}.tmp-{}-{sequence}",
        std::process::id()
    ));
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("cannot encode scheduler state: {error}"))?;
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("cannot create scheduler temporary state: {error}"))?;
        file.write_all(&bytes)
            .map_err(|error| format!("cannot write scheduler state: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("cannot flush scheduler state: {error}"))?;
        replace_file(&temporary, path)
            .map_err(|error| format!("cannot publish scheduler state: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both buffers are valid NUL-terminated UTF-16 paths for this call.
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
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
        node_named("B01")
    }

    fn node_named(id: &str) -> ScheduledNode {
        ScheduledNode {
            id: id.to_string(),
            wave: 1,
            depends_on: Vec::new(),
            attempts: 0,
            status: NodeStatus::Ready,
            lease: None,
            stall_alarm_fence: None,
        }
    }

    #[test]
    fn persisted_parallel_limit_enforces_serial_and_bounded_parallel_admission() {
        let serial_root = temp_dir("serial-limit");
        let serial = SchedulerStore::open_with_limit(
            serial_root.join("leases.json"),
            serial_root.clone(),
            vec![node_named("A01"), node_named("A02")],
            1,
        )
        .expect("serial scheduler");
        serial
            .claim("A01", "worker-a", 100, 1_000)
            .expect("first claim");
        let serial_error = serial
            .claim("A02", "worker-b", 100, 1_000)
            .expect_err("serial scheduler must refuse a second active lease");
        assert!(serial_error.contains("parallel worker limit reached (1/1)"));

        let parallel_root = temp_dir("parallel-limit");
        let parallel = SchedulerStore::open_with_limit(
            parallel_root.join("leases.json"),
            parallel_root.clone(),
            vec![node_named("A01"), node_named("A02")],
            4,
        )
        .expect("parallel scheduler");
        parallel
            .claim("A01", "worker-a", 100, 1_000)
            .expect("first claim");
        parallel
            .claim("A02", "worker-b", 100, 1_000)
            .expect("parallel claim");
        assert_eq!(parallel.public_snapshot().unwrap().max_parallel_workers, 4);

        drop(parallel);
        let reopened = SchedulerStore::open(
            parallel_root.join("leases.json"),
            parallel_root.clone(),
            Vec::new(),
        )
        .expect("reopen persisted scheduler");
        assert_eq!(reopened.snapshot().unwrap().max_parallel_workers, 4);

        let _ = fs::remove_dir_all(serial_root);
        let _ = fs::remove_dir_all(parallel_root);
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

    #[test]
    fn authority_rotation_recovers_an_unexpired_previous_epoch_lease() {
        let root = temp_dir("authority-recovery");
        let store = SchedulerStore::open(root.join("leases.json"), root.clone(), vec![node()])
            .expect("store");
        let lease = store
            .claim("B01", "worker-old", 100, 10_000)
            .expect("claim");
        store
            .mutate(|state| {
                let lease = state
                    .nodes
                    .get_mut("B01")
                    .and_then(|node| node.lease.as_mut())
                    .expect("lease");
                lease.authority_epoch = Some(7);
                lease.authorization_id = Some("authorization-old".to_string());
                lease.git_baseline = Some(AdmissionGitBaseline {
                    head_commit: "a".repeat(40),
                    outside_manifest_digest: "b".repeat(64),
                });
                Ok(())
            })
            .expect("persist authority-backed lease");
        let evidence = root.join("evidence/B01/worker-old");
        fs::create_dir_all(&evidence).expect("evidence dir");
        fs::write(evidence.join("partial.log"), b"partial proof").expect("partial proof");

        let actions = store
            .recover_stale_authority(8, 200)
            .expect("recover stale authority");
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            ReapAction::Reassigned {
                node_id,
                worker_id,
                preserved_evidence: Some(path),
            } if node_id == "B01" && worker_id == "worker-old" && path.is_dir()
        ));
        let snapshot = store.snapshot().expect("snapshot");
        assert_eq!(snapshot.nodes["B01"].status, NodeStatus::Ready);
        assert!(snapshot.nodes["B01"].lease.is_none());
        assert_eq!(snapshot.nodes["B01"].stall_alarm_fence, Some(lease.fence));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn upgrade_revokes_legacy_leases_until_their_audit_is_acknowledged() {
        let root = temp_dir("legacy-authority");
        let state_path = root.join("leases.json");
        let store =
            SchedulerStore::open(state_path.clone(), root.clone(), vec![node()]).expect("store");
        let legacy_lease = store
            .claim("B01", "legacy-renderer", 100, 10_000)
            .expect("legacy claim");
        let mut legacy_state = store.snapshot().expect("legacy snapshot");
        legacy_state.authority_schema_version = 0;
        persist(&state_path, &legacy_state).expect("persist legacy shape");
        drop(store);

        let upgraded =
            SchedulerStore::open(state_path, root.clone(), Vec::new()).expect("upgrade scheduler");
        let snapshot = upgraded.snapshot().expect("upgraded snapshot");
        assert_eq!(snapshot.authority_schema_version, AUTHORITY_SCHEMA_VERSION);
        assert_eq!(snapshot.nodes["B01"].status, NodeStatus::Ready);
        assert!(snapshot.nodes["B01"].lease.is_none());
        assert_eq!(
            snapshot.nodes["B01"].stall_alarm_fence,
            Some(legacy_lease.fence)
        );
        assert!(upgraded
            .authorize_commit("B01", &legacy_lease.token, 101)
            .is_err());

        let pending = upgraded
            .pending_legacy_revocations()
            .expect("pending revocations");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].worker_id, "legacy-renderer");
        assert!(upgraded.acknowledge_legacy_revocations(&[]).is_err());
        upgraded
            .acknowledge_legacy_revocations(&pending)
            .expect("acknowledge audit");
        assert!(upgraded
            .pending_legacy_revocations()
            .expect("empty revocations")
            .is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
