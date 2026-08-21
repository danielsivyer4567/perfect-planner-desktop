//! Immutable, path-free collision assessment snapshots.

use super::analyzer::{
    analyze_registry_read, AnalysisUnknownReason, CollisionAnalysis, CollisionBasis,
};
use super::journal::{AssessmentJournal, JournalError};
use super::model::{ActiveClaimState, CollisionVerdict, ConflictDisposition};
use super::registry::RegistryRead;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const MAX_SNAPSHOT_TTL_MS: u64 = 10 * 60 * 1_000;
const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PARTICIPANTS: usize = 4_096;
const MAX_CONFLICTS: usize = 16_384;
const MAX_DEPENDENCY_EDGES: usize = 65_536;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum AssessmentVerdict {
    Clear,
    Wait,
    Replan,
    UserDecision,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum SnapshotUnknownReason {
    RegistryUnknown,
    InvalidRegistry,
    InvalidCensus,
    StaleCensus,
    IncompleteCoverage,
    InvalidManifest,
    InvalidClaimSnapshot,
    InvalidActiveContract,
    MissingDisposition,
    ContradictoryDisposition,
    MixedDispositions,
    LimitExceeded,
}

impl From<AnalysisUnknownReason> for SnapshotUnknownReason {
    fn from(value: AnalysisUnknownReason) -> Self {
        match value {
            AnalysisUnknownReason::RegistryUnknown => Self::RegistryUnknown,
            AnalysisUnknownReason::InvalidRegistry => Self::InvalidRegistry,
            AnalysisUnknownReason::InvalidCensus => Self::InvalidCensus,
            AnalysisUnknownReason::StaleCensus => Self::StaleCensus,
            AnalysisUnknownReason::IncompleteCoverage => Self::IncompleteCoverage,
            AnalysisUnknownReason::InvalidManifest => Self::InvalidManifest,
            AnalysisUnknownReason::InvalidClaimSnapshot => Self::InvalidClaimSnapshot,
            AnalysisUnknownReason::InvalidActiveContract => Self::InvalidActiveContract,
            AnalysisUnknownReason::MissingDisposition => Self::MissingDisposition,
            AnalysisUnknownReason::ContradictoryDisposition => Self::ContradictoryDisposition,
            AnalysisUnknownReason::MixedDispositions => Self::MixedDispositions,
            AnalysisUnknownReason::LimitExceeded => Self::LimitExceeded,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum SnapshotClaimState {
    Planned,
    Claimed,
    Running,
    Waiting,
    Completed,
    Released,
}

impl From<ActiveClaimState> for SnapshotClaimState {
    fn from(value: ActiveClaimState) -> Self {
        match value {
            ActiveClaimState::Planned => Self::Planned,
            ActiveClaimState::Claimed => Self::Claimed,
            ActiveClaimState::Running => Self::Running,
            ActiveClaimState::Waiting => Self::Waiting,
            ActiveClaimState::Completed => Self::Completed,
            ActiveClaimState::Released => Self::Released,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SnapshotParticipant {
    pub(crate) participant_id: String,
    pub(crate) planner_id: String,
    pub(crate) plan_id: String,
    pub(crate) node_id: String,
    pub(crate) repository_identity: String,
    pub(crate) worktree_identity: String,
    pub(crate) branch_digest: String,
    pub(crate) plan_content_digest: String,
    pub(crate) planner_manifest_digest: String,
    pub(crate) claim_snapshot_digest: String,
    pub(crate) file_manifest_digest: String,
    pub(crate) resource_manifest_digest: String,
    pub(crate) run_identity: String,
    pub(crate) worker_identity: String,
    pub(crate) fence: u64,
    pub(crate) lease_generation: u64,
    pub(crate) state: SnapshotClaimState,
    pub(crate) dependencies: Vec<String>,
    pub(crate) assumption_digest: String,
    pub(crate) policy_digest: String,
    pub(crate) active_state_digest: String,
    pub(crate) observed_at_ms: u64,
    pub(crate) expires_at_ms: u64,
}

impl From<CollisionVerdict> for AssessmentVerdict {
    fn from(value: CollisionVerdict) -> Self {
        match value {
            CollisionVerdict::Clear => Self::Clear,
            CollisionVerdict::Wait => Self::Wait,
            CollisionVerdict::Replan => Self::Replan,
            CollisionVerdict::UserDecision => Self::UserDecision,
            CollisionVerdict::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum SnapshotConflictBasis {
    ExactFile,
    PhysicalAlias,
    DirectoryPrefix,
    Resource,
}

impl From<CollisionBasis> for SnapshotConflictBasis {
    fn from(value: CollisionBasis) -> Self {
        match value {
            CollisionBasis::ExactFile => Self::ExactFile,
            CollisionBasis::PhysicalAlias => Self::PhysicalAlias,
            CollisionBasis::DirectoryPrefix => Self::DirectoryPrefix,
            CollisionBasis::Resource => Self::Resource,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SnapshotConflict {
    pub(crate) conflict_id: String,
    pub(crate) left_participant_id: String,
    pub(crate) right_participant_id: String,
    pub(crate) left_claim_id: String,
    pub(crate) right_claim_id: String,
    pub(crate) bases: Vec<SnapshotConflictBasis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) disposition: Option<ConflictDisposition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssessmentSnapshot {
    schema_version: u32,
    registry_generation: u64,
    census_input_digest: String,
    participants: Vec<SnapshotParticipant>,
    conflicts: Vec<SnapshotConflict>,
    verdict: AssessmentVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unknown_reason: Option<SnapshotUnknownReason>,
    captured_at_ms: u64,
    expires_at_ms: u64,
    snapshot_hash: String,
}

/// Unforgeable in production: only this module can wrap a validated, internally analyzed census.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct VerifiedAssessmentSnapshot(AssessmentSnapshot);

impl fmt::Debug for VerifiedAssessmentSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedAssessmentSnapshot")
            .field("snapshot_hash", &self.0.snapshot_hash)
            .field("registry_generation", &self.0.registry_generation)
            .field("verdict", &self.0.verdict)
            .field("participant_count", &self.0.participants.len())
            .field("conflict_count", &self.0.conflicts.len())
            .finish()
    }
}

impl VerifiedAssessmentSnapshot {
    pub(crate) fn snapshot_hash(&self) -> &str {
        &self.0.snapshot_hash
    }

    pub(crate) fn registry_generation(&self) -> u64 {
        self.0.registry_generation
    }

    pub(crate) fn census_input_digest(&self) -> &str {
        &self.0.census_input_digest
    }

    pub(crate) fn verdict(&self) -> AssessmentVerdict {
        self.0.verdict
    }

    pub(crate) fn unknown_reason(&self) -> Option<SnapshotUnknownReason> {
        self.0.unknown_reason
    }

    pub(crate) fn participants(&self) -> &[SnapshotParticipant] {
        &self.0.participants
    }

    pub(crate) fn conflicts(&self) -> &[SnapshotConflict] {
        &self.0.conflicts
    }

    pub(crate) fn captured_at_ms(&self) -> u64 {
        self.0.captured_at_ms
    }

    pub(crate) fn expires_at_ms(&self) -> u64 {
        self.0.expires_at_ms
    }
}

/// Sealed proof that exact snapshot bytes were fsynced, published without replacement, reopened,
/// and byte-verified by the native snapshot store.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StoredSnapshotReceipt {
    snapshot_hash: String,
    encoded_bytes: u64,
    store_binding: String,
}

impl fmt::Debug for StoredSnapshotReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredSnapshotReceipt")
            .field("snapshot_hash", &self.snapshot_hash)
            .field("encoded_bytes", &self.encoded_bytes)
            .field("store_binding", &self.store_binding)
            .finish()
    }
}

impl StoredSnapshotReceipt {
    pub(crate) fn snapshot_hash(&self) -> &str {
        &self.snapshot_hash
    }

    pub(crate) fn encoded_bytes(&self) -> u64 {
        self.encoded_bytes
    }

    pub(crate) fn store_binding(&self) -> &str {
        &self.store_binding
    }

    pub(crate) fn matches(&self, snapshot: &VerifiedAssessmentSnapshot) -> bool {
        self.snapshot_hash == snapshot.snapshot_hash()
    }
}

pub(crate) enum SnapshotError {
    RegistryUnknown,
    InvalidAnalysis,
    InvalidTimeline,
    LimitExceeded,
    InvalidHash,
    AlreadyExistsMismatch,
    Journal(JournalError),
    Io(io::Error),
    Json(serde_json::Error),
}

impl fmt::Debug for SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RegistryUnknown => formatter.write_str("registry is not completely validated"),
            Self::InvalidAnalysis => {
                formatter.write_str("assessment analysis is not self-consistent")
            }
            Self::InvalidTimeline => formatter.write_str("assessment snapshot timeline is invalid"),
            Self::LimitExceeded => formatter.write_str("assessment snapshot exceeds a hard bound"),
            Self::InvalidHash => formatter.write_str("assessment snapshot hash is invalid"),
            Self::AlreadyExistsMismatch => {
                formatter.write_str("immutable snapshot path contains different bytes")
            }
            Self::Journal(error) => write!(formatter, "snapshot journal check failed: {error}"),
            Self::Io(_) => formatter.write_str("snapshot I/O failed"),
            Self::Json(_) => formatter.write_str("snapshot serialization failed"),
        }
    }
}

impl From<io::Error> for SnapshotError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for SnapshotError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<JournalError> for SnapshotError {
    fn from(value: JournalError) -> Self {
        Self::Journal(value)
    }
}

fn prepare_assessment_snapshot(
    read: &RegistryRead,
    captured_at_ms: u64,
    ttl_ms: u64,
) -> Result<VerifiedAssessmentSnapshot, SnapshotError> {
    let analysis = analyze_registry_read(read, captured_at_ms);
    prepare_assessment_snapshot_inner(read, &analysis, captured_at_ms, ttl_ms)
}

fn prepare_assessment_snapshot_inner(
    read: &RegistryRead,
    analysis: &CollisionAnalysis,
    captured_at_ms: u64,
    ttl_ms: u64,
) -> Result<VerifiedAssessmentSnapshot, SnapshotError> {
    let RegistryRead::Complete(document) = read else {
        return Err(SnapshotError::RegistryUnknown);
    };
    if captured_at_ms == 0 || ttl_ms == 0 || ttl_ms > MAX_SNAPSHOT_TTL_MS {
        return Err(SnapshotError::InvalidTimeline);
    }
    let requested_expires_at_ms = captured_at_ms
        .checked_add(ttl_ms)
        .ok_or(SnapshotError::InvalidTimeline)?;
    let census = document
        .census
        .as_ref()
        .ok_or(SnapshotError::InvalidAnalysis)?;
    if analysis.registry_generation != Some(document.generation)
        || analysis.census_input_digest.as_deref() != Some(census.input_digest.as_str())
        || analysis.schema_version == 0
    {
        return Err(SnapshotError::InvalidAnalysis);
    }

    let mut participants = Vec::new();
    let mut dependency_edges = 0usize;
    for planner in &census.planners {
        let contracts = planner
            .claim_snapshot
            .contracts
            .iter()
            .map(|contract| (contract.participant_id.as_str(), contract))
            .collect::<std::collections::BTreeMap<_, _>>();
        if contracts.len() != planner.claim_snapshot.contracts.len()
            || contracts.len() != planner.nodes.len()
        {
            return Err(SnapshotError::InvalidAnalysis);
        }
        for node in &planner.nodes {
            let participant_id = participant_binding(
                &planner.repository_identity,
                &planner.planner_id,
                &planner.plan_id,
                &node.node_id,
            );
            let contract = contracts
                .get(participant_id.as_str())
                .copied()
                .ok_or(SnapshotError::InvalidAnalysis)?;
            dependency_edges = dependency_edges
                .checked_add(contract.dependencies.len())
                .ok_or(SnapshotError::LimitExceeded)?;
            if dependency_edges > MAX_DEPENDENCY_EDGES {
                return Err(SnapshotError::LimitExceeded);
            }
            participants.push(SnapshotParticipant {
                participant_id,
                planner_id: planner.planner_id.clone(),
                plan_id: planner.plan_id.clone(),
                node_id: node.node_id.clone(),
                repository_identity: planner.repository_identity.clone(),
                worktree_identity: planner.worktree_identity.clone(),
                branch_digest: opaque_parts(
                    b"perfect-planner:snapshot-branch:v1",
                    &[planner.branch.as_str()],
                ),
                plan_content_digest: planner.plan_content_digest.clone(),
                planner_manifest_digest: planner.manifest_digest.clone(),
                claim_snapshot_digest: planner.claim_snapshot.digest.clone(),
                file_manifest_digest: manifest_list_digest(
                    b"perfect-planner:snapshot-file-manifest:v1",
                    &node.files,
                ),
                resource_manifest_digest: manifest_list_digest(
                    b"perfect-planner:snapshot-resource-manifest:v1",
                    &node.resources,
                ),
                run_identity: contract.run_identity.clone(),
                worker_identity: contract.worker_identity.clone(),
                fence: contract.fence,
                lease_generation: contract.lease_generation,
                state: contract.state.into(),
                dependencies: contract.dependencies.clone(),
                assumption_digest: contract.assumption_digest.clone(),
                policy_digest: contract.policy_digest.clone(),
                active_state_digest: contract.active_state_digest.clone(),
                observed_at_ms: contract.observed_at_ms,
                expires_at_ms: contract.expires_at_ms,
            });
        }
    }
    participants.sort_by(|left, right| left.participant_id.cmp(&right.participant_id));
    if participants.is_empty()
        || participants.len() > MAX_PARTICIPANTS
        || participants
            .windows(2)
            .any(|pair| pair[0].participant_id >= pair[1].participant_id)
        || analysis.conflicts.len() > MAX_CONFLICTS
    {
        return Err(SnapshotError::LimitExceeded);
    }
    let expires_at_ms = participants
        .iter()
        .map(|participant| participant.expires_at_ms)
        .chain(std::iter::once(census.expires_at_ms))
        .chain(std::iter::once(requested_expires_at_ms))
        .min()
        .ok_or(SnapshotError::InvalidTimeline)?;
    if expires_at_ms <= captured_at_ms {
        return Err(SnapshotError::InvalidTimeline);
    }

    let conflicts = analysis
        .conflicts
        .iter()
        .map(|conflict| SnapshotConflict {
            conflict_id: conflict.conflict_id.clone(),
            left_participant_id: conflict.left_participant_id.clone(),
            right_participant_id: conflict.right_participant_id.clone(),
            left_claim_id: conflict.left_claim_id.clone(),
            right_claim_id: conflict.right_claim_id.clone(),
            bases: conflict.bases.iter().copied().map(Into::into).collect(),
            disposition: conflict.disposition,
        })
        .collect::<Vec<_>>();
    let mut snapshot = AssessmentSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        registry_generation: document.generation,
        census_input_digest: census.input_digest.clone(),
        participants,
        conflicts,
        verdict: analysis.verdict.into(),
        unknown_reason: analysis.unknown_reason.map(Into::into),
        captured_at_ms,
        expires_at_ms,
        snapshot_hash: String::new(),
    };
    snapshot.snapshot_hash = assessment_snapshot_hash(&snapshot)?;
    validate_assessment_snapshot(&snapshot)?;
    Ok(VerifiedAssessmentSnapshot(snapshot))
}

fn validate_assessment_snapshot(snapshot: &AssessmentSnapshot) -> Result<(), SnapshotError> {
    if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION
        || snapshot.registry_generation == 0
        || snapshot.captured_at_ms == 0
        || snapshot.expires_at_ms <= snapshot.captured_at_ms
        || snapshot
            .expires_at_ms
            .saturating_sub(snapshot.captured_at_ms)
            > MAX_SNAPSHOT_TTL_MS
        || !is_sha256(&snapshot.census_input_digest)
        || snapshot.participants.is_empty()
        || snapshot.participants.len() > MAX_PARTICIPANTS
        || snapshot
            .participants
            .windows(2)
            .any(|pair| pair[0].participant_id >= pair[1].participant_id)
        || snapshot.conflicts.len() > MAX_CONFLICTS
        || snapshot
            .conflicts
            .windows(2)
            .any(|pair| pair[0].conflict_id >= pair[1].conflict_id)
    {
        return Err(SnapshotError::InvalidAnalysis);
    }
    let participant_ids = snapshot
        .participants
        .iter()
        .map(|participant| participant.participant_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut dependency_edges = 0usize;
    for participant in &snapshot.participants {
        dependency_edges = dependency_edges
            .checked_add(participant.dependencies.len())
            .ok_or(SnapshotError::LimitExceeded)?;
        if dependency_edges > MAX_DEPENDENCY_EDGES
            || !is_bounded_id(&participant.planner_id)
            || !is_bounded_id(&participant.plan_id)
            || !is_bounded_id(&participant.node_id)
            || !is_sha256(&participant.participant_id)
            || participant.participant_id
                != participant_binding(
                    &participant.repository_identity,
                    &participant.planner_id,
                    &participant.plan_id,
                    &participant.node_id,
                )
            || !is_sha256(&participant.repository_identity)
            || !is_sha256(&participant.worktree_identity)
            || !is_sha256(&participant.branch_digest)
            || !is_sha256(&participant.plan_content_digest)
            || !is_sha256(&participant.planner_manifest_digest)
            || !is_sha256(&participant.claim_snapshot_digest)
            || !is_sha256(&participant.file_manifest_digest)
            || !is_sha256(&participant.resource_manifest_digest)
            || !is_sha256(&participant.run_identity)
            || !is_sha256(&participant.worker_identity)
            || participant.fence == 0
            || participant.lease_generation == 0
            || participant.observed_at_ms > snapshot.captured_at_ms
            || participant.expires_at_ms < snapshot.expires_at_ms
            || participant.expires_at_ms <= participant.observed_at_ms
            || !is_sha256(&participant.assumption_digest)
            || !is_sha256(&participant.policy_digest)
            || !is_sha256(&participant.active_state_digest)
            || participant
                .dependencies
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || participant.dependencies.iter().any(|dependency| {
                !is_sha256(dependency)
                    || dependency == &participant.participant_id
                    || !participant_ids.contains(dependency.as_str())
            })
        {
            return Err(SnapshotError::InvalidAnalysis);
        }
    }
    if dependency_graph_has_cycle(&snapshot.participants) {
        return Err(SnapshotError::InvalidAnalysis);
    }
    for conflict in &snapshot.conflicts {
        if !is_sha256(&conflict.conflict_id)
            || !is_sha256(&conflict.left_participant_id)
            || !is_sha256(&conflict.right_participant_id)
            || !is_sha256(&conflict.left_claim_id)
            || !is_sha256(&conflict.right_claim_id)
            || conflict.left_participant_id == conflict.right_participant_id
            || conflict.left_participant_id >= conflict.right_participant_id
            || !participant_ids.contains(conflict.left_participant_id.as_str())
            || !participant_ids.contains(conflict.right_participant_id.as_str())
            || conflict.bases.is_empty()
            || conflict.bases.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(SnapshotError::InvalidAnalysis);
        }
    }
    let verdict_shape_is_valid = match snapshot.verdict {
        AssessmentVerdict::Clear => {
            snapshot.conflicts.is_empty() && snapshot.unknown_reason.is_none()
        }
        AssessmentVerdict::Wait => {
            !snapshot.conflicts.is_empty()
                && snapshot.unknown_reason.is_none()
                && snapshot
                    .conflicts
                    .iter()
                    .all(|conflict| conflict.disposition == Some(ConflictDisposition::Wait))
        }
        AssessmentVerdict::Replan => {
            !snapshot.conflicts.is_empty()
                && snapshot.unknown_reason.is_none()
                && snapshot
                    .conflicts
                    .iter()
                    .all(|conflict| conflict.disposition == Some(ConflictDisposition::Replan))
        }
        AssessmentVerdict::UserDecision => {
            !snapshot.conflicts.is_empty()
                && snapshot.unknown_reason.is_none()
                && snapshot
                    .conflicts
                    .iter()
                    .all(|conflict| conflict.disposition == Some(ConflictDisposition::UserDecision))
        }
        AssessmentVerdict::Unknown => snapshot.unknown_reason.is_some(),
    };
    if !verdict_shape_is_valid {
        return Err(SnapshotError::InvalidAnalysis);
    }
    if assessment_snapshot_hash(snapshot)? != snapshot.snapshot_hash {
        return Err(SnapshotError::InvalidHash);
    }
    Ok(())
}

fn assessment_snapshot_hash(snapshot: &AssessmentSnapshot) -> Result<String, SnapshotError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct HashPayload<'a> {
        schema_version: u32,
        registry_generation: u64,
        census_input_digest: &'a str,
        participants: &'a [SnapshotParticipant],
        conflicts: &'a [SnapshotConflict],
        verdict: AssessmentVerdict,
        unknown_reason: Option<SnapshotUnknownReason>,
        captured_at_ms: u64,
        expires_at_ms: u64,
    }
    let payload = serde_json::to_vec(&HashPayload {
        schema_version: snapshot.schema_version,
        registry_generation: snapshot.registry_generation,
        census_input_digest: &snapshot.census_input_digest,
        participants: &snapshot.participants,
        conflicts: &snapshot.conflicts,
        verdict: snapshot.verdict,
        unknown_reason: snapshot.unknown_reason,
        captured_at_ms: snapshot.captured_at_ms,
        expires_at_ms: snapshot.expires_at_ms,
    })?;
    let mut digest = Sha256::new();
    digest.update(b"perfect-planner:assessment-snapshot:v1");
    digest.update((payload.len() as u64).to_le_bytes());
    digest.update(payload);
    Ok(format!("{:x}", digest.finalize()))
}

#[derive(Clone)]
pub(crate) struct SnapshotStore {
    directory: PathBuf,
    store_binding: String,
}

impl fmt::Debug for SnapshotStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotStore")
            .field("directory", &"<native-app-data>")
            .field("store_binding", &self.store_binding)
            .finish()
    }
}

impl SnapshotStore {
    /// B07 has no production constructor. B20 must construct the canonical store from a
    /// native-owned, physically verified app-data directory and anchor its authority.
    #[cfg(test)]
    pub(crate) fn new_for_test(directory: impl Into<PathBuf>) -> Self {
        let directory = directory.into();
        let store_binding = opaque_parts(
            b"perfect-planner:snapshot-store-binding:v1",
            &[&directory.to_string_lossy()],
        );
        Self {
            directory,
            store_binding,
        }
    }

    pub(crate) fn persist(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
    ) -> Result<StoredSnapshotReceipt, SnapshotError> {
        let snapshot = &snapshot.0;
        validate_assessment_snapshot(snapshot)?;
        fs::create_dir_all(&self.directory)?;
        let final_path = self.path_for_hash(&snapshot.snapshot_hash)?;
        let mut encoded = serde_json::to_vec(snapshot)?;
        encoded.push(b'\n');
        if encoded.len() as u64 > MAX_SNAPSHOT_BYTES {
            return Err(SnapshotError::LimitExceeded);
        }
        if final_path.exists() {
            return self
                .verify_existing(&final_path, &encoded)
                .map(|_| self.receipt(snapshot, encoded.len() as u64));
        }

        let temp_path = self.directory.join(format!(
            ".{}.{}.{}.tmp",
            snapshot.snapshot_hash,
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let write_result = (|| -> Result<(), SnapshotError> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)?;
            file.write_all(&encoded)?;
            file.sync_all()?;
            drop(file);
            match fs::hard_link(&temp_path, &final_path) {
                Ok(()) => {
                    fs::remove_file(&temp_path)?;
                    self.verify_existing(&final_path, &encoded)?;
                    Ok(())
                }
                Err(_error) if final_path.exists() => {
                    self.verify_existing(&final_path, &encoded)?;
                    fs::remove_file(&temp_path)?;
                    Ok(())
                }
                Err(error) => Err(error.into()),
            }
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        write_result?;
        self.verify_existing(&final_path, &encoded)?;
        Ok(self.receipt(snapshot, encoded.len() as u64))
    }

    fn read_unrecorded(
        &self,
        snapshot_hash: &str,
    ) -> Result<(VerifiedAssessmentSnapshot, StoredSnapshotReceipt), SnapshotError> {
        let path = self.path_for_hash(snapshot_hash)?;
        let file = open_snapshot_read(&path)?;
        let size = file.metadata()?.len();
        if size == 0 || size > MAX_SNAPSHOT_BYTES {
            return Err(SnapshotError::LimitExceeded);
        }
        let mut bytes = Vec::with_capacity(size as usize);
        file.take(MAX_SNAPSHOT_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
            return Err(SnapshotError::LimitExceeded);
        }
        let snapshot: AssessmentSnapshot = serde_json::from_slice(&bytes)?;
        validate_assessment_snapshot(&snapshot)?;
        if snapshot.snapshot_hash != snapshot_hash {
            return Err(SnapshotError::InvalidHash);
        }
        let receipt = self.receipt(&snapshot, size);
        Ok((VerifiedAssessmentSnapshot(snapshot), receipt))
    }

    /// Reopens a snapshot only when the exact store receipt was already committed to the
    /// verified journal and has not subsequently been revoked.
    pub(crate) fn read_recorded(
        &self,
        snapshot_hash: &str,
        journal: &AssessmentJournal,
    ) -> Result<(VerifiedAssessmentSnapshot, StoredSnapshotReceipt), SnapshotError> {
        let (snapshot, receipt) = self.read_unrecorded(snapshot_hash)?;
        if !journal.assessment_is_live(&snapshot, &receipt)? {
            return Err(SnapshotError::AlreadyExistsMismatch);
        }
        Ok((snapshot, receipt))
    }

    pub(crate) fn verify_receipt(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        receipt: &StoredSnapshotReceipt,
    ) -> Result<(), SnapshotError> {
        if receipt.store_binding != self.store_binding
            || receipt.snapshot_hash != snapshot.snapshot_hash()
        {
            return Err(SnapshotError::AlreadyExistsMismatch);
        }
        let path = self.path_for_hash(snapshot.snapshot_hash())?;
        let mut expected = serde_json::to_vec(&snapshot.0)?;
        expected.push(b'\n');
        if receipt.encoded_bytes != expected.len() as u64 {
            return Err(SnapshotError::AlreadyExistsMismatch);
        }
        self.verify_existing(&path, &expected)
    }

    fn path_for_hash(&self, snapshot_hash: &str) -> Result<PathBuf, SnapshotError> {
        if !is_sha256(snapshot_hash) {
            return Err(SnapshotError::InvalidHash);
        }
        Ok(self.directory.join(format!("{snapshot_hash}.json")))
    }

    fn verify_existing(&self, path: &Path, expected: &[u8]) -> Result<(), SnapshotError> {
        let mut file = open_snapshot_read(path)?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() != expected.len() as u64
            || metadata.len() > MAX_SNAPSHOT_BYTES
        {
            return Err(SnapshotError::AlreadyExistsMismatch);
        }
        let mut actual = Vec::with_capacity(metadata.len() as usize);
        (&mut file)
            .take(MAX_SNAPSHOT_BYTES + 1)
            .read_to_end(&mut actual)?;
        if actual != expected {
            return Err(SnapshotError::AlreadyExistsMismatch);
        }
        Ok(())
    }

    fn receipt(&self, snapshot: &AssessmentSnapshot, encoded_bytes: u64) -> StoredSnapshotReceipt {
        StoredSnapshotReceipt {
            snapshot_hash: snapshot.snapshot_hash.clone(),
            encoded_bytes,
            store_binding: self.store_binding.clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn delete_for_test(&self, snapshot_hash: &str) {
        let path = self.path_for_hash(snapshot_hash).unwrap();
        fs::remove_file(path).unwrap();
    }
}

fn dependency_graph_has_cycle(participants: &[SnapshotParticipant]) -> bool {
    let mut indegree = participants
        .iter()
        .map(|participant| {
            (
                participant.participant_id.as_str(),
                participant.dependencies.len(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut dependents = BTreeMap::<&str, Vec<&str>>::new();
    for participant in participants {
        for dependency in &participant.dependencies {
            dependents
                .entry(dependency.as_str())
                .or_default()
                .push(participant.participant_id.as_str());
        }
    }
    let mut ready = indegree
        .iter()
        .filter_map(|(participant, degree)| (*degree == 0).then_some(*participant))
        .collect::<BTreeSet<_>>();
    let mut visited = 0usize;
    while let Some(participant) = ready.pop_first() {
        visited += 1;
        if let Some(items) = dependents.get(participant) {
            for dependent in items {
                let Some(degree) = indegree.get_mut(dependent) else {
                    return true;
                };
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(dependent);
                }
            }
        }
    }
    visited != participants.len()
}

#[cfg(windows)]
fn open_snapshot_read(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(windows))]
fn open_snapshot_read(path: &Path) -> io::Result<File> {
    File::open(path)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn is_bounded_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b':'))
}

fn participant_binding(
    repository_identity: &str,
    planner_id: &str,
    plan_id: &str,
    node_id: &str,
) -> String {
    opaque_parts(
        b"perfect-planner:claim-participant:v1",
        &[repository_identity, planner_id, plan_id, node_id],
    )
}

fn manifest_list_digest(domain: &[u8], values: &[String]) -> String {
    let parts = values.iter().map(String::as_str).collect::<Vec<_>>();
    opaque_parts(domain, &parts)
}

fn opaque_parts(domain: &[u8], parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    digest.update((domain.len() as u64).to_le_bytes());
    digest.update(domain);
    for part in parts {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::collision_assessor::model::{
        ActiveClaimContract, CanonicalClaimSnapshot, ClaimSnapshotStatus,
    };
    use crate::collision_assessor::registry::{
        DiscoveryCensus, PlannerCensusMetadata, PlannerNodeManifest, RegistryDocument,
        REGISTRY_SCHEMA_VERSION,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Debug, Clone, Copy)]
    pub(crate) enum FixtureSnapshotDrift {
        RegistryGeneration,
        CapturedAt,
        RepositoryIdentity,
        PlanContentDigest,
        PlannerManifestDigest,
        ClaimSnapshotDigest,
        FileManifestDigest,
        ResourceManifestDigest,
        RunIdentity,
        WorkerIdentity,
        Fence,
        LeaseGeneration,
        AssumptionDigest,
        PolicyDigest,
        ActiveStateDigest,
    }

    fn temp_store(name: &str) -> SnapshotStore {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        SnapshotStore::new_for_test(std::env::temp_dir().join(format!(
            "perfect-planner-snapshot-{name}-{}-{nonce}",
            std::process::id()
        )))
    }

    pub(crate) fn fixture_snapshot(verdict: AssessmentVerdict) -> VerifiedAssessmentSnapshot {
        let mut snapshot = AssessmentSnapshot {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            registry_generation: 7,
            census_input_digest: "a".repeat(64),
            participants: vec![fixture_participant("b", "node-a")],
            conflicts: Vec::new(),
            verdict,
            unknown_reason: (verdict == AssessmentVerdict::Unknown)
                .then_some(SnapshotUnknownReason::RegistryUnknown),
            captured_at_ms: 1_000,
            expires_at_ms: 5_000,
            snapshot_hash: String::new(),
        };
        snapshot.snapshot_hash = assessment_snapshot_hash(&snapshot).unwrap();
        VerifiedAssessmentSnapshot(snapshot)
    }

    pub(crate) fn fixture_snapshot_with_drift(
        drift: FixtureSnapshotDrift,
    ) -> VerifiedAssessmentSnapshot {
        let mut snapshot = fixture_snapshot(AssessmentVerdict::Clear);
        let participant = &mut snapshot.0.participants[0];
        match drift {
            FixtureSnapshotDrift::RegistryGeneration => snapshot.0.registry_generation += 1,
            FixtureSnapshotDrift::CapturedAt => snapshot.0.captured_at_ms += 1,
            FixtureSnapshotDrift::RepositoryIdentity => {
                participant.repository_identity = "e".repeat(64);
                participant.participant_id = participant_binding(
                    &participant.repository_identity,
                    &participant.planner_id,
                    &participant.plan_id,
                    &participant.node_id,
                );
            }
            FixtureSnapshotDrift::PlanContentDigest => {
                participant.plan_content_digest = "e".repeat(64)
            }
            FixtureSnapshotDrift::PlannerManifestDigest => {
                participant.planner_manifest_digest = "e".repeat(64)
            }
            FixtureSnapshotDrift::ClaimSnapshotDigest => {
                participant.claim_snapshot_digest = "e".repeat(64)
            }
            FixtureSnapshotDrift::FileManifestDigest => {
                participant.file_manifest_digest = "e".repeat(64)
            }
            FixtureSnapshotDrift::ResourceManifestDigest => {
                participant.resource_manifest_digest = "e".repeat(64)
            }
            FixtureSnapshotDrift::RunIdentity => participant.run_identity = "e".repeat(64),
            FixtureSnapshotDrift::WorkerIdentity => participant.worker_identity = "e".repeat(64),
            FixtureSnapshotDrift::Fence => participant.fence += 1,
            FixtureSnapshotDrift::LeaseGeneration => participant.lease_generation += 1,
            FixtureSnapshotDrift::AssumptionDigest => {
                participant.assumption_digest = "e".repeat(64)
            }
            FixtureSnapshotDrift::PolicyDigest => participant.policy_digest = "e".repeat(64),
            FixtureSnapshotDrift::ActiveStateDigest => {
                participant.active_state_digest = "e".repeat(64)
            }
        }
        rehash_snapshot(&mut snapshot);
        validate_assessment_snapshot(&snapshot.0).unwrap();
        snapshot
    }

    pub(crate) fn rehash_snapshot(snapshot: &mut VerifiedAssessmentSnapshot) {
        snapshot.0.snapshot_hash = assessment_snapshot_hash(&snapshot.0).unwrap();
    }

    fn fixture_participant(hex: &str, node_id: &str) -> SnapshotParticipant {
        let repository_identity = hex.repeat(64);
        let planner_id = "planner-a".to_string();
        let plan_id = "plan-a".to_string();
        let participant_id =
            participant_binding(&repository_identity, &planner_id, &plan_id, node_id);
        SnapshotParticipant {
            participant_id,
            planner_id,
            plan_id,
            node_id: node_id.into(),
            repository_identity,
            worktree_identity: "0".repeat(64),
            branch_digest: "2".repeat(64),
            plan_content_digest: "3".repeat(64),
            planner_manifest_digest: "4".repeat(64),
            claim_snapshot_digest: "5".repeat(64),
            file_manifest_digest: "6".repeat(64),
            resource_manifest_digest: "7".repeat(64),
            run_identity: "8".repeat(64),
            worker_identity: "9".repeat(64),
            fence: 1,
            lease_generation: 1,
            state: SnapshotClaimState::Planned,
            dependencies: Vec::new(),
            assumption_digest: "a".repeat(64),
            policy_digest: "b".repeat(64),
            active_state_digest: "c".repeat(64),
            observed_at_ms: 900,
            expires_at_ms: 6_000,
        }
    }

    fn census_planner(name: &str, state: SnapshotClaimState) -> PlannerCensusMetadata {
        let repository_identity = "1".repeat(64);
        let planner_id = format!("planner-{name}");
        let plan_id = format!("PP-{name}");
        let node_id = format!("B-{name}");
        let participant_id =
            participant_binding(&repository_identity, &planner_id, &plan_id, &node_id);
        let active_state = match state {
            SnapshotClaimState::Planned => ActiveClaimState::Planned,
            SnapshotClaimState::Claimed => ActiveClaimState::Claimed,
            SnapshotClaimState::Running => ActiveClaimState::Running,
            SnapshotClaimState::Waiting => ActiveClaimState::Waiting,
            SnapshotClaimState::Completed => ActiveClaimState::Completed,
            SnapshotClaimState::Released => ActiveClaimState::Released,
        };
        let contract = ActiveClaimContract {
            participant_id,
            run_identity: "2".repeat(64),
            worker_identity: "3".repeat(64),
            fence: 1,
            lease_generation: 2,
            state: active_state,
            dependencies: Vec::new(),
            assumption_digest: "4".repeat(64),
            policy_digest: "5".repeat(64),
            active_state_digest: "6".repeat(64),
            disposition_rules: Vec::new(),
            observed_at_ms: 1_500,
            expires_at_ms: 5_000,
        };
        PlannerCensusMetadata {
            planner_id,
            repository_id: format!("display-{name}"),
            repository_identity: repository_identity.clone(),
            worktree_identity: if name == "active" {
                "7".repeat(64)
            } else {
                "8".repeat(64)
            },
            branch: format!("feature/private-{name}"),
            plan_id,
            plan_content_digest: "9".repeat(64),
            manifest_digest: "a".repeat(64),
            claim_snapshot: CanonicalClaimSnapshot {
                schema_version: 1,
                status: ClaimSnapshotStatus::Complete,
                failure: None,
                repository_identity,
                source_manifest_digest: "b".repeat(64),
                claims: Vec::new(),
                contracts: vec![contract],
                digest: "c".repeat(64),
            },
            files: vec![format!("src/private-{name}.rs")],
            resources: vec![format!("database:private-{name}")],
            nodes: vec![PlannerNodeManifest {
                node_id,
                files: vec![format!("src/private-{name}.rs")],
                resources: vec![format!("database:private-{name}")],
            }],
            lease_generation: 2,
            registered_at_ms: 1_000,
            updated_at_ms: 1_500,
            heartbeat_at_ms: 1_500,
            lease_expires_at_ms: 5_000,
        }
    }

    #[test]
    fn sealed_census_snapshot_preserves_active_and_terminal_participants_without_raw_paths() {
        let planners = vec![
            census_planner("active", SnapshotClaimState::Planned),
            census_planner("terminal", SnapshotClaimState::Completed),
        ];
        let census_digest = "d".repeat(64);
        let read = RegistryRead::complete_for_test(RegistryDocument {
            schema_version: REGISTRY_SCHEMA_VERSION,
            generation: 7,
            updated_at_ms: 1_500,
            authority_issuer_epoch: 1,
            authority_verifying_key: "e".repeat(64),
            authority_key_fingerprint: "f".repeat(64),
            configured_roots: Vec::new(),
            registrations: Vec::new(),
            claim_authorities: Vec::new(),
            authority_set_receipt: None,
            census: Some(DiscoveryCensus {
                registry_generation: 7,
                input_digest: census_digest.clone(),
                captured_at_ms: 1_500,
                expires_at_ms: 4_000,
                roots: Vec::new(),
                planners,
            }),
        });
        let analysis = CollisionAnalysis {
            schema_version: 1,
            verdict: CollisionVerdict::Clear,
            registry_generation: Some(7),
            census_input_digest: Some(census_digest),
            conflicts: Vec::new(),
            unknown_reason: None,
        };
        let snapshot = prepare_assessment_snapshot_inner(&read, &analysis, 1_600, 1_000).unwrap();
        assert_eq!(snapshot.participants().len(), 2);
        assert!(snapshot
            .participants()
            .iter()
            .any(|participant| participant.state == SnapshotClaimState::Completed));
        assert!(snapshot
            .participants()
            .iter()
            .any(|participant| participant.state == SnapshotClaimState::Planned));
        let encoded = serde_json::to_string(&snapshot.0).unwrap();
        for forbidden in [
            "src/private-active.rs",
            "database:private-active",
            "feature/private-active",
            "display-active",
        ] {
            assert!(!encoded.contains(forbidden));
        }
        validate_assessment_snapshot(&snapshot.0).unwrap();
    }

    #[test]
    fn immutable_store_round_trips_and_rejects_different_existing_bytes() {
        let store = temp_store("immutable");
        let snapshot = fixture_snapshot(AssessmentVerdict::Clear);
        let receipt = store.persist(&snapshot).unwrap();
        assert_eq!(store.persist(&snapshot).unwrap(), receipt);
        let (read, read_receipt) = store.read_unrecorded(snapshot.snapshot_hash()).unwrap();
        assert_eq!(read, snapshot);
        assert_eq!(read_receipt, receipt);
        store.verify_receipt(&snapshot, &receipt).unwrap();

        let path = store.path_for_hash(snapshot.snapshot_hash()).unwrap();
        fs::write(&path, b"different\n").unwrap();
        assert!(matches!(
            store.persist(&snapshot),
            Err(SnapshotError::AlreadyExistsMismatch)
        ));
        let _ = fs::remove_dir_all(store.directory);
    }

    #[test]
    fn tamper_expiry_and_participant_order_invalidate_the_hash_or_shape() {
        let snapshot = fixture_snapshot(AssessmentVerdict::Clear);
        let mut expiry = snapshot.clone();
        expiry.0.expires_at_ms += 1;
        assert!(matches!(
            validate_assessment_snapshot(&expiry.0),
            Err(SnapshotError::InvalidHash)
        ));

        let mut order = snapshot;
        let mut participants = vec![
            fixture_participant("c", "node-c"),
            fixture_participant("b", "node-b"),
        ];
        participants.sort_by(|left, right| left.participant_id.cmp(&right.participant_id));
        participants.reverse();
        order.0.participants = participants;
        rehash_snapshot(&mut order);
        assert!(matches!(
            validate_assessment_snapshot(&order.0),
            Err(SnapshotError::InvalidAnalysis)
        ));
    }

    #[test]
    fn clear_snapshot_cannot_hide_a_conflict() {
        let mut snapshot = fixture_snapshot(AssessmentVerdict::Clear);
        snapshot
            .0
            .participants
            .push(fixture_participant("c", "node-c"));
        snapshot
            .0
            .participants
            .sort_by(|left, right| left.participant_id.cmp(&right.participant_id));
        let left_participant_id = snapshot.0.participants[0].participant_id.clone();
        let right_participant_id = snapshot.0.participants[1].participant_id.clone();
        snapshot.0.conflicts.push(SnapshotConflict {
            conflict_id: "d".repeat(64),
            left_participant_id,
            right_participant_id,
            left_claim_id: "e".repeat(64),
            right_claim_id: "f".repeat(64),
            bases: vec![SnapshotConflictBasis::Resource],
            disposition: Some(ConflictDisposition::Wait),
        });
        rehash_snapshot(&mut snapshot);
        assert!(matches!(
            validate_assessment_snapshot(&snapshot.0),
            Err(SnapshotError::InvalidAnalysis)
        ));
    }

    #[test]
    fn self_rehashed_participant_binding_and_dependency_cycle_are_rejected() {
        let mut forged = fixture_snapshot(AssessmentVerdict::Clear);
        forged.0.participants[0].participant_id = "d".repeat(64);
        rehash_snapshot(&mut forged);
        assert!(matches!(
            validate_assessment_snapshot(&forged.0),
            Err(SnapshotError::InvalidAnalysis)
        ));

        let mut cyclic = fixture_snapshot(AssessmentVerdict::Clear);
        cyclic.0.participants = vec![
            fixture_participant("b", "node-b"),
            fixture_participant("c", "node-c"),
        ];
        cyclic
            .0
            .participants
            .sort_by(|left, right| left.participant_id.cmp(&right.participant_id));
        let left = cyclic.0.participants[0].participant_id.clone();
        let right = cyclic.0.participants[1].participant_id.clone();
        cyclic.0.participants[0].dependencies = vec![right];
        cyclic.0.participants[1].dependencies = vec![left];
        rehash_snapshot(&mut cyclic);
        assert!(matches!(
            validate_assessment_snapshot(&cyclic.0),
            Err(SnapshotError::InvalidAnalysis)
        ));
    }

    #[test]
    fn non_clear_verdict_requires_conflicts_with_one_exact_disposition() {
        let mut empty_wait = fixture_snapshot(AssessmentVerdict::Clear);
        empty_wait.0.verdict = AssessmentVerdict::Wait;
        rehash_snapshot(&mut empty_wait);
        assert!(matches!(
            validate_assessment_snapshot(&empty_wait.0),
            Err(SnapshotError::InvalidAnalysis)
        ));

        let mut mismatched = fixture_snapshot(AssessmentVerdict::Clear);
        mismatched
            .0
            .participants
            .push(fixture_participant("c", "node-c"));
        mismatched
            .0
            .participants
            .sort_by(|left, right| left.participant_id.cmp(&right.participant_id));
        mismatched.0.verdict = AssessmentVerdict::Wait;
        mismatched.0.conflicts.push(SnapshotConflict {
            conflict_id: "d".repeat(64),
            left_participant_id: mismatched.0.participants[0].participant_id.clone(),
            right_participant_id: mismatched.0.participants[1].participant_id.clone(),
            left_claim_id: "e".repeat(64),
            right_claim_id: "f".repeat(64),
            bases: vec![SnapshotConflictBasis::Resource],
            disposition: Some(ConflictDisposition::Replan),
        });
        rehash_snapshot(&mut mismatched);
        assert!(matches!(
            validate_assessment_snapshot(&mismatched.0),
            Err(SnapshotError::InvalidAnalysis)
        ));
    }
}
