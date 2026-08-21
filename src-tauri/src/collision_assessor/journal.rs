//! Strict, append-only, hash-chained collision assessor audit journal.

use super::snapshot::{AssessmentVerdict, StoredSnapshotReceipt, VerifiedAssessmentSnapshot};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const JOURNAL_SCHEMA_VERSION: u32 = 1;
const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 1024 * 1024;
const MAX_EVENTS: usize = 200_000;
const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const TRUST_LIVE_PROCESS: u8 = 1;
const TRUST_RECOVERED_UNANCHORED: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum JournalVerdict {
    Clear,
    Wait,
    Replan,
    UserDecision,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub(crate) enum JournalPayload {
    Assessment {
        snapshot_hash: String,
        registry_generation: u64,
        census_input_digest: String,
        verdict: JournalVerdict,
        participant_count: u32,
        participant_ids: Vec<String>,
        conflict_count: u32,
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
        conflict_id: String,
        disposition: String,
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
    verdict: JournalVerdict,
    participant_ids: BTreeSet<String>,
    conflict_count: u32,
    expires_at_ms: u64,
}

#[derive(Default)]
struct ReplayState {
    assessments: BTreeMap<String, AssessmentJournalRecord>,
    clearances: BTreeMap<String, ClearanceJournalRecord>,
    conflict_tickets: BTreeSet<(String, String)>,
}

impl ReplayState {
    fn apply(&mut self, payload: &JournalPayload, recorded_at_ms: u64) -> Result<(), JournalError> {
        match payload {
            JournalPayload::Assessment {
                snapshot_hash,
                verdict,
                participant_ids,
                conflict_count,
                expires_at_ms,
                ..
            } => {
                if self
                    .assessments
                    .insert(
                        snapshot_hash.clone(),
                        AssessmentJournalRecord {
                            live: true,
                            verdict: *verdict,
                            participant_ids: participant_ids.iter().cloned().collect(),
                            conflict_count: *conflict_count,
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
                if !live.live {
                    return Err(JournalError::InvalidEvent);
                }
                live.live = false;
            }
            JournalPayload::ConflictTicket {
                snapshot_hash,
                conflict_id,
                ..
            } => {
                let Some(assessment) = self.assessments.get(snapshot_hash) else {
                    return Err(JournalError::InvalidEvent);
                };
                if !assessment.live
                    || assessment.conflict_count == 0
                    || !self
                        .conflict_tickets
                        .insert((snapshot_hash.clone(), conflict_id.clone()))
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
                    || !assessment.participant_ids.contains(participant_id)
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

pub(crate) enum JournalError {
    InvalidEvent,
    CorruptChain,
    TornTail,
    RollbackDetected,
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
            Self::LimitExceeded => formatter.write_str("journal exceeds a hard bound"),
            Self::LockTimeout => formatter.write_str("journal lock timed out"),
            Self::Io(_) => formatter.write_str("journal I/O failed"),
            Self::Json(_) => formatter.write_str("journal serialization failed"),
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
        }
    }

    #[cfg(test)]
    pub(crate) fn new_live_for_test(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
            high_water: Arc::new(Mutex::new(None)),
            trust: Arc::new(AtomicU8::new(TRUST_LIVE_PROCESS)),
        }
    }

    #[cfg(test)]
    fn with_lock_timeout(path: impl Into<PathBuf>, lock_timeout: Duration) -> Self {
        Self {
            path: path.into(),
            lock_timeout,
            high_water: Arc::new(Mutex::new(None)),
            trust: Arc::new(AtomicU8::new(TRUST_RECOVERED_UNANCHORED)),
        }
    }

    fn append(
        &self,
        recorded_at_ms: u64,
        payload: JournalPayload,
    ) -> Result<JournalEvent, JournalError> {
        if recorded_at_ms == 0 {
            return Err(JournalError::InvalidEvent);
        }
        validate_payload(&payload)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock_path = lock_path(&self.path);
        let _lock = JournalLock::acquire(&lock_path, self.lock_timeout)?;
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&self.path)?;
        let protected_len = self
            .high_water
            .lock()
            .map_err(|_| JournalError::RollbackDetected)?
            .as_ref()
            .map(|water| water.file_len);
        let (events, repaired) = load_chain(&mut file, true, protected_len)?;
        if repaired {
            file.sync_all()?;
        }
        let file_len = file.metadata()?.len();
        self.enforce_high_water(events.last(), file_len)?;
        let last = events.last().cloned();
        let mut replay = replay_events(&events)?;
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
        if encoded.len() > MAX_LINE_BYTES
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
        self.set_high_water(&event, end)?;
        Ok(event)
    }

    pub(crate) fn read_verified(&self) -> Result<Vec<JournalEvent>, JournalError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let _lock = JournalLock::acquire(&lock_path(&self.path), self.lock_timeout)?;
        let mut file = match OpenOptions::new().read(true).open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let (events, _) = load_chain(&mut file, false, None)?;
        let file_len = file.metadata()?.len();
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
        if !receipt.matches(snapshot) {
            return Err(JournalError::InvalidEvent);
        }
        self.append(
            recorded_at_ms,
            JournalPayload::Assessment {
                snapshot_hash: snapshot.snapshot_hash().to_string(),
                registry_generation: snapshot.registry_generation(),
                census_input_digest: snapshot.census_input_digest().to_string(),
                verdict: snapshot.verdict().into(),
                participant_count: u32::try_from(snapshot.participants().len())
                    .map_err(|_| JournalError::LimitExceeded)?,
                participant_ids: snapshot
                    .participants()
                    .iter()
                    .map(|participant| participant.participant_id.clone())
                    .collect(),
                conflict_count: u32::try_from(snapshot.conflicts().len())
                    .map_err(|_| JournalError::LimitExceeded)?,
                captured_at_ms: snapshot.captured_at_ms(),
                expires_at_ms: snapshot.expires_at_ms(),
                encoded_bytes: receipt.encoded_bytes(),
                store_binding: receipt.store_binding().to_string(),
            },
        )
    }

    pub(crate) fn assessment_is_live(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        receipt: &StoredSnapshotReceipt,
    ) -> Result<bool, JournalError> {
        if !receipt.matches(snapshot) {
            return Ok(false);
        }
        let expected = JournalPayload::Assessment {
            snapshot_hash: snapshot.snapshot_hash().to_string(),
            registry_generation: snapshot.registry_generation(),
            census_input_digest: snapshot.census_input_digest().to_string(),
            verdict: snapshot.verdict().into(),
            participant_count: u32::try_from(snapshot.participants().len())
                .map_err(|_| JournalError::LimitExceeded)?,
            participant_ids: snapshot
                .participants()
                .iter()
                .map(|participant| participant.participant_id.clone())
                .collect(),
            conflict_count: u32::try_from(snapshot.conflicts().len())
                .map_err(|_| JournalError::LimitExceeded)?,
            captured_at_ms: snapshot.captured_at_ms(),
            expires_at_ms: snapshot.expires_at_ms(),
            encoded_bytes: receipt.encoded_bytes(),
            store_binding: receipt.store_binding().to_string(),
        };
        let events = self.read_verified()?;
        let mut found = false;
        let mut revoked = false;
        for event in events {
            match event.payload {
                payload if payload == expected => found = true,
                JournalPayload::Revocation { snapshot_hash, .. }
                    if snapshot_hash == snapshot.snapshot_hash() =>
                {
                    revoked = true;
                }
                _ => {}
            }
        }
        Ok(found && !revoked)
    }

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
            participant_ids,
            conflict_count,
            captured_at_ms,
            expires_at_ms,
            encoded_bytes,
            store_binding,
        } => {
            is_sha256(snapshot_hash)
                && *registry_generation > 0
                && is_sha256(census_input_digest)
                && *participant_count > 0
                && usize::try_from(*participant_count) == Ok(participant_ids.len())
                && participant_ids
                    .iter()
                    .all(|participant_id| is_sha256(participant_id))
                && participant_ids.windows(2).all(|pair| pair[0] < pair[1])
                && (*verdict != JournalVerdict::Clear || *conflict_count == 0)
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
            conflict_id,
            disposition,
        } => {
            is_sha256(snapshot_hash)
                && is_sha256(conflict_id)
                && matches!(disposition.as_str(), "WAIT" | "REPLAN" | "USER_DECISION")
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

fn lock_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "assessment.jsonl".into(), |value| value.to_os_string());
    name.push(".lock");
    path.with_file_name(name)
}

#[cfg(windows)]
struct JournalLock {
    handle: *mut std::ffi::c_void,
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
                return Ok(Self { handle });
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
            CloseHandle(self.handle);
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
        JournalPayload::Assessment {
            snapshot_hash: format!("{index:064x}"),
            registry_generation: 7,
            census_input_digest: "a".repeat(64),
            verdict: JournalVerdict::Clear,
            participant_count: 1,
            participant_ids: vec!["d".repeat(64)],
            conflict_count: 0,
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
    fn restarted_history_is_unanchored_and_cannot_authorize_clearance() {
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
        assert_eq!(rolled_back_restart.read_verified().unwrap().len(), 1);
        assert!(!rolled_back_restart.clearance_authority_ready());
        rolled_back_restart.append(1_002, assessment(3)).unwrap();
        assert!(!rolled_back_restart.clearance_authority_ready());
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
            assert!(restarted.read_verified().unwrap().is_empty());
            assert!(!restarted.clearance_authority_ready());
            restarted.append(1_002, assessment(2)).unwrap();
            assert!(!restarted.clearance_authority_ready());
            let _ = fs::remove_dir_all(path.parent().unwrap());
        }
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
        assert!(!journal.path.exists());
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
            let JournalPayload::Assessment {
                verdict: stored_verdict,
                conflict_count,
                ..
            } = &mut payload
            else {
                unreachable!();
            };
            *stored_verdict = verdict;
            *conflict_count = 1;
            let snapshot = format!("{:064x}", index + 1);
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
