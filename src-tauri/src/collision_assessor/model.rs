//! Fail-closed trust-boundary types for the cross-repository collision assessor.
//!
//! This module intentionally has no I/O or scheduler authority. Later components may serialize
//! these values, but the admission predicate remains a pure function that defaults to denial.

use serde::{Deserialize, Serialize};

/// Native, metadata-only claim inventory frozen by the read-only census. None of these DTOs
/// contain a filesystem path, branch, resource spelling, worker token, or other caller authority.
/// Equality is expressed only through domain-separated opaque SHA-256 keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CanonicalClaimKind {
    ExactFile,
    DirectoryTree,
    Resource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClaimSnapshotStatus {
    Complete,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClaimSnapshotFailure {
    MissingGitAuthority,
    InvalidGitAuthority,
    UnsupportedGlob,
    InvalidClaim,
    MissingActiveContract,
    InvalidActiveContract,
    IdentityChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ActiveClaimState {
    Planned,
    Claimed,
    Running,
    Waiting,
    Completed,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConflictDisposition {
    Wait,
    Replan,
    UserDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CanonicalClaim {
    pub claim_id: String,
    pub participant_id: String,
    pub kind: CanonicalClaimKind,
    pub canonical_key: String,
    #[serde(default)]
    pub ancestor_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physical_alias_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimDispositionRule {
    pub claim_id: String,
    pub disposition: ConflictDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActiveClaimContract {
    pub participant_id: String,
    pub run_identity: String,
    pub worker_identity: String,
    pub fence: u64,
    pub lease_generation: u64,
    pub state: ActiveClaimState,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub assumption_digest: String,
    pub policy_digest: String,
    pub active_state_digest: String,
    #[serde(default)]
    pub disposition_rules: Vec<ClaimDispositionRule>,
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CanonicalClaimSnapshot {
    pub schema_version: u32,
    pub status: ClaimSnapshotStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<ClaimSnapshotFailure>,
    pub repository_identity: String,
    pub source_manifest_digest: String,
    #[serde(default)]
    pub claims: Vec<CanonicalClaim>,
    #[serde(default)]
    pub contracts: Vec<ActiveClaimContract>,
    pub digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionVerdict {
    Clear,
    Wait,
    Replan,
    UserDecision,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    CollisionAssessor,
    HeadOrchestrator,
    Worker,
}

impl Authority {
    pub const fn responsibility(self) -> &'static str {
        match self {
            Self::CollisionAssessor => "observes",
            Self::HeadOrchestrator => "decides",
            Self::Worker => "executes",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimAdmissionInput<'a> {
    pub verdict: CollisionVerdict,
    pub run_id: &'a str,
    pub node_id: &'a str,
    pub repository_id: &'a str,
    pub branch: &'a str,
    pub file_manifest_hash: &'a str,
    pub resource_manifest_hash: &'a str,
    pub snapshot_hash: &'a str,
    pub registry_generation: u64,
    pub worker_fence: u64,
    pub clearance_expires_at_ms: u64,
    pub now_ms: u64,
    pub discovery_revoked: bool,
    pub originating_chat_route: &'a str,
    pub approval_delivery_receipt: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionDenial {
    NonClearVerdict(CollisionVerdict),
    MissingBinding(&'static str),
    MissingRegistryGeneration,
    MissingWorkerFence,
    ExpiredClearance,
    DiscoveryStillOpen,
    ApprovalNotDelivered,
}

pub fn validate_claim_admission(input: &ClaimAdmissionInput<'_>) -> Result<(), AdmissionDenial> {
    if input.verdict != CollisionVerdict::Clear {
        return Err(AdmissionDenial::NonClearVerdict(input.verdict));
    }

    for (name, value) in [
        ("run_id", input.run_id),
        ("node_id", input.node_id),
        ("repository_id", input.repository_id),
        ("branch", input.branch),
        ("file_manifest_hash", input.file_manifest_hash),
        ("resource_manifest_hash", input.resource_manifest_hash),
        ("snapshot_hash", input.snapshot_hash),
    ] {
        if value.trim().is_empty() {
            return Err(AdmissionDenial::MissingBinding(name));
        }
    }

    if input.registry_generation == 0 {
        return Err(AdmissionDenial::MissingRegistryGeneration);
    }
    if input.worker_fence == 0 {
        return Err(AdmissionDenial::MissingWorkerFence);
    }
    if input.clearance_expires_at_ms <= input.now_ms {
        return Err(AdmissionDenial::ExpiredClearance);
    }
    if !input.discovery_revoked {
        return Err(AdmissionDenial::DiscoveryStillOpen);
    }
    if input.originating_chat_route.trim().is_empty()
        || input.approval_delivery_receipt.trim().is_empty()
    {
        return Err(AdmissionDenial::ApprovalNotDelivered);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_input() -> ClaimAdmissionInput<'static> {
        ClaimAdmissionInput {
            verdict: CollisionVerdict::Clear,
            run_id: "run-01",
            node_id: "B01",
            repository_id: "repo-01",
            branch: "feature/collision-assessor",
            file_manifest_hash: "files-sha256",
            resource_manifest_hash: "resources-sha256",
            snapshot_hash: "snapshot-sha256",
            registry_generation: 7,
            worker_fence: 3,
            clearance_expires_at_ms: 2_000,
            now_ms: 1_000,
            discovery_revoked: true,
            originating_chat_route: "codex-exec:repo-01:task-01",
            approval_delivery_receipt: "receipt-01",
        }
    }

    #[test]
    fn authorities_have_one_non_overlapping_responsibility() {
        assert_eq!(Authority::CollisionAssessor.responsibility(), "observes");
        assert_eq!(Authority::HeadOrchestrator.responsibility(), "decides");
        assert_eq!(Authority::Worker.responsibility(), "executes");
    }

    #[test]
    fn only_clear_can_admit_a_claim() {
        for verdict in [
            CollisionVerdict::Wait,
            CollisionVerdict::Replan,
            CollisionVerdict::UserDecision,
            CollisionVerdict::Unknown,
        ] {
            let mut input = clear_input();
            input.verdict = verdict;
            assert_eq!(
                validate_claim_admission(&input),
                Err(AdmissionDenial::NonClearVerdict(verdict))
            );
        }
        assert_eq!(validate_claim_admission(&clear_input()), Ok(()));
    }

    #[test]
    fn every_exact_identity_and_manifest_binding_is_required() {
        let setters: [fn(&mut ClaimAdmissionInput<'static>); 7] = [
            |input| input.run_id = "",
            |input| input.node_id = "",
            |input| input.repository_id = "",
            |input| input.branch = "",
            |input| input.file_manifest_hash = "",
            |input| input.resource_manifest_hash = "",
            |input| input.snapshot_hash = "",
        ];
        for remove_binding in setters {
            let mut input = clear_input();
            remove_binding(&mut input);
            assert!(matches!(
                validate_claim_admission(&input),
                Err(AdmissionDenial::MissingBinding(_))
            ));
        }
    }

    #[test]
    fn stale_or_unrevoked_clearance_is_denied() {
        let mut expired = clear_input();
        expired.now_ms = expired.clearance_expires_at_ms;
        assert_eq!(
            validate_claim_admission(&expired),
            Err(AdmissionDenial::ExpiredClearance)
        );

        let mut discovery_open = clear_input();
        discovery_open.discovery_revoked = false;
        assert_eq!(
            validate_claim_admission(&discovery_open),
            Err(AdmissionDenial::DiscoveryStillOpen)
        );
    }

    #[test]
    fn board_approval_without_chat_delivery_is_denied() {
        let mut no_route = clear_input();
        no_route.originating_chat_route = "";
        assert_eq!(
            validate_claim_admission(&no_route),
            Err(AdmissionDenial::ApprovalNotDelivered)
        );

        let mut no_receipt = clear_input();
        no_receipt.approval_delivery_receipt = "";
        assert_eq!(
            validate_claim_admission(&no_receipt),
            Err(AdmissionDenial::ApprovalNotDelivered)
        );
    }
}
