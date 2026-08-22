//! Strict, append-only, hash-chained collision assessor audit journal.

use super::model::ConflictDisposition;
use super::snapshot::{
    verify_conflict_proof, AssessmentVerdict, ConflictProofStep, SnapshotConflict,
    StoredSnapshotReceipt, VerifiedAssessmentSnapshot, VerifiedConflictProof,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc, Mutex, OnceLock, Weak,
};
use std::thread;
use std::time::{Duration, Instant};

const JOURNAL_SCHEMA_VERSION: u32 = 5;
const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 1024 * 1024;
const MAX_TICKET_LINE_BYTES: usize = 16 * 1024;
const MAX_EVENTS: usize = 200_000;
const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const TRUST_LIVE_PROCESS: u8 = 1;
const TRUST_RECOVERED_UNANCHORED: u8 = 2;
const JOURNAL_ANCHOR_VERSION: u32 = 1;
const JOURNAL_WRITER_LOCK_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_ANCHOR_BYTES: u64 = 4 * 1024;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JournalAnchor {
    version: u32,
    writer_epoch: u64,
    sequence: u64,
    event_hash: String,
    file_len: u64,
    verifying_key: String,
    key_fingerprint: String,
    checkpoint_signature: String,
}

/// One secret owner exists per journal path in this process. Cloned journals share this object;
/// they never clone or serialize the signing key. The lifetime-held writer lock excludes another
/// process, while `append_gate` bounds all local writers to one authenticated transition at a time.
struct NativeJournalWriter {
    epoch: u64,
    signing_key: SigningKey,
    append_gate: Mutex<()>,
    _exclusive_writer: JournalLock,
}

impl NativeJournalWriter {
    fn verification_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    fn fingerprint(&self) -> String {
        journal_key_fingerprint(&self.verification_key())
    }

    fn checkpoint(&self, high_water: &HighWater) -> JournalAnchor {
        let verifying_key = self.verification_key();
        let mut anchor = JournalAnchor {
            version: JOURNAL_ANCHOR_VERSION,
            writer_epoch: self.epoch,
            sequence: high_water.sequence,
            event_hash: high_water.event_hash.clone(),
            file_len: high_water.file_len,
            verifying_key: encode_hex(&verifying_key),
            key_fingerprint: journal_key_fingerprint(&verifying_key),
            checkpoint_signature: String::new(),
        };
        anchor.checkpoint_signature = encode_hex(
            &self
                .signing_key
                .sign(&journal_anchor_message(&anchor))
                .to_bytes(),
        );
        anchor
    }
}

fn live_writers() -> &'static Mutex<BTreeMap<PathBuf, Weak<NativeJournalWriter>>> {
    static WRITERS: OnceLock<Mutex<BTreeMap<PathBuf, Weak<NativeJournalWriter>>>> = OnceLock::new();
    WRITERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum JournalVerdict {
    Clear,
    Wait,
    Replan,
    UserDecision,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum TicketSignalKind {
    NodeDone,
    LeaseReleased,
    ManifestChanged,
    DecisionRequired,
    ReplanRequired,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub(crate) enum JournalPayload {
    Assessment {
        snapshot_hash: String,
        registry_generation: u64,
        census_input_digest: String,
        verdict: JournalVerdict,
        participant_count: u32,
        /// Fixed-width binary commitments, base64 encoded as one bounded value. Each participant
        /// contributes 32 bytes of participant ID plus 32 bytes of source-node digest.
        participant_node_bindings_packed: String,
        /// Sorted unique pairs of big-endian u16 indexes into the participant binding table.
        participant_conflict_edges_packed: String,
        conflict_count: u32,
        conflict_commitment_root: String,
        captured_at_ms: u64,
        expires_at_ms: u64,
        encoded_bytes: u64,
        store_binding: String,
    },
    Revocation {
        snapshot_hash: String,
        reason_digest: String,
    },
    ConflictTicket {
        snapshot_hash: String,
        ticket_id: String,
        conflict_id: String,
        conflict: SnapshotConflict,
        leaf_index: u32,
        leaf_count: u32,
        proof: Vec<ConflictProofStep>,
        commitment_root: String,
    },
    ConflictTicketSignal {
        snapshot_hash: String,
        signal_id: String,
        actor_participant_id: String,
        source_node_id: String,
        signal_kind: TicketSignalKind,
        source_state_digest: String,
        source_event_id: String,
    },
    ConflictTicketAcknowledged {
        snapshot_hash: String,
        signal_id: String,
        recipient_participant_id: String,
        acknowledgement_digest: String,
    },
    ClearanceIssued {
        clearance_id_hash: String,
        snapshot_hash: String,
        participant_id: String,
        issuer_epoch: u64,
        expires_at_ms: u64,
    },
    ClearanceConsuming {
        clearance_id_hash: String,
        snapshot_hash: String,
        participant_id: String,
        issuer_epoch: u64,
    },
    ClearanceConsumed {
        clearance_id_hash: String,
        snapshot_hash: String,
        participant_id: String,
        issuer_epoch: u64,
        executor_receipt_digest: String,
    },
    ClearanceRevoked {
        clearance_id_hash: String,
        snapshot_hash: String,
        participant_id: String,
        issuer_epoch: u64,
        reason_digest: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct JournalEvent {
    pub(crate) schema_version: u32,
    pub(crate) sequence: u64,
    pub(crate) previous_hash: String,
    pub(crate) recorded_at_ms: u64,
    pub(crate) payload: JournalPayload,
    pub(crate) event_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HighWater {
    sequence: u64,
    event_hash: String,
    file_len: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClearanceJournalState {
    Issued,
    Consuming,
    Consumed,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClearanceJournalRecord {
    snapshot_hash: String,
    participant_id: String,
    issuer_epoch: u64,
    expires_at_ms: u64,
    state: ClearanceJournalState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AssessmentJournalRecord {
    live: bool,
    revoked_by_signal_id: Option<String>,
    verdict: JournalVerdict,
    participant_indices: BTreeMap<String, u16>,
    participant_node_digests: BTreeMap<String, String>,
    participant_conflict_edges: BTreeSet<(u16, u16)>,
    conflict_count: u32,
    conflict_commitment_root: String,
    captured_at_ms: u64,
    expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TicketJournalRecord {
    snapshot_hash: String,
    conflict: SnapshotConflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TicketSignalJournalRecord {
    snapshot_hash: String,
    actor_participant_id: String,
}

#[derive(Clone, Default)]
struct ReplayState {
    assessments: BTreeMap<String, AssessmentJournalRecord>,
    clearances: BTreeMap<String, ClearanceJournalRecord>,
    conflict_tickets: BTreeSet<(String, String)>,
    tickets: BTreeMap<String, TicketJournalRecord>,
    ticket_signals: BTreeMap<String, TicketSignalJournalRecord>,
    ticket_source_events: BTreeMap<(String, String, String, String), String>,
    ticket_acknowledgements: BTreeSet<(String, String)>,
}

impl ReplayState {
    fn apply(&mut self, payload: &JournalPayload, recorded_at_ms: u64) -> Result<(), JournalError> {
        match payload {
            JournalPayload::Assessment {
                snapshot_hash,
                verdict,
                participant_count,
                participant_node_bindings_packed,
                participant_conflict_edges_packed,
                conflict_count,
                conflict_commitment_root,
                captured_at_ms,
                expires_at_ms,
                ..
            } => {
                if recorded_at_ms < *captured_at_ms || recorded_at_ms >= *expires_at_ms {
                    return Err(JournalError::InvalidEvent);
                }
                let participant_node_digests = unpack_participant_node_bindings(
                    participant_node_bindings_packed,
                    *participant_count,
                )?;
                let participant_indices = participant_node_digests
                    .keys()
                    .enumerate()
                    .map(|(index, participant_id)| {
                        Ok((
                            participant_id.clone(),
                            u16::try_from(index).map_err(|_| JournalError::LimitExceeded)?,
                        ))
                    })
                    .collect::<Result<BTreeMap<_, _>, JournalError>>()?;
                let participant_conflict_edges = unpack_participant_conflict_edges(
                    participant_conflict_edges_packed,
                    *participant_count,
                    *conflict_count,
                )?;
                if self
                    .assessments
                    .insert(
                        snapshot_hash.clone(),
                        AssessmentJournalRecord {
                            live: true,
                            revoked_by_signal_id: None,
                            verdict: *verdict,
                            participant_indices,
                            participant_node_digests,
                            participant_conflict_edges,
                            conflict_count: *conflict_count,
                            conflict_commitment_root: conflict_commitment_root.clone(),
                            captured_at_ms: *captured_at_ms,
                            expires_at_ms: *expires_at_ms,
                        },
                    )
                    .is_some()
                {
                    return Err(JournalError::InvalidEvent);
                }
            }
            JournalPayload::Revocation { snapshot_hash, .. } => {
                let Some(live) = self.assessments.get_mut(snapshot_hash) else {
                    return Err(JournalError::InvalidEvent);
                };
                if !live.live || recorded_at_ms < live.captured_at_ms {
                    return Err(JournalError::InvalidEvent);
                }
                live.live = false;
                live.revoked_by_signal_id = None;
            }
            JournalPayload::ConflictTicket {
                snapshot_hash,
                ticket_id,
                conflict_id,
                conflict,
                leaf_index,
                leaf_count,
                proof,
                commitment_root,
            } => {
                let Some(assessment) = self.assessments.get(snapshot_hash) else {
                    return Err(JournalError::InvalidEvent);
                };
                if !assessment.live
                    || assessment.conflict_count == 0
                    || recorded_at_ms < assessment.captured_at_ms
                    || recorded_at_ms >= assessment.expires_at_ms
                    || *leaf_count != assessment.conflict_count
                    || commitment_root != &assessment.conflict_commitment_root
                    || conflict_id != &conflict.conflict_id
                    || conflict.disposition.is_none()
                    || !assessment
                        .participant_indices
                        .contains_key(&conflict.left_participant_id)
                    || !assessment
                        .participant_indices
                        .contains_key(&conflict.right_participant_id)
                    || !ticket_disposition_matches(assessment.verdict, conflict.disposition)
                    || !verify_conflict_proof(
                        conflict,
                        *leaf_index,
                        *leaf_count,
                        proof,
                        commitment_root,
                    )
                    || ticket_id != &conflict_ticket_id(snapshot_hash, commitment_root, conflict_id)
                    || !self
                        .conflict_tickets
                        .insert((snapshot_hash.clone(), conflict_id.clone()))
                    || self
                        .tickets
                        .insert(
                            ticket_id.clone(),
                            TicketJournalRecord {
                                snapshot_hash: snapshot_hash.clone(),
                                conflict: conflict.clone(),
                            },
                        )
                        .is_some()
                {
                    return Err(JournalError::InvalidEvent);
                }
            }
            JournalPayload::ConflictTicketSignal {
                snapshot_hash,
                signal_id,
                actor_participant_id,
                source_node_id,
                signal_kind,
                source_state_digest,
                source_event_id,
            } => {
                let Some(assessment) = self.assessments.get(snapshot_hash) else {
                    return Err(JournalError::InvalidEvent);
                };
                let source_slot = (
                    snapshot_hash.clone(),
                    actor_participant_id.clone(),
                    source_node_id.clone(),
                    source_event_id.clone(),
                );
                if !assessment.live
                    || recorded_at_ms < assessment.captured_at_ms
                    || recorded_at_ms >= assessment.expires_at_ms
                    || assessment
                        .participant_node_digests
                        .get(actor_participant_id)
                        != Some(&source_node_digest(source_node_id))
                    || !transition_signal_matches_verdict(*signal_kind, assessment.verdict)
                    || !is_sha256(source_state_digest)
                    || !is_sha256(source_event_id)
                    || signal_id
                        != &conflict_ticket_signal_id(
                            snapshot_hash,
                            actor_participant_id,
                            source_node_id,
                            *signal_kind,
                            source_event_id,
                        )
                    || self
                        .ticket_source_events
                        .insert(source_slot, signal_id.clone())
                        .is_some()
                    || self
                        .ticket_signals
                        .insert(
                            signal_id.clone(),
                            TicketSignalJournalRecord {
                                snapshot_hash: snapshot_hash.clone(),
                                actor_participant_id: actor_participant_id.clone(),
                            },
                        )
                        .is_some()
                {
                    return Err(JournalError::InvalidEvent);
                }
                if *signal_kind == TicketSignalKind::ManifestChanged {
                    let assessment = self
                        .assessments
                        .get_mut(snapshot_hash)
                        .expect("the assessment was validated above");
                    assessment.live = false;
                    assessment.revoked_by_signal_id = Some(signal_id.clone());
                }
            }
            JournalPayload::ConflictTicketAcknowledged {
                snapshot_hash,
                signal_id,
                recipient_participant_id,
                acknowledgement_digest,
            } => {
                let Some(assessment) = self.assessments.get(snapshot_hash) else {
                    return Err(JournalError::InvalidEvent);
                };
                let Some(signal) = self.ticket_signals.get(signal_id) else {
                    return Err(JournalError::InvalidEvent);
                };
                if recorded_at_ms < assessment.captured_at_ms
                    || signal.snapshot_hash != *snapshot_hash
                    || signal.actor_participant_id == *recipient_participant_id
                    || !assessment
                        .participant_indices
                        .contains_key(recipient_participant_id)
                    || !assessment.participant_conflict_edges.contains(
                        &participant_index_edge(
                            &assessment.participant_indices,
                            &signal.actor_participant_id,
                            recipient_participant_id,
                        )
                        .ok_or(JournalError::InvalidEvent)?,
                    )
                    || !is_sha256(acknowledgement_digest)
                    || !self
                        .ticket_acknowledgements
                        .insert((signal_id.clone(), recipient_participant_id.clone()))
                {
                    return Err(JournalError::InvalidEvent);
                }
            }
            JournalPayload::ClearanceIssued {
                clearance_id_hash,
                snapshot_hash,
                participant_id,
                issuer_epoch,
                expires_at_ms,
            } => {
                let Some(assessment) = self.assessments.get(snapshot_hash) else {
                    return Err(JournalError::InvalidEvent);
                };
                if !assessment.live
                    || assessment.verdict != JournalVerdict::Clear
                    || assessment.conflict_count != 0
                    || !assessment.participant_indices.contains_key(participant_id)
                    || recorded_at_ms < assessment.captured_at_ms
                    || *expires_at_ms > assessment.expires_at_ms
                    || *expires_at_ms <= recorded_at_ms
                    || self.clearances.contains_key(clearance_id_hash)
                {
                    return Err(JournalError::InvalidEvent);
                }
                self.clearances.insert(
                    clearance_id_hash.clone(),
                    ClearanceJournalRecord {
                        snapshot_hash: snapshot_hash.clone(),
                        participant_id: participant_id.clone(),
                        issuer_epoch: *issuer_epoch,
                        expires_at_ms: *expires_at_ms,
                        state: ClearanceJournalState::Issued,
                    },
                );
            }
            JournalPayload::ClearanceConsuming {
                clearance_id_hash,
                snapshot_hash,
                participant_id,
                issuer_epoch,
            } => {
                if !self.transition_is_live(clearance_id_hash, snapshot_hash, recorded_at_ms) {
                    return Err(JournalError::InvalidEvent);
                }
                self.transition_clearance(
                    clearance_id_hash,
                    snapshot_hash,
                    participant_id,
                    *issuer_epoch,
                    &[ClearanceJournalState::Issued],
                    ClearanceJournalState::Consuming,
                )?;
            }
            JournalPayload::ClearanceConsumed {
                clearance_id_hash,
                snapshot_hash,
                participant_id,
                issuer_epoch,
                ..
            } => {
                if !self.transition_is_live(clearance_id_hash, snapshot_hash, recorded_at_ms) {
                    return Err(JournalError::InvalidEvent);
                }
                self.transition_clearance(
                    clearance_id_hash,
                    snapshot_hash,
                    participant_id,
                    *issuer_epoch,
                    &[ClearanceJournalState::Consuming],
                    ClearanceJournalState::Consumed,
                )?;
            }
            JournalPayload::ClearanceRevoked {
                clearance_id_hash,
                snapshot_hash,
                participant_id,
                issuer_epoch,
                ..
            } => self.transition_clearance(
                clearance_id_hash,
                snapshot_hash,
                participant_id,
                *issuer_epoch,
                &[
                    ClearanceJournalState::Issued,
                    ClearanceJournalState::Consuming,
                ],
                ClearanceJournalState::Revoked,
            )?,
        }
        Ok(())
    }

    /// Replays an exact lost-response retry against the current state while temporarily removing
    /// only its already-occupied idempotency slot. A ticket or signal therefore cannot report
    /// success after a concurrent revocation; an acknowledgement remains audit-only and may be
    /// repeated after revocation while the snapshot delivery window is still open.
    fn validate_idempotent_retry(
        &self,
        payload: &JournalPayload,
        recorded_at_ms: u64,
    ) -> Result<(), JournalError> {
        let mut candidate_state = self.clone();
        match payload {
            JournalPayload::ConflictTicket {
                snapshot_hash,
                ticket_id,
                conflict_id,
                ..
            } => {
                candidate_state
                    .conflict_tickets
                    .remove(&(snapshot_hash.clone(), conflict_id.clone()));
                candidate_state.tickets.remove(ticket_id);
            }
            JournalPayload::ConflictTicketSignal {
                snapshot_hash,
                signal_id,
                actor_participant_id,
                source_node_id,
                source_event_id,
                ..
            } => {
                candidate_state.ticket_signals.remove(signal_id);
                candidate_state.ticket_source_events.remove(&(
                    snapshot_hash.clone(),
                    actor_participant_id.clone(),
                    source_node_id.clone(),
                    source_event_id.clone(),
                ));
                if let JournalPayload::ConflictTicketSignal {
                    snapshot_hash,
                    signal_kind: TicketSignalKind::ManifestChanged,
                    ..
                } = payload
                {
                    let assessment = candidate_state
                        .assessments
                        .get_mut(snapshot_hash)
                        .ok_or(JournalError::InvalidEvent)?;
                    if assessment.revoked_by_signal_id.as_deref() != Some(signal_id) {
                        return Err(JournalError::InvalidEvent);
                    }
                    assessment.live = true;
                    assessment.revoked_by_signal_id = None;
                }
            }
            JournalPayload::ConflictTicketAcknowledged {
                signal_id,
                recipient_participant_id,
                ..
            } => {
                candidate_state
                    .ticket_acknowledgements
                    .remove(&(signal_id.clone(), recipient_participant_id.clone()));
            }
            _ => return Err(JournalError::InvalidEvent),
        }
        candidate_state.apply(payload, recorded_at_ms)
    }

    fn transition_clearance(
        &mut self,
        clearance_id_hash: &str,
        snapshot_hash: &str,
        participant_id: &str,
        issuer_epoch: u64,
        allowed: &[ClearanceJournalState],
        next: ClearanceJournalState,
    ) -> Result<(), JournalError> {
        let Some(record) = self.clearances.get_mut(clearance_id_hash) else {
            return Err(JournalError::InvalidEvent);
        };
        if record.snapshot_hash != snapshot_hash
            || record.participant_id != participant_id
            || record.issuer_epoch != issuer_epoch
            || !allowed.contains(&record.state)
        {
            return Err(JournalError::InvalidEvent);
        }
        record.state = next;
        Ok(())
    }

    fn transition_is_live(
        &self,
        clearance_id_hash: &str,
        snapshot_hash: &str,
        recorded_at_ms: u64,
    ) -> bool {
        let Some(assessment) = self.assessments.get(snapshot_hash) else {
            return false;
        };
        let Some(clearance) = self.clearances.get(clearance_id_hash) else {
            return false;
        };
        assessment.live
            && recorded_at_ms < assessment.expires_at_ms
            && recorded_at_ms < clearance.expires_at_ms
    }
}

fn ticket_disposition_matches(
    verdict: JournalVerdict,
    disposition: Option<ConflictDisposition>,
) -> bool {
    matches!(
        (verdict, disposition),
        (JournalVerdict::Wait, Some(ConflictDisposition::Wait))
            | (JournalVerdict::Replan, Some(ConflictDisposition::Replan))
            | (
                JournalVerdict::UserDecision,
                Some(ConflictDisposition::UserDecision)
            )
    )
}

fn transition_signal_matches_verdict(kind: TicketSignalKind, verdict: JournalVerdict) -> bool {
    match kind {
        TicketSignalKind::DecisionRequired => verdict == JournalVerdict::UserDecision,
        TicketSignalKind::ReplanRequired => verdict == JournalVerdict::Replan,
        TicketSignalKind::NodeDone
        | TicketSignalKind::LeaseReleased
        | TicketSignalKind::ManifestChanged => true,
    }
}

pub(crate) fn conflict_ticket_id(
    snapshot_hash: &str,
    commitment_root: &str,
    conflict_id: &str,
) -> String {
    journal_digest(
        b"perfect-planner:conflict-ticket-id:v1",
        &[snapshot_hash, commitment_root, conflict_id],
    )
}

pub(crate) fn conflict_ticket_signal_id(
    snapshot_hash: &str,
    actor_participant_id: &str,
    source_node_id: &str,
    kind: TicketSignalKind,
    source_event_id: &str,
) -> String {
    let kind = match kind {
        TicketSignalKind::NodeDone => "NODE_DONE",
        TicketSignalKind::LeaseReleased => "LEASE_RELEASED",
        TicketSignalKind::ManifestChanged => "MANIFEST_CHANGED",
        TicketSignalKind::DecisionRequired => "DECISION_REQUIRED",
        TicketSignalKind::ReplanRequired => "REPLAN_REQUIRED",
    };
    journal_digest(
        b"perfect-planner:conflict-ticket-signal-id:v1",
        &[
            snapshot_hash,
            actor_participant_id,
            source_node_id,
            kind,
            source_event_id,
        ],
    )
}

fn source_node_digest(node_id: &str) -> String {
    journal_digest(b"perfect-planner:participant-source-node:v1", &[node_id])
}

fn participant_index_edge(
    participant_indices: &BTreeMap<String, u16>,
    left_participant_id: &str,
    right_participant_id: &str,
) -> Option<(u16, u16)> {
    let left = *participant_indices.get(left_participant_id)?;
    let right = *participant_indices.get(right_participant_id)?;
    (left != right).then_some(if left < right {
        (left, right)
    } else {
        (right, left)
    })
}

fn decode_sha256_hex(value: &str) -> Option<[u8; 32]> {
    if !is_sha256(value) {
        return None;
    }
    let mut decoded = [0u8; 32];
    for (index, byte) in decoded.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(decoded)
}

fn encode_sha256_hex(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn pack_participant_node_bindings(
    snapshot: &VerifiedAssessmentSnapshot,
) -> Result<String, JournalError> {
    let mut participants = snapshot.participants().iter().collect::<Vec<_>>();
    participants.sort_by(|left, right| left.participant_id.cmp(&right.participant_id));
    let mut packed = Vec::with_capacity(participants.len().saturating_mul(64));
    for participant in participants {
        let participant_id =
            decode_sha256_hex(&participant.participant_id).ok_or(JournalError::InvalidEvent)?;
        if !is_bounded_id(&participant.node_id) {
            return Err(JournalError::InvalidEvent);
        }
        let node_digest = decode_sha256_hex(&source_node_digest(&participant.node_id))
            .ok_or(JournalError::InvalidEvent)?;
        packed.extend_from_slice(&participant_id);
        packed.extend_from_slice(&node_digest);
    }
    Ok(BASE64_STANDARD.encode(packed))
}

fn unpack_participant_node_bindings(
    packed: &str,
    participant_count: u32,
) -> Result<BTreeMap<String, String>, JournalError> {
    let expected_count =
        usize::try_from(participant_count).map_err(|_| JournalError::LimitExceeded)?;
    if expected_count == 0 || expected_count > 4_096 {
        return Err(JournalError::LimitExceeded);
    }
    let bytes = BASE64_STANDARD
        .decode(packed)
        .map_err(|_| JournalError::InvalidEvent)?;
    if bytes.len() != expected_count.saturating_mul(64) {
        return Err(JournalError::InvalidEvent);
    }
    let mut bindings = BTreeMap::new();
    let mut previous: Option<String> = None;
    for chunk in bytes.chunks_exact(64) {
        let participant_id = encode_sha256_hex(&chunk[..32]);
        let node_digest = encode_sha256_hex(&chunk[32..]);
        if previous
            .as_ref()
            .is_some_and(|prior| prior >= &participant_id)
            || bindings
                .insert(participant_id.clone(), node_digest)
                .is_some()
        {
            return Err(JournalError::InvalidEvent);
        }
        previous = Some(participant_id);
    }
    Ok(bindings)
}

fn pack_participant_conflict_edges(
    snapshot: &VerifiedAssessmentSnapshot,
) -> Result<String, JournalError> {
    let participant_indices = snapshot
        .participants()
        .iter()
        .map(|participant| participant.participant_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(index, participant_id)| {
            Ok((
                participant_id,
                u16::try_from(index).map_err(|_| JournalError::LimitExceeded)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, JournalError>>()?;
    let edges = snapshot
        .conflicts()
        .iter()
        .map(|conflict| {
            participant_index_edge(
                &participant_indices,
                &conflict.left_participant_id,
                &conflict.right_participant_id,
            )
            .ok_or(JournalError::InvalidEvent)
        })
        .collect::<Result<BTreeSet<_>, JournalError>>()?;
    let mut packed = Vec::with_capacity(edges.len().saturating_mul(4));
    for (left, right) in edges {
        packed.extend_from_slice(&left.to_be_bytes());
        packed.extend_from_slice(&right.to_be_bytes());
    }
    Ok(BASE64_STANDARD.encode(packed))
}

fn unpack_participant_conflict_edges(
    packed: &str,
    participant_count: u32,
    conflict_count: u32,
) -> Result<BTreeSet<(u16, u16)>, JournalError> {
    let bytes = BASE64_STANDARD
        .decode(packed)
        .map_err(|_| JournalError::InvalidEvent)?;
    if bytes.len() % 4 != 0 || bytes.len() / 4 > 8_192 {
        return Err(JournalError::LimitExceeded);
    }
    let edge_count = bytes.len() / 4;
    if edge_count > usize::try_from(conflict_count).map_err(|_| JournalError::LimitExceeded)?
        || (conflict_count > 0 && edge_count == 0)
    {
        return Err(JournalError::InvalidEvent);
    }
    let mut edges = BTreeSet::new();
    let mut previous: Option<(u16, u16)> = None;
    for chunk in bytes.chunks_exact(4) {
        let edge = (
            u16::from_be_bytes([chunk[0], chunk[1]]),
            u16::from_be_bytes([chunk[2], chunk[3]]),
        );
        if edge.0 >= edge.1
            || u32::from(edge.1) >= participant_count
            || previous.is_some_and(|prior| prior >= edge)
            || !edges.insert(edge)
        {
            return Err(JournalError::InvalidEvent);
        }
        previous = Some(edge);
    }
    Ok(edges)
}

fn assessment_payload(
    snapshot: &VerifiedAssessmentSnapshot,
    receipt: &StoredSnapshotReceipt,
) -> Result<JournalPayload, JournalError> {
    if !receipt.matches(snapshot) {
        return Err(JournalError::InvalidEvent);
    }
    Ok(JournalPayload::Assessment {
        snapshot_hash: snapshot.snapshot_hash().to_string(),
        registry_generation: snapshot.registry_generation(),
        census_input_digest: snapshot.census_input_digest().to_string(),
        verdict: snapshot.verdict().into(),
        participant_count: u32::try_from(snapshot.participants().len())
            .map_err(|_| JournalError::LimitExceeded)?,
        participant_node_bindings_packed: pack_participant_node_bindings(snapshot)?,
        participant_conflict_edges_packed: pack_participant_conflict_edges(snapshot)?,
        conflict_count: u32::try_from(snapshot.conflicts().len())
            .map_err(|_| JournalError::LimitExceeded)?,
        conflict_commitment_root: snapshot.conflict_commitment_root(),
        captured_at_ms: snapshot.captured_at_ms(),
        expires_at_ms: snapshot.expires_at_ms(),
        encoded_bytes: receipt.encoded_bytes(),
        store_binding: receipt.store_binding().to_string(),
    })
}

fn journal_digest(domain: &[u8], values: &[&str]) -> String {
    let mut digest = Sha256::new();
    digest.update((domain.len() as u64).to_le_bytes());
    digest.update(domain);
    for value in values {
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

pub(crate) enum JournalError {
    InvalidEvent,
    CorruptChain,
    TornTail,
    RollbackDetected,
    AnchorMissing,
    AnchorInvalid,
    WriterAuthorityUnavailable,
    WriterAuthorityHeld,
    InvalidWriterEpoch,
    LimitExceeded,
    LockTimeout,
    Io(io::Error),
    Json(serde_json::Error),
}

impl fmt::Debug for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEvent => formatter.write_str("journal event is invalid"),
            Self::CorruptChain => formatter.write_str("journal chain is corrupt"),
            Self::TornTail => formatter.write_str("journal has an unterminated tail"),
            Self::RollbackDetected => {
                formatter.write_str("journal rolled behind its live high-water")
            }
            Self::AnchorMissing => formatter.write_str("external journal anchor is missing"),
            Self::AnchorInvalid => {
                formatter.write_str("external journal anchor is invalid or unauthenticated")
            }
            Self::WriterAuthorityUnavailable => {
                formatter.write_str("exclusive native journal writer authority is unavailable")
            }
            Self::WriterAuthorityHeld => {
                formatter.write_str("exclusive native journal writer authority is already held")
            }
            Self::InvalidWriterEpoch => {
                formatter.write_str("native journal writer epoch is invalid or non-monotonic")
            }
            Self::LimitExceeded => formatter.write_str("journal exceeds a hard bound"),
            Self::LockTimeout => formatter.write_str("journal lock timed out"),
            Self::Io(error) => write!(formatter, "journal I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "journal serialization failed: {error}"),
        }
    }
}

impl From<io::Error> for JournalError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for JournalError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Clone)]
pub(crate) struct AssessmentJournal {
    path: PathBuf,
    lock_timeout: Duration,
    high_water: Arc<Mutex<Option<HighWater>>>,
    trust: Arc<AtomicU8>,
    writer: Option<Arc<NativeJournalWriter>>,
}

impl fmt::Debug for AssessmentJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentJournal")
            .field("path", &"<native-app-data>")
            .field("authority_ready", &self.clearance_authority_ready())
            .finish()
    }
}

impl AssessmentJournal {
    /// Recovered journals are never clearance-authoritative without an external native anchor.
    /// B20 owns the only future production path that may establish a trusted first-install epoch.
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
            high_water: Arc::new(Mutex::new(None)),
            trust: Arc::new(AtomicU8::new(TRUST_RECOVERED_UNANCHORED)),
            writer: None,
        }
    }

    /// Establish the only production writer for this journal process epoch. The epoch must come
    /// from scheduler-owned state outside the journal. A non-empty journal requires an exact,
    /// authenticated prior anchor and the immediately following epoch.
    pub(crate) fn open_native_writer(
        path: impl Into<PathBuf>,
        writer_epoch: u64,
    ) -> Result<Self, JournalError> {
        Self::open_native_writer_with(
            path.into(),
            writer_epoch,
            DEFAULT_LOCK_TIMEOUT,
            fill_os_random,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_live_for_test(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let epoch = next_test_writer_epoch(&path).unwrap_or(1);
        Self::open_native_writer_with(path, epoch, DEFAULT_LOCK_TIMEOUT, fill_os_random)
            .expect("test journal writer authority")
    }

    #[cfg(test)]
    fn with_lock_timeout(path: impl Into<PathBuf>, lock_timeout: Duration) -> Self {
        let path = path.into();
        let epoch = next_test_writer_epoch(&path).unwrap_or(1);
        Self::open_native_writer_with(path, epoch, lock_timeout, fill_os_random)
            .expect("test journal writer authority")
    }

    fn open_native_writer_with<F>(
        path: PathBuf,
        writer_epoch: u64,
        lock_timeout: Duration,
        mut fill: F,
    ) -> Result<Self, JournalError>
    where
        F: FnMut(&mut [u8]) -> Result<(), JournalError>,
    {
        if writer_epoch == 0 {
            return Err(JournalError::InvalidWriterEpoch);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let registry_key = canonical_writer_key(&path)?;
        let mut writers = live_writers()
            .lock()
            .map_err(|_| JournalError::WriterAuthorityUnavailable)?;
        writers.retain(|_, writer| writer.strong_count() > 0);
        if let Some(existing) = writers.get(&registry_key).and_then(Weak::upgrade) {
            if existing.epoch != writer_epoch {
                return Err(JournalError::WriterAuthorityHeld);
            }
            return Ok(Self {
                path,
                lock_timeout,
                high_water: Arc::new(Mutex::new(None)),
                trust: Arc::new(AtomicU8::new(TRUST_LIVE_PROCESS)),
                writer: Some(existing),
            });
        }

        let exclusive_writer = JournalLock::acquire(
            &writer_lock_path(&path),
            JOURNAL_WRITER_LOCK_TIMEOUT.min(lock_timeout),
        )
        .map_err(|error| match error {
            JournalError::LockTimeout => JournalError::WriterAuthorityHeld,
            other => other,
        })?;
        let _journal_lock = JournalLock::acquire(&lock_path(&path), lock_timeout)?;
        let mut secret = [0_u8; 32];
        fill(&mut secret)?;
        if secret == [0; 32] {
            secret.fill(0);
            return Err(JournalError::WriterAuthorityUnavailable);
        }
        let signing_key = SigningKey::from_bytes(&secret);
        secret.fill(0);
        let writer = Arc::new(NativeJournalWriter {
            epoch: writer_epoch,
            signing_key,
            append_gate: Mutex::new(()),
            _exclusive_writer: exclusive_writer,
        });

        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        let (events, _) = load_chain(&mut file, false, None)?;
        replay_events(&events)?;
        let file_len = file.metadata()?.len();
        let current = high_water_from(events.last(), file_len);
        let anchor_path = anchor_path(&path);
        match load_anchor(&anchor_path) {
            Ok(previous) => {
                verify_anchor(&previous)?;
                anchor_matches(&previous, &current)?;
                if previous
                    .writer_epoch
                    .checked_add(1)
                    .filter(|next| *next == writer_epoch)
                    .is_none()
                {
                    return Err(JournalError::InvalidWriterEpoch);
                }
            }
            Err(JournalError::AnchorMissing) if current.sequence == 0 && current.file_len == 0 => {}
            Err(error) => return Err(error),
        }
        persist_anchor(&anchor_path, &writer.checkpoint(&current))?;
        writers.insert(registry_key, Arc::downgrade(&writer));
        Ok(Self {
            path,
            lock_timeout,
            high_water: Arc::new(Mutex::new(if current.sequence == 0 {
                None
            } else {
                Some(current)
            })),
            trust: Arc::new(AtomicU8::new(TRUST_LIVE_PROCESS)),
            writer: Some(writer),
        })
    }

    fn append(
        &self,
        recorded_at_ms: u64,
        payload: JournalPayload,
    ) -> Result<JournalEvent, JournalError> {
        self.append_internal(recorded_at_ms, payload, false)
    }

    fn append_idempotent(
        &self,
        recorded_at_ms: u64,
        payload: JournalPayload,
    ) -> Result<JournalEvent, JournalError> {
        self.append_internal(recorded_at_ms, payload, true)
    }

    fn append_internal(
        &self,
        recorded_at_ms: u64,
        payload: JournalPayload,
        idempotent: bool,
    ) -> Result<JournalEvent, JournalError> {
        let writer = self
            .writer
            .as_ref()
            .ok_or(JournalError::WriterAuthorityUnavailable)?;
        let _writer_gate = writer
            .append_gate
            .lock()
            .map_err(|_| JournalError::WriterAuthorityUnavailable)?;
        if recorded_at_ms == 0 {
            return Err(JournalError::InvalidEvent);
        }
        validate_payload(&payload)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock_path = lock_path(&self.path);
        let _lock = JournalLock::acquire(&lock_path, self.lock_timeout)?;
        let anchor = load_anchor(&anchor_path(&self.path))?;
        verify_writer_anchor(&anchor, writer)?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.path)?;
        let memory_protected_len = self
            .high_water
            .lock()
            .map_err(|_| JournalError::RollbackDetected)?
            .as_ref()
            .map(|water| water.file_len);
        let protected_len = Some(
            memory_protected_len.map_or(anchor.file_len, |memory| memory.max(anchor.file_len)),
        );
        let (events, repaired) = load_chain(&mut file, true, protected_len)?;
        if repaired {
            file.sync_all()?;
        }
        let file_len = file.metadata()?.len();
        anchor_matches(&anchor, &high_water_from(events.last(), file_len))?;
        self.enforce_high_water(events.last(), file_len)?;
        let last = events.last().cloned();
        let mut replay = replay_events(&events)?;
        if idempotent {
            if let Some(existing) = find_idempotent_event(&events, &payload)? {
                replay.validate_idempotent_retry(&payload, recorded_at_ms)?;
                return Ok(existing.clone());
            }
        }
        if last
            .as_ref()
            .is_some_and(|event| event.sequence as usize >= MAX_EVENTS)
            || file_len >= MAX_JOURNAL_BYTES
        {
            return Err(JournalError::LimitExceeded);
        }
        let sequence = last
            .as_ref()
            .map_or(1, |event| event.sequence.saturating_add(1));
        if sequence == 0 {
            return Err(JournalError::LimitExceeded);
        }
        let previous_hash = last.as_ref().map_or_else(
            || GENESIS_HASH.to_string(),
            |event| event.event_hash.clone(),
        );
        let mut event = JournalEvent {
            schema_version: JOURNAL_SCHEMA_VERSION,
            sequence,
            previous_hash,
            recorded_at_ms,
            payload,
            event_hash: String::new(),
        };
        event.event_hash = event_hash(&event)?;
        validate_event(&event, last.as_ref())?;
        replay.apply(&event.payload, event.recorded_at_ms)?;
        let mut encoded = serde_json::to_vec(&event)?;
        encoded.push(b'\n');
        let is_ticket_event = matches!(
            &event.payload,
            JournalPayload::ConflictTicket { .. }
                | JournalPayload::ConflictTicketSignal { .. }
                | JournalPayload::ConflictTicketAcknowledged { .. }
        );
        if encoded.len() > MAX_LINE_BYTES
            || (is_ticket_event && encoded.len() > MAX_TICKET_LINE_BYTES)
            || file_len
                .checked_add(encoded.len() as u64)
                .is_none_or(|size| size > MAX_JOURNAL_BYTES)
        {
            return Err(JournalError::LimitExceeded);
        }
        file.seek(SeekFrom::End(0))?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        let end = file_len + encoded.len() as u64;
        persist_anchor(
            &anchor_path(&self.path),
            &writer.checkpoint(&HighWater {
                sequence: event.sequence,
                event_hash: event.event_hash.clone(),
                file_len: end,
            }),
        )?;
        self.set_high_water(&event, end)?;
        Ok(event)
    }

    pub(crate) fn read_verified(&self) -> Result<Vec<JournalEvent>, JournalError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let _lock = JournalLock::acquire(&lock_path(&self.path), self.lock_timeout)?;
        let anchor = load_anchor(&anchor_path(&self.path))?;
        verify_anchor(&anchor)?;
        if let Some(writer) = &self.writer {
            verify_writer_anchor(&anchor, writer)?;
        }
        let mut file = match OpenOptions::new().read(true).open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(JournalError::RollbackDetected)
            }
            Err(error) => return Err(error.into()),
        };
        let (events, _) = load_chain(&mut file, false, None)?;
        let file_len = file.metadata()?.len();
        anchor_matches(&anchor, &high_water_from(events.last(), file_len))?;
        self.enforce_high_water(events.last(), file_len)?;
        replay_events(&events)?;
        Ok(events)
    }

    pub(crate) fn clearance_authority_ready(&self) -> bool {
        self.trust.load(Ordering::Acquire) == TRUST_LIVE_PROCESS
    }

    pub(crate) fn record_assessment(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        receipt: &StoredSnapshotReceipt,
        recorded_at_ms: u64,
    ) -> Result<JournalEvent, JournalError> {
        self.append(recorded_at_ms, assessment_payload(snapshot, receipt)?)
    }

    pub(crate) fn assessment_is_live(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        receipt: &StoredSnapshotReceipt,
    ) -> Result<bool, JournalError> {
        if !receipt.matches(snapshot) {
            return Ok(false);
        }
        let expected = assessment_payload(snapshot, receipt)?;
        let events = self.read_verified()?;
        let found = events.iter().any(|event| event.payload == expected);
        let replay = replay_events(&events)?;
        Ok(found
            && replay
                .assessments
                .get(snapshot.snapshot_hash())
                .is_some_and(|assessment| assessment.live))
    }

    /// Confirms the exact immutable snapshot/store receipt was journaled, regardless of its
    /// current liveness. This is read/audit recovery authority only; mutation replay still enforces
    /// live assessment state for every new ticket or signal.
    pub(crate) fn assessment_was_recorded(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        receipt: &StoredSnapshotReceipt,
    ) -> Result<bool, JournalError> {
        if !receipt.matches(snapshot) {
            return Ok(false);
        }
        let expected = assessment_payload(snapshot, receipt)?;
        Ok(self
            .read_verified()?
            .iter()
            .any(|event| event.payload == expected))
    }

    #[allow(dead_code)]
    pub(super) fn conflict_ticket_was_recorded(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        proof: &VerifiedConflictProof,
    ) -> Result<bool, JournalError> {
        let conflict = proof.conflict();
        let ticket_id = conflict_ticket_id(
            snapshot.snapshot_hash(),
            proof.commitment_root(),
            &conflict.conflict_id,
        );
        Ok(self.read_verified()?.iter().any(|event| {
            matches!(
                &event.payload,
                JournalPayload::ConflictTicket {
                    snapshot_hash,
                    ticket_id: recorded_ticket_id,
                    conflict_id,
                    conflict: recorded_conflict,
                    leaf_index,
                    leaf_count,
                    proof: recorded_proof,
                    commitment_root,
                } if snapshot_hash == snapshot.snapshot_hash()
                    && recorded_ticket_id == &ticket_id
                    && conflict_id == &conflict.conflict_id
                    && recorded_conflict == conflict
                    && *leaf_index == proof.leaf_index()
                    && *leaf_count == proof.leaf_count()
                    && recorded_proof == proof.steps()
                    && commitment_root == proof.commitment_root()
            )
        }))
    }

    #[allow(dead_code)]
    pub(super) fn record_conflict_ticket(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        receipt: &StoredSnapshotReceipt,
        proof: &VerifiedConflictProof,
        recorded_at_ms: u64,
    ) -> Result<JournalEvent, JournalError> {
        let conflict = proof.conflict();
        if !receipt.matches(snapshot)
            || recorded_at_ms == 0
            || recorded_at_ms >= snapshot.expires_at_ms()
            || proof.commitment_root() != snapshot.conflict_commitment_root()
            || proof.leaf_count() as usize != snapshot.conflicts().len()
            || snapshot
                .conflicts()
                .binary_search_by(|item| item.conflict_id.cmp(&conflict.conflict_id))
                .ok()
                .and_then(|index| snapshot.conflicts().get(index))
                != Some(conflict)
            || !self.assessment_is_live(snapshot, receipt)?
        {
            return Err(JournalError::InvalidEvent);
        }
        let ticket_id = conflict_ticket_id(
            snapshot.snapshot_hash(),
            proof.commitment_root(),
            &conflict.conflict_id,
        );
        self.append_idempotent(
            recorded_at_ms,
            JournalPayload::ConflictTicket {
                snapshot_hash: snapshot.snapshot_hash().to_string(),
                ticket_id,
                conflict_id: conflict.conflict_id.clone(),
                conflict: conflict.clone(),
                leaf_index: proof.leaf_index(),
                leaf_count: proof.leaf_count(),
                proof: proof.steps().to_vec(),
                commitment_root: proof.commitment_root().to_string(),
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn record_conflict_ticket_signal(
        &self,
        recorded_at_ms: u64,
        snapshot_hash: String,
        actor_participant_id: String,
        source_node_id: String,
        kind: TicketSignalKind,
        source_state_digest: String,
        source_event_id: String,
    ) -> Result<JournalEvent, JournalError> {
        let signal_id = conflict_ticket_signal_id(
            &snapshot_hash,
            &actor_participant_id,
            &source_node_id,
            kind,
            &source_event_id,
        );
        self.append_idempotent(
            recorded_at_ms,
            JournalPayload::ConflictTicketSignal {
                snapshot_hash,
                signal_id,
                actor_participant_id,
                source_node_id,
                signal_kind: kind,
                source_state_digest,
                source_event_id,
            },
        )
    }

    pub(super) fn record_conflict_ticket_acknowledgement(
        &self,
        recorded_at_ms: u64,
        snapshot_hash: String,
        signal_id: String,
        recipient_participant_id: String,
        acknowledgement_digest: String,
    ) -> Result<JournalEvent, JournalError> {
        self.append_idempotent(
            recorded_at_ms,
            JournalPayload::ConflictTicketAcknowledged {
                snapshot_hash,
                signal_id,
                recipient_participant_id,
                acknowledgement_digest,
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_clearance_issued(
        &self,
        recorded_at_ms: u64,
        clearance_id_hash: String,
        snapshot: &VerifiedAssessmentSnapshot,
        receipt: &StoredSnapshotReceipt,
        participant_id: String,
        issuer_epoch: u64,
        expires_at_ms: u64,
    ) -> Result<JournalEvent, JournalError> {
        if !self.clearance_authority_ready()
            || !receipt.matches(snapshot)
            || snapshot.verdict() != AssessmentVerdict::Clear
            || !snapshot.conflicts().is_empty()
            || !snapshot
                .participants()
                .iter()
                .any(|participant| participant.participant_id == participant_id)
            || expires_at_ms > snapshot.expires_at_ms()
            || expires_at_ms <= recorded_at_ms
        {
            return Err(JournalError::InvalidEvent);
        }
        self.append(
            recorded_at_ms,
            JournalPayload::ClearanceIssued {
                clearance_id_hash,
                snapshot_hash: snapshot.snapshot_hash().to_string(),
                participant_id,
                issuer_epoch,
                expires_at_ms,
            },
        )
    }

    pub(crate) fn record_revocation(
        &self,
        recorded_at_ms: u64,
        snapshot_hash: String,
        reason_digest: String,
    ) -> Result<JournalEvent, JournalError> {
        self.append(
            recorded_at_ms,
            JournalPayload::Revocation {
                snapshot_hash,
                reason_digest,
            },
        )
    }

    pub(crate) fn record_clearance_consuming(
        &self,
        recorded_at_ms: u64,
        clearance_id_hash: String,
        snapshot_hash: String,
        participant_id: String,
        issuer_epoch: u64,
    ) -> Result<JournalEvent, JournalError> {
        if !self.clearance_authority_ready() {
            return Err(JournalError::InvalidEvent);
        }
        self.append(
            recorded_at_ms,
            JournalPayload::ClearanceConsuming {
                clearance_id_hash,
                snapshot_hash,
                participant_id,
                issuer_epoch,
            },
        )
    }

    #[allow(dead_code)]
    pub(crate) fn record_clearance_consumed(
        &self,
        recorded_at_ms: u64,
        clearance_id_hash: String,
        snapshot_hash: String,
        participant_id: String,
        issuer_epoch: u64,
        executor_receipt_digest: String,
    ) -> Result<JournalEvent, JournalError> {
        if !self.clearance_authority_ready() {
            return Err(JournalError::InvalidEvent);
        }
        self.append(
            recorded_at_ms,
            JournalPayload::ClearanceConsumed {
                clearance_id_hash,
                snapshot_hash,
                participant_id,
                issuer_epoch,
                executor_receipt_digest,
            },
        )
    }

    pub(crate) fn record_clearance_revoked(
        &self,
        recorded_at_ms: u64,
        clearance_id_hash: String,
        snapshot_hash: String,
        participant_id: String,
        issuer_epoch: u64,
        reason_digest: String,
    ) -> Result<JournalEvent, JournalError> {
        self.append(
            recorded_at_ms,
            JournalPayload::ClearanceRevoked {
                clearance_id_hash,
                snapshot_hash,
                participant_id,
                issuer_epoch,
                reason_digest,
            },
        )
    }

    fn enforce_high_water(
        &self,
        last: Option<&JournalEvent>,
        file_len: u64,
    ) -> Result<(), JournalError> {
        let mut high_water = self
            .high_water
            .lock()
            .map_err(|_| JournalError::RollbackDetected)?;
        if let Some(expected) = high_water.as_ref() {
            let actual_sequence = last.map_or(0, |event| event.sequence);
            let actual_hash = last.map_or(GENESIS_HASH, |event| event.event_hash.as_str());
            if actual_sequence < expected.sequence
                || (actual_sequence == expected.sequence && actual_hash != expected.event_hash)
                || file_len < expected.file_len
            {
                return Err(JournalError::RollbackDetected);
            }
        }
        if let Some(event) = last {
            *high_water = Some(HighWater {
                sequence: event.sequence,
                event_hash: event.event_hash.clone(),
                file_len,
            });
        }
        Ok(())
    }

    fn set_high_water(&self, event: &JournalEvent, file_len: u64) -> Result<(), JournalError> {
        let mut high_water = self
            .high_water
            .lock()
            .map_err(|_| JournalError::RollbackDetected)?;
        *high_water = Some(HighWater {
            sequence: event.sequence,
            event_hash: event.event_hash.clone(),
            file_len,
        });
        Ok(())
    }
}

fn load_chain(
    file: &mut File,
    repair_torn_tail: bool,
    protected_len: Option<u64>,
) -> Result<(Vec<JournalEvent>, bool), JournalError> {
    let size = file.metadata()?.len();
    if size > MAX_JOURNAL_BYTES {
        return Err(JournalError::LimitExceeded);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_JOURNAL_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(JournalError::LimitExceeded);
    }
    let mut repaired = false;
    if !bytes.is_empty() && bytes.last() != Some(&b'\n') {
        let tail_start = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        let tail = &bytes[tail_start..];
        let prefix_events = parse_complete_lines(&bytes[..tail_start])?;
        let parsed_tail = serde_json::from_slice::<JournalEvent>(tail)
            .ok()
            .filter(|event| validate_event(event, prefix_events.last()).is_ok());
        if !repair_torn_tail
            || protected_len.is_none_or(|committed| (tail_start as u64) < committed)
        {
            return Err(JournalError::TornTail);
        }
        if parsed_tail.is_some() {
            file.seek(SeekFrom::End(0))?;
            file.write_all(b"\n")?;
            bytes.push(b'\n');
        } else {
            file.set_len(tail_start as u64)?;
            file.seek(SeekFrom::Start(tail_start as u64))?;
            bytes.truncate(tail_start);
        }
        repaired = true;
    }
    Ok((parse_complete_lines(&bytes)?, repaired))
}

fn parse_complete_lines(bytes: &[u8]) -> Result<Vec<JournalEvent>, JournalError> {
    let mut events = Vec::new();
    let mut replay = ReplayState::default();
    let lines = bytes.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if line.is_empty() {
            if index + 1 != lines.len() {
                return Err(JournalError::CorruptChain);
            }
            continue;
        }
        if line.len() > MAX_LINE_BYTES || events.len() >= MAX_EVENTS {
            return Err(JournalError::LimitExceeded);
        }
        let event: JournalEvent =
            serde_json::from_slice(line).map_err(|_| JournalError::CorruptChain)?;
        validate_event(&event, events.last())?;
        replay
            .apply(&event.payload, event.recorded_at_ms)
            .map_err(|_| JournalError::CorruptChain)?;
        events.push(event);
    }
    Ok(events)
}

fn replay_events(events: &[JournalEvent]) -> Result<ReplayState, JournalError> {
    let mut state = ReplayState::default();
    for event in events {
        state.apply(&event.payload, event.recorded_at_ms)?;
    }
    Ok(state)
}

fn find_idempotent_event<'a>(
    events: &'a [JournalEvent],
    candidate: &JournalPayload,
) -> Result<Option<&'a JournalEvent>, JournalError> {
    for event in events {
        let same_slot = match (&event.payload, candidate) {
            (
                JournalPayload::ConflictTicket {
                    ticket_id: left, ..
                },
                JournalPayload::ConflictTicket {
                    ticket_id: right, ..
                },
            ) => left == right,
            (
                JournalPayload::ConflictTicketSignal {
                    signal_id: left, ..
                },
                JournalPayload::ConflictTicketSignal {
                    signal_id: right, ..
                },
            ) => left == right,
            (
                JournalPayload::ConflictTicketAcknowledged {
                    signal_id: left_signal,
                    recipient_participant_id: left_recipient,
                    ..
                },
                JournalPayload::ConflictTicketAcknowledged {
                    signal_id: right_signal,
                    recipient_participant_id: right_recipient,
                    ..
                },
            ) => left_signal == right_signal && left_recipient == right_recipient,
            _ => false,
        };
        if same_slot {
            return if event.payload == *candidate {
                Ok(Some(event))
            } else {
                Err(JournalError::InvalidEvent)
            };
        }
    }
    Ok(None)
}

fn validate_event(
    event: &JournalEvent,
    previous: Option<&JournalEvent>,
) -> Result<(), JournalError> {
    let expected_sequence = previous.map_or(1, |prior| prior.sequence.saturating_add(1));
    let expected_previous = previous.map_or(GENESIS_HASH, |prior| prior.event_hash.as_str());
    if event.schema_version != JOURNAL_SCHEMA_VERSION
        || event.sequence != expected_sequence
        || event.sequence == 0
        || event.previous_hash != expected_previous
        || event.recorded_at_ms == 0
        || !is_sha256(&event.previous_hash)
        || !is_sha256(&event.event_hash)
        || event_hash(event)? != event.event_hash
    {
        return Err(JournalError::CorruptChain);
    }
    validate_payload(&event.payload)
}

fn validate_payload(payload: &JournalPayload) -> Result<(), JournalError> {
    let valid = match payload {
        JournalPayload::Assessment {
            snapshot_hash,
            registry_generation,
            census_input_digest,
            verdict,
            participant_count,
            participant_node_bindings_packed,
            participant_conflict_edges_packed,
            conflict_count,
            conflict_commitment_root,
            captured_at_ms,
            expires_at_ms,
            encoded_bytes,
            store_binding,
        } => {
            let bindings = unpack_participant_node_bindings(
                participant_node_bindings_packed,
                *participant_count,
            );
            let edges = unpack_participant_conflict_edges(
                participant_conflict_edges_packed,
                *participant_count,
                *conflict_count,
            );
            is_sha256(snapshot_hash)
                && *registry_generation > 0
                && is_sha256(census_input_digest)
                && *participant_count > 0
                && bindings.is_ok()
                && edges.is_ok()
                && (*conflict_count == 0
                    || (*participant_count >= 2
                        && edges.as_ref().is_ok_and(|value| !value.is_empty())))
                && (*verdict != JournalVerdict::Clear || *conflict_count == 0)
                && is_sha256(conflict_commitment_root)
                && *captured_at_ms > 0
                && *expires_at_ms > *captured_at_ms
                && *encoded_bytes > 0
                && is_sha256(store_binding)
        }
        JournalPayload::Revocation {
            snapshot_hash,
            reason_digest,
        } => is_sha256(snapshot_hash) && is_sha256(reason_digest),
        JournalPayload::ConflictTicket {
            snapshot_hash,
            ticket_id,
            conflict_id,
            conflict,
            leaf_index,
            leaf_count,
            proof,
            commitment_root,
        } => {
            is_sha256(snapshot_hash)
                && is_sha256(ticket_id)
                && is_sha256(conflict_id)
                && *conflict_id == conflict.conflict_id
                && is_sha256(&conflict.left_participant_id)
                && is_sha256(&conflict.right_participant_id)
                && is_sha256(&conflict.left_claim_id)
                && is_sha256(&conflict.right_claim_id)
                && conflict.left_participant_id < conflict.right_participant_id
                && conflict.disposition.is_some()
                && *leaf_count > 0
                && *leaf_index < *leaf_count
                && *leaf_count <= 8_192
                && proof.len() <= 13
                && proof.iter().all(|step| is_sha256(&step.sibling_hash))
                && is_sha256(commitment_root)
        }
        JournalPayload::ConflictTicketSignal {
            snapshot_hash,
            signal_id,
            actor_participant_id,
            source_node_id,
            source_state_digest,
            source_event_id,
            ..
        } => {
            is_sha256(snapshot_hash)
                && is_sha256(signal_id)
                && is_sha256(actor_participant_id)
                && is_bounded_id(source_node_id)
                && is_sha256(source_state_digest)
                && is_sha256(source_event_id)
        }
        JournalPayload::ConflictTicketAcknowledged {
            snapshot_hash,
            signal_id,
            recipient_participant_id,
            acknowledgement_digest,
        } => {
            is_sha256(snapshot_hash)
                && is_sha256(signal_id)
                && is_sha256(recipient_participant_id)
                && is_sha256(acknowledgement_digest)
        }
        JournalPayload::ClearanceIssued {
            clearance_id_hash,
            snapshot_hash,
            participant_id,
            issuer_epoch,
            expires_at_ms,
        } => {
            is_sha256(clearance_id_hash)
                && is_sha256(snapshot_hash)
                && is_sha256(participant_id)
                && *issuer_epoch > 0
                && *expires_at_ms > 0
        }
        JournalPayload::ClearanceConsuming {
            clearance_id_hash,
            snapshot_hash,
            participant_id,
            issuer_epoch,
        } => {
            is_sha256(clearance_id_hash)
                && is_sha256(snapshot_hash)
                && is_sha256(participant_id)
                && *issuer_epoch > 0
        }
        JournalPayload::ClearanceConsumed {
            clearance_id_hash,
            snapshot_hash,
            participant_id,
            issuer_epoch,
            executor_receipt_digest,
        } => {
            is_sha256(clearance_id_hash)
                && is_sha256(snapshot_hash)
                && is_sha256(participant_id)
                && *issuer_epoch > 0
                && is_sha256(executor_receipt_digest)
        }
        JournalPayload::ClearanceRevoked {
            clearance_id_hash,
            snapshot_hash,
            participant_id,
            issuer_epoch,
            reason_digest,
        } => {
            is_sha256(clearance_id_hash)
                && is_sha256(snapshot_hash)
                && is_sha256(participant_id)
                && *issuer_epoch > 0
                && is_sha256(reason_digest)
        }
    };
    if valid {
        Ok(())
    } else {
        Err(JournalError::InvalidEvent)
    }
}

impl From<AssessmentVerdict> for JournalVerdict {
    fn from(value: AssessmentVerdict) -> Self {
        match value {
            AssessmentVerdict::Clear => Self::Clear,
            AssessmentVerdict::Wait => Self::Wait,
            AssessmentVerdict::Replan => Self::Replan,
            AssessmentVerdict::UserDecision => Self::UserDecision,
            AssessmentVerdict::Unknown => Self::Unknown,
        }
    }
}

fn event_hash(event: &JournalEvent) -> Result<String, JournalError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct HashPayload<'a> {
        schema_version: u32,
        sequence: u64,
        previous_hash: &'a str,
        recorded_at_ms: u64,
        payload: &'a JournalPayload,
    }
    let bytes = serde_json::to_vec(&HashPayload {
        schema_version: event.schema_version,
        sequence: event.sequence,
        previous_hash: &event.previous_hash,
        recorded_at_ms: event.recorded_at_ms,
        payload: &event.payload,
    })?;
    let mut digest = Sha256::new();
    digest.update(b"perfect-planner:assessment-journal-event:v1");
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
    Ok(format!("{:x}", digest.finalize()))
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

fn high_water_from(last: Option<&JournalEvent>, file_len: u64) -> HighWater {
    HighWater {
        sequence: last.map_or(0, |event| event.sequence),
        event_hash: last.map_or_else(
            || GENESIS_HASH.to_string(),
            |event| event.event_hash.clone(),
        ),
        file_len,
    }
}

fn anchor_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "assessment.jsonl".into(), |value| value.to_os_string());
    name.push(".anchor.json");
    path.with_file_name(name)
}

fn writer_lock_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "assessment.jsonl".into(), |value| value.to_os_string());
    name.push(".writer.lock");
    path.with_file_name(name)
}

fn canonical_writer_key(path: &Path) -> Result<PathBuf, JournalError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let canonical_parent = fs::canonicalize(parent)?;
    Ok(canonical_parent.join(
        path.file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("assessment.jsonl")),
    ))
}

fn load_anchor(path: &Path) -> Result<JournalAnchor, JournalError> {
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(JournalError::AnchorMissing)
        }
        Err(error) => return Err(error.into()),
    };
    if file.metadata()?.len() > MAX_ANCHOR_BYTES {
        return Err(JournalError::AnchorInvalid);
    }
    let mut bytes = Vec::new();
    file.take(MAX_ANCHOR_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_ANCHOR_BYTES {
        return Err(JournalError::AnchorInvalid);
    }
    serde_json::from_slice(&bytes).map_err(|_| JournalError::AnchorInvalid)
}

fn verify_anchor(anchor: &JournalAnchor) -> Result<(), JournalError> {
    if anchor.version != JOURNAL_ANCHOR_VERSION
        || anchor.writer_epoch == 0
        || !is_sha256(&anchor.event_hash)
        || !is_sha256(&anchor.key_fingerprint)
        || anchor.verifying_key.len() != 64
        || anchor.checkpoint_signature.len() != 128
        || (anchor.sequence == 0 && (anchor.event_hash != GENESIS_HASH || anchor.file_len != 0))
        || (anchor.sequence > 0 && anchor.file_len == 0)
    {
        return Err(JournalError::AnchorInvalid);
    }
    let verifying_key = decode_journal_hex::<32>(&anchor.verifying_key)?;
    if journal_key_fingerprint(&verifying_key) != anchor.key_fingerprint {
        return Err(JournalError::AnchorInvalid);
    }
    let signature = decode_journal_hex::<64>(&anchor.checkpoint_signature)?;
    let verifier =
        VerifyingKey::from_bytes(&verifying_key).map_err(|_| JournalError::AnchorInvalid)?;
    verifier
        .verify_strict(
            &journal_anchor_message(anchor),
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| JournalError::AnchorInvalid)
}

fn verify_writer_anchor(
    anchor: &JournalAnchor,
    writer: &NativeJournalWriter,
) -> Result<(), JournalError> {
    verify_anchor(anchor)?;
    if anchor.writer_epoch != writer.epoch
        || anchor.verifying_key != encode_hex(&writer.verification_key())
        || anchor.key_fingerprint != writer.fingerprint()
    {
        return Err(JournalError::InvalidWriterEpoch);
    }
    Ok(())
}

fn anchor_matches(anchor: &JournalAnchor, current: &HighWater) -> Result<(), JournalError> {
    if anchor.sequence != current.sequence
        || anchor.event_hash != current.event_hash
        || anchor.file_len != current.file_len
    {
        return Err(JournalError::RollbackDetected);
    }
    Ok(())
}

fn journal_anchor_message(anchor: &JournalAnchor) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"perfect-planner:assessment-journal-anchor:v1");
    digest.update(anchor.version.to_le_bytes());
    digest.update(anchor.writer_epoch.to_le_bytes());
    digest.update(anchor.sequence.to_le_bytes());
    digest.update((anchor.event_hash.len() as u64).to_le_bytes());
    digest.update(anchor.event_hash.as_bytes());
    digest.update(anchor.file_len.to_le_bytes());
    digest.update((anchor.verifying_key.len() as u64).to_le_bytes());
    digest.update(anchor.verifying_key.as_bytes());
    digest.update((anchor.key_fingerprint.len() as u64).to_le_bytes());
    digest.update(anchor.key_fingerprint.as_bytes());
    digest.finalize().into()
}

fn journal_key_fingerprint(verifying_key: &[u8; 32]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"perfect-planner:assessment-journal-writer-key:v1");
    digest.update(verifying_key);
    format!("{:x}", digest.finalize())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn decode_journal_hex<const N: usize>(value: &str) -> Result<[u8; N], JournalError> {
    if value.len() != N * 2 {
        return Err(JournalError::AnchorInvalid);
    }
    let mut output = [0_u8; N];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = (chunk[0] as char)
            .to_digit(16)
            .ok_or(JournalError::AnchorInvalid)? as u8;
        let low = (chunk[1] as char)
            .to_digit(16)
            .ok_or(JournalError::AnchorInvalid)? as u8;
        output[index] = (high << 4) | low;
    }
    Ok(output)
}

fn persist_anchor(path: &Path, anchor: &JournalAnchor) -> Result<(), JournalError> {
    let mut bytes = serde_json::to_vec(anchor)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_ANCHOR_BYTES {
        return Err(JournalError::LimitExceeded);
    }
    let mut temp_name = path
        .file_name()
        .map_or_else(|| "assessment.anchor".into(), |value| value.to_os_string());
    temp_name.push(".tmp");
    let temp_path = path.with_file_name(temp_name);
    let mut temp = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp_path)?;
    temp.write_all(&bytes)?;
    temp.sync_all()?;
    drop(temp);
    replace_anchor(&temp_path, path)?;
    Ok(())
}

#[cfg(windows)]
fn replace_anchor(source: &Path, destination: &Path) -> Result<(), JournalError> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new_name: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
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
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_anchor(source: &Path, destination: &Path) -> Result<(), JournalError> {
    fs::rename(source, destination)?;
    if let Some(parent) = destination.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(windows)]
fn fill_os_random(bytes: &mut [u8]) -> Result<(), JournalError> {
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    #[link(name = "Bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> i32;
    }
    let length = u32::try_from(bytes.len()).map_err(|_| JournalError::LimitExceeded)?;
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            length,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status >= 0 {
        Ok(())
    } else {
        Err(JournalError::WriterAuthorityUnavailable)
    }
}

#[cfg(unix)]
fn fill_os_random(bytes: &mut [u8]) -> Result<(), JournalError> {
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(bytes))
        .map_err(JournalError::Io)
}

#[cfg(not(any(windows, unix)))]
fn fill_os_random(_bytes: &mut [u8]) -> Result<(), JournalError> {
    Err(JournalError::WriterAuthorityUnavailable)
}

#[cfg(test)]
fn next_test_writer_epoch(path: &Path) -> Result<u64, JournalError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let key = canonical_writer_key(path)?;
    if let Some(writer) = live_writers()
        .lock()
        .map_err(|_| JournalError::WriterAuthorityUnavailable)?
        .get(&key)
        .and_then(Weak::upgrade)
    {
        return Ok(writer.epoch);
    }
    match load_anchor(&anchor_path(path)) {
        Ok(anchor) => anchor
            .writer_epoch
            .checked_add(1)
            .ok_or(JournalError::InvalidWriterEpoch),
        Err(JournalError::AnchorMissing) => Ok(1),
        Err(error) => Err(error),
    }
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "assessment.jsonl".into(), |value| value.to_os_string());
    name.push(".lock");
    path.with_file_name(name)
}

#[cfg(windows)]
struct JournalLock {
    // HANDLE is pointer-sized but carries no Rust provenance. Storing the owned value as `isize`
    // lets the guard move with its Arc owner without asserting blanket Send/Sync for raw pointers.
    handle: isize,
}

#[cfg(windows)]
impl JournalLock {
    fn acquire(path: &Path, timeout: Duration) -> Result<Self, JournalError> {
        use std::os::windows::ffi::OsStrExt;
        use std::ptr;

        #[link(name = "kernel32")]
        extern "system" {
            fn CreateFileW(
                name: *const u16,
                access: u32,
                share_mode: u32,
                security: *mut std::ffi::c_void,
                creation: u32,
                flags: u32,
                template: *mut std::ffi::c_void,
            ) -> *mut std::ffi::c_void;
        }
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const OPEN_ALWAYS: u32 = 4;
        const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
        const INVALID_HANDLE_VALUE: *mut std::ffi::c_void = -1_isize as *mut std::ffi::c_void;

        let encoded = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let started = Instant::now();
        let mut attempt = 0usize;
        loop {
            let handle = unsafe {
                CreateFileW(
                    encoded.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    ptr::null_mut(),
                    OPEN_ALWAYS,
                    FILE_ATTRIBUTE_NORMAL,
                    ptr::null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                return Ok(Self {
                    handle: handle as isize,
                });
            }
            let error = io::Error::last_os_error();
            if !matches!(error.raw_os_error(), Some(5 | 32 | 33)) {
                return Err(error.into());
            }
            if started.elapsed() >= timeout {
                return Err(JournalError::LockTimeout);
            }
            thread::sleep(retry_delay(attempt));
            attempt = attempt.saturating_add(1);
        }
    }
}

#[cfg(windows)]
impl Drop for JournalLock {
    fn drop(&mut self) {
        #[link(name = "kernel32")]
        extern "system" {
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        unsafe {
            CloseHandle(self.handle as *mut std::ffi::c_void);
        }
    }
}

#[cfg(not(windows))]
struct JournalLock {
    path: PathBuf,
}

#[cfg(not(windows))]
impl JournalLock {
    fn acquire(path: &Path, timeout: Duration) -> Result<Self, JournalError> {
        let started = Instant::now();
        let mut attempt = 0usize;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    file.write_all(b"locked\n")?;
                    file.sync_all()?;
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if started.elapsed() >= timeout {
                        return Err(JournalError::LockTimeout);
                    }
                    thread::sleep(retry_delay(attempt));
                    attempt = attempt.saturating_add(1);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

#[cfg(not(windows))]
impl Drop for JournalLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn retry_delay(attempt: usize) -> Duration {
    const DELAYS_MS: [u64; 6] = [2, 4, 8, 16, 25, 40];
    Duration::from_millis(DELAYS_MS[attempt.min(DELAYS_MS.len() - 1)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_journal(name: &str) -> AssessmentJournal {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        AssessmentJournal::new_live_for_test(std::env::temp_dir().join(format!(
            "perfect-planner-journal-{name}-{}-{nonce}/assessment.jsonl",
            std::process::id()
        )))
    }

    fn assessment(index: usize) -> JournalPayload {
        let mut participant_binding = Vec::with_capacity(64);
        participant_binding.extend_from_slice(&decode_sha256_hex(&"d".repeat(64)).unwrap());
        participant_binding
            .extend_from_slice(&decode_sha256_hex(&source_node_digest("node-a")).unwrap());
        JournalPayload::Assessment {
            snapshot_hash: format!("{index:064x}"),
            registry_generation: 7,
            census_input_digest: "a".repeat(64),
            verdict: JournalVerdict::Clear,
            participant_count: 1,
            participant_node_bindings_packed: BASE64_STANDARD.encode(participant_binding),
            participant_conflict_edges_packed: String::new(),
            conflict_count: 0,
            conflict_commitment_root: "c".repeat(64),
            captured_at_ms: 900,
            expires_at_ms: 5_000,
            encoded_bytes: 512,
            store_binding: "b".repeat(64),
        }
    }

    fn issued(clearance: &str, snapshot: &str, participant: &str) -> JournalPayload {
        JournalPayload::ClearanceIssued {
            clearance_id_hash: clearance.to_string(),
            snapshot_hash: snapshot.to_string(),
            participant_id: participant.to_string(),
            issuer_epoch: 7,
            expires_at_ms: 5_000,
        }
    }

    #[test]
    fn maximum_participant_and_unique_edge_assessment_fits_and_restarts() {
        let snapshot = super::super::snapshot::tests::fixture_maximum_unique_edge_snapshot();
        assert_eq!(snapshot.participants().len(), 4_096);
        assert_eq!(snapshot.conflicts().len(), 8_192);
        let journal = temp_journal("maximum-packed-assessment");
        let root = journal.path.parent().unwrap().to_path_buf();
        let store = super::super::snapshot::SnapshotStore::new_for_test(root.join("snapshots"));
        let receipt = store.persist(&snapshot).unwrap();

        journal
            .record_assessment(&snapshot, &receipt, 1_000)
            .unwrap();
        let line_bytes = fs::metadata(&journal.path).unwrap().len();
        assert!(line_bytes < MAX_LINE_BYTES as u64, "{line_bytes}");
        let events = journal.read_verified().unwrap();
        assert_eq!(events.len(), 1);
        let JournalPayload::Assessment {
            participant_count,
            participant_node_bindings_packed,
            participant_conflict_edges_packed,
            conflict_count,
            ..
        } = &events[0].payload
        else {
            panic!("expected assessment");
        };
        assert_eq!(*participant_count, 4_096);
        assert_eq!(*conflict_count, 8_192);
        assert_eq!(
            unpack_participant_node_bindings(participant_node_bindings_packed, *participant_count,)
                .unwrap()
                .len(),
            4_096
        );
        assert_eq!(
            unpack_participant_conflict_edges(
                participant_conflict_edges_packed,
                *participant_count,
                *conflict_count,
            )
            .unwrap()
            .len(),
            8_192
        );

        let restarted = AssessmentJournal::new(&journal.path);
        assert!(restarted
            .assessment_was_recorded(&snapshot, &receipt)
            .unwrap());
        let _ = fs::remove_dir_all(root);
    }

    fn consuming(clearance: &str, snapshot: &str, participant: &str) -> JournalPayload {
        JournalPayload::ClearanceConsuming {
            clearance_id_hash: clearance.to_string(),
            snapshot_hash: snapshot.to_string(),
            participant_id: participant.to_string(),
            issuer_epoch: 7,
        }
    }

    #[test]
    fn concurrent_appenders_produce_one_complete_hash_chain() {
        let journal = Arc::new(temp_journal("concurrent"));
        let workers = (1..=100)
            .map(|index| {
                let journal = Arc::clone(&journal);
                thread::spawn(move || journal.append(1_000 + index as u64, assessment(index)))
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap().unwrap();
        }
        let events = journal.read_verified().unwrap();
        assert_eq!(events.len(), 100);
        assert_eq!(events.first().unwrap().sequence, 1);
        assert_eq!(events.last().unwrap().sequence, 100);
        for pair in events.windows(2) {
            assert_eq!(pair[1].previous_hash, pair[0].event_hash);
        }
        let anchor = load_anchor(&anchor_path(&journal.path)).unwrap();
        verify_anchor(&anchor).unwrap();
        assert_eq!(anchor.sequence, 100);
        assert_eq!(anchor.event_hash, events.last().unwrap().event_hash);
        assert_eq!(anchor.file_len, fs::metadata(&journal.path).unwrap().len());
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }

    #[test]
    fn malformed_torn_tail_is_truncated_but_middle_corruption_blocks_append() {
        let journal = temp_journal("torn");
        journal.append(1_000, assessment(1)).unwrap();
        {
            let mut file = OpenOptions::new().append(true).open(&journal.path).unwrap();
            file.write_all(b"{\"torn\":").unwrap();
            file.sync_all().unwrap();
        }
        journal.append(1_001, assessment(2)).unwrap();
        assert_eq!(journal.read_verified().unwrap().len(), 2);

        let mut bytes = fs::read(&journal.path).unwrap();
        let first_newline = bytes.iter().position(|byte| *byte == b'\n').unwrap();
        bytes[first_newline / 2] ^= 1;
        fs::write(&journal.path, bytes).unwrap();
        assert!(matches!(
            journal.append(1_002, assessment(3)),
            Err(JournalError::CorruptChain | JournalError::RollbackDetected)
        ));
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }

    #[test]
    fn damage_to_a_committed_final_newline_is_not_silently_repaired() {
        let journal = temp_journal("newline");
        journal.append(1_000, assessment(1)).unwrap();
        let mut bytes = fs::read(&journal.path).unwrap();
        assert_eq!(bytes.pop(), Some(b'\n'));
        fs::write(&journal.path, bytes).unwrap();
        assert!(matches!(
            journal.append(1_001, assessment(2)),
            Err(JournalError::TornTail | JournalError::RollbackDetected)
        ));
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }

    #[test]
    fn live_high_water_rejects_full_file_rollback() {
        let journal = temp_journal("rollback");
        journal.append(1_000, assessment(1)).unwrap();
        let first = fs::read(&journal.path).unwrap();
        journal.append(1_001, assessment(2)).unwrap();
        fs::write(&journal.path, first).unwrap();
        assert!(matches!(
            journal.read_verified(),
            Err(JournalError::RollbackDetected)
        ));
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }

    #[test]
    fn restarted_reader_requires_the_exact_external_head_anchor() {
        let journal = temp_journal("restart-unanchored");
        journal.append(1_000, assessment(1)).unwrap();
        let prefix = fs::read(&journal.path).unwrap();
        journal.append(1_001, assessment(2)).unwrap();
        assert!(journal.clearance_authority_ready());

        let exact_restart = AssessmentJournal::new(&journal.path);
        assert_eq!(exact_restart.read_verified().unwrap().len(), 2);
        assert!(!exact_restart.clearance_authority_ready());

        fs::write(&journal.path, prefix).unwrap();
        let rolled_back_restart = AssessmentJournal::new(&journal.path);
        assert!(matches!(
            rolled_back_restart.read_verified(),
            Err(JournalError::RollbackDetected)
        ));
        assert!(!rolled_back_restart.clearance_authority_ready());
        assert!(matches!(
            rolled_back_restart.append(1_002, assessment(3)),
            Err(JournalError::WriterAuthorityUnavailable)
        ));
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }

    #[test]
    fn missing_or_zeroed_history_after_restart_never_becomes_authoritative() {
        for zero_file in [false, true] {
            let journal = temp_journal(if zero_file {
                "restart-zeroed"
            } else {
                "restart-missing"
            });
            journal.append(1_000, assessment(1)).unwrap();
            journal
                .record_revocation(1_001, format!("{:064x}", 1), "a".repeat(64))
                .unwrap();
            let path = journal.path.clone();
            drop(journal);

            if zero_file {
                fs::write(&path, []).unwrap();
            } else {
                fs::remove_file(&path).unwrap();
            }
            let restarted = AssessmentJournal::new(&path);
            assert!(matches!(
                restarted.read_verified(),
                Err(JournalError::RollbackDetected)
            ));
            assert!(!restarted.clearance_authority_ready());
            assert!(matches!(
                restarted.append(1_002, assessment(2)),
                Err(JournalError::WriterAuthorityUnavailable)
            ));
            let _ = fs::remove_dir_all(path.parent().unwrap());
        }
    }

    #[test]
    fn missing_tampered_and_rolled_back_anchors_fail_closed() {
        let journal = temp_journal("anchor-fail-closed");
        journal.append(1_000, assessment(1)).unwrap();
        let first_anchor = fs::read(anchor_path(&journal.path)).unwrap();
        journal.append(1_001, assessment(2)).unwrap();

        fs::write(anchor_path(&journal.path), &first_anchor).unwrap();
        assert!(matches!(
            journal.read_verified(),
            Err(JournalError::RollbackDetected)
        ));

        let current = journal.writer.as_ref().unwrap().checkpoint(&HighWater {
            sequence: 2,
            event_hash: load_chain(
                &mut OpenOptions::new().read(true).open(&journal.path).unwrap(),
                false,
                None,
            )
            .unwrap()
            .0
            .last()
            .unwrap()
            .event_hash
            .clone(),
            file_len: fs::metadata(&journal.path).unwrap().len(),
        });
        persist_anchor(&anchor_path(&journal.path), &current).unwrap();
        let mut tampered = fs::read(anchor_path(&journal.path)).unwrap();
        let position = tampered.iter().position(|byte| *byte == b'1').unwrap();
        tampered[position] = b'2';
        fs::write(anchor_path(&journal.path), tampered).unwrap();
        assert!(matches!(
            journal.read_verified(),
            Err(JournalError::AnchorInvalid)
        ));

        fs::remove_file(anchor_path(&journal.path)).unwrap();
        assert!(matches!(
            journal.read_verified(),
            Err(JournalError::AnchorMissing)
        ));
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }

    #[test]
    fn one_process_writer_is_shared_and_restart_requires_next_external_epoch() {
        let journal = temp_journal("exclusive-writer-epoch");
        journal.append(1_000, assessment(1)).unwrap();
        let same = AssessmentJournal::open_native_writer(&journal.path, 1).unwrap();
        assert!(Arc::ptr_eq(
            journal.writer.as_ref().unwrap(),
            same.writer.as_ref().unwrap()
        ));
        assert!(matches!(
            AssessmentJournal::open_native_writer(&journal.path, 2),
            Err(JournalError::WriterAuthorityHeld)
        ));
        let path = journal.path.clone();
        drop(same);
        drop(journal);

        assert!(matches!(
            AssessmentJournal::open_native_writer(&path, 1),
            Err(JournalError::InvalidWriterEpoch)
        ));
        let rotated = AssessmentJournal::open_native_writer(&path, 2).unwrap();
        assert_eq!(rotated.read_verified().unwrap().len(), 1);
        let anchor = load_anchor(&anchor_path(&path)).unwrap();
        assert_eq!(anchor.writer_epoch, 2);
        verify_writer_anchor(&anchor, rotated.writer.as_ref().unwrap()).unwrap();
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn lock_timeout_does_not_append() {
        let journal = temp_journal("lock-timeout");
        if let Some(parent) = journal.path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let lock = JournalLock::acquire(&lock_path(&journal.path), Duration::from_secs(1)).unwrap();
        let contender =
            AssessmentJournal::with_lock_timeout(&journal.path, Duration::from_millis(5));
        assert!(matches!(
            contender.append(1_000, assessment(1)),
            Err(JournalError::LockTimeout)
        ));
        drop(lock);
        assert_eq!(fs::metadata(&journal.path).unwrap().len(), 0);
        let anchor = load_anchor(&anchor_path(&journal.path)).unwrap();
        assert_eq!(anchor.sequence, 0);
        assert_eq!(anchor.event_hash, GENESIS_HASH);
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }

    #[test]
    fn complete_blank_lines_are_rejected_without_imposing_cross_writer_clock_order() {
        let journal = temp_journal("strict-lines-time");
        journal.append(1_000, assessment(1)).unwrap();
        journal.append(999, assessment(2)).unwrap();
        assert_eq!(journal.read_verified().unwrap().len(), 2);

        let mut file = OpenOptions::new().append(true).open(&journal.path).unwrap();
        file.write_all(b"\n").unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert!(matches!(
            journal.read_verified(),
            Err(JournalError::CorruptChain | JournalError::RollbackDetected)
        ));
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }

    #[test]
    fn assessment_and_clearance_timestamps_stay_inside_snapshot_window() {
        for (index, recorded_at_ms) in [899u64, 5_000, 5_001].into_iter().enumerate() {
            let journal = temp_journal(&format!("assessment-time-boundary-{index}"));
            assert!(matches!(
                journal.append(recorded_at_ms, assessment(index + 1)),
                Err(JournalError::InvalidEvent)
            ));
            assert!(journal.read_verified().unwrap().is_empty());
            let _ = fs::remove_dir_all(journal.path.parent().unwrap());
        }

        let journal = temp_journal("clearance-capture-lower-bound");
        let snapshot = format!("{:064x}", 9);
        journal.append(1_000, assessment(9)).unwrap();
        assert!(matches!(
            journal.append(899, issued(&"c".repeat(64), &snapshot, &"d".repeat(64))),
            Err(JournalError::InvalidEvent)
        ));
        assert_eq!(journal.read_verified().unwrap().len(), 1);
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());

        let journal = temp_journal("revocation-capture-lower-bound");
        let snapshot = format!("{:064x}", 10);
        journal.append(1_000, assessment(10)).unwrap();
        assert!(matches!(
            journal.record_revocation(899, snapshot, "a".repeat(64)),
            Err(JournalError::InvalidEvent)
        ));
        assert_eq!(journal.read_verified().unwrap().len(), 1);
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }

    #[test]
    fn clearance_lifecycle_is_ordered_single_use_and_binding_exact() {
        let journal = temp_journal("clearance-lifecycle");
        let snapshot = format!("{:064x}", 1);
        let clearance = "c".repeat(64);
        let participant = "d".repeat(64);
        journal.append(1_000, assessment(1)).unwrap();

        assert!(matches!(
            journal.append(1_001, consuming(&clearance, &snapshot, &participant)),
            Err(JournalError::InvalidEvent)
        ));
        journal
            .append(1_002, issued(&clearance, &snapshot, &participant))
            .unwrap();
        assert!(matches!(
            journal.append(1_003, issued(&clearance, &snapshot, &participant)),
            Err(JournalError::InvalidEvent)
        ));
        assert!(matches!(
            journal.append(1_004, consuming(&clearance, &snapshot, &"e".repeat(64))),
            Err(JournalError::InvalidEvent)
        ));
        journal
            .append(1_005, consuming(&clearance, &snapshot, &participant))
            .unwrap();
        journal
            .append(
                1_006,
                JournalPayload::ClearanceRevoked {
                    clearance_id_hash: clearance.clone(),
                    snapshot_hash: snapshot.clone(),
                    participant_id: participant.clone(),
                    issuer_epoch: 7,
                    reason_digest: "f".repeat(64),
                },
            )
            .unwrap();
        assert!(matches!(
            journal.append(
                1_007,
                JournalPayload::ClearanceConsumed {
                    clearance_id_hash: clearance,
                    snapshot_hash: snapshot,
                    participant_id: participant,
                    issuer_epoch: 7,
                    executor_receipt_digest: "a".repeat(64),
                }
            ),
            Err(JournalError::InvalidEvent)
        ));
        assert_eq!(journal.read_verified().unwrap().len(), 4);
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }

    #[test]
    fn snapshot_revocation_is_single_and_blocks_new_clearance() {
        let journal = temp_journal("snapshot-revocation");
        let snapshot = format!("{:064x}", 1);
        journal.append(1_000, assessment(1)).unwrap();
        journal
            .record_revocation(1_001, snapshot.clone(), "a".repeat(64))
            .unwrap();
        assert!(matches!(
            journal.record_revocation(1_002, snapshot.clone(), "b".repeat(64)),
            Err(JournalError::InvalidEvent)
        ));
        assert!(matches!(
            journal.append(1_003, issued(&"c".repeat(64), &snapshot, &"d".repeat(64))),
            Err(JournalError::InvalidEvent)
        ));
        assert_eq!(journal.read_verified().unwrap().len(), 2);
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }

    #[test]
    fn non_clear_foreign_and_expired_clearance_events_are_rejected() {
        for (index, verdict) in [
            JournalVerdict::Wait,
            JournalVerdict::Replan,
            JournalVerdict::UserDecision,
            JournalVerdict::Unknown,
        ]
        .into_iter()
        .enumerate()
        {
            let journal = temp_journal(&format!("non-clear-{index}"));
            let mut payload = assessment(index + 1);
            let snapshot = format!("{:064x}", index + 1);
            let JournalPayload::Assessment {
                verdict: stored_verdict,
                participant_count,
                participant_node_bindings_packed,
                participant_conflict_edges_packed,
                conflict_count,
                ..
            } = &mut payload
            else {
                unreachable!();
            };
            *stored_verdict = verdict;
            *participant_count = 2;
            let mut bindings = BASE64_STANDARD
                .decode(participant_node_bindings_packed.as_bytes())
                .unwrap();
            bindings.extend_from_slice(&decode_sha256_hex(&"e".repeat(64)).unwrap());
            bindings.extend_from_slice(&decode_sha256_hex(&source_node_digest("node-b")).unwrap());
            *participant_node_bindings_packed = BASE64_STANDARD.encode(bindings);
            *participant_conflict_edges_packed = BASE64_STANDARD.encode([0u8, 0u8, 0u8, 1u8]);
            *conflict_count = 1;
            journal.append(1_000, payload).unwrap();
            assert!(matches!(
                journal.append(1_001, issued(&"c".repeat(64), &snapshot, &"d".repeat(64))),
                Err(JournalError::InvalidEvent)
            ));
            assert_eq!(journal.read_verified().unwrap().len(), 1);
            let _ = fs::remove_dir_all(journal.path.parent().unwrap());
        }

        let journal = temp_journal("foreign-expired");
        let snapshot = format!("{:064x}", 9);
        journal.append(1_000, assessment(9)).unwrap();
        assert!(matches!(
            journal.append(1_001, issued(&"c".repeat(64), &snapshot, &"e".repeat(64))),
            Err(JournalError::InvalidEvent)
        ));
        let mut expired = issued(&"f".repeat(64), &snapshot, &"d".repeat(64));
        let JournalPayload::ClearanceIssued { expires_at_ms, .. } = &mut expired else {
            unreachable!();
        };
        *expires_at_ms = 1_002;
        assert!(matches!(
            journal.append(1_002, expired),
            Err(JournalError::InvalidEvent)
        ));
        assert_eq!(journal.read_verified().unwrap().len(), 1);
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }

    #[test]
    fn snapshot_revocation_between_consuming_and_consumed_blocks_commit() {
        let journal = temp_journal("revoke-during-consume");
        let snapshot = format!("{:064x}", 1);
        let clearance = "c".repeat(64);
        let participant = "d".repeat(64);
        journal.append(1_000, assessment(1)).unwrap();
        journal
            .append(1_001, issued(&clearance, &snapshot, &participant))
            .unwrap();
        journal
            .append(1_002, consuming(&clearance, &snapshot, &participant))
            .unwrap();
        journal
            .record_revocation(1_003, snapshot.clone(), "a".repeat(64))
            .unwrap();
        assert!(matches!(
            journal.append(
                1_004,
                JournalPayload::ClearanceConsumed {
                    clearance_id_hash: clearance,
                    snapshot_hash: snapshot,
                    participant_id: participant,
                    issuer_epoch: 7,
                    executor_receipt_digest: "b".repeat(64),
                }
            ),
            Err(JournalError::InvalidEvent)
        ));
        assert_eq!(journal.read_verified().unwrap().len(), 4);
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }

    #[test]
    fn expired_or_unanchored_clearance_transition_never_appends() {
        let journal = temp_journal("transition-expiry-anchor");
        let snapshot = format!("{:064x}", 1);
        let clearance = "c".repeat(64);
        let participant = "d".repeat(64);
        journal.append(1_000, assessment(1)).unwrap();
        journal
            .append(1_001, issued(&clearance, &snapshot, &participant))
            .unwrap();
        assert!(matches!(
            journal.append(5_000, consuming(&clearance, &snapshot, &participant)),
            Err(JournalError::InvalidEvent)
        ));
        assert_eq!(journal.read_verified().unwrap().len(), 2);

        let restarted = AssessmentJournal::new(&journal.path);
        assert_eq!(restarted.read_verified().unwrap().len(), 2);
        assert!(!restarted.clearance_authority_ready());
        assert!(matches!(
            restarted.record_clearance_consuming(1_002, clearance, snapshot, participant, 7,),
            Err(JournalError::InvalidEvent)
        ));
        assert_eq!(restarted.read_verified().unwrap().len(), 2);
        let _ = fs::remove_dir_all(journal.path.parent().unwrap());
    }
}
