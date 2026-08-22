//! Sealed, path-free conflict-ticket broker.
//!
//! The generic renderer control plane is deliberately not imported here. Ticket authority comes
//! only from an immutable assessment snapshot and its hash-chained native journal. Production
//! construction remains disabled until B15/B20 provide native route and journal-anchor receipts.

use super::journal::{
    conflict_ticket_id, conflict_ticket_signal_id, AssessmentJournal, JournalError, JournalPayload,
    TicketSignalKind,
};
use super::model::ConflictDisposition;
use super::snapshot::{
    SnapshotConflictBasis, SnapshotConflictOverlap, StoredSnapshotReceipt,
    VerifiedAssessmentSnapshot,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

const MAX_PAGE_SIZE: usize = 100;
const MAX_TICKET_INDEXES: usize = 4;
static BROKER_EPOCH: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketEndpointView {
    pub(crate) participant_id: String,
    pub(crate) plan_id: String,
    pub(crate) node_id: String,
    pub(crate) claim_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketView {
    pub(crate) ticket_id: String,
    pub(crate) snapshot_hash: String,
    pub(crate) left: TicketEndpointView,
    pub(crate) right: TicketEndpointView,
    pub(crate) bases: Vec<SnapshotConflictBasis>,
    pub(crate) overlaps: Vec<SnapshotConflictOverlap>,
    pub(crate) disposition: ConflictDisposition,
    pub(crate) revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketPage {
    pub(crate) tickets: Vec<TicketView>,
    pub(crate) next_cursor: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketSignalView {
    pub(crate) signal_id: String,
    pub(crate) ticket_ids: Vec<String>,
    pub(crate) snapshot_hash: String,
    pub(crate) actor_participant_id: String,
    pub(crate) signal_kind: TicketSignalKind,
    pub(crate) source_state_digest: String,
    pub(crate) acknowledged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TicketSignalPage {
    pub(crate) signals: Vec<TicketSignalView>,
    /// Journal sequence of the last returned signal. Unlike an offset, this remains stable when
    /// concurrent writers append new events between pages.
    pub(crate) next_cursor: Option<u64>,
}

/// Non-serializable mailbox capability. There is intentionally no token/string accessor.
#[derive(Clone)]
pub(crate) struct TicketMailboxCapability {
    broker_epoch: u64,
    snapshot_hash: String,
    participant_id: String,
    node_id: String,
    run_identity: String,
    fence: u64,
    lease_generation: u64,
    allowed_ticket_ids: Arc<BTreeSet<String>>,
    tickets: Arc<BTreeMap<String, TicketView>>,
    allowed_ticket_set_digest: String,
    store_binding: String,
    orchestrator_id: String,
    destination_registration_digest: String,
    route_store_binding: String,
    app_instance_digest: String,
    route_generation: u64,
    route_issuer_epoch: u64,
    route_expires_at_ms: u64,
    route_binding_digest: String,
    mutation_expires_at_ms: u64,
    publish_allowed: bool,
}

impl std::fmt::Debug for TicketMailboxCapability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TicketMailboxCapability")
            .field("snapshot_hash", &self.snapshot_hash)
            .field("participant_id", &self.participant_id)
            .field("ticket_count", &self.allowed_ticket_ids.len())
            .field("route_expires_at_ms", &self.route_expires_at_ms)
            .field("mutation_expires_at_ms", &self.mutation_expires_at_ms)
            .finish()
    }
}

/// Sealed native orchestrator-route fact. B20 will construct this from the durable route store;
/// callers cannot supply a participant ID to mint mailbox authority.
#[derive(Clone)]
pub(crate) struct MachineMailboxRouteReceipt {
    snapshot_hash: String,
    participant_id: String,
    orchestrator_id: String,
    run_identity: String,
    fence: u64,
    lease_generation: u64,
    snapshot_store_binding: String,
    destination_registration_digest: String,
    route_store_binding: String,
    app_instance_digest: String,
    route_generation: u64,
    issuer_epoch: u64,
    route_expires_at_ms: u64,
}

/// Sealed scheduler/registry fact. B09/B20 will own production construction from native stores.
#[derive(Clone)]
pub(crate) struct MachineTicketSignalReceipt {
    snapshot_hash: String,
    participant_id: String,
    source_node_id: String,
    run_identity: String,
    fence: u64,
    lease_generation: u64,
    kind: TicketSignalKind,
    source_state_digest: String,
    source_event_id: String,
}

#[derive(Clone)]
pub(crate) struct MachineTicketAcknowledgementReceipt {
    snapshot_hash: String,
    signal_id: String,
    participant_id: String,
    run_identity: String,
    fence: u64,
    lease_generation: u64,
    acknowledgement_digest: String,
}

#[derive(Debug)]
pub(crate) enum TicketError {
    ProductionDisabled,
    Denied,
    InvalidSnapshot,
    InvalidReceipt,
    LimitExceeded,
    Journal(JournalError),
}

impl From<JournalError> for TicketError {
    fn from(value: JournalError) -> Self {
        Self::Journal(value)
    }
}

#[derive(Clone)]
pub(crate) struct TicketBroker {
    journal: AssessmentJournal,
    broker_epoch: u64,
    enabled: bool,
    ticket_indexes: Arc<Mutex<TicketIndexCache>>,
}

type ParticipantMailboxIndex = (Arc<BTreeSet<String>>, Arc<BTreeMap<String, TicketView>>);

struct TicketIndex {
    snapshot_hash: String,
    store_binding: String,
    commitment_root: String,
    expires_at_ms: u64,
    mailboxes: BTreeMap<String, ParticipantMailboxIndex>,
}

#[derive(Default)]
struct TicketIndexCache {
    entries: BTreeMap<String, Arc<TicketIndex>>,
    least_recently_used: VecDeque<String>,
}

impl TicketIndexCache {
    fn prune_expired(&mut self, now_ms: u64) {
        self.entries
            .retain(|_, index| now_ms < index.expires_at_ms);
        self.least_recently_used
            .retain(|key| self.entries.contains_key(key));
    }

    fn get(&mut self, key: &str) -> Option<Arc<TicketIndex>> {
        let index = self.entries.get(key)?.clone();
        self.least_recently_used.retain(|candidate| candidate != key);
        self.least_recently_used.push_back(key.to_string());
        Some(index)
    }

    fn insert(&mut self, key: String, index: Arc<TicketIndex>) {
        self.entries.insert(key.clone(), index);
        self.least_recently_used
            .retain(|candidate| candidate != &key);
        self.least_recently_used.push_back(key);
        while self.entries.len() > MAX_TICKET_INDEXES {
            if let Some(oldest) = self.least_recently_used.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }

    fn remove(&mut self, key: &str) {
        self.entries.remove(key);
        self.least_recently_used.retain(|candidate| candidate != key);
    }
}

impl TicketBroker {
    /// Fail-closed production primitive. B15/B20 must replace this with a native-authority
    /// constructor; exposing a renderer command here would recreate the caller-asserted bypass.
    pub(crate) fn new_disabled(journal: AssessmentJournal) -> Self {
        Self {
            journal,
            broker_epoch: next_broker_epoch(),
            enabled: false,
            ticket_indexes: Arc::new(Mutex::new(TicketIndexCache::default())),
        }
    }

    #[cfg(test)]
    fn new_enabled_for_test(journal: AssessmentJournal) -> Self {
        Self {
            journal,
            broker_epoch: next_broker_epoch(),
            enabled: true,
            ticket_indexes: Arc::new(Mutex::new(TicketIndexCache::default())),
        }
    }

    pub(crate) fn materialize_for_participant(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        receipt: &StoredSnapshotReceipt,
        route: &MachineMailboxRouteReceipt,
        now_ms: u64,
    ) -> Result<TicketMailboxCapability, TicketError> {
        self.materialize_for_participants(snapshot, receipt, std::slice::from_ref(route), now_ms)?
            .pop()
            .ok_or(TicketError::Denied)
    }

    /// Verifies one immutable assessment, indexes its topology once, then binds each sealed native
    /// route independently. This is the bounded startup path for a 100-worker cohort.
    pub(crate) fn materialize_for_participants(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        receipt: &StoredSnapshotReceipt,
        routes: &[MachineMailboxRouteReceipt],
        now_ms: u64,
    ) -> Result<Vec<TicketMailboxCapability>, TicketError> {
        self.require_enabled()?;
        if now_ms == 0
            || now_ms >= snapshot.expires_at_ms()
            || !receipt.matches(snapshot)
            || snapshot.conflicts().is_empty()
            || routes.is_empty()
            || routes.len() > snapshot.participants().len()
        {
            return Err(TicketError::InvalidSnapshot);
        }
        if !self.journal.assessment_is_live(snapshot, receipt)? {
            self.evict_ticket_index(snapshot.snapshot_hash(), receipt.store_binding())?;
            return Err(TicketError::InvalidSnapshot);
        }
        let index = self.ticket_index(snapshot, receipt, now_ms, true)?;
        let mut seen = BTreeSet::new();
        routes
            .iter()
            .map(|route| {
                if !seen.insert(route.participant_id.clone()) {
                    return Err(TicketError::InvalidReceipt);
                }
                self.capability_from_index(snapshot, receipt, route, now_ms, &index, true)
            })
            .collect()
    }

    /// Reconstruct an audit-only mailbox from the immutable snapshot and persisted journal after
    /// a crash, revocation, or snapshot expiry. It never reopens discovery and cannot publish a
    /// new transition; it can only list owned tickets/signals and acknowledge an existing signal.
    pub(crate) fn resume_for_participant(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        receipt: &StoredSnapshotReceipt,
        route: &MachineMailboxRouteReceipt,
        now_ms: u64,
    ) -> Result<TicketMailboxCapability, TicketError> {
        self.require_enabled()?;
        if now_ms == 0 || !receipt.matches(snapshot) {
            return Err(TicketError::InvalidSnapshot);
        }
        let participant = snapshot
            .participants()
            .iter()
            .find(|candidate| candidate.participant_id == route.participant_id)
            .ok_or(TicketError::Denied)?;
        self.validate_route(snapshot, receipt, participant, route, now_ms)?;
        if !self.journal.assessment_was_recorded(snapshot, receipt)? {
            return Err(TicketError::InvalidSnapshot);
        }
        let index = self.ticket_index(snapshot, receipt, now_ms, false)?;
        self.capability_from_index(snapshot, receipt, route, now_ms, &index, false)
    }

    pub(crate) fn list_own_signals(
        &self,
        capability: &TicketMailboxCapability,
        cursor: u64,
        limit: usize,
        now_ms: u64,
    ) -> Result<TicketSignalPage, TicketError> {
        self.validate_capability_shape(capability, now_ms)?;
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(TicketError::LimitExceeded);
        }
        let events = self.journal.read_verified()?;
        if cursor > events.last().map_or(0, |event| event.sequence) {
            return Err(TicketError::LimitExceeded);
        }
        let acknowledgements = events
            .iter()
            .filter_map(|event| match &event.payload {
                JournalPayload::ConflictTicketAcknowledged {
                    signal_id,
                    recipient_participant_id,
                    ..
                } if recipient_participant_id == &capability.participant_id => {
                    Some(signal_id.as_str())
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let signals = events
            .iter()
            .filter_map(|event| match &event.payload {
                JournalPayload::ConflictTicketSignal {
                    snapshot_hash,
                    signal_id,
                    actor_participant_id,
                    signal_kind,
                    source_state_digest,
                    ..
                } if snapshot_hash == &capability.snapshot_hash
                    && actor_participant_id != &capability.participant_id =>
                {
                    let ticket_ids = capability
                        .tickets
                        .values()
                        .filter(|ticket| {
                            ticket.left.participant_id == *actor_participant_id
                                || ticket.right.participant_id == *actor_participant_id
                        })
                        .map(|ticket| ticket.ticket_id.clone())
                        .collect::<Vec<_>>();
                    (event.sequence > cursor && !ticket_ids.is_empty()).then_some((
                        event.sequence,
                        TicketSignalView {
                            signal_id: signal_id.clone(),
                            ticket_ids,
                            snapshot_hash: snapshot_hash.clone(),
                            actor_participant_id: actor_participant_id.clone(),
                            signal_kind: *signal_kind,
                            source_state_digest: source_state_digest.clone(),
                            acknowledged: acknowledgements.contains(signal_id.as_str()),
                        },
                    ))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let page = signals
            .iter()
            .take(limit)
            .map(|(_, signal)| signal.clone())
            .collect::<Vec<_>>();
        let next = signals
            .iter()
            .take(limit)
            .last()
            .map(|(sequence, _)| *sequence);
        Ok(TicketSignalPage {
            signals: page,
            next_cursor: next,
        })
    }

    pub(crate) fn list_own(
        &self,
        capability: &TicketMailboxCapability,
        cursor: usize,
        limit: usize,
        now_ms: u64,
    ) -> Result<TicketPage, TicketError> {
        self.validate_capability_shape(capability, now_ms)?;
        if limit == 0 || limit > MAX_PAGE_SIZE || cursor > capability.allowed_ticket_ids.len() {
            return Err(TicketError::LimitExceeded);
        }
        let events = self.journal.read_verified()?;
        let revoked = snapshot_is_revoked(&events, &capability.snapshot_hash);
        let tickets = capability
            .tickets
            .values()
            .cloned()
            .map(|mut ticket| {
                ticket.revoked = revoked;
                ticket
            })
            .collect::<Vec<_>>();
        let page = tickets
            .into_iter()
            .skip(cursor)
            .take(limit)
            .collect::<Vec<_>>();
        let next = cursor.saturating_add(page.len());
        Ok(TicketPage {
            tickets: page,
            next_cursor: (next < capability.allowed_ticket_ids.len()).then_some(next),
        })
    }

    pub(crate) fn publish_signal(
        &self,
        capability: &TicketMailboxCapability,
        receipt: &MachineTicketSignalReceipt,
        now_ms: u64,
    ) -> Result<String, TicketError> {
        self.validate_capability_shape(capability, now_ms)?;
        if !capability.publish_allowed || now_ms >= capability.mutation_expires_at_ms {
            return Err(TicketError::Denied);
        }
        if receipt.snapshot_hash != capability.snapshot_hash
            || receipt.participant_id != capability.participant_id
            || receipt.source_node_id != capability.node_id
            || receipt.run_identity != capability.run_identity
            || receipt.fence != capability.fence
            || receipt.lease_generation != capability.lease_generation
            || !is_sha256(&receipt.source_state_digest)
            || !is_sha256(&receipt.source_event_id)
        {
            return Err(TicketError::InvalidReceipt);
        }
        let signal_id = conflict_ticket_signal_id(
            &receipt.snapshot_hash,
            &receipt.participant_id,
            &receipt.source_node_id,
            receipt.kind,
            &receipt.source_event_id,
        );
        self.journal.record_conflict_ticket_signal(
            now_ms,
            capability.snapshot_hash.clone(),
            receipt.participant_id.clone(),
            receipt.source_node_id.clone(),
            receipt.kind,
            receipt.source_state_digest.clone(),
            receipt.source_event_id.clone(),
        )?;
        if receipt.kind == TicketSignalKind::ManifestChanged {
            self.evict_ticket_index(&capability.snapshot_hash, &capability.store_binding)?;
        }
        Ok(signal_id)
    }

    pub(crate) fn acknowledge_signal(
        &self,
        capability: &TicketMailboxCapability,
        receipt: &MachineTicketAcknowledgementReceipt,
        now_ms: u64,
    ) -> Result<(), TicketError> {
        self.validate_capability_shape(capability, now_ms)?;
        if receipt.snapshot_hash != capability.snapshot_hash
            || receipt.participant_id != capability.participant_id
            || receipt.run_identity != capability.run_identity
            || receipt.fence != capability.fence
            || receipt.lease_generation != capability.lease_generation
            || !is_sha256(&receipt.signal_id)
            || !is_sha256(&receipt.acknowledgement_digest)
        {
            return Err(TicketError::InvalidReceipt);
        }
        self.journal.record_conflict_ticket_acknowledgement(
            now_ms,
            capability.snapshot_hash.clone(),
            receipt.signal_id.clone(),
            receipt.participant_id.clone(),
            receipt.acknowledgement_digest.clone(),
        )?;
        Ok(())
    }

    fn require_enabled(&self) -> Result<(), TicketError> {
        if self.enabled {
            Ok(())
        } else {
            Err(TicketError::ProductionDisabled)
        }
    }

    fn ticket_index(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        receipt: &StoredSnapshotReceipt,
        now_ms: u64,
        cacheable: bool,
    ) -> Result<Arc<TicketIndex>, TicketError> {
        let cache_key = digest_strings(
            b"perfect-planner:ticket-index:v1",
            [snapshot.snapshot_hash(), receipt.store_binding()],
        );
        let mut indexes = self
            .ticket_indexes
            .lock()
            .map_err(|_| TicketError::Denied)?;
        indexes.prune_expired(now_ms);
        if let Some(index) = indexes.get(&cache_key) {
            return Ok(index);
        }
        let commitment_root = snapshot.conflict_commitment_root();
        let mut builders = snapshot
            .participants()
            .iter()
            .map(|participant| {
                (
                    participant.participant_id.clone(),
                    (BTreeSet::new(), BTreeMap::new()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for conflict in snapshot.conflicts() {
            let ticket_id = conflict_ticket_id(
                snapshot.snapshot_hash(),
                &commitment_root,
                &conflict.conflict_id,
            );
            let view = ticket_view(snapshot.snapshot_hash(), &ticket_id, conflict, false);
            for participant_id in [
                &conflict.left_participant_id,
                &conflict.right_participant_id,
            ] {
                let (ticket_ids, tickets) = builders
                    .get_mut(participant_id)
                    .ok_or(TicketError::InvalidSnapshot)?;
                if !ticket_ids.insert(ticket_id.clone())
                    || tickets.insert(ticket_id.clone(), view.clone()).is_some()
                {
                    return Err(TicketError::InvalidSnapshot);
                }
            }
        }
        let mailboxes = builders
            .into_iter()
            .map(|(participant_id, (ticket_ids, tickets))| {
                (
                    participant_id,
                    (Arc::new(ticket_ids), Arc::new(tickets)),
                )
            })
            .collect();
        let index = Arc::new(TicketIndex {
            snapshot_hash: snapshot.snapshot_hash().to_string(),
            store_binding: receipt.store_binding().to_string(),
            commitment_root,
            expires_at_ms: snapshot.expires_at_ms(),
            mailboxes,
        });
        if cacheable && now_ms < index.expires_at_ms {
            indexes.insert(cache_key, index.clone());
        }
        Ok(index)
    }

    fn evict_ticket_index(
        &self,
        snapshot_hash: &str,
        store_binding: &str,
    ) -> Result<(), TicketError> {
        let cache_key = digest_strings(
            b"perfect-planner:ticket-index:v1",
            [snapshot_hash, store_binding],
        );
        self.ticket_indexes
            .lock()
            .map_err(|_| TicketError::Denied)?
            .remove(&cache_key);
        Ok(())
    }

    fn capability_from_index(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        receipt: &StoredSnapshotReceipt,
        route: &MachineMailboxRouteReceipt,
        now_ms: u64,
        index: &TicketIndex,
        publish_allowed: bool,
    ) -> Result<TicketMailboxCapability, TicketError> {
        let participant = snapshot
            .participants()
            .iter()
            .find(|candidate| candidate.participant_id == route.participant_id)
            .ok_or(TicketError::Denied)?;
        self.validate_route(snapshot, receipt, participant, route, now_ms)?;
        if index.snapshot_hash != snapshot.snapshot_hash()
            || index.store_binding != receipt.store_binding()
            || !is_sha256(&index.commitment_root)
        {
            return Err(TicketError::InvalidSnapshot);
        }
        let (allowed_ticket_ids, tickets) = index
            .mailboxes
            .get(&participant.participant_id)
            .ok_or(TicketError::Denied)?;
        let allowed_ticket_set_digest = digest_strings(
            b"perfect-planner:ticket-mailbox-set:v1",
            allowed_ticket_ids.iter().map(String::as_str),
        );
        Ok(TicketMailboxCapability {
            broker_epoch: self.broker_epoch,
            snapshot_hash: snapshot.snapshot_hash().to_string(),
            participant_id: participant.participant_id.clone(),
            node_id: participant.node_id.clone(),
            run_identity: participant.run_identity.clone(),
            fence: participant.fence,
            lease_generation: participant.lease_generation,
            allowed_ticket_ids: allowed_ticket_ids.clone(),
            tickets: tickets.clone(),
            allowed_ticket_set_digest,
            store_binding: receipt.store_binding().to_string(),
            orchestrator_id: route.orchestrator_id.clone(),
            destination_registration_digest: route.destination_registration_digest.clone(),
            route_store_binding: route.route_store_binding.clone(),
            app_instance_digest: route.app_instance_digest.clone(),
            route_generation: route.route_generation,
            route_issuer_epoch: route.issuer_epoch,
            route_expires_at_ms: route.route_expires_at_ms,
            route_binding_digest: mailbox_route_binding_digest(route),
            mutation_expires_at_ms: snapshot.expires_at_ms().min(route.route_expires_at_ms),
            publish_allowed,
        })
    }

    fn validate_capability_shape(
        &self,
        capability: &TicketMailboxCapability,
        now_ms: u64,
    ) -> Result<(), TicketError> {
        self.require_enabled()?;
        let expected = digest_strings(
            b"perfect-planner:ticket-mailbox-set:v1",
            capability.allowed_ticket_ids.iter().map(String::as_str),
        );
        let ticket_keys = capability.tickets.keys().cloned().collect::<BTreeSet<_>>();
        let expected_route_binding = mailbox_capability_route_binding_digest(capability);
        if capability.broker_epoch != self.broker_epoch
            || now_ms == 0
            || now_ms >= capability.route_expires_at_ms
            || expected != capability.allowed_ticket_set_digest
            || expected_route_binding != capability.route_binding_digest
            || ticket_keys != *capability.allowed_ticket_ids
            || !is_sha256(&capability.snapshot_hash)
            || !is_sha256(&capability.participant_id)
            || !is_sha256(&capability.run_identity)
            || !is_sha256(&capability.store_binding)
            || !is_sha256(&capability.orchestrator_id)
            || !is_sha256(&capability.destination_registration_digest)
            || !is_sha256(&capability.route_store_binding)
            || !is_sha256(&capability.app_instance_digest)
            || capability.fence == 0
            || capability.lease_generation == 0
            || capability.route_generation == 0
            || capability.route_issuer_epoch == 0
            || capability.mutation_expires_at_ms > capability.route_expires_at_ms
        {
            return Err(TicketError::Denied);
        }
        Ok(())
    }

    fn validate_route(
        &self,
        snapshot: &VerifiedAssessmentSnapshot,
        receipt: &StoredSnapshotReceipt,
        participant: &super::snapshot::SnapshotParticipant,
        route: &MachineMailboxRouteReceipt,
        now_ms: u64,
    ) -> Result<(), TicketError> {
        if route.snapshot_hash != snapshot.snapshot_hash()
            || route.participant_id != participant.participant_id
            || route.run_identity != participant.run_identity
            || route.fence != participant.fence
            || route.lease_generation != participant.lease_generation
            || route.snapshot_store_binding != receipt.store_binding()
            || route.route_generation == 0
            || route.issuer_epoch == 0
            || route.route_expires_at_ms <= now_ms
            || !is_sha256(&route.snapshot_hash)
            || !is_sha256(&route.participant_id)
            || !is_sha256(&route.orchestrator_id)
            || !is_sha256(&route.run_identity)
            || !is_sha256(&route.snapshot_store_binding)
            || !is_sha256(&route.destination_registration_digest)
            || !is_sha256(&route.route_store_binding)
            || !is_sha256(&route.app_instance_digest)
        {
            return Err(TicketError::InvalidReceipt);
        }
        Ok(())
    }
}

fn ticket_view(
    snapshot_hash: &str,
    ticket_id: &str,
    conflict: &super::snapshot::SnapshotConflict,
    revoked: bool,
) -> TicketView {
    TicketView {
        ticket_id: ticket_id.to_string(),
        snapshot_hash: snapshot_hash.to_string(),
        left: TicketEndpointView {
            participant_id: conflict.left_participant_id.clone(),
            plan_id: conflict.left_plan_id.clone(),
            node_id: conflict.left_node_id.clone(),
            claim_id: conflict.left_claim_id.clone(),
        },
        right: TicketEndpointView {
            participant_id: conflict.right_participant_id.clone(),
            plan_id: conflict.right_plan_id.clone(),
            node_id: conflict.right_node_id.clone(),
            claim_id: conflict.right_claim_id.clone(),
        },
        bases: conflict.bases.clone(),
        overlaps: conflict.overlaps.clone(),
        disposition: conflict
            .disposition
            .expect("journal replay requires a ticket disposition"),
        revoked,
    }
}

fn snapshot_is_revoked(events: &[super::journal::JournalEvent], snapshot_hash: &str) -> bool {
    events.iter().any(|event| match &event.payload {
        JournalPayload::Revocation {
            snapshot_hash: revoked,
            ..
        } if revoked == snapshot_hash => true,
        JournalPayload::ConflictTicketSignal {
            snapshot_hash: changed,
            signal_kind: TicketSignalKind::ManifestChanged,
            ..
        } if changed == snapshot_hash => true,
        _ => false,
    })
}

fn next_broker_epoch() -> u64 {
    BROKER_EPOCH.fetch_add(1, Ordering::AcqRel).max(1)
}

fn mailbox_route_binding_digest(route: &MachineMailboxRouteReceipt) -> String {
    route_binding_digest(
        &route.snapshot_hash,
        &route.participant_id,
        &route.orchestrator_id,
        &route.run_identity,
        route.fence,
        route.lease_generation,
        &route.snapshot_store_binding,
        &route.destination_registration_digest,
        &route.route_store_binding,
        &route.app_instance_digest,
        route.route_generation,
        route.issuer_epoch,
        route.route_expires_at_ms,
    )
}

fn mailbox_capability_route_binding_digest(capability: &TicketMailboxCapability) -> String {
    route_binding_digest(
        &capability.snapshot_hash,
        &capability.participant_id,
        &capability.orchestrator_id,
        &capability.run_identity,
        capability.fence,
        capability.lease_generation,
        &capability.store_binding,
        &capability.destination_registration_digest,
        &capability.route_store_binding,
        &capability.app_instance_digest,
        capability.route_generation,
        capability.route_issuer_epoch,
        capability.route_expires_at_ms,
    )
}

#[allow(clippy::too_many_arguments)]
fn route_binding_digest(
    snapshot_hash: &str,
    participant_id: &str,
    orchestrator_id: &str,
    run_identity: &str,
    fence: u64,
    lease_generation: u64,
    snapshot_store_binding: &str,
    destination_registration_digest: &str,
    route_store_binding: &str,
    app_instance_digest: &str,
    route_generation: u64,
    issuer_epoch: u64,
    route_expires_at_ms: u64,
) -> String {
    let fence = fence.to_string();
    let lease_generation = lease_generation.to_string();
    let route_generation = route_generation.to_string();
    let issuer_epoch = issuer_epoch.to_string();
    let route_expires_at_ms = route_expires_at_ms.to_string();
    digest_strings(
        b"perfect-planner:ticket-mailbox-route-binding:v1",
        [
            snapshot_hash,
            participant_id,
            orchestrator_id,
            run_identity,
            fence.as_str(),
            lease_generation.as_str(),
            snapshot_store_binding,
            destination_registration_digest,
            route_store_binding,
            app_instance_digest,
            route_generation.as_str(),
            issuer_epoch.as_str(),
            route_expires_at_ms.as_str(),
        ],
    )
}

fn digest_strings<'a>(domain: &[u8], values: impl IntoIterator<Item = &'a str>) -> String {
    let mut digest = Sha256::new();
    digest.update((domain.len() as u64).to_le_bytes());
    digest.update(domain);
    for value in values {
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision_assessor::snapshot::{
        tests::{
            fixture_conflict_snapshot, fixture_conflict_snapshot_with_count,
            fixture_conflict_snapshot_with_generation, fixture_participant_chain_snapshot,
            fixture_participant_clique_snapshot,
        },
        SnapshotStore,
    };
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn root(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "perfect-planner-ticket-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn setup(
        name: &str,
        disposition: ConflictDisposition,
    ) -> (
        PathBuf,
        TicketBroker,
        VerifiedAssessmentSnapshot,
        StoredSnapshotReceipt,
    ) {
        let root = root(name);
        let snapshot = fixture_conflict_snapshot(disposition);
        let store = SnapshotStore::new_for_test(root.join("snapshots"));
        let receipt = store.persist(&snapshot).unwrap();
        let journal = AssessmentJournal::new_live_for_test(root.join("assessment.jsonl"));
        journal
            .record_assessment(&snapshot, &receipt, 1_100)
            .unwrap();
        (
            root,
            TicketBroker::new_enabled_for_test(journal),
            snapshot,
            receipt,
        )
    }

    fn signal_receipt(
        snapshot: &VerifiedAssessmentSnapshot,
        participant_id: &str,
        _ticket_id: &str,
        kind: TicketSignalKind,
        source_event_id: &str,
    ) -> MachineTicketSignalReceipt {
        let participant = snapshot
            .participants()
            .iter()
            .find(|candidate| candidate.participant_id == participant_id)
            .unwrap();
        MachineTicketSignalReceipt {
            snapshot_hash: snapshot.snapshot_hash().to_string(),
            participant_id: participant.participant_id.clone(),
            source_node_id: participant.node_id.clone(),
            run_identity: participant.run_identity.clone(),
            fence: participant.fence,
            lease_generation: participant.lease_generation,
            kind,
            source_state_digest: "2".repeat(64),
            source_event_id: source_event_id.to_string(),
        }
    }

    fn route_receipt(
        snapshot: &VerifiedAssessmentSnapshot,
        stored: &StoredSnapshotReceipt,
        participant_id: &str,
    ) -> MachineMailboxRouteReceipt {
        let participant = snapshot
            .participants()
            .iter()
            .find(|candidate| candidate.participant_id == participant_id)
            .unwrap();
        MachineMailboxRouteReceipt {
            snapshot_hash: snapshot.snapshot_hash().to_string(),
            participant_id: participant.participant_id.clone(),
            orchestrator_id: digest_strings(
                b"perfect-planner:test-orchestrator-route:v1",
                [participant.participant_id.as_str()],
            ),
            run_identity: participant.run_identity.clone(),
            fence: participant.fence,
            lease_generation: participant.lease_generation,
            snapshot_store_binding: stored.store_binding().to_string(),
            destination_registration_digest: digest_strings(
                b"perfect-planner:test-destination-registration:v1",
                [participant.participant_id.as_str(), participant.node_id.as_str()],
            ),
            route_store_binding: digest_strings(
                b"perfect-planner:test-route-store:v1",
                [stored.store_binding()],
            ),
            app_instance_digest: digest_strings(
                b"perfect-planner:test-app-instance:v1",
                [snapshot.snapshot_hash()],
            ),
            route_generation: 1,
            issuer_epoch: 1,
            route_expires_at_ms: 10_000,
        }
    }

    fn acknowledgement_receipt(
        snapshot: &VerifiedAssessmentSnapshot,
        participant_id: &str,
        _ticket_id: &str,
        signal_id: &str,
    ) -> MachineTicketAcknowledgementReceipt {
        let participant = snapshot
            .participants()
            .iter()
            .find(|candidate| candidate.participant_id == participant_id)
            .unwrap();
        MachineTicketAcknowledgementReceipt {
            snapshot_hash: snapshot.snapshot_hash().to_string(),
            signal_id: signal_id.to_string(),
            participant_id: participant.participant_id.clone(),
            run_identity: participant.run_identity.clone(),
            fence: participant.fence,
            lease_generation: participant.lease_generation,
            acknowledgement_digest: "3".repeat(64),
        }
    }

    #[test]
    fn broker_is_production_disabled_and_has_no_generic_control_plane_dependency() {
        let root = root("disabled");
        let broker = TicketBroker::new_disabled(AssessmentJournal::new(root.join("journal")));
        let snapshot = fixture_conflict_snapshot(ConflictDisposition::Wait);
        let store = SnapshotStore::new_for_test(root.join("snapshots"));
        let receipt = store.persist(&snapshot).unwrap();
        assert!(matches!(
            broker.materialize_for_participant(
                &snapshot,
                &receipt,
                &route_receipt(
                    &snapshot,
                    &receipt,
                    &snapshot.participants()[0].participant_id
                ),
                1_200
            ),
            Err(TicketError::ProductionDisabled)
        ));
        let source = include_str!("tickets.rs");
        assert!(!source.contains(&["crate::", "control_plane"].concat()));
        assert!(!source.contains(&["crate::", "connectors"].concat()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn both_mailboxes_see_only_the_exact_snapshot_bound_ticket() {
        let (root, broker, snapshot, receipt) = setup("mailboxes", ConflictDisposition::Wait);
        for participant in snapshot.participants() {
            let capability = broker
                .materialize_for_participant(
                    &snapshot,
                    &receipt,
                    &route_receipt(&snapshot, &receipt, &participant.participant_id),
                    1_200,
                )
                .unwrap();
            let page = broker.list_own(&capability, 0, 100, 1_300).unwrap();
            assert_eq!(page.tickets.len(), 1);
            assert_eq!(page.tickets[0].disposition, ConflictDisposition::Wait);
            assert_eq!(page.tickets[0].overlaps[0].canonical_key, "1".repeat(64));
            assert!(page.next_cursor.is_none());
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn capability_is_owner_scoped_epoch_bound_and_not_a_ticket_lookup_api() {
        let (root, broker, snapshot, receipt) = setup("scope", ConflictDisposition::Wait);
        let capability = broker
            .materialize_for_participant(
                &snapshot,
                &receipt,
                &route_receipt(
                    &snapshot,
                    &receipt,
                    &snapshot.participants()[0].participant_id,
                ),
                1_200,
            )
            .unwrap();
        let restarted = TicketBroker::new_enabled_for_test(broker.journal.clone());
        assert!(matches!(
            restarted.list_own(&capability, 0, 100, 1_300),
            Err(TicketError::Denied)
        ));
        assert!(matches!(
            broker.list_own(&capability, 0, 101, 1_300),
            Err(TicketError::LimitExceeded)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mailbox_route_receipt_is_registration_bound_and_expiring() {
        let (root, broker, snapshot, receipt) =
            setup("route-binding", ConflictDisposition::Wait);
        let participant_id = &snapshot.participants()[0].participant_id;
        let valid = route_receipt(&snapshot, &receipt, participant_id);
        broker
            .materialize_for_participant(&snapshot, &receipt, &valid, 1_200)
            .unwrap();

        let mut stale = valid.clone();
        stale.route_expires_at_ms = 1_200;
        assert!(matches!(
            broker.materialize_for_participant(&snapshot, &receipt, &stale, 1_200),
            Err(TicketError::InvalidReceipt)
        ));

        let mut unregistered = valid.clone();
        unregistered.destination_registration_digest = "not-a-route".to_string();
        assert!(matches!(
            broker.materialize_for_participant(&snapshot, &receipt, &unregistered, 1_200),
            Err(TicketError::InvalidReceipt)
        ));

        let mut prior_epoch = valid;
        prior_epoch.issuer_epoch = 0;
        assert!(matches!(
            broker.materialize_for_participant(&snapshot, &receipt, &prior_epoch, 1_200),
            Err(TicketError::InvalidReceipt)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn minted_mailbox_cannot_outlive_or_mutate_its_route_authority() {
        let (root, broker, snapshot, stored) =
            setup("minted-route-expiry", ConflictDisposition::Wait);
        let mut short_route = route_receipt(
            &snapshot,
            &stored,
            &snapshot.participants()[0].participant_id,
        );
        short_route.route_expires_at_ms = 1_300;
        let actor = broker
            .materialize_for_participant(&snapshot, &stored, &short_route, 1_200)
            .unwrap();
        assert_eq!(actor.mutation_expires_at_ms, 1_300);
        let ticket_id = actor.allowed_ticket_ids.iter().next().unwrap().clone();

        let mut drifted = actor.clone();
        drifted.route_generation += 1;
        assert!(matches!(
            broker.list_own(&drifted, 0, 100, 1_201),
            Err(TicketError::Denied)
        ));
        assert_eq!(broker.journal.read_verified().unwrap().len(), 1);

        let before_expiry = signal_receipt(
            &snapshot,
            &actor.participant_id,
            &ticket_id,
            TicketSignalKind::NodeDone,
            &"b".repeat(64),
        );
        let signal_id = broker
            .publish_signal(&actor, &before_expiry, 1_299)
            .unwrap();
        let after_expiry = signal_receipt(
            &snapshot,
            &actor.participant_id,
            &ticket_id,
            TicketSignalKind::NodeDone,
            &"c".repeat(64),
        );
        assert!(matches!(
            broker.publish_signal(&actor, &after_expiry, 1_300),
            Err(TicketError::Denied)
        ));
        let expired_ack = acknowledgement_receipt(
            &snapshot,
            &actor.participant_id,
            &ticket_id,
            &signal_id,
        );
        assert!(matches!(
            broker.acknowledge_signal(&actor, &expired_ack, 1_300),
            Err(TicketError::Denied)
        ));
        assert_eq!(broker.journal.read_verified().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn signal_and_ack_are_exactly_idempotent_and_survive_restart() {
        let (root, broker, snapshot, receipt) = setup("signal", ConflictDisposition::Wait);
        let left = broker
            .materialize_for_participant(
                &snapshot,
                &receipt,
                &route_receipt(
                    &snapshot,
                    &receipt,
                    &snapshot.participants()[0].participant_id,
                ),
                1_200,
            )
            .unwrap();
        let right = broker
            .materialize_for_participant(
                &snapshot,
                &receipt,
                &route_receipt(
                    &snapshot,
                    &receipt,
                    &snapshot.participants()[1].participant_id,
                ),
                1_201,
            )
            .unwrap();
        let ticket_id = broker.list_own(&left, 0, 1, 1_300).unwrap().tickets[0]
            .ticket_id
            .clone();
        let machine_receipt = signal_receipt(
            &snapshot,
            &left.participant_id,
            &ticket_id,
            TicketSignalKind::NodeDone,
            &"4".repeat(64),
        );
        let first = broker
            .publish_signal(&left, &machine_receipt, 1_400)
            .unwrap();
        let duplicate = broker
            .publish_signal(&left, &machine_receipt, 1_401)
            .unwrap();
        assert_eq!(first, duplicate);
        let mut contradictory = machine_receipt.clone();
        contradictory.source_state_digest = "7".repeat(64);
        assert!(matches!(
            broker.publish_signal(&left, &contradictory, 1_402,),
            Err(TicketError::Journal(_))
        ));
        let mut foreign_ticket = machine_receipt.clone();
        foreign_ticket.source_node_id = "foreign-node".to_string();
        assert!(matches!(
            broker.publish_signal(&left, &foreign_ticket, 1_403,),
            Err(TicketError::InvalidReceipt)
        ));
        let pending = broker.list_own_signals(&right, 0, 100, 1_450).unwrap();
        assert_eq!(pending.signals.len(), 1);
        assert_eq!(pending.signals[0].signal_id, first);
        assert!(!pending.signals[0].acknowledged);

        // Reopen from the actual journal path, mint a fresh epoch-bound mailbox, and resume the
        // persisted recipient delivery without registry or repository discovery.
        let restarted = TicketBroker::new_enabled_for_test(AssessmentJournal::new_live_for_test(
            root.join("assessment.jsonl"),
        ));
        assert!(matches!(
            restarted.list_own_signals(&right, 0, 100, 1_451),
            Err(TicketError::Denied)
        ));
        let restarted_right = restarted
            .materialize_for_participant(
                &snapshot,
                &receipt,
                &route_receipt(
                    &snapshot,
                    &receipt,
                    &snapshot.participants()[1].participant_id,
                ),
                1_460,
            )
            .unwrap();
        let pending_after_reopen = restarted
            .list_own_signals(&restarted_right, 0, 100, 1_470)
            .unwrap();
        assert_eq!(pending_after_reopen.signals.len(), 1);
        assert!(!pending_after_reopen.signals[0].acknowledged);

        let ack = acknowledgement_receipt(
            &snapshot,
            &restarted_right.participant_id,
            &ticket_id,
            &first,
        );
        restarted
            .acknowledge_signal(&restarted_right, &ack, 1_500)
            .unwrap();
        restarted
            .acknowledge_signal(&restarted_right, &ack, 1_501)
            .unwrap();
        let mut contradictory_ack = ack.clone();
        contradictory_ack.acknowledgement_digest = "8".repeat(64);
        assert!(matches!(
            restarted.acknowledge_signal(&restarted_right, &contradictory_ack, 1_502,),
            Err(TicketError::Journal(_))
        ));
        let reopened_again = TicketBroker::new_enabled_for_test(
            AssessmentJournal::new_live_for_test(root.join("assessment.jsonl")),
        );
        let final_right = reopened_again
            .materialize_for_participant(
                &snapshot,
                &receipt,
                &route_receipt(
                    &snapshot,
                    &receipt,
                    &snapshot.participants()[1].participant_id,
                ),
                1_600,
            )
            .unwrap();
        let delivered = reopened_again
            .list_own_signals(&final_right, 0, 100, 1_610)
            .unwrap();
        assert_eq!(delivered.signals.len(), 1);
        assert!(delivered.signals[0].acknowledged);
        let events = reopened_again.journal.read_verified().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.payload,
                    JournalPayload::ConflictTicketSignal { .. }
                ))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.payload,
                    JournalPayload::ConflictTicketAcknowledged { .. }
                ))
                .count(),
            1
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn disposition_and_manifest_revocation_are_fail_closed() {
        let (root, broker, snapshot, receipt) = setup("revoke", ConflictDisposition::Replan);
        let capability = broker
            .materialize_for_participant(
                &snapshot,
                &receipt,
                &route_receipt(
                    &snapshot,
                    &receipt,
                    &snapshot.participants()[0].participant_id,
                ),
                1_200,
            )
            .unwrap();
        let recipient = broker
            .materialize_for_participant(
                &snapshot,
                &receipt,
                &route_receipt(
                    &snapshot,
                    &receipt,
                    &snapshot.participants()[1].participant_id,
                ),
                1_201,
            )
            .unwrap();
        let ticket_id = broker.list_own(&capability, 0, 1, 1_300).unwrap().tickets[0]
            .ticket_id
            .clone();
        let wrong = signal_receipt(
            &snapshot,
            &capability.participant_id,
            &ticket_id,
            TicketSignalKind::DecisionRequired,
            &"5".repeat(64),
        );
        assert!(matches!(
            broker.publish_signal(&capability, &wrong, 1_400),
            Err(TicketError::Journal(_))
        ));
        let changed = signal_receipt(
            &snapshot,
            &capability.participant_id,
            &ticket_id,
            TicketSignalKind::ManifestChanged,
            &"6".repeat(64),
        );
        let prior = signal_receipt(
            &snapshot,
            &capability.participant_id,
            &ticket_id,
            TicketSignalKind::NodeDone,
            &"9".repeat(64),
        );
        let prior_signal_id = broker.publish_signal(&capability, &prior, 1_450).unwrap();
        let signal_id = broker.publish_signal(&capability, &changed, 1_500).unwrap();
        let audit_page = broker.list_own(&capability, 0, 100, 1_600).unwrap();
        assert!(audit_page.tickets[0].revoked);
        let recipient_inbox = broker.list_own_signals(&recipient, 0, 100, 1_600).unwrap();
        assert_eq!(recipient_inbox.signals.len(), 2);
        assert!(recipient_inbox
            .signals
            .iter()
            .all(|signal| !signal.acknowledged));
        let ack =
            acknowledgement_receipt(&snapshot, &recipient.participant_id, &ticket_id, &signal_id);
        broker.acknowledge_signal(&recipient, &ack, 1_601).unwrap();
        let delivered = broker.list_own_signals(&recipient, 0, 100, 1_602).unwrap();
        assert!(
            delivered
                .signals
                .iter()
                .find(|signal| signal.signal_id == signal_id)
                .unwrap()
                .acknowledged
        );
        // The self-revoking ManifestChanged fact remains exactly retryable after a lost response,
        // but an older signal or a new digest cannot cross the revoked boundary.
        assert_eq!(
            broker.publish_signal(&capability, &changed, 1_603).unwrap(),
            signal_id
        );
        assert!(matches!(
            broker.publish_signal(&capability, &prior, 1_604,),
            Err(TicketError::Journal(_))
        ));
        let mut new_manifest_fact = changed.clone();
        new_manifest_fact.source_event_id = "7".repeat(64);
        assert!(matches!(
            broker.publish_signal(&capability, &new_manifest_fact, 1_605,),
            Err(TicketError::Journal(_))
        ));
        assert_ne!(prior_signal_id, signal_id);
        assert_eq!(
            broker
                .journal
                .read_verified()
                .unwrap()
                .iter()
                .filter(|event| matches!(
                    event.payload,
                    JournalPayload::ConflictTicketSignal { .. }
                ))
                .count(),
            2
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn revoked_mailbox_recovers_from_disk_after_expiry_without_discovery() {
        let (root, broker, snapshot, receipt) =
            setup("revoked-recovery", ConflictDisposition::Wait);
        let sender = broker
            .materialize_for_participant(
                &snapshot,
                &receipt,
                &route_receipt(
                    &snapshot,
                    &receipt,
                    &snapshot.participants()[0].participant_id,
                ),
                1_200,
            )
            .unwrap();
        let ticket_id = broker.list_own(&sender, 0, 1, 1_300).unwrap().tickets[0]
            .ticket_id
            .clone();
        let manifest_changed = signal_receipt(
            &snapshot,
            &sender.participant_id,
            &ticket_id,
            TicketSignalKind::ManifestChanged,
            &"d".repeat(64),
        );
        let signal_id = broker
            .publish_signal(&sender, &manifest_changed, 1_400)
            .unwrap();

        let reopened_journal = AssessmentJournal::new_live_for_test(root.join("assessment.jsonl"));
        let reopened_store = SnapshotStore::new_for_test(root.join("snapshots"));
        let (recovered_snapshot, recovered_receipt) = reopened_store
            .read_for_ticket_recovery(snapshot.snapshot_hash(), &reopened_journal)
            .unwrap();
        let restarted = TicketBroker::new_enabled_for_test(reopened_journal);
        let recipient_route = route_receipt(
            &recovered_snapshot,
            &recovered_receipt,
            &recovered_snapshot.participants()[1].participant_id,
        );
        let recipient = restarted
            .resume_for_participant(
                &recovered_snapshot,
                &recovered_receipt,
                &recipient_route,
                6_000,
            )
            .unwrap();
        let inbox = restarted
            .list_own_signals(&recipient, 0, 100, 6_001)
            .unwrap();
        assert_eq!(inbox.signals.len(), 1);
        assert_eq!(inbox.signals[0].signal_id, signal_id);
        assert!(!inbox.signals[0].acknowledged);
        let acknowledgement = acknowledgement_receipt(
            &recovered_snapshot,
            &recipient.participant_id,
            &ticket_id,
            &signal_id,
        );
        restarted
            .acknowledge_signal(&recipient, &acknowledgement, 6_002)
            .unwrap();
        let forbidden_publish = signal_receipt(
            &recovered_snapshot,
            &recipient.participant_id,
            &ticket_id,
            TicketSignalKind::NodeDone,
            &"e".repeat(64),
        );
        assert!(matches!(
            restarted.publish_signal(&recipient, &forbidden_publish, 6_003),
            Err(TicketError::Denied)
        ));

        let reopened_again = TicketBroker::new_enabled_for_test(
            AssessmentJournal::new_live_for_test(root.join("assessment.jsonl")),
        );
        let final_recipient = reopened_again
            .resume_for_participant(
                &recovered_snapshot,
                &recovered_receipt,
                &recipient_route,
                7_000,
            )
            .unwrap();
        assert!(
            reopened_again
                .list_own_signals(&final_recipient, 0, 100, 7_001)
                .unwrap()
                .signals[0]
                .acknowledged
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn all_typed_signals_deliver_idempotently_only_for_their_disposition() {
        let cases = [
            (TicketSignalKind::NodeDone, ConflictDisposition::Wait),
            (TicketSignalKind::LeaseReleased, ConflictDisposition::Wait),
            (TicketSignalKind::ManifestChanged, ConflictDisposition::Wait),
            (
                TicketSignalKind::DecisionRequired,
                ConflictDisposition::UserDecision,
            ),
            (
                TicketSignalKind::ReplanRequired,
                ConflictDisposition::Replan,
            ),
        ];
        for (index, (kind, disposition)) in cases.into_iter().enumerate() {
            let (root, broker, snapshot, stored) = setup(&format!("typed-{index}"), disposition);
            let left = broker
                .materialize_for_participant(
                    &snapshot,
                    &stored,
                    &route_receipt(
                        &snapshot,
                        &stored,
                        &snapshot.participants()[0].participant_id,
                    ),
                    1_200,
                )
                .unwrap();
            let right = broker
                .materialize_for_participant(
                    &snapshot,
                    &stored,
                    &route_receipt(
                        &snapshot,
                        &stored,
                        &snapshot.participants()[1].participant_id,
                    ),
                    1_201,
                )
                .unwrap();
            let ticket_id = broker.list_own(&left, 0, 1, 1_300).unwrap().tickets[0]
                .ticket_id
                .clone();
            let signal = signal_receipt(
                &snapshot,
                &left.participant_id,
                &ticket_id,
                kind,
                &format!("{:064x}", index + 1),
            );
            let first = broker.publish_signal(&left, &signal, 1_400).unwrap();
            assert_eq!(broker.publish_signal(&left, &signal, 1_401).unwrap(), first);
            let inbox = broker.list_own_signals(&right, 0, 100, 1_500).unwrap();
            assert_eq!(inbox.signals.len(), 1);
            assert_eq!(inbox.signals[0].signal_kind, kind);
            let ack = acknowledgement_receipt(&snapshot, &right.participant_id, &ticket_id, &first);
            broker.acknowledge_signal(&right, &ack, 1_501).unwrap();
            assert!(
                broker
                    .list_own_signals(&right, 0, 100, 1_502)
                    .unwrap()
                    .signals[0]
                    .acknowledged
            );
            let _ = std::fs::remove_dir_all(root);
        }

        let (root, broker, snapshot, stored) =
            setup("typed-wrong-disposition", ConflictDisposition::Wait);
        let left = broker
            .materialize_for_participant(
                &snapshot,
                &stored,
                &route_receipt(
                    &snapshot,
                    &stored,
                    &snapshot.participants()[0].participant_id,
                ),
                1_200,
            )
            .unwrap();
        let ticket_id = broker.list_own(&left, 0, 1, 1_300).unwrap().tickets[0]
            .ticket_id
            .clone();
        for (index, kind) in [
            TicketSignalKind::DecisionRequired,
            TicketSignalKind::ReplanRequired,
        ]
        .into_iter()
        .enumerate()
        {
            let wrong = signal_receipt(
                &snapshot,
                &left.participant_id,
                &ticket_id,
                kind,
                &format!("{:064x}", index + 20),
            );
            assert!(matches!(
                broker.publish_signal(&left, &wrong, 1_400 + index as u64),
                Err(TicketError::Journal(_))
            ));
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn live_inbox_sequence_cursor_never_skips_concurrent_appends() {
        let (root, broker, snapshot, stored) = setup("stable-cursor", ConflictDisposition::Wait);
        let left = broker
            .materialize_for_participant(
                &snapshot,
                &stored,
                &route_receipt(
                    &snapshot,
                    &stored,
                    &snapshot.participants()[0].participant_id,
                ),
                1_200,
            )
            .unwrap();
        let right = broker
            .materialize_for_participant(
                &snapshot,
                &stored,
                &route_receipt(
                    &snapshot,
                    &stored,
                    &snapshot.participants()[1].participant_id,
                ),
                1_201,
            )
            .unwrap();
        let ticket_id = broker.list_own(&left, 0, 1, 1_300).unwrap().tickets[0]
            .ticket_id
            .clone();
        for index in 0..101u64 {
            let signal = signal_receipt(
                &snapshot,
                &left.participant_id,
                &ticket_id,
                TicketSignalKind::NodeDone,
                &format!("{index:064x}"),
            );
            broker
                .publish_signal(&left, &signal, 1_400 + index)
                .unwrap();
        }
        let first_page = broker.list_own_signals(&right, 0, 100, 1_600).unwrap();
        assert_eq!(first_page.signals.len(), 100);
        let cursor = first_page.next_cursor.unwrap();

        let concurrent = signal_receipt(
            &snapshot,
            &left.participant_id,
            &ticket_id,
            TicketSignalKind::NodeDone,
            &format!("{:064x}", 101u64),
        );
        broker.publish_signal(&left, &concurrent, 1_601).unwrap();
        let second_page = broker.list_own_signals(&right, cursor, 100, 1_602).unwrap();
        assert_eq!(second_page.signals.len(), 2);
        let all_ids = first_page
            .signals
            .iter()
            .chain(second_page.signals.iter())
            .map(|signal| signal.signal_id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(all_ids.len(), 102);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn maximum_ticket_set_materializes_lazily_and_journals_only_active_delivery() {
        let root = root("maximum-lazy");
        let snapshot = fixture_conflict_snapshot_with_count(ConflictDisposition::Wait, 8_192);
        let store = SnapshotStore::new_for_test(root.join("snapshots"));
        let stored = store.persist(&snapshot).unwrap();
        let journal = AssessmentJournal::new_live_for_test(root.join("assessment.jsonl"));
        journal
            .record_assessment(&snapshot, &stored, 1_100)
            .unwrap();
        let broker = TicketBroker::new_enabled_for_test(journal);
        let left = broker
            .materialize_for_participant(
                &snapshot,
                &stored,
                &route_receipt(
                    &snapshot,
                    &stored,
                    &snapshot.participants()[0].participant_id,
                ),
                1_200,
            )
            .unwrap();
        assert_eq!(left.allowed_ticket_ids.len(), 8_192);
        assert_eq!(broker.journal.read_verified().unwrap().len(), 1);

        let ticket_id = left.allowed_ticket_ids.iter().next().unwrap().clone();
        let signal = signal_receipt(
            &snapshot,
            &left.participant_id,
            &ticket_id,
            TicketSignalKind::NodeDone,
            &"b".repeat(64),
        );
        broker.publish_signal(&left, &signal, 1_300).unwrap();
        let events = broker.journal.read_verified().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.payload, JournalPayload::ConflictTicket { .. }))
                .count(),
            0
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ticket_index_cache_is_bounded_and_eviction_does_not_mutate_issued_capability() {
        let root = root("bounded-index-cache");
        let store = SnapshotStore::new_for_test(root.join("snapshots"));
        let journal = AssessmentJournal::new_live_for_test(root.join("assessment.jsonl"));
        let broker = TicketBroker::new_enabled_for_test(journal.clone());
        let mut first: Option<(
            VerifiedAssessmentSnapshot,
            StoredSnapshotReceipt,
            TicketMailboxCapability,
        )> = None;

        for offset in 0..(MAX_TICKET_INDEXES + 2) {
            let snapshot = fixture_conflict_snapshot_with_generation(
                ConflictDisposition::Wait,
                100 + offset as u64,
            );
            let stored = store.persist(&snapshot).unwrap();
            journal
                .record_assessment(&snapshot, &stored, 1_100 + offset as u64)
                .unwrap();
            let capability = broker
                .materialize_for_participant(
                    &snapshot,
                    &stored,
                    &route_receipt(
                        &snapshot,
                        &stored,
                        &snapshot.participants()[0].participant_id,
                    ),
                    1_200 + offset as u64,
                )
                .unwrap();
            if first.is_none() {
                first = Some((snapshot, stored, capability));
            }
        }

        let (first_snapshot, first_stored, first_capability) = first.unwrap();
        let first_key = digest_strings(
            b"perfect-planner:ticket-index:v1",
            [first_snapshot.snapshot_hash(), first_stored.store_binding()],
        );
        let cache = broker.ticket_indexes.lock().unwrap();
        assert_eq!(cache.entries.len(), MAX_TICKET_INDEXES);
        assert!(!cache.entries.contains_key(&first_key));
        drop(cache);

        assert_eq!(
            broker
                .list_own(&first_capability, 0, 100, 1_400)
                .unwrap()
                .tickets
                .len(),
            1
        );
        let transition = signal_receipt(
            &first_snapshot,
            &first_capability.participant_id,
            "",
            TicketSignalKind::NodeDone,
            &"9".repeat(64),
        );
        broker
            .publish_signal(&first_capability, &transition, 1_500)
            .unwrap();
        assert_eq!(broker.journal.read_verified().unwrap().len(), MAX_TICKET_INDEXES + 3);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn one_hundred_distinct_participants_publish_and_ack_owned_edges_in_parallel() {
        let root = root("hundred-participants");
        let snapshot = fixture_participant_chain_snapshot(100);
        let store = SnapshotStore::new_for_test(root.join("snapshots"));
        let stored = store.persist(&snapshot).unwrap();
        let journal = AssessmentJournal::new_live_for_test(root.join("assessment.jsonl"));
        journal
            .record_assessment(&snapshot, &stored, 1_100)
            .unwrap();
        let broker = TicketBroker::new_enabled_for_test(journal);
        let routes = snapshot
            .participants()
            .iter()
            .map(|participant| route_receipt(&snapshot, &stored, &participant.participant_id))
            .collect::<Vec<_>>();
        let capabilities = broker
            .materialize_for_participants(&snapshot, &stored, &routes, 1_200)
            .unwrap()
            .into_iter()
            .map(|capability| (capability.participant_id.clone(), capability))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(capabilities.len(), 100);
        assert_eq!(broker.journal.read_verified().unwrap().len(), 1);

        let mut handles = Vec::new();
        for (index, conflict) in snapshot.conflicts().iter().enumerate() {
            let left = capabilities[&conflict.left_participant_id].clone();
            let ticket_id = left
                .tickets
                .values()
                .find(|ticket| {
                    ticket.left.participant_id == conflict.left_participant_id
                        && ticket.right.participant_id == conflict.right_participant_id
                })
                .unwrap()
                .ticket_id
                .clone();
            let signal = signal_receipt(
                &snapshot,
                &left.participant_id,
                &ticket_id,
                TicketSignalKind::NodeDone,
                &format!("{:064x}", index + 30_000),
            );
            let broker = broker.clone();
            let right_id = conflict.right_participant_id.clone();
            handles.push(std::thread::spawn(move || {
                let signal_id = broker
                    .publish_signal(&left, &signal, 1_300 + index as u64)
                    .unwrap();
                (right_id, ticket_id, signal_id)
            }));
        }
        let delivered = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(delivered.len(), 99);
        assert_eq!(
            delivered
                .iter()
                .map(|(_, _, signal_id)| signal_id)
                .collect::<BTreeSet<_>>()
                .len(),
            99
        );
        for (index, (recipient_id, ticket_id, signal_id)) in delivered.iter().enumerate() {
            let recipient = &capabilities[recipient_id];
            let ack =
                acknowledgement_receipt(&snapshot, &recipient.participant_id, ticket_id, signal_id);
            broker
                .acknowledge_signal(recipient, &ack, 1_600 + index as u64)
                .unwrap();
        }
        let events = broker.journal.read_verified().unwrap();
        assert_eq!(events.len(), 1 + 99 * 2);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.payload, JournalPayload::ConflictTicket { .. }))
                .count(),
            0
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn one_hundred_agent_complete_conflict_graph_stays_bounded_and_fans_out_once() {
        let root = root("hundred-clique");
        let snapshot = fixture_participant_clique_snapshot(100);
        assert_eq!(snapshot.conflicts().len(), 4_950);
        let store = SnapshotStore::new_for_test(root.join("snapshots"));
        let stored = store.persist(&snapshot).unwrap();
        let journal_path = root.join("assessment.jsonl");
        let journal = AssessmentJournal::new_live_for_test(&journal_path);
        journal
            .record_assessment(&snapshot, &stored, 1_100)
            .unwrap();
        assert!(std::fs::metadata(&journal_path).unwrap().len() < 1_048_576);
        let broker = TicketBroker::new_enabled_for_test(journal);
        let routes = snapshot
            .participants()
            .iter()
            .map(|participant| route_receipt(&snapshot, &stored, &participant.participant_id))
            .collect::<Vec<_>>();
        let capabilities = broker
            .materialize_for_participants(&snapshot, &stored, &routes, 1_200)
            .unwrap();
        assert_eq!(capabilities.len(), 100);
        assert!(capabilities
            .iter()
            .all(|capability| capability.allowed_ticket_ids.len() == 99));
        assert_eq!(
            capabilities
                .iter()
                .map(|capability| capability.allowed_ticket_ids.len())
                .sum::<usize>(),
            9_900
        );

        let publish_barrier = Arc::new(std::sync::Barrier::new(capabilities.len()));
        let transitions = capabilities
            .iter()
            .enumerate()
            .map(|(index, capability)| {
                signal_receipt(
                    &snapshot,
                    &capability.participant_id,
                    "",
                    TicketSignalKind::NodeDone,
                    &format!("{:064x}", index + 500_000),
                )
            })
            .collect::<Vec<_>>();
        let publish_handles = capabilities
            .iter()
            .cloned()
            .zip(transitions)
            .enumerate()
            .map(|(index, (capability, transition))| {
                let broker = broker.clone();
                let barrier = publish_barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let signal_id = broker
                        .publish_signal(&capability, &transition, 1_300 + index as u64)
                        .unwrap();
                    (capability.participant_id.clone(), signal_id)
                })
            })
            .collect::<Vec<_>>();
        let signals_by_actor = publish_handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<BTreeMap<_, _>>();
        assert_eq!(signals_by_actor.len(), 100);
        assert_eq!(broker.journal.read_verified().unwrap().len(), 101);

        let acknowledge_barrier = Arc::new(std::sync::Barrier::new(capabilities.len()));
        let acknowledgements = capabilities
            .iter()
            .enumerate()
            .map(|(index, recipient)| {
                let actor = &capabilities[(index + 1) % capabilities.len()];
                let signal_id = signals_by_actor[&actor.participant_id].clone();
                acknowledgement_receipt(
                    &snapshot,
                    &recipient.participant_id,
                    "",
                    &signal_id,
                )
            })
            .collect::<Vec<_>>();
        let acknowledge_handles = capabilities
            .iter()
            .cloned()
            .zip(acknowledgements)
            .enumerate()
            .map(|(index, (recipient, acknowledgement))| {
                let broker = broker.clone();
                let barrier = acknowledge_barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    broker
                        .acknowledge_signal(
                            &recipient,
                            &acknowledgement,
                            1_600 + index as u64,
                        )
                        .unwrap();
                })
            })
            .collect::<Vec<_>>();
        for handle in acknowledge_handles {
            handle.join().unwrap();
        }
        assert_eq!(broker.journal.read_verified().unwrap().len(), 201);
        for recipient in &capabilities {
            let inbox = broker.list_own_signals(recipient, 0, 100, 1_800).unwrap();
            assert_eq!(inbox.signals.len(), 99);
            assert!(inbox
                .signals
                .iter()
                .all(|signal| signal.ticket_ids.len() == 1));
            assert_eq!(
                inbox
                    .signals
                    .iter()
                    .filter(|signal| signal.acknowledged)
                    .count(),
                1
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn non_neighbor_cannot_acknowledge_a_signal_it_could_not_receive() {
        let root = root("foreign-ack");
        let snapshot = fixture_participant_chain_snapshot(3);
        let store = SnapshotStore::new_for_test(root.join("snapshots"));
        let stored = store.persist(&snapshot).unwrap();
        let journal = AssessmentJournal::new_live_for_test(root.join("assessment.jsonl"));
        journal
            .record_assessment(&snapshot, &stored, 1_100)
            .unwrap();
        let broker = TicketBroker::new_enabled_for_test(journal);
        let capabilities = snapshot
            .participants()
            .iter()
            .map(|participant| {
                broker
                    .materialize_for_participant(
                        &snapshot,
                        &stored,
                        &route_receipt(&snapshot, &stored, &participant.participant_id),
                        1_200,
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let actor = &capabilities[0];
        let distant = &capabilities[2];
        let ticket_id = actor.allowed_ticket_ids.iter().next().unwrap().clone();
        let signal = signal_receipt(
            &snapshot,
            &actor.participant_id,
            &ticket_id,
            TicketSignalKind::NodeDone,
            &"a".repeat(64),
        );
        let signal_id = broker.publish_signal(actor, &signal, 1_300).unwrap();
        assert!(broker
            .list_own_signals(distant, 0, 100, 1_301)
            .unwrap()
            .signals
            .is_empty());
        let forged_ack = acknowledgement_receipt(
            &snapshot,
            &distant.participant_id,
            &ticket_id,
            &signal_id,
        );
        assert!(matches!(
            broker.acknowledge_signal(distant, &forged_ack, 1_302),
            Err(TicketError::Journal(_))
        ));
        assert_eq!(broker.journal.read_verified().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn one_machine_source_event_cannot_change_signal_kind() {
        let cases = [
            (
                ConflictDisposition::Wait,
                TicketSignalKind::NodeDone,
                TicketSignalKind::LeaseReleased,
            ),
            (
                ConflictDisposition::Wait,
                TicketSignalKind::LeaseReleased,
                TicketSignalKind::ManifestChanged,
            ),
            (
                ConflictDisposition::UserDecision,
                TicketSignalKind::DecisionRequired,
                TicketSignalKind::NodeDone,
            ),
            (
                ConflictDisposition::Replan,
                TicketSignalKind::ReplanRequired,
                TicketSignalKind::NodeDone,
            ),
        ];
        for (index, (disposition, first_kind, contradictory_kind)) in
            cases.into_iter().enumerate()
        {
            let (root, broker, snapshot, stored) =
                setup(&format!("source-slot-{index}"), disposition);
            let actor = broker
                .materialize_for_participant(
                    &snapshot,
                    &stored,
                    &route_receipt(
                        &snapshot,
                        &stored,
                        &snapshot.participants()[0].participant_id,
                    ),
                    1_200,
                )
                .unwrap();
            let ticket_id = actor.allowed_ticket_ids.iter().next().unwrap().clone();
            let source_event_id = format!("{:064x}", 50_000 + index);
            let first = signal_receipt(
                &snapshot,
                &actor.participant_id,
                &ticket_id,
                first_kind,
                &source_event_id,
            );
            broker.publish_signal(&actor, &first, 1_300).unwrap();
            let contradictory = signal_receipt(
                &snapshot,
                &actor.participant_id,
                &ticket_id,
                contradictory_kind,
                &source_event_id,
            );
            assert!(matches!(
                broker.publish_signal(&actor, &contradictory, 1_301),
                Err(TicketError::Journal(_))
            ));
            assert_eq!(broker.journal.read_verified().unwrap().len(), 2);
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn one_manifest_transition_atomically_reaches_every_conflicting_peer() {
        let root = root("manifest-fanout");
        let snapshot = fixture_participant_chain_snapshot(3);
        let store = SnapshotStore::new_for_test(root.join("snapshots"));
        let stored = store.persist(&snapshot).unwrap();
        let journal = AssessmentJournal::new_live_for_test(root.join("assessment.jsonl"));
        journal
            .record_assessment(&snapshot, &stored, 1_100)
            .unwrap();
        let broker = TicketBroker::new_enabled_for_test(journal);
        let capabilities = snapshot
            .participants()
            .iter()
            .map(|participant| {
                broker
                    .materialize_for_participant(
                        &snapshot,
                        &stored,
                        &route_receipt(&snapshot, &stored, &participant.participant_id),
                        1_200,
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let actor = &capabilities[1];
        assert_eq!(actor.allowed_ticket_ids.len(), 2);
        let any_ticket = actor.allowed_ticket_ids.iter().next().unwrap().clone();
        let transition = signal_receipt(
            &snapshot,
            &actor.participant_id,
            &any_ticket,
            TicketSignalKind::ManifestChanged,
            &"f".repeat(64),
        );
        let signal_id = broker.publish_signal(actor, &transition, 1_300).unwrap();
        assert_eq!(
            broker.publish_signal(actor, &transition, 1_301).unwrap(),
            signal_id
        );
        for peer in [&capabilities[0], &capabilities[2]] {
            let inbox = broker.list_own_signals(peer, 0, 100, 1_400).unwrap();
            assert_eq!(inbox.signals.len(), 1);
            assert_eq!(inbox.signals[0].signal_id, signal_id);
            assert_eq!(inbox.signals[0].ticket_ids.len(), 1);
        }
        assert!(broker
            .list_own_signals(actor, 0, 100, 1_400)
            .unwrap()
            .signals
            .is_empty());
        assert_eq!(
            broker
                .journal
                .read_verified()
                .unwrap()
                .iter()
                .filter(|event| matches!(
                    event.payload,
                    JournalPayload::ConflictTicketSignal { .. }
                ))
                .count(),
            1
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn one_hundred_parallel_signal_retries_append_exactly_once() {
        let (root, broker, snapshot, receipt) = setup("parallel", ConflictDisposition::Wait);
        let capability = broker
            .materialize_for_participant(
                &snapshot,
                &receipt,
                &route_receipt(
                    &snapshot,
                    &receipt,
                    &snapshot.participants()[0].participant_id,
                ),
                1_200,
            )
            .unwrap();
        let ticket_id = broker.list_own(&capability, 0, 1, 1_300).unwrap().tickets[0]
            .ticket_id
            .clone();
        let machine_receipt = signal_receipt(
            &snapshot,
            &capability.participant_id,
            &ticket_id,
            TicketSignalKind::LeaseReleased,
            &"a".repeat(64),
        );
        let mut handles = Vec::new();
        for index in 0..100u64 {
            let broker = broker.clone();
            let capability = capability.clone();
            let machine_receipt = machine_receipt.clone();
            handles.push(std::thread::spawn(move || {
                broker.publish_signal(&capability, &machine_receipt, 1_400 + index)
            }));
        }
        let ids = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), 1);
        assert_eq!(
            broker
                .journal
                .read_verified()
                .unwrap()
                .iter()
                .filter(|event| matches!(
                    event.payload,
                    JournalPayload::ConflictTicketSignal { .. }
                ))
                .count(),
            1
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
