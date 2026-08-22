//! Native-only, single-use clearance capabilities for an exact immutable assessment.
//!
//! Production issuance is structurally dormant in B07: there is no production constructor for
//! either the MAC/clock owner or the machine-verified receipt bundle. B15/B20 must add those
//! constructors inside this module after their native owners exist; sibling modules cannot fake
//! either authority.

use super::journal::AssessmentJournal;
use super::snapshot::{
    AssessmentVerdict, SnapshotClaimState, SnapshotError, SnapshotParticipant, SnapshotStore,
    StoredSnapshotReceipt, VerifiedAssessmentSnapshot,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const TOKEN_PREFIX: &str = "ppc1";
const TOKEN_BYTES: usize = 32;
const MAX_CLEARANCE_RECORDS: usize = 65_536;
const ENTROPY_ATTEMPTS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrustedTime {
    wall_ms: u64,
    monotonic_ms: u64,
}

trait NativeClock: Send + Sync {
    fn now(&self) -> Result<TrustedTime, ClearanceError>;
}

struct NativeClearanceAuthority {
    issuer_epoch: u64,
    key: [u8; TOKEN_BYTES],
    clock: Arc<dyn NativeClock>,
}

impl NativeClearanceAuthority {
    fn mac(&self, message: &[u8]) -> [u8; TOKEN_BYTES] {
        hmac_sha256(&self.key, message)
    }
}

impl Drop for NativeClearanceAuthority {
    fn drop(&mut self) {
        self.key.fill(0);
    }
}

/// Sealed join of discovery-revocation, originating-chat and delivered-approval receipts.
/// B07 deliberately provides no production constructor for caller-shaped digests.
#[derive(PartialEq, Eq)]
pub(crate) struct ClearanceReceiptBundle {
    participant_id: String,
    discovery_revocation_digest: String,
    originating_chat_digest: String,
    approval_delivery_digest: String,
}

impl fmt::Debug for ClearanceReceiptBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClearanceReceiptBundle")
            .field("participant_id", &self.participant_id)
            .field("verified_receipts", &"<native-sealed>")
            .finish()
    }
}

#[cfg(test)]
impl ClearanceReceiptBundle {
    fn for_test(participant_id: &str, approval_delivery_digest: &str) -> Self {
        Self {
            participant_id: participant_id.to_string(),
            discovery_revocation_digest:
                "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".into(),
            originating_chat_digest:
                "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".into(),
            approval_delivery_digest: approval_delivery_digest.to_string(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClearanceBinding {
    snapshot_hash: String,
    registry_generation: u64,
    census_input_digest: String,
    participant_id: String,
    planner_id: String,
    plan_id: String,
    node_id: String,
    repository_identity: String,
    worktree_identity: String,
    branch_digest: String,
    plan_content_digest: String,
    planner_manifest_digest: String,
    claim_snapshot_digest: String,
    file_manifest_digest: String,
    resource_manifest_digest: String,
    run_identity: String,
    worker_identity: String,
    fence: u64,
    lease_generation: u64,
    assumption_digest: String,
    policy_digest: String,
    active_state_digest: String,
    discovery_revocation_digest: String,
    originating_chat_digest: String,
    approval_delivery_digest: String,
    snapshot_captured_at_ms: u64,
    expires_at_ms: u64,
}

impl fmt::Debug for ClearanceBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClearanceBinding")
            .field("snapshot_hash", &self.snapshot_hash)
            .field("registry_generation", &self.registry_generation)
            .field("participant_id", &self.participant_id)
            .field("node_id", &self.node_id)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("remaining_bindings", &"<redacted>")
            .finish()
    }
}

impl ClearanceBinding {
    fn matches_current(&self, current: &Self) -> bool {
        self.snapshot_hash == current.snapshot_hash
            && self.registry_generation == current.registry_generation
            && self.census_input_digest == current.census_input_digest
            && self.participant_id == current.participant_id
            && self.planner_id == current.planner_id
            && self.plan_id == current.plan_id
            && self.node_id == current.node_id
            && self.repository_identity == current.repository_identity
            && self.worktree_identity == current.worktree_identity
            && self.branch_digest == current.branch_digest
            && self.plan_content_digest == current.plan_content_digest
            && self.planner_manifest_digest == current.planner_manifest_digest
            && self.claim_snapshot_digest == current.claim_snapshot_digest
            && self.file_manifest_digest == current.file_manifest_digest
            && self.resource_manifest_digest == current.resource_manifest_digest
            && self.run_identity == current.run_identity
            && self.worker_identity == current.worker_identity
            && self.fence == current.fence
            && self.lease_generation == current.lease_generation
            && self.assumption_digest == current.assumption_digest
            && self.policy_digest == current.policy_digest
            && self.active_state_digest == current.active_state_digest
            && self.discovery_revocation_digest == current.discovery_revocation_digest
            && self.originating_chat_digest == current.originating_chat_digest
            && self.approval_delivery_digest == current.approval_delivery_digest
            && self.snapshot_captured_at_ms == current.snapshot_captured_at_ms
            && self.expires_at_ms == current.expires_at_ms
    }
}

pub(crate) struct ClearanceToken(String);

impl ClearanceToken {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ClearanceToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ClearanceToken(<redacted>)")
    }
}

impl Drop for ClearanceToken {
    fn drop(&mut self) {
        unsafe { self.0.as_mut_vec().fill(0) };
    }
}

pub(crate) struct IssuedClearance {
    pub(crate) token: ClearanceToken,
    pub(crate) snapshot_hash: String,
    pub(crate) participant_id: String,
    pub(crate) registry_generation: u64,
    pub(crate) issuer_epoch: u64,
    pub(crate) issued_at_ms: u64,
    pub(crate) expires_at_ms: u64,
}

impl fmt::Debug for IssuedClearance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IssuedClearance")
            .field("token", &"<redacted>")
            .field("snapshot_hash", &self.snapshot_hash)
            .field("participant_id", &self.participant_id)
            .field("registry_generation", &self.registry_generation)
            .field("issuer_epoch", &self.issuer_epoch)
            .field("issued_at_ms", &self.issued_at_ms)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

#[allow(dead_code)]
pub(crate) struct ClearancePermit {
    clearance_id_hash: String,
    binding: ClearanceBinding,
    issuer_epoch: u64,
    consuming_wall_ms: u64,
    consuming_monotonic_ms: u64,
    monotonic_deadline_ms: u64,
}

impl fmt::Debug for ClearancePermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClearancePermit")
            .field("clearance_id_hash", &self.clearance_id_hash)
            .field("snapshot_hash", &self.binding.snapshot_hash)
            .field("participant_id", &self.binding.participant_id)
            .field("issuer_epoch", &self.issuer_epoch)
            .field("remaining_bindings", &"<redacted>")
            .finish()
    }
}

#[allow(dead_code)]
impl ClearancePermit {
    pub(crate) fn snapshot_hash(&self) -> &str {
        &self.binding.snapshot_hash
    }
    pub(crate) fn participant_id(&self) -> &str {
        &self.binding.participant_id
    }
    pub(crate) fn node_id(&self) -> &str {
        &self.binding.node_id
    }
    pub(crate) fn fence(&self) -> u64 {
        self.binding.fence
    }
    pub(crate) fn expires_at_ms(&self) -> u64 {
        self.binding.expires_at_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum ClearanceLifecycle {
    Issued,
    Consuming { wall_ms: u64, monotonic_ms: u64 },
    Consumed,
    Revoked,
}

#[derive(Clone)]
struct ClearanceRecord {
    snapshot: VerifiedAssessmentSnapshot,
    snapshot_receipt: StoredSnapshotReceipt,
    binding: ClearanceBinding,
    issuer_epoch: u64,
    issued_wall_ms: u64,
    issued_monotonic_ms: u64,
    monotonic_deadline_ms: u64,
    lifecycle: ClearanceLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClearanceError {
    InvalidSnapshot,
    AssessmentNotRecorded,
    NonClearAssessment,
    ParticipantUnavailable,
    ParticipantNotClaimable,
    InvalidReceiptBinding,
    InvalidTimeline,
    Expired,
    ClockRollback,
    InvalidToken,
    UnknownToken,
    PriorIssuer,
    BindingMismatch,
    AlreadyUsed,
    CapacityExceeded,
    EntropyUnavailable,
    EntropyCollision,
    AuthorityUnavailable,
    JournalUnavailable,
    StoreUnavailable,
}

impl fmt::Display for ClearanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSnapshot => "clearance assessment snapshot is invalid",
            Self::AssessmentNotRecorded => "clearance assessment is not durably recorded",
            Self::NonClearAssessment => "only an unqualified CLEAR assessment can issue clearance",
            Self::ParticipantUnavailable => "clearance participant is absent from the assessment",
            Self::ParticipantNotClaimable => "clearance participant is not in a claimable state",
            Self::InvalidReceiptBinding => "clearance receipt binding is invalid",
            Self::InvalidTimeline => "clearance timeline is invalid",
            Self::Expired => "clearance has expired",
            Self::ClockRollback => "trusted time moved behind clearance issuance",
            Self::InvalidToken => "clearance token has an invalid shape or authenticator",
            Self::UnknownToken => "clearance token is unknown or from a prior process",
            Self::PriorIssuer => "clearance belongs to a prior native issuer epoch",
            Self::BindingMismatch => "clearance no longer matches the sealed assessment",
            Self::AlreadyUsed => "clearance has already entered a terminal state",
            Self::CapacityExceeded => "clearance tombstone capacity is exhausted",
            Self::EntropyUnavailable => "operating-system entropy is unavailable",
            Self::EntropyCollision => "operating-system entropy repeated a clearance identifier",
            Self::AuthorityUnavailable => "native clearance authority is unavailable",
            Self::JournalUnavailable => "clearance audit journal is unavailable",
            Self::StoreUnavailable => "clearance store is unavailable",
        })
    }
}

impl From<SnapshotError> for ClearanceError {
    fn from(_: SnapshotError) -> Self {
        Self::InvalidSnapshot
    }
}

pub(crate) struct ClearanceStore {
    authority: Option<NativeClearanceAuthority>,
    snapshots: SnapshotStore,
    records: Mutex<BTreeMap<String, ClearanceRecord>>,
    journal: AssessmentJournal,
    audit_fail_stop: AtomicBool,
}

impl ClearanceStore {
    pub(crate) fn new_disabled(journal: AssessmentJournal, snapshots: SnapshotStore) -> Self {
        Self {
            authority: None,
            snapshots,
            records: Mutex::new(BTreeMap::new()),
            journal,
            audit_fail_stop: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    fn new_for_test(
        journal: AssessmentJournal,
        snapshots: SnapshotStore,
        issuer_epoch: u64,
        key: [u8; TOKEN_BYTES],
        clock: Arc<dyn NativeClock>,
    ) -> Self {
        Self {
            authority: Some(NativeClearanceAuthority {
                issuer_epoch,
                key,
                clock,
            }),
            snapshots,
            records: Mutex::new(BTreeMap::new()),
            journal,
            audit_fail_stop: AtomicBool::new(false),
        }
    }

    pub(crate) fn issue(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        snapshot_receipt: &StoredSnapshotReceipt,
        receipts: &ClearanceReceiptBundle,
    ) -> Result<IssuedClearance, ClearanceError> {
        self.issue_with_entropy(snapshot, snapshot_receipt, receipts, fill_os_random)
    }

    fn issue_with_entropy<F>(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        snapshot_receipt: &StoredSnapshotReceipt,
        receipts: &ClearanceReceiptBundle,
        mut fill: F,
    ) -> Result<IssuedClearance, ClearanceError>
    where
        F: FnMut(&mut [u8]) -> Result<(), ClearanceError>,
    {
        self.require_audit()?;
        let authority = self.authority()?;
        if !self.journal.clearance_authority_ready() {
            return Err(ClearanceError::JournalUnavailable);
        }
        self.snapshots.verify_receipt(snapshot, snapshot_receipt)?;
        if !self
            .journal
            .assessment_is_live(snapshot, snapshot_receipt)
            .map_err(|_| self.audit_error())?
        {
            return Err(ClearanceError::AssessmentNotRecorded);
        }
        let mut records = self
            .records
            .lock()
            .map_err(|_| ClearanceError::StoreUnavailable)?;
        let now = authority.clock.now()?;
        let binding = binding_from_snapshot(snapshot, receipts, now.wall_ms)?;
        let ttl = binding
            .expires_at_ms
            .checked_sub(now.wall_ms)
            .ok_or(ClearanceError::InvalidTimeline)?;
        let deadline = now
            .monotonic_ms
            .checked_add(ttl)
            .ok_or(ClearanceError::InvalidTimeline)?;
        if records.len() >= MAX_CLEARANCE_RECORDS {
            return Err(ClearanceError::CapacityExceeded);
        }
        for _ in 0..ENTROPY_ATTEMPTS {
            let mut identifier = [0_u8; TOKEN_BYTES];
            fill(&mut identifier)?;
            let identifier_hex = hex_encode(&identifier);
            let id_hash = clearance_identifier_hash(&identifier);
            identifier.fill(0);
            if records.contains_key(&id_hash) {
                continue;
            }
            let message = clearance_mac_message(&identifier_hex, authority.issuer_epoch, &binding)?;
            let mut mac = authority.mac(&message);
            let raw_token = format!("{TOKEN_PREFIX}.{identifier_hex}.{}", hex_encode(&mac));
            mac.fill(0);
            self.journal
                .record_clearance_issued(
                    now.wall_ms,
                    id_hash.clone(),
                    snapshot,
                    snapshot_receipt,
                    binding.participant_id.clone(),
                    authority.issuer_epoch,
                    binding.expires_at_ms,
                )
                .map_err(|_| self.audit_error())?;
            records.insert(
                id_hash,
                ClearanceRecord {
                    snapshot: snapshot.clone(),
                    snapshot_receipt: snapshot_receipt.clone(),
                    binding: binding.clone(),
                    issuer_epoch: authority.issuer_epoch,
                    issued_wall_ms: now.wall_ms,
                    issued_monotonic_ms: now.monotonic_ms,
                    monotonic_deadline_ms: deadline,
                    lifecycle: ClearanceLifecycle::Issued,
                },
            );
            return Ok(IssuedClearance {
                token: ClearanceToken(raw_token),
                snapshot_hash: binding.snapshot_hash,
                participant_id: binding.participant_id,
                registry_generation: binding.registry_generation,
                issuer_epoch: authority.issuer_epoch,
                issued_at_ms: now.wall_ms,
                expires_at_ms: binding.expires_at_ms,
            });
        }
        Err(ClearanceError::EntropyCollision)
    }

    /// Test-only entry for the B07 primitive. B09 must add the production entry by owning the
    /// native registry read, B15 receipt resolution and executor CAS inside one admission fence;
    /// accepting caller-supplied "current" state here would reintroduce a stale-read bypass.
    #[cfg(test)]
    fn begin_verified_for_test(
        &self,
        token: &str,
        current: &VerifiedAssessmentSnapshot,
        receipts: &ClearanceReceiptBundle,
    ) -> Result<ClearancePermit, ClearanceError> {
        let parsed = ParsedToken::parse(token)?;
        let id_hash = clearance_identifier_hash(&parsed.identifier);
        let mut records = self.lock_records()?;
        let authority = self.authority()?;
        let now = authority.clock.now()?;
        let record = records
            .get_mut(&id_hash)
            .ok_or(ClearanceError::UnknownToken)?;
        verify_token(authority, &parsed, record)?;
        self.require_live_or_revoke(&id_hash, record, now)?;
        self.begin_with_verified(&id_hash, record, current, receipts, authority, now)
    }

    fn begin_with_verified(
        &self,
        id_hash: &str,
        record: &mut ClearanceRecord,
        current: &VerifiedAssessmentSnapshot,
        receipts: &ClearanceReceiptBundle,
        authority: &NativeClearanceAuthority,
        now: TrustedTime,
    ) -> Result<ClearancePermit, ClearanceError> {
        if record.lifecycle != ClearanceLifecycle::Issued {
            return Err(ClearanceError::AlreadyUsed);
        }
        if self
            .snapshots
            .verify_receipt(&record.snapshot, &record.snapshot_receipt)
            .is_err()
        {
            self.revoke_record(id_hash, record, now.wall_ms, "snapshot-store-drift")?;
            return Err(ClearanceError::InvalidSnapshot);
        }
        if !self
            .journal
            .assessment_is_live(&record.snapshot, &record.snapshot_receipt)
            .map_err(|_| self.audit_error())?
        {
            self.revoke_record(id_hash, record, now.wall_ms, "assessment-not-live")?;
            return Err(ClearanceError::AssessmentNotRecorded);
        }
        // Snapshot and journal verification can block. Re-sample the native clock after those
        // operations so a clearance that expired while waiting cannot enter Consuming.
        let final_now = authority.clock.now()?;
        self.require_live_or_revoke(id_hash, record, final_now)?;
        let current_binding = match binding_from_snapshot(current, receipts, final_now.wall_ms) {
            Ok(binding) => binding,
            Err(_) => {
                self.revoke_record(id_hash, record, final_now.wall_ms, "binding-drift")?;
                return Err(ClearanceError::BindingMismatch);
            }
        };
        if !record.binding.matches_current(&current_binding) {
            self.revoke_record(id_hash, record, final_now.wall_ms, "binding-drift")?;
            return Err(ClearanceError::BindingMismatch);
        }
        self.journal
            .record_clearance_consuming(
                final_now.wall_ms,
                id_hash.to_string(),
                record.binding.snapshot_hash.clone(),
                record.binding.participant_id.clone(),
                authority.issuer_epoch,
            )
            .map_err(|_| self.audit_error())?;
        record.lifecycle = ClearanceLifecycle::Consuming {
            wall_ms: final_now.wall_ms,
            monotonic_ms: final_now.monotonic_ms,
        };
        Ok(ClearancePermit {
            clearance_id_hash: id_hash.to_string(),
            binding: record.binding.clone(),
            issuer_epoch: record.issuer_epoch,
            consuming_wall_ms: final_now.wall_ms,
            consuming_monotonic_ms: final_now.monotonic_ms,
            monotonic_deadline_ms: record.monotonic_deadline_ms,
        })
    }

    pub(crate) fn revoke(&self, token: &str, reason_digest: &str) -> Result<(), ClearanceError> {
        if !is_lower_sha256(reason_digest) {
            return Err(ClearanceError::InvalidReceiptBinding);
        }
        let parsed = ParsedToken::parse(token)?;
        let id_hash = clearance_identifier_hash(&parsed.identifier);
        let mut records = self.lock_records()?;
        let authority = self.authority()?;
        let now = authority.clock.now()?;
        let record = records
            .get_mut(&id_hash)
            .ok_or(ClearanceError::UnknownToken)?;
        verify_token(authority, &parsed, record)?;
        if matches!(
            record.lifecycle,
            ClearanceLifecycle::Consumed | ClearanceLifecycle::Revoked
        ) {
            return Err(ClearanceError::AlreadyUsed);
        }
        self.journal
            .record_clearance_revoked(
                now.wall_ms,
                id_hash,
                record.binding.snapshot_hash.clone(),
                record.binding.participant_id.clone(),
                record.issuer_epoch,
                reason_digest.to_string(),
            )
            .map_err(|_| self.audit_error())?;
        record.lifecycle = ClearanceLifecycle::Revoked;
        Ok(())
    }

    fn revoke_record(
        &self,
        id_hash: &str,
        record: &mut ClearanceRecord,
        wall_ms: u64,
        reason: &str,
    ) -> Result<(), ClearanceError> {
        let reason_digest = domain_hash(
            b"perfect-planner:clearance-revocation:v1",
            reason.as_bytes(),
        );
        let result = self.journal.record_clearance_revoked(
            wall_ms,
            id_hash.to_string(),
            record.binding.snapshot_hash.clone(),
            record.binding.participant_id.clone(),
            record.issuer_epoch,
            reason_digest,
        );
        match result {
            Ok(_) => {
                record.lifecycle = ClearanceLifecycle::Revoked;
                Ok(())
            }
            Err(_) => Err(self.audit_error()),
        }
    }

    fn require_live_or_revoke(
        &self,
        id_hash: &str,
        record: &mut ClearanceRecord,
        now: TrustedTime,
    ) -> Result<(), ClearanceError> {
        match live_error(record, now) {
            None => Ok(()),
            Some(ClearanceError::Expired) => {
                self.revoke_record(id_hash, record, now.wall_ms, "expired")?;
                Err(ClearanceError::Expired)
            }
            Some(ClearanceError::ClockRollback) => {
                self.revoke_record(id_hash, record, now.wall_ms, "clock-rollback")?;
                Err(ClearanceError::ClockRollback)
            }
            Some(error) => Err(error),
        }
    }

    fn authority(&self) -> Result<&NativeClearanceAuthority, ClearanceError> {
        self.authority
            .as_ref()
            .filter(|item| item.issuer_epoch > 0)
            .ok_or(ClearanceError::AuthorityUnavailable)
    }
    fn lock_records(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, ClearanceRecord>>, ClearanceError> {
        self.require_audit()?;
        self.records
            .lock()
            .map_err(|_| ClearanceError::StoreUnavailable)
    }
    fn require_audit(&self) -> Result<(), ClearanceError> {
        if self.audit_fail_stop.load(Ordering::Acquire) {
            Err(ClearanceError::JournalUnavailable)
        } else {
            Ok(())
        }
    }
    fn audit_error(&self) -> ClearanceError {
        self.audit_fail_stop.store(true, Ordering::Release);
        ClearanceError::JournalUnavailable
    }
}

fn live_error(record: &ClearanceRecord, now: TrustedTime) -> Option<ClearanceError> {
    if matches!(
        record.lifecycle,
        ClearanceLifecycle::Consumed | ClearanceLifecycle::Revoked
    ) {
        return Some(ClearanceError::AlreadyUsed);
    }
    if now.wall_ms < record.issued_wall_ms || now.monotonic_ms < record.issued_monotonic_ms {
        return Some(ClearanceError::ClockRollback);
    }
    if now.wall_ms >= record.binding.expires_at_ms
        || now.monotonic_ms >= record.monotonic_deadline_ms
    {
        return Some(ClearanceError::Expired);
    }
    None
}

fn binding_from_snapshot(
    snapshot: &VerifiedAssessmentSnapshot,
    receipts: &ClearanceReceiptBundle,
    wall_ms: u64,
) -> Result<ClearanceBinding, ClearanceError> {
    if snapshot.verdict() != AssessmentVerdict::Clear
        || !snapshot.conflicts().is_empty()
        || snapshot.unknown_reason().is_some()
    {
        return Err(ClearanceError::NonClearAssessment);
    }
    if wall_ms < snapshot.captured_at_ms() || wall_ms >= snapshot.expires_at_ms() {
        return Err(if wall_ms < snapshot.captured_at_ms() {
            ClearanceError::ClockRollback
        } else {
            ClearanceError::Expired
        });
    }
    for digest in [
        &receipts.participant_id,
        &receipts.discovery_revocation_digest,
        &receipts.originating_chat_digest,
        &receipts.approval_delivery_digest,
    ] {
        if !is_lower_sha256(digest) {
            return Err(ClearanceError::InvalidReceiptBinding);
        }
    }
    let participant = snapshot
        .participants()
        .binary_search_by(|candidate| candidate.participant_id.cmp(&receipts.participant_id))
        .ok()
        .map(|index| &snapshot.participants()[index])
        .ok_or(ClearanceError::ParticipantUnavailable)?;
    if participant.state != SnapshotClaimState::Planned {
        return Err(ClearanceError::ParticipantNotClaimable);
    }
    Ok(binding_from_participant(snapshot, participant, receipts))
}

fn binding_from_participant(
    snapshot: &VerifiedAssessmentSnapshot,
    p: &SnapshotParticipant,
    r: &ClearanceReceiptBundle,
) -> ClearanceBinding {
    ClearanceBinding {
        snapshot_hash: snapshot.snapshot_hash().into(),
        registry_generation: snapshot.registry_generation(),
        census_input_digest: snapshot.census_input_digest().into(),
        participant_id: p.participant_id.clone(),
        planner_id: p.planner_id.clone(),
        plan_id: p.plan_id.clone(),
        node_id: p.node_id.clone(),
        repository_identity: p.repository_identity.clone(),
        worktree_identity: p.worktree_identity.clone(),
        branch_digest: p.branch_digest.clone(),
        plan_content_digest: p.plan_content_digest.clone(),
        planner_manifest_digest: p.planner_manifest_digest.clone(),
        claim_snapshot_digest: p.claim_snapshot_digest.clone(),
        file_manifest_digest: p.file_manifest_digest.clone(),
        resource_manifest_digest: p.resource_manifest_digest.clone(),
        run_identity: p.run_identity.clone(),
        worker_identity: p.worker_identity.clone(),
        fence: p.fence,
        lease_generation: p.lease_generation,
        assumption_digest: p.assumption_digest.clone(),
        policy_digest: p.policy_digest.clone(),
        active_state_digest: p.active_state_digest.clone(),
        discovery_revocation_digest: r.discovery_revocation_digest.clone(),
        originating_chat_digest: r.originating_chat_digest.clone(),
        approval_delivery_digest: r.approval_delivery_digest.clone(),
        snapshot_captured_at_ms: snapshot.captured_at_ms(),
        expires_at_ms: snapshot.expires_at_ms(),
    }
}

fn verify_token(
    authority: &NativeClearanceAuthority,
    parsed: &ParsedToken,
    record: &mut ClearanceRecord,
) -> Result<(), ClearanceError> {
    if authority.issuer_epoch != record.issuer_epoch {
        return Err(ClearanceError::PriorIssuer);
    }
    let message =
        clearance_mac_message(&parsed.identifier_hex, record.issuer_epoch, &record.binding)?;
    let mut expected = authority.mac(&message);
    let accepted = constant_time_equal(&expected, &parsed.mac);
    expected.fill(0);
    if accepted {
        Ok(())
    } else {
        Err(ClearanceError::InvalidToken)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MacPayload<'a> {
    token_version: &'static str,
    identifier: &'a str,
    issuer_epoch: u64,
    binding: &'a ClearanceBinding,
}

fn clearance_mac_message(
    identifier: &str,
    epoch: u64,
    binding: &ClearanceBinding,
) -> Result<Vec<u8>, ClearanceError> {
    let payload = serde_json::to_vec(&MacPayload {
        token_version: TOKEN_PREFIX,
        identifier,
        issuer_epoch: epoch,
        binding,
    })
    .map_err(|_| ClearanceError::AuthorityUnavailable)?;
    let mut message = b"perfect-planner:clearance-mac:v1".to_vec();
    message.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    message.extend_from_slice(&payload);
    Ok(message)
}

struct ParsedToken {
    identifier: [u8; TOKEN_BYTES],
    identifier_hex: String,
    mac: [u8; TOKEN_BYTES],
}
impl ParsedToken {
    fn parse(token: &str) -> Result<Self, ClearanceError> {
        let mut parts = token.split('.');
        let prefix = parts.next().ok_or(ClearanceError::InvalidToken)?;
        let identifier = parts.next().ok_or(ClearanceError::InvalidToken)?;
        let mac = parts.next().ok_or(ClearanceError::InvalidToken)?;
        if prefix != TOKEN_PREFIX || parts.next().is_some() {
            return Err(ClearanceError::InvalidToken);
        }
        Ok(Self {
            identifier: decode_lower_hex(identifier)?,
            identifier_hex: identifier.into(),
            mac: decode_lower_hex(mac)?,
        })
    }
}
impl Drop for ParsedToken {
    fn drop(&mut self) {
        self.identifier.fill(0);
        self.mac.fill(0);
        unsafe { self.identifier_hex.as_mut_vec().fill(0) };
    }
}

fn decode_lower_hex<const N: usize>(value: &str) -> Result<[u8; N], ClearanceError> {
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    {
        return Err(ClearanceError::InvalidToken);
    }
    let mut output = [0_u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(output)
}
fn hex_nibble(value: u8) -> Result<u8, ClearanceError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(ClearanceError::InvalidToken),
    }
}
fn clearance_identifier_hash(value: &[u8; TOKEN_BYTES]) -> String {
    domain_hash(b"perfect-planner:clearance-id:v1", value)
}
fn domain_hash(domain: &[u8], value: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update((domain.len() as u64).to_le_bytes());
    digest.update(domain);
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
    format!("{:x}", digest.finalize())
}
fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}
fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().min(right.len()) {
        difference |= usize::from(left[index] ^ right[index]);
    }
    difference == 0
}
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut key_block = [0_u8; 64];
    if key.len() > 64 {
        key_block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    for index in 0..64 {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_hash);
    let result = outer.finalize().into();
    key_block.fill(0);
    inner_pad.fill(0);
    outer_pad.fill(0);
    result
}
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(windows)]
fn fill_os_random(bytes: &mut [u8]) -> Result<(), ClearanceError> {
    use std::ffi::c_void;
    use std::ptr;
    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut c_void,
            buffer: *mut u8,
            buffer_len: u32,
            flags: u32,
        ) -> i32;
    }
    let length = u32::try_from(bytes.len()).map_err(|_| ClearanceError::EntropyUnavailable)?;
    let status =
        unsafe { BCryptGenRandom(ptr::null_mut(), bytes.as_mut_ptr(), length, 0x0000_0002) };
    if status == 0 {
        Ok(())
    } else {
        Err(ClearanceError::EntropyUnavailable)
    }
}
#[cfg(unix)]
fn fill_os_random(bytes: &mut [u8]) -> Result<(), ClearanceError> {
    use std::{fs::File, io::Read};
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(bytes))
        .map_err(|_| ClearanceError::EntropyUnavailable)
}
#[cfg(not(any(windows, unix)))]
fn fill_os_random(_: &mut [u8]) -> Result<(), ClearanceError> {
    Err(ClearanceError::EntropyUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision_assessor::journal::JournalPayload;
    use crate::collision_assessor::snapshot::tests::{
        fixture_snapshot, fixture_snapshot_with_drift, FixtureSnapshotDrift,
    };
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestClock {
        wall_ms: AtomicU64,
        monotonic_ms: AtomicU64,
    }

    struct JumpClock {
        calls: AtomicU64,
    }

    impl NativeClock for JumpClock {
        fn now(&self) -> Result<TrustedTime, ClearanceError> {
            let call = self.calls.fetch_add(1, AtomicOrdering::AcqRel);
            Ok(if call < 2 {
                TrustedTime {
                    wall_ms: 1_100,
                    monotonic_ms: 100 + call,
                }
            } else {
                TrustedTime {
                    wall_ms: 5_000,
                    monotonic_ms: 4_000,
                }
            })
        }
    }

    impl TestClock {
        fn new(wall_ms: u64, monotonic_ms: u64) -> Self {
            Self {
                wall_ms: AtomicU64::new(wall_ms),
                monotonic_ms: AtomicU64::new(monotonic_ms),
            }
        }

        fn set(&self, wall_ms: u64, monotonic_ms: u64) {
            self.wall_ms.store(wall_ms, AtomicOrdering::Release);
            self.monotonic_ms
                .store(monotonic_ms, AtomicOrdering::Release);
        }
    }

    impl NativeClock for TestClock {
        fn now(&self) -> Result<TrustedTime, ClearanceError> {
            Ok(TrustedTime {
                wall_ms: self.wall_ms.load(AtomicOrdering::Acquire),
                monotonic_ms: self.monotonic_ms.load(AtomicOrdering::Acquire),
            })
        }
    }

    struct Rig {
        store: Arc<ClearanceStore>,
        snapshots: SnapshotStore,
        journal: AssessmentJournal,
        clock: Arc<TestClock>,
        snapshot: VerifiedAssessmentSnapshot,
        receipt: StoredSnapshotReceipt,
        receipts: Arc<ClearanceReceiptBundle>,
    }

    fn rig(verdict: AssessmentVerdict) -> Rig {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "perfect-planner-clearance-{}-{nonce}",
            std::process::id()
        ));
        let snapshots = SnapshotStore::new_for_test(root.join("snapshots"));
        let journal = AssessmentJournal::new_live_for_test(root.join("assessment.jsonl"));
        let snapshot = fixture_snapshot(verdict);
        let receipt = snapshots.persist(&snapshot).unwrap();
        journal
            .record_assessment(&snapshot, &receipt, 1_000)
            .unwrap();
        let participant_id = snapshot.participants()[0].participant_id.clone();
        let receipts = Arc::new(ClearanceReceiptBundle::for_test(
            &participant_id,
            &"f".repeat(64),
        ));
        let clock = Arc::new(TestClock::new(1_100, 100));
        let clock_authority: Arc<dyn NativeClock> = clock.clone();
        let store = Arc::new(ClearanceStore::new_for_test(
            journal.clone(),
            snapshots.clone(),
            7,
            [0x5a; TOKEN_BYTES],
            clock_authority,
        ));
        Rig {
            store,
            snapshots,
            journal,
            clock,
            snapshot,
            receipt,
            receipts,
        }
    }

    fn issue(rig: &Rig) -> IssuedClearance {
        rig.store
            .issue_with_entropy(&rig.snapshot, &rig.receipt, &rig.receipts, |bytes| {
                bytes.fill(0x7b);
                Ok(())
            })
            .unwrap()
    }

    #[test]
    fn production_constructor_is_fail_closed_without_native_authority() {
        let rig = rig(AssessmentVerdict::Clear);
        let disabled = ClearanceStore::new_disabled(rig.journal, rig.snapshots);
        assert!(matches!(
            disabled.issue(&rig.snapshot, &rig.receipt, &rig.receipts),
            Err(ClearanceError::AuthorityUnavailable)
        ));
    }

    #[test]
    fn every_clearance_binding_field_rejects_drift() {
        let rig = rig(AssessmentVerdict::Clear);
        let binding = binding_from_snapshot(&rig.snapshot, &rig.receipts, 1_100).unwrap();
        macro_rules! reject_drift {
            ($field:ident, $value:expr) => {{
                let mut changed = binding.clone();
                changed.$field = $value;
                assert!(
                    !binding.matches_current(&changed),
                    "{} drift was accepted",
                    stringify!($field)
                );
            }};
        }
        reject_drift!(snapshot_hash, "f".repeat(64));
        reject_drift!(registry_generation, binding.registry_generation + 1);
        reject_drift!(census_input_digest, "f".repeat(64));
        reject_drift!(participant_id, "f".repeat(64));
        reject_drift!(planner_id, "planner-drift".into());
        reject_drift!(plan_id, "plan-drift".into());
        reject_drift!(node_id, "node-drift".into());
        reject_drift!(repository_identity, "f".repeat(64));
        reject_drift!(worktree_identity, "f".repeat(64));
        reject_drift!(branch_digest, "f".repeat(64));
        reject_drift!(plan_content_digest, "f".repeat(64));
        reject_drift!(planner_manifest_digest, "f".repeat(64));
        reject_drift!(claim_snapshot_digest, "f".repeat(64));
        reject_drift!(file_manifest_digest, "f".repeat(64));
        reject_drift!(resource_manifest_digest, "f".repeat(64));
        reject_drift!(run_identity, "f".repeat(64));
        reject_drift!(worker_identity, "f".repeat(64));
        reject_drift!(fence, binding.fence + 1);
        reject_drift!(lease_generation, binding.lease_generation + 1);
        reject_drift!(assumption_digest, "f".repeat(64));
        reject_drift!(policy_digest, "f".repeat(64));
        reject_drift!(active_state_digest, "f".repeat(64));
        reject_drift!(discovery_revocation_digest, "e".repeat(64));
        reject_drift!(originating_chat_digest, "1".repeat(64));
        reject_drift!(approval_delivery_digest, "e".repeat(64));
        reject_drift!(snapshot_captured_at_ms, binding.snapshot_captured_at_ms + 1);
        reject_drift!(expires_at_ms, binding.expires_at_ms + 1);
    }

    #[test]
    fn valid_sealed_snapshot_and_receipt_drift_revokes_before_consuming() {
        let drifts = [
            FixtureSnapshotDrift::RegistryGeneration,
            FixtureSnapshotDrift::CapturedAt,
            FixtureSnapshotDrift::RepositoryIdentity,
            FixtureSnapshotDrift::PlanContentDigest,
            FixtureSnapshotDrift::PlannerManifestDigest,
            FixtureSnapshotDrift::ClaimSnapshotDigest,
            FixtureSnapshotDrift::FileManifestDigest,
            FixtureSnapshotDrift::ResourceManifestDigest,
            FixtureSnapshotDrift::RunIdentity,
            FixtureSnapshotDrift::WorkerIdentity,
            FixtureSnapshotDrift::Fence,
            FixtureSnapshotDrift::LeaseGeneration,
            FixtureSnapshotDrift::AssumptionDigest,
            FixtureSnapshotDrift::PolicyDigest,
            FixtureSnapshotDrift::ActiveStateDigest,
        ];
        for drift in drifts {
            let rig = rig(AssessmentVerdict::Clear);
            let issued = issue(&rig);
            let current = fixture_snapshot_with_drift(drift);
            assert!(
                matches!(
                    rig.store.begin_verified_for_test(
                        issued.token.as_str(),
                        &current,
                        &rig.receipts,
                    ),
                    Err(ClearanceError::BindingMismatch)
                ),
                "{drift:?} was not rejected"
            );
            let events = rig.journal.read_verified().unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(
                        event.payload,
                        JournalPayload::ClearanceRevoked { .. }
                    ))
                    .count(),
                1,
                "{drift:?} was not durably revoked"
            );
            assert!(!events
                .iter()
                .any(|event| matches!(event.payload, JournalPayload::ClearanceConsuming { .. })));
            assert!(matches!(
                rig.store.begin_verified_for_test(
                    issued.token.as_str(),
                    &rig.snapshot,
                    &rig.receipts,
                ),
                Err(ClearanceError::AlreadyUsed)
            ));
        }

        let participant_id = fixture_snapshot(AssessmentVerdict::Clear).participants()[0]
            .participant_id
            .clone();
        for (name, receipts) in [
            (
                "discovery",
                ClearanceReceiptBundle {
                    participant_id: participant_id.clone(),
                    discovery_revocation_digest: "1".repeat(64),
                    originating_chat_digest: "e".repeat(64),
                    approval_delivery_digest: "f".repeat(64),
                },
            ),
            (
                "chat",
                ClearanceReceiptBundle {
                    participant_id: participant_id.clone(),
                    discovery_revocation_digest: "d".repeat(64),
                    originating_chat_digest: "1".repeat(64),
                    approval_delivery_digest: "f".repeat(64),
                },
            ),
            (
                "approval",
                ClearanceReceiptBundle {
                    participant_id: participant_id.clone(),
                    discovery_revocation_digest: "d".repeat(64),
                    originating_chat_digest: "e".repeat(64),
                    approval_delivery_digest: "1".repeat(64),
                },
            ),
        ] {
            let rig = rig(AssessmentVerdict::Clear);
            let issued = issue(&rig);
            assert!(
                matches!(
                    rig.store.begin_verified_for_test(
                        issued.token.as_str(),
                        &rig.snapshot,
                        &receipts,
                    ),
                    Err(ClearanceError::BindingMismatch)
                ),
                "{name} receipt drift was not rejected"
            );
            let events = rig.journal.read_verified().unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(
                        event.payload,
                        JournalPayload::ClearanceRevoked { .. }
                    ))
                    .count(),
                1
            );
            assert!(!events
                .iter()
                .any(|event| matches!(event.payload, JournalPayload::ClearanceConsuming { .. })));
        }
    }

    #[test]
    fn clearance_from_prior_issuer_epoch_is_denied_before_state_change() {
        let rig = rig(AssessmentVerdict::Clear);
        let issued = issue(&rig);
        let parsed = ParsedToken::parse(issued.token.as_str()).unwrap();
        let id_hash = clearance_identifier_hash(&parsed.identifier);
        let record = rig
            .store
            .records
            .lock()
            .unwrap()
            .get(&id_hash)
            .unwrap()
            .clone();
        let rotated = ClearanceStore::new_for_test(
            rig.journal.clone(),
            rig.snapshots.clone(),
            8,
            [0x5a; TOKEN_BYTES],
            rig.clock.clone(),
        );
        rotated
            .records
            .lock()
            .unwrap()
            .insert(id_hash.clone(), record);
        assert!(matches!(
            rotated.begin_verified_for_test(issued.token.as_str(), &rig.snapshot, &rig.receipts,),
            Err(ClearanceError::PriorIssuer)
        ));
        assert_eq!(
            rotated
                .records
                .lock()
                .unwrap()
                .get(&id_hash)
                .unwrap()
                .lifecycle,
            ClearanceLifecycle::Issued
        );
        assert_eq!(rig.journal.read_verified().unwrap().len(), 2);
    }

    #[test]
    fn expiry_during_snapshot_and_journal_revalidation_is_revoked() {
        let rig = rig(AssessmentVerdict::Clear);
        let clock: Arc<dyn NativeClock> = Arc::new(JumpClock {
            calls: AtomicU64::new(0),
        });
        let store = ClearanceStore::new_for_test(
            rig.journal.clone(),
            rig.snapshots.clone(),
            7,
            [0x5a; TOKEN_BYTES],
            clock,
        );
        let issued = store
            .issue_with_entropy(&rig.snapshot, &rig.receipt, &rig.receipts, |bytes| {
                bytes.fill(0x6c);
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            store.begin_verified_for_test(issued.token.as_str(), &rig.snapshot, &rig.receipts),
            Err(ClearanceError::Expired)
        ));
        assert!(matches!(
            rig.journal.read_verified().unwrap().last().unwrap().payload,
            JournalPayload::ClearanceRevoked { .. }
        ));
    }

    #[test]
    fn only_clear_issues_and_token_debug_is_redacted() {
        let clear = rig(AssessmentVerdict::Clear);
        let issued = issue(&clear);
        assert_eq!(format!("{:?}", issued.token), "ClearanceToken(<redacted>)");
        assert!(!format!("{:?}", issued).contains(issued.token.as_str()));
        let events = clear.journal.read_verified().unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[1].payload,
            JournalPayload::ClearanceIssued { .. }
        ));

        let unknown = rig(AssessmentVerdict::Unknown);
        assert!(matches!(
            unknown
                .store
                .issue(&unknown.snapshot, &unknown.receipt, &unknown.receipts),
            Err(ClearanceError::NonClearAssessment)
        ));
        assert_eq!(unknown.journal.read_verified().unwrap().len(), 1);
    }

    #[test]
    fn one_hundred_simultaneous_replays_have_exactly_one_winner() {
        let rig = rig(AssessmentVerdict::Clear);
        let issued = issue(&rig);
        let token = Arc::new(issued.token.as_str().to_string());
        let workers = (0..100)
            .map(|_| {
                let store = Arc::clone(&rig.store);
                let snapshot = rig.snapshot.clone();
                let receipts = Arc::clone(&rig.receipts);
                let token = Arc::clone(&token);
                thread::spawn(move || store.begin_verified_for_test(&token, &snapshot, &receipts))
            })
            .collect::<Vec<_>>();
        let winners = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(Result::is_ok)
            .count();
        assert_eq!(winners, 1);
        let events = rig.journal.read_verified().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.payload, JournalPayload::ClearanceConsuming { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn exact_expiry_is_denied_and_durably_revoked() {
        let rig = rig(AssessmentVerdict::Clear);
        let issued = issue(&rig);
        rig.clock.set(issued.expires_at_ms, 4_000);
        assert!(matches!(
            rig.store
                .begin_verified_for_test(issued.token.as_str(), &rig.snapshot, &rig.receipts),
            Err(ClearanceError::Expired)
        ));
        let events = rig.journal.read_verified().unwrap();
        assert!(matches!(
            events.last().unwrap().payload,
            JournalPayload::ClearanceRevoked { .. }
        ));
    }

    #[test]
    fn consuming_clearance_can_be_revoked_and_never_replayed() {
        let rig = rig(AssessmentVerdict::Clear);
        let issued = issue(&rig);
        let permit = rig
            .store
            .begin_verified_for_test(issued.token.as_str(), &rig.snapshot, &rig.receipts)
            .unwrap();
        assert_eq!(permit.snapshot_hash(), rig.snapshot.snapshot_hash());
        rig.store
            .revoke(issued.token.as_str(), &"a".repeat(64))
            .unwrap();
        assert!(matches!(
            rig.store
                .begin_verified_for_test(issued.token.as_str(), &rig.snapshot, &rig.receipts),
            Err(ClearanceError::AlreadyUsed)
        ));
        assert!(matches!(
            rig.journal.read_verified().unwrap().last().unwrap().payload,
            JournalPayload::ClearanceRevoked { .. }
        ));
    }

    #[test]
    fn snapshot_loss_before_consumption_revokes_instead_of_admitting() {
        let rig = rig(AssessmentVerdict::Clear);
        let issued = issue(&rig);
        rig.snapshots.delete_for_test(rig.snapshot.snapshot_hash());
        assert!(matches!(
            rig.store
                .begin_verified_for_test(issued.token.as_str(), &rig.snapshot, &rig.receipts),
            Err(ClearanceError::InvalidSnapshot)
        ));
        assert!(matches!(
            rig.journal.read_verified().unwrap().last().unwrap().payload,
            JournalPayload::ClearanceRevoked { .. }
        ));
    }

    #[test]
    fn invalid_mac_neither_consumes_nor_appends() {
        let rig = rig(AssessmentVerdict::Clear);
        let issued = issue(&rig);
        let mut forged = issued.token.as_str().to_string();
        let replacement = if forged.ends_with('0') { '1' } else { '0' };
        forged.pop();
        forged.push(replacement);
        assert!(matches!(
            rig.store
                .begin_verified_for_test(&forged, &rig.snapshot, &rig.receipts),
            Err(ClearanceError::InvalidToken)
        ));
        assert_eq!(rig.journal.read_verified().unwrap().len(), 2);
    }

    #[test]
    fn hmac_sha256_matches_rfc_4231_case_one() {
        assert_eq!(
            hex_encode(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }
}
