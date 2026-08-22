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
const MAX_CONFLICTS: usize = 8_192;
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
pub(crate) struct SnapshotConflictOverlap {
    pub(crate) basis: SnapshotConflictBasis,
    /// Opaque canonical claim/alias digest. This identifies the exact overlap without leaking a
    /// foreign repository path or resource spelling.
    pub(crate) canonical_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SnapshotConflict {
    pub(crate) conflict_id: String,
    pub(crate) left_participant_id: String,
    pub(crate) right_participant_id: String,
    pub(crate) left_plan_id: String,
    pub(crate) left_node_id: String,
    pub(crate) right_plan_id: String,
    pub(crate) right_node_id: String,
    pub(crate) left_claim_id: String,
    pub(crate) right_claim_id: String,
    pub(crate) bases: Vec<SnapshotConflictBasis>,
    pub(crate) overlaps: Vec<SnapshotConflictOverlap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) disposition: Option<ConflictDisposition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConflictProofStep {
    pub(crate) sibling_hash: String,
    pub(crate) sibling_on_left: bool,
}

/// Sealed inclusion proof for one exact conflict in the immutable snapshot commitment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedConflictProof {
    conflict: SnapshotConflict,
    leaf_index: u32,
    leaf_count: u32,
    steps: Vec<ConflictProofStep>,
    commitment_root: String,
}

impl VerifiedConflictProof {
    pub(crate) fn conflict(&self) -> &SnapshotConflict {
        &self.conflict
    }

    pub(crate) fn leaf_index(&self) -> u32 {
        self.leaf_index
    }

    pub(crate) fn leaf_count(&self) -> u32 {
        self.leaf_count
    }

    pub(crate) fn steps(&self) -> &[ConflictProofStep] {
        &self.steps
    }

    pub(crate) fn commitment_root(&self) -> &str {
        &self.commitment_root
    }
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

    pub(crate) fn conflict_commitment_root(&self) -> String {
        conflict_commitment_root(&self.0.conflicts)
    }

    pub(crate) fn conflict_proof(&self, conflict_id: &str) -> Option<VerifiedConflictProof> {
        let index = self
            .0
            .conflicts
            .binary_search_by(|conflict| conflict.conflict_id.as_str().cmp(conflict_id))
            .ok()?;
        let leaf_hashes = self
            .0
            .conflicts
            .iter()
            .map(conflict_leaf_hash)
            .collect::<Vec<_>>();
        let levels = merkle_levels(&leaf_hashes);
        let steps = merkle_proof_from_levels(&levels, index);
        let top = merkle_root_from_levels(&levels);
        Some(VerifiedConflictProof {
            conflict: self.0.conflicts[index].clone(),
            leaf_index: u32::try_from(index).ok()?,
            leaf_count: u32::try_from(leaf_hashes.len()).ok()?,
            steps,
            commitment_root: wrap_conflict_root(&top, leaf_hashes.len()),
        })
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
        .map(|conflict| {
            let left = participants
                .binary_search_by(|item| item.participant_id.cmp(&conflict.left_participant_id))
                .ok()
                .and_then(|index| participants.get(index))
                .ok_or(SnapshotError::InvalidAnalysis)?;
            let right = participants
                .binary_search_by(|item| item.participant_id.cmp(&conflict.right_participant_id))
                .ok()
                .and_then(|index| participants.get(index))
                .ok_or(SnapshotError::InvalidAnalysis)?;
            Ok(SnapshotConflict {
                conflict_id: conflict.conflict_id.clone(),
                left_participant_id: conflict.left_participant_id.clone(),
                right_participant_id: conflict.right_participant_id.clone(),
                left_plan_id: left.plan_id.clone(),
                left_node_id: left.node_id.clone(),
                right_plan_id: right.plan_id.clone(),
                right_node_id: right.node_id.clone(),
                left_claim_id: conflict.left_claim_id.clone(),
                right_claim_id: conflict.right_claim_id.clone(),
                bases: conflict.bases.iter().copied().map(Into::into).collect(),
                overlaps: conflict
                    .overlaps
                    .iter()
                    .map(|overlap| SnapshotConflictOverlap {
                        basis: overlap.basis.into(),
                        canonical_key: overlap.canonical_key.clone(),
                    })
                    .collect(),
                disposition: conflict.disposition,
            })
        })
        .collect::<Result<Vec<_>, SnapshotError>>()?;
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
        let left = snapshot
            .participants
            .binary_search_by(|item| item.participant_id.cmp(&conflict.left_participant_id))
            .ok()
            .and_then(|index| snapshot.participants.get(index));
        let right = snapshot
            .participants
            .binary_search_by(|item| item.participant_id.cmp(&conflict.right_participant_id))
            .ok()
            .and_then(|index| snapshot.participants.get(index));
        if !is_sha256(&conflict.conflict_id)
            || !is_sha256(&conflict.left_participant_id)
            || !is_sha256(&conflict.right_participant_id)
            || !is_sha256(&conflict.left_claim_id)
            || !is_sha256(&conflict.right_claim_id)
            || conflict.left_participant_id == conflict.right_participant_id
            || conflict.left_participant_id >= conflict.right_participant_id
            || !participant_ids.contains(conflict.left_participant_id.as_str())
            || !participant_ids.contains(conflict.right_participant_id.as_str())
            || left.is_none_or(|participant| {
                participant.plan_id != conflict.left_plan_id
                    || participant.node_id != conflict.left_node_id
            })
            || right.is_none_or(|participant| {
                participant.plan_id != conflict.right_plan_id
                    || participant.node_id != conflict.right_node_id
            })
            || conflict.bases.is_empty()
            || conflict.bases.windows(2).any(|pair| pair[0] >= pair[1])
            || conflict.overlaps.len() != conflict.bases.len()
            || conflict
                .overlaps
                .iter()
                .zip(&conflict.bases)
                .any(|(overlap, basis)| {
                    overlap.basis != *basis || !is_sha256(&overlap.canonical_key)
                })
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

fn conflict_leaf_hash(conflict: &SnapshotConflict) -> String {
    let payload = serde_json::to_vec(conflict).expect("snapshot conflicts are serializable");
    digest_parts(
        b"perfect-planner:conflict-ticket-leaf:v1",
        &[payload.as_slice()],
    )
}

fn merkle_node_hash(left: &str, right: &str) -> String {
    digest_parts(
        b"perfect-planner:conflict-ticket-node:v1",
        &[left.as_bytes(), right.as_bytes()],
    )
}

fn empty_conflict_root() -> String {
    digest_parts(b"perfect-planner:conflict-ticket-empty:v1", &[])
}

fn merkle_top(leaves: &[String]) -> String {
    let levels = merkle_levels(leaves);
    merkle_root_from_levels(&levels)
}

fn merkle_levels(leaves: &[String]) -> Vec<Vec<String>> {
    if leaves.is_empty() {
        return Vec::new();
    }
    let mut levels = vec![leaves.to_vec()];
    while levels.last().is_some_and(|level| level.len() > 1) {
        let level = levels.last().expect("a non-empty Merkle level exists");
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        for pair in level.chunks(2) {
            let right = pair.get(1).unwrap_or(&pair[0]);
            next.push(merkle_node_hash(&pair[0], right));
        }
        levels.push(next);
    }
    levels
}

fn merkle_root_from_levels(levels: &[Vec<String>]) -> String {
    levels
        .last()
        .and_then(|level| level.first())
        .cloned()
        .unwrap_or_else(empty_conflict_root)
}

fn wrap_conflict_root(top: &str, leaf_count: usize) -> String {
    digest_parts(
        b"perfect-planner:conflict-ticket-root:v1",
        &[&(leaf_count as u64).to_le_bytes(), top.as_bytes()],
    )
}

fn conflict_commitment_root(conflicts: &[SnapshotConflict]) -> String {
    let leaves = conflicts.iter().map(conflict_leaf_hash).collect::<Vec<_>>();
    wrap_conflict_root(&merkle_top(&leaves), leaves.len())
}

fn merkle_proof_from_levels(levels: &[Vec<String>], mut index: usize) -> Vec<ConflictProofStep> {
    let mut proof = Vec::new();
    for level in levels.iter().take(levels.len().saturating_sub(1)) {
        let sibling_index = if index % 2 == 0 {
            (index + 1).min(level.len() - 1)
        } else {
            index - 1
        };
        proof.push(ConflictProofStep {
            sibling_hash: level[sibling_index].clone(),
            sibling_on_left: sibling_index < index,
        });
        index /= 2;
    }
    proof
}

pub(crate) fn verify_conflict_proof(
    conflict: &SnapshotConflict,
    leaf_index: u32,
    leaf_count: u32,
    steps: &[ConflictProofStep],
    commitment_root: &str,
) -> bool {
    let leaf_count = leaf_count as usize;
    let leaf_index = leaf_index as usize;
    if leaf_count == 0
        || leaf_count > MAX_CONFLICTS
        || leaf_index >= leaf_count
        || !is_sha256(commitment_root)
        || steps.len() > 13
    {
        return false;
    }
    let mut width = leaf_count;
    let mut index = leaf_index;
    let mut current = conflict_leaf_hash(conflict);
    for step in steps {
        if width <= 1 || !is_sha256(&step.sibling_hash) {
            return false;
        }
        let expected_left = index % 2 == 1;
        if step.sibling_on_left != expected_left {
            return false;
        }
        current = if step.sibling_on_left {
            merkle_node_hash(&step.sibling_hash, &current)
        } else {
            merkle_node_hash(&current, &step.sibling_hash)
        };
        index /= 2;
        width = width.div_ceil(2);
    }
    width == 1 && wrap_conflict_root(&current, leaf_count) == commitment_root
}

fn digest_parts(domain: &[u8], parts: &[&[u8]]) -> String {
    let mut digest = Sha256::new();
    digest.update((domain.len() as u64).to_le_bytes());
    digest.update(domain);
    for part in parts {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part);
    }
    format!("{:x}", digest.finalize())
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

    /// Reopen immutable snapshot bytes for an already-recorded ticket inbox after revocation or
    /// expiry. The caller receives no live assessment authority; the broker only permits audit
    /// reads and acknowledgement of an existing inbound signal from this recovery path.
    pub(crate) fn read_for_ticket_recovery(
        &self,
        snapshot_hash: &str,
        journal: &AssessmentJournal,
    ) -> Result<(VerifiedAssessmentSnapshot, StoredSnapshotReceipt), SnapshotError> {
        let (snapshot, receipt) = self.read_unrecorded(snapshot_hash)?;
        if !journal.assessment_was_recorded(&snapshot, &receipt)? {
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

    pub(crate) fn fixture_conflict_snapshot(
        disposition: ConflictDisposition,
    ) -> VerifiedAssessmentSnapshot {
        let verdict = match disposition {
            ConflictDisposition::Wait => AssessmentVerdict::Wait,
            ConflictDisposition::Replan => AssessmentVerdict::Replan,
            ConflictDisposition::UserDecision => AssessmentVerdict::UserDecision,
        };
        let mut snapshot = fixture_snapshot(verdict);
        snapshot
            .0
            .participants
            .push(fixture_participant("c", "node-b"));
        snapshot
            .0
            .participants
            .sort_by(|left, right| left.participant_id.cmp(&right.participant_id));
        let left = &snapshot.0.participants[0];
        let right = &snapshot.0.participants[1];
        snapshot.0.conflicts = vec![SnapshotConflict {
            conflict_id: "d".repeat(64),
            left_participant_id: left.participant_id.clone(),
            right_participant_id: right.participant_id.clone(),
            left_plan_id: left.plan_id.clone(),
            left_node_id: left.node_id.clone(),
            right_plan_id: right.plan_id.clone(),
            right_node_id: right.node_id.clone(),
            left_claim_id: "e".repeat(64),
            right_claim_id: "f".repeat(64),
            bases: vec![SnapshotConflictBasis::Resource],
            overlaps: vec![SnapshotConflictOverlap {
                basis: SnapshotConflictBasis::Resource,
                canonical_key: "1".repeat(64),
            }],
            disposition: Some(disposition),
        }];
        rehash_snapshot(&mut snapshot);
        validate_assessment_snapshot(&snapshot.0).unwrap();
        snapshot
    }

    pub(crate) fn fixture_conflict_snapshot_with_count(
        disposition: ConflictDisposition,
        conflict_count: usize,
    ) -> VerifiedAssessmentSnapshot {
        assert!((1..=MAX_CONFLICTS).contains(&conflict_count));
        let mut snapshot = fixture_conflict_snapshot(disposition);
        let template = snapshot.0.conflicts[0].clone();
        snapshot.0.conflicts = (0..conflict_count)
            .map(|index| SnapshotConflict {
                conflict_id: format!("{index:064x}"),
                ..template.clone()
            })
            .collect();
        rehash_snapshot(&mut snapshot);
        validate_assessment_snapshot(&snapshot.0).unwrap();
        snapshot
    }

    pub(crate) fn fixture_conflict_snapshot_with_generation(
        disposition: ConflictDisposition,
        registry_generation: u64,
    ) -> VerifiedAssessmentSnapshot {
        assert!(registry_generation > 0);
        let mut snapshot = fixture_conflict_snapshot(disposition);
        snapshot.0.registry_generation = registry_generation;
        rehash_snapshot(&mut snapshot);
        validate_assessment_snapshot(&snapshot.0).unwrap();
        snapshot
    }

    pub(crate) fn fixture_participant_chain_snapshot(
        participant_count: usize,
    ) -> VerifiedAssessmentSnapshot {
        assert!((2..=MAX_PARTICIPANTS).contains(&participant_count));
        let mut snapshot = fixture_snapshot(AssessmentVerdict::Wait);
        snapshot.0.participants = (0..participant_count)
            .map(|index| {
                let mut participant = fixture_participant("b", &format!("node-{index:03}"));
                participant.repository_identity = format!("{:064x}", index + 1);
                participant.planner_id = format!("planner-{index:03}");
                participant.plan_id = format!("plan-{index:03}");
                participant.node_id = format!("node-{index:03}");
                participant.participant_id = participant_binding(
                    &participant.repository_identity,
                    &participant.planner_id,
                    &participant.plan_id,
                    &participant.node_id,
                );
                participant.run_identity = format!("{:064x}", index + 1_000);
                participant.worker_identity = format!("{:064x}", index + 2_000);
                participant.fence = index as u64 + 1;
                participant.lease_generation = index as u64 + 1;
                participant
            })
            .collect();
        snapshot
            .0
            .participants
            .sort_by(|left, right| left.participant_id.cmp(&right.participant_id));
        snapshot.0.conflicts = snapshot
            .0
            .participants
            .windows(2)
            .enumerate()
            .map(|(index, pair)| SnapshotConflict {
                conflict_id: format!("{index:064x}"),
                left_participant_id: pair[0].participant_id.clone(),
                right_participant_id: pair[1].participant_id.clone(),
                left_plan_id: pair[0].plan_id.clone(),
                left_node_id: pair[0].node_id.clone(),
                right_plan_id: pair[1].plan_id.clone(),
                right_node_id: pair[1].node_id.clone(),
                left_claim_id: format!("{:064x}", index * 2 + 10_000),
                right_claim_id: format!("{:064x}", index * 2 + 10_001),
                bases: vec![SnapshotConflictBasis::Resource],
                overlaps: vec![SnapshotConflictOverlap {
                    basis: SnapshotConflictBasis::Resource,
                    canonical_key: format!("{:064x}", index + 20_000),
                }],
                disposition: Some(ConflictDisposition::Wait),
            })
            .collect();
        rehash_snapshot(&mut snapshot);
        validate_assessment_snapshot(&snapshot.0).unwrap();
        snapshot
    }

    pub(crate) fn fixture_participant_clique_snapshot(
        participant_count: usize,
    ) -> VerifiedAssessmentSnapshot {
        assert!((2..=100).contains(&participant_count));
        let mut snapshot = fixture_participant_chain_snapshot(participant_count);
        let mut conflicts = Vec::with_capacity(participant_count * (participant_count - 1) / 2);
        for left_index in 0..participant_count {
            for right_index in (left_index + 1)..participant_count {
                let index = conflicts.len();
                let left = &snapshot.0.participants[left_index];
                let right = &snapshot.0.participants[right_index];
                conflicts.push(SnapshotConflict {
                    conflict_id: format!("{index:064x}"),
                    left_participant_id: left.participant_id.clone(),
                    right_participant_id: right.participant_id.clone(),
                    left_plan_id: left.plan_id.clone(),
                    left_node_id: left.node_id.clone(),
                    right_plan_id: right.plan_id.clone(),
                    right_node_id: right.node_id.clone(),
                    left_claim_id: format!("{:064x}", index * 2 + 100_000),
                    right_claim_id: format!("{:064x}", index * 2 + 100_001),
                    bases: vec![SnapshotConflictBasis::Resource],
                    overlaps: vec![SnapshotConflictOverlap {
                        basis: SnapshotConflictBasis::Resource,
                        canonical_key: format!("{:064x}", index + 200_000),
                    }],
                    disposition: Some(ConflictDisposition::Wait),
                });
            }
        }
        assert!(conflicts.len() <= MAX_CONFLICTS);
        snapshot.0.conflicts = conflicts;
        rehash_snapshot(&mut snapshot);
        validate_assessment_snapshot(&snapshot.0).unwrap();
        snapshot
    }

    pub(crate) fn fixture_maximum_unique_edge_snapshot() -> VerifiedAssessmentSnapshot {
        let mut snapshot = fixture_participant_chain_snapshot(MAX_PARTICIPANTS);
        let mut conflicts = Vec::with_capacity(MAX_CONFLICTS);
        'outer: for left_index in 0..MAX_PARTICIPANTS {
            for right_index in (left_index + 1)..MAX_PARTICIPANTS {
                let index = conflicts.len();
                let left = &snapshot.0.participants[left_index];
                let right = &snapshot.0.participants[right_index];
                conflicts.push(SnapshotConflict {
                    conflict_id: format!("{index:064x}"),
                    left_participant_id: left.participant_id.clone(),
                    right_participant_id: right.participant_id.clone(),
                    left_plan_id: left.plan_id.clone(),
                    left_node_id: left.node_id.clone(),
                    right_plan_id: right.plan_id.clone(),
                    right_node_id: right.node_id.clone(),
                    left_claim_id: format!("{:064x}", index * 2 + 300_000),
                    right_claim_id: format!("{:064x}", index * 2 + 300_001),
                    bases: vec![SnapshotConflictBasis::Resource],
                    overlaps: vec![SnapshotConflictOverlap {
                        basis: SnapshotConflictBasis::Resource,
                        canonical_key: format!("{:064x}", index + 400_000),
                    }],
                    disposition: Some(ConflictDisposition::Wait),
                });
                if conflicts.len() == MAX_CONFLICTS {
                    break 'outer;
                }
            }
        }
        assert_eq!(conflicts.len(), MAX_CONFLICTS);
        snapshot.0.conflicts = conflicts;
        rehash_snapshot(&mut snapshot);
        validate_assessment_snapshot(&snapshot.0).unwrap();
        snapshot
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
        let left_node_id = snapshot.0.participants[0].node_id.clone();
        let right_node_id = snapshot.0.participants[1].node_id.clone();
        snapshot.0.conflicts.push(SnapshotConflict {
            conflict_id: "d".repeat(64),
            left_participant_id,
            right_participant_id,
            left_plan_id: "plan-a".into(),
            left_node_id,
            right_plan_id: "plan-a".into(),
            right_node_id,
            left_claim_id: "e".repeat(64),
            right_claim_id: "f".repeat(64),
            bases: vec![SnapshotConflictBasis::Resource],
            overlaps: vec![SnapshotConflictOverlap {
                basis: SnapshotConflictBasis::Resource,
                canonical_key: "1".repeat(64),
            }],
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
        let left_node_id = mismatched.0.participants[0].node_id.clone();
        let right_node_id = mismatched.0.participants[1].node_id.clone();
        mismatched.0.conflicts.push(SnapshotConflict {
            conflict_id: "d".repeat(64),
            left_participant_id: mismatched.0.participants[0].participant_id.clone(),
            right_participant_id: mismatched.0.participants[1].participant_id.clone(),
            left_plan_id: "plan-a".into(),
            left_node_id,
            right_plan_id: "plan-a".into(),
            right_node_id,
            left_claim_id: "e".repeat(64),
            right_claim_id: "f".repeat(64),
            bases: vec![SnapshotConflictBasis::Resource],
            overlaps: vec![SnapshotConflictOverlap {
                basis: SnapshotConflictBasis::Resource,
                canonical_key: "1".repeat(64),
            }],
            disposition: Some(ConflictDisposition::Replan),
        });
        rehash_snapshot(&mut mismatched);
        assert!(matches!(
            validate_assessment_snapshot(&mismatched.0),
            Err(SnapshotError::InvalidAnalysis)
        ));
    }

    #[test]
    fn conflict_commitment_proves_every_exact_field_and_rejects_splices() {
        let snapshot = fixture_conflict_snapshot(ConflictDisposition::Wait);
        let proof = snapshot
            .conflict_proof(&snapshot.conflicts()[0].conflict_id)
            .unwrap();
        assert!(verify_conflict_proof(
            proof.conflict(),
            proof.leaf_index(),
            proof.leaf_count(),
            proof.steps(),
            proof.commitment_root(),
        ));

        let mut mutations = Vec::new();
        let original = proof.conflict().clone();
        let mut changed = original.clone();
        changed.left_claim_id = "2".repeat(64);
        mutations.push(changed);
        let mut changed = original.clone();
        changed.left_plan_id = "other-plan".into();
        mutations.push(changed);
        let mut changed = original.clone();
        changed.left_node_id = "other-node".into();
        mutations.push(changed);
        let mut changed = original.clone();
        changed.overlaps[0].canonical_key = "3".repeat(64);
        mutations.push(changed);
        let mut changed = original.clone();
        changed.disposition = Some(ConflictDisposition::Replan);
        mutations.push(changed);
        for changed in mutations {
            assert!(!verify_conflict_proof(
                &changed,
                proof.leaf_index(),
                proof.leaf_count(),
                proof.steps(),
                proof.commitment_root(),
            ));
        }
        assert!(!verify_conflict_proof(
            proof.conflict(),
            proof.leaf_index(),
            proof.leaf_count() + 1,
            proof.steps(),
            proof.commitment_root(),
        ));
        assert!(!verify_conflict_proof(
            proof.conflict(),
            proof.leaf_count(),
            proof.leaf_count(),
            proof.steps(),
            proof.commitment_root(),
        ));
    }

    #[test]
    fn non_power_of_two_commitments_reject_truncated_extra_and_flipped_proofs() {
        for count in [3usize, 5, 16, 383] {
            let snapshot = fixture_conflict_snapshot_with_count(ConflictDisposition::Wait, count);
            for index in [0usize, count / 2, count - 1] {
                let proof = snapshot.conflict_proof(&format!("{index:064x}")).unwrap();
                assert!(verify_conflict_proof(
                    proof.conflict(),
                    proof.leaf_index(),
                    proof.leaf_count(),
                    proof.steps(),
                    proof.commitment_root(),
                ));

                let mut truncated = proof.steps().to_vec();
                truncated.pop();
                assert!(!verify_conflict_proof(
                    proof.conflict(),
                    proof.leaf_index(),
                    proof.leaf_count(),
                    &truncated,
                    proof.commitment_root(),
                ));

                let mut extra = proof.steps().to_vec();
                extra.push(proof.steps()[0].clone());
                assert!(!verify_conflict_proof(
                    proof.conflict(),
                    proof.leaf_index(),
                    proof.leaf_count(),
                    &extra,
                    proof.commitment_root(),
                ));

                let mut flipped = proof.steps().to_vec();
                flipped[0].sibling_on_left = !flipped[0].sibling_on_left;
                assert!(!verify_conflict_proof(
                    proof.conflict(),
                    proof.leaf_index(),
                    proof.leaf_count(),
                    &flipped,
                    proof.commitment_root(),
                ));
            }
            let mut reversed = snapshot.conflicts().to_vec();
            reversed.reverse();
            assert_ne!(
                conflict_commitment_root(snapshot.conflicts()),
                conflict_commitment_root(&reversed)
            );
        }
    }

    #[test]
    fn maximum_conflict_commitment_has_bounded_thirteen_hash_proofs() {
        let mut snapshot = fixture_conflict_snapshot(ConflictDisposition::Wait);
        let template = snapshot.0.conflicts[0].clone();
        snapshot.0.conflicts = (0..MAX_CONFLICTS)
            .map(|index| SnapshotConflict {
                conflict_id: format!("{index:064x}"),
                ..template.clone()
            })
            .collect();
        rehash_snapshot(&mut snapshot);
        validate_assessment_snapshot(&snapshot.0).unwrap();
        for index in [0, MAX_CONFLICTS / 2, MAX_CONFLICTS - 1] {
            let proof = snapshot.conflict_proof(&format!("{index:064x}")).unwrap();
            assert_eq!(proof.steps().len(), 13);
            assert!(verify_conflict_proof(
                proof.conflict(),
                proof.leaf_index(),
                proof.leaf_count(),
                proof.steps(),
                proof.commitment_root(),
            ));
        }
    }
}
