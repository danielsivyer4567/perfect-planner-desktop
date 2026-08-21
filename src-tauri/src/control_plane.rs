use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_RECENT_EVENTS: usize = 500;
const MAX_ID_LENGTH: usize = 512;
const MAX_SUBJECT_LENGTH: usize = 4_096;
const MAX_BODY_LENGTH: usize = 256 * 1_024;
const MAX_DELIVERY_ATTEMPTS: u32 = 100;
pub const DEFAULT_MAX_DELIVERY_ATTEMPTS: u32 = 3;
pub const DEFAULT_DELIVERY_LEASE_MS: u64 = 30_000;
pub const MAX_DELIVERY_LEASE_MS: u64 = 10 * 60 * 1_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageScope {
    pub organization_id: String,
    pub repository_id: String,
    pub repository_root: String,
    pub worktree_path: String,
    pub branch_name: String,
    pub plan_id: String,
    pub plan_path: String,
    pub node_id: String,
    #[serde(default)]
    pub item_id: Option<String>,
    pub worker_id: String,
    #[serde(default)]
    pub orchestrator_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageScopeFilter {
    #[serde(default)]
    pub organization_id: Option<String>,
    #[serde(default)]
    pub repository_id: Option<String>,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub branch_name: Option<String>,
    #[serde(default)]
    pub plan_id: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub worker_id: Option<String>,
}

impl MessageScopeFilter {
    fn matches(&self, scope: &MessageScope) -> bool {
        option_matches(&self.organization_id, &scope.organization_id, true)
            && option_matches(&self.repository_id, &scope.repository_id, true)
            && option_matches_path(&self.worktree_path, &scope.worktree_path)
            && option_matches(&self.branch_name, &scope.branch_name, false)
            && option_matches(&self.plan_id, &scope.plan_id, true)
            && option_matches(&self.node_id, &scope.node_id, true)
            && option_matches(&self.worker_id, &scope.worker_id, true)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MessageKind {
    WorkerNote,
    Handoff,
    DecisionRequest,
    DecisionResponse,
    Alert,
    Status,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ActorKind {
    Worker,
    Orchestrator,
    User,
    System,
    Connector,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageActor {
    pub kind: ActorKind,
    pub actor_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DestinationKind {
    Orchestrator,
    Worker,
    Chat,
    Ide,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageDestination {
    pub kind: DestinationKind,
    pub target_id: String,
    #[serde(default)]
    pub connector_id: Option<String>,
    /// A registered route identifier. A message without one remains UNROUTED.
    #[serde(default)]
    pub route_id: Option<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub requires_acknowledgement: bool,
    #[serde(default)]
    pub retry_base_ms: u64,
    #[serde(default)]
    pub registered_at_ms: Option<u64>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ControlMessage {
    pub id: String,
    pub scope: MessageScope,
    pub kind: MessageKind,
    pub sender: MessageActor,
    pub destination: MessageDestination,
    pub subject: String,
    pub body: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
    pub max_delivery_attempts: u32,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostMessageRequest {
    pub scope: MessageScope,
    pub kind: MessageKind,
    pub sender: MessageActor,
    pub destination: MessageDestination,
    #[serde(default)]
    pub subject: String,
    pub body: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
    #[serde(default = "default_max_delivery_attempts")]
    pub max_delivery_attempts: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
// Retained as the audited recovery primitive for explicitly routing an UNROUTED message.
// The current UI asks the operator to retry instead of silently using it.
#[allow(dead_code)]
pub struct RouteMessageRequest {
    pub message_id: String,
    pub route_id: String,
    pub routed_by: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimDeliveryRequest {
    pub message_id: String,
    pub claimant_id: String,
    #[serde(default = "default_delivery_lease_ms")]
    pub lease_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
// Retained for route-specific connector implementations; the Codex connector additionally
// validates destination metadata before claiming an exact message.
#[allow(dead_code)]
pub struct ClaimNextDeliveryRequest {
    pub route_id: String,
    pub claimant_id: String,
    #[serde(default = "default_delivery_lease_ms")]
    pub lease_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordDeliveryResultRequest {
    pub message_id: String,
    pub claim_id: String,
    pub claimant_id: String,
    pub succeeded: bool,
    #[serde(default)]
    pub receipt: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub retry_at_ms: Option<u64>,
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcknowledgeMessageRequest {
    pub message_id: String,
    pub acknowledged_by: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeliveryState {
    Unrouted,
    Queued,
    Claimed,
    Delivered,
    Acknowledged,
    DeadLetter,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryClaim {
    pub claim_id: String,
    pub message_id: String,
    pub route_id: String,
    pub claimant_id: String,
    pub attempt: u32,
    pub claimed_at_ms: u64,
    pub lease_expires_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    pub message: ControlMessage,
    pub state: DeliveryState,
    #[serde(default)]
    pub route_id: Option<String>,
    pub attempt_count: u32,
    #[serde(default)]
    pub active_claim: Option<DeliveryClaim>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub next_attempt_at_ms: Option<u64>,
    #[serde(default)]
    pub delivered_at_ms: Option<u64>,
    #[serde(default)]
    pub delivery_receipt: Option<String>,
    #[serde(default)]
    pub acknowledged_at_ms: Option<u64>,
    #[serde(default)]
    pub acknowledged_by: Option<String>,
    #[serde(default)]
    pub acknowledgement_note: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostMessageOutcome {
    pub message: MessageView,
    pub duplicate: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPlaneSnapshot {
    pub now_ms: u64,
    pub messages: Vec<MessageView>,
    pub recent_events: Vec<ControlPlaneEvent>,
    pub unrouted_count: usize,
    pub queued_count: usize,
    pub claimed_count: usize,
    pub delivered_count: usize,
    pub acknowledged_count: usize,
    pub dead_letter_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPlaneEvent {
    pub id: String,
    pub at_ms: u64,
    #[serde(flatten)]
    pub payload: ControlPlaneEventKind,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "type",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
pub enum ControlPlaneEventKind {
    MessageCreated {
        message: ControlMessage,
    },
    MessageRouted {
        message_id: String,
        route_id: String,
        routed_by: String,
    },
    DeliveryClaimed {
        claim: DeliveryClaim,
    },
    DeliverySucceeded {
        message_id: String,
        claim_id: String,
        claimant_id: String,
        #[serde(default)]
        receipt: Option<String>,
    },
    DeliveryFailed {
        message_id: String,
        claim_id: String,
        claimant_id: String,
        error: String,
        #[serde(default)]
        retry_at_ms: Option<u64>,
        terminal: bool,
    },
    DeliveryLeaseExpired {
        message_id: String,
        claim_id: String,
        claimant_id: String,
        error: String,
        #[serde(default)]
        retry_at_ms: Option<u64>,
        terminal: bool,
    },
    MessageAcknowledged {
        message_id: String,
        acknowledged_by: String,
        #[serde(default)]
        note: Option<String>,
    },
}

#[derive(Clone, Debug)]
struct MessageRuntime {
    message: ControlMessage,
    route_id: Option<String>,
    attempt_count: u32,
    active_claim: Option<DeliveryClaim>,
    last_error: Option<String>,
    next_attempt_at_ms: Option<u64>,
    delivered_at_ms: Option<u64>,
    delivery_receipt: Option<String>,
    dead_letter: bool,
    acknowledged_at_ms: Option<u64>,
    acknowledged_by: Option<String>,
    acknowledgement_note: Option<String>,
}

impl MessageRuntime {
    fn new(message: ControlMessage) -> Self {
        let route_id = message.destination.route_id.clone();
        Self {
            message,
            route_id,
            attempt_count: 0,
            active_claim: None,
            last_error: None,
            next_attempt_at_ms: None,
            delivered_at_ms: None,
            delivery_receipt: None,
            dead_letter: false,
            acknowledged_at_ms: None,
            acknowledged_by: None,
            acknowledgement_note: None,
        }
    }

    fn state(&self) -> DeliveryState {
        if self.acknowledged_at_ms.is_some() {
            DeliveryState::Acknowledged
        } else if self.delivered_at_ms.is_some() {
            DeliveryState::Delivered
        } else if self.dead_letter {
            DeliveryState::DeadLetter
        } else if self.active_claim.is_some() {
            DeliveryState::Claimed
        } else if self.route_id.is_some() {
            DeliveryState::Queued
        } else {
            DeliveryState::Unrouted
        }
    }

    fn view(&self) -> MessageView {
        MessageView {
            message: self.message.clone(),
            state: self.state(),
            route_id: self.route_id.clone(),
            attempt_count: self.attempt_count,
            active_claim: self.active_claim.clone(),
            last_error: self.last_error.clone(),
            next_attempt_at_ms: self.next_attempt_at_ms,
            delivered_at_ms: self.delivered_at_ms,
            delivery_receipt: self.delivery_receipt.clone(),
            acknowledged_at_ms: self.acknowledged_at_ms,
            acknowledged_by: self.acknowledged_by.clone(),
            acknowledgement_note: self.acknowledgement_note.clone(),
        }
    }
}

#[derive(Default)]
struct ControlPlaneInner {
    messages: BTreeMap<String, MessageRuntime>,
    idempotency: HashMap<String, String>,
    event_ids: HashSet<String>,
    recent_events: VecDeque<ControlPlaneEvent>,
    event_counter: u64,
    message_counter: u64,
    claim_counter: u64,
}

#[derive(Clone)]
pub struct ControlPlaneStore {
    inner: Arc<Mutex<ControlPlaneInner>>,
    ledger_path: Arc<PathBuf>,
}

impl ControlPlaneStore {
    pub fn open(ledger_path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = ledger_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create control-plane data directory: {error}"))?;
        }
        let mut inner = ControlPlaneInner::default();
        load_ledger(&ledger_path, &mut inner)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
            ledger_path: Arc::new(ledger_path),
        })
    }

    pub fn post_message(
        &self,
        request: PostMessageRequest,
        now_ms: u64,
    ) -> Result<PostMessageOutcome, String> {
        validate_post_request(&request)?;
        let mut inner = self.lock()?;
        reap_expired_claims_locked(&mut inner, &self.ledger_path, now_ms)?;

        let idempotency_index = idempotency_index(&request.scope, &request.idempotency_key);
        if let Some(existing_id) = inner.idempotency.get(&idempotency_index) {
            let existing = inner
                .messages
                .get(existing_id)
                .ok_or_else(|| "control-plane idempotency index is inconsistent".to_string())?;
            if !message_matches_request(&existing.message, &request) {
                return Err(format!(
                    "idempotency key already belongs to a different message in this scope: {}",
                    request.idempotency_key
                ));
            }
            return Ok(PostMessageOutcome {
                message: existing.view(),
                duplicate: true,
            });
        }

        inner.message_counter = inner.message_counter.saturating_add(1);
        let message = ControlMessage {
            id: format!("pp-message-{now_ms}-{}", inner.message_counter),
            scope: request.scope,
            kind: request.kind,
            sender: request.sender,
            destination: request.destination,
            subject: request.subject,
            body: request.body,
            idempotency_key: request.idempotency_key,
            correlation_id: request.correlation_id,
            reply_to_message_id: request.reply_to_message_id,
            max_delivery_attempts: request.max_delivery_attempts,
            created_at_ms: now_ms,
        };
        let event = next_event(
            &mut inner,
            now_ms,
            ControlPlaneEventKind::MessageCreated {
                message: message.clone(),
            },
        );
        commit_events(&mut inner, &self.ledger_path, vec![event])?;
        let view = inner
            .messages
            .get(&message.id)
            .expect("a committed message event must produce a message")
            .view();
        Ok(PostMessageOutcome {
            message: view,
            duplicate: false,
        })
    }

    #[allow(dead_code)]
    pub fn route_message(
        &self,
        request: RouteMessageRequest,
        now_ms: u64,
    ) -> Result<MessageView, String> {
        validate_id("messageId", &request.message_id)?;
        validate_id("routeId", &request.route_id)?;
        validate_id("routedBy", &request.routed_by)?;
        let mut inner = self.lock()?;
        reap_expired_claims_locked(&mut inner, &self.ledger_path, now_ms)?;
        let existing = inner
            .messages
            .get(&request.message_id)
            .ok_or_else(|| format!("unknown control-plane message: {}", request.message_id))?;
        if existing.delivered_at_ms.is_some() || existing.dead_letter {
            return Err("cannot route a delivered or dead-letter message".to_string());
        }
        if existing.active_claim.is_some() {
            return Err("cannot change routing while delivery is claimed".to_string());
        }
        if let Some(route_id) = &existing.route_id {
            if route_id == &request.route_id {
                return Ok(existing.view());
            }
            return Err(format!("message is already assigned to route: {route_id}"));
        }
        let event = next_event(
            &mut inner,
            now_ms,
            ControlPlaneEventKind::MessageRouted {
                message_id: request.message_id.clone(),
                route_id: request.route_id,
                routed_by: request.routed_by,
            },
        );
        commit_events(&mut inner, &self.ledger_path, vec![event])?;
        Ok(inner
            .messages
            .get(&request.message_id)
            .expect("a routed message must still exist")
            .view())
    }

    pub fn claim_delivery(
        &self,
        request: ClaimDeliveryRequest,
        now_ms: u64,
    ) -> Result<DeliveryClaim, String> {
        validate_id("messageId", &request.message_id)?;
        validate_id("claimantId", &request.claimant_id)?;
        validate_lease(request.lease_ms)?;
        let mut inner = self.lock()?;
        reap_expired_claims_locked(&mut inner, &self.ledger_path, now_ms)?;
        claim_delivery_locked(
            &mut inner,
            &self.ledger_path,
            &request.message_id,
            &request.claimant_id,
            request.lease_ms,
            now_ms,
        )
    }

    #[allow(dead_code)]
    pub fn claim_next_delivery(
        &self,
        request: ClaimNextDeliveryRequest,
        now_ms: u64,
    ) -> Result<Option<DeliveryClaim>, String> {
        validate_id("routeId", &request.route_id)?;
        validate_id("claimantId", &request.claimant_id)?;
        validate_lease(request.lease_ms)?;
        let mut inner = self.lock()?;
        reap_expired_claims_locked(&mut inner, &self.ledger_path, now_ms)?;
        let message_id = inner
            .messages
            .values()
            .filter(|runtime| {
                runtime.route_id.as_deref() == Some(request.route_id.as_str())
                    && runtime.state() == DeliveryState::Queued
                    && runtime
                        .next_attempt_at_ms
                        .is_none_or(|retry_at| retry_at <= now_ms)
            })
            .min_by_key(|runtime| (runtime.message.created_at_ms, runtime.message.id.clone()))
            .map(|runtime| runtime.message.id.clone());
        match message_id {
            Some(message_id) => claim_delivery_locked(
                &mut inner,
                &self.ledger_path,
                &message_id,
                &request.claimant_id,
                request.lease_ms,
                now_ms,
            )
            .map(Some),
            None => Ok(None),
        }
    }

    pub fn record_delivery_result(
        &self,
        request: RecordDeliveryResultRequest,
        now_ms: u64,
    ) -> Result<MessageView, String> {
        validate_id("messageId", &request.message_id)?;
        validate_id("claimId", &request.claim_id)?;
        validate_id("claimantId", &request.claimant_id)?;
        if request.succeeded {
            if request
                .error
                .as_ref()
                .is_some_and(|error| !error.trim().is_empty())
            {
                return Err("a successful delivery cannot include an error".to_string());
            }
        } else {
            validate_required_text(
                "error",
                request.error.as_deref().unwrap_or_default(),
                MAX_BODY_LENGTH,
            )?;
            if request.receipt.is_some() {
                return Err("a failed delivery cannot include a receipt".to_string());
            }
        }

        let mut inner = self.lock()?;
        reap_expired_claims_locked(&mut inner, &self.ledger_path, now_ms)?;
        let runtime = inner
            .messages
            .get(&request.message_id)
            .ok_or_else(|| format!("unknown control-plane message: {}", request.message_id))?;
        let claim = runtime.active_claim.as_ref().ok_or_else(|| {
            "delivery result rejected because the message has no active claim".to_string()
        })?;
        if claim.claim_id != request.claim_id || claim.claimant_id != request.claimant_id {
            return Err("delivery result does not own the active claim".to_string());
        }

        let payload = if request.succeeded {
            ControlPlaneEventKind::DeliverySucceeded {
                message_id: request.message_id.clone(),
                claim_id: request.claim_id,
                claimant_id: request.claimant_id,
                receipt: request.receipt,
            }
        } else {
            let terminal =
                request.terminal || runtime.attempt_count >= runtime.message.max_delivery_attempts;
            ControlPlaneEventKind::DeliveryFailed {
                message_id: request.message_id.clone(),
                claim_id: request.claim_id,
                claimant_id: request.claimant_id,
                error: request
                    .error
                    .unwrap_or_else(|| "delivery failed".to_string()),
                retry_at_ms: (!terminal).then_some(request.retry_at_ms.unwrap_or(now_ms)),
                terminal,
            }
        };
        let event = next_event(&mut inner, now_ms, payload);
        commit_events(&mut inner, &self.ledger_path, vec![event])?;
        Ok(inner
            .messages
            .get(&request.message_id)
            .expect("a delivery result cannot remove its message")
            .view())
    }

    pub fn acknowledge_message(
        &self,
        request: AcknowledgeMessageRequest,
        now_ms: u64,
    ) -> Result<MessageView, String> {
        validate_id("messageId", &request.message_id)?;
        validate_id("acknowledgedBy", &request.acknowledged_by)?;
        if let Some(note) = &request.note {
            validate_optional_text("note", note, MAX_BODY_LENGTH)?;
        }
        let mut inner = self.lock()?;
        reap_expired_claims_locked(&mut inner, &self.ledger_path, now_ms)?;
        let runtime = inner
            .messages
            .get(&request.message_id)
            .ok_or_else(|| format!("unknown control-plane message: {}", request.message_id))?;
        if let Some(existing_actor) = &runtime.acknowledged_by {
            if existing_actor == &request.acknowledged_by {
                return Ok(runtime.view());
            }
            return Err(format!(
                "message was already acknowledged by {existing_actor}"
            ));
        }
        if runtime.delivered_at_ms.is_none() {
            return Err(
                "message cannot be acknowledged before durable delivery success".to_string(),
            );
        }
        let event = next_event(
            &mut inner,
            now_ms,
            ControlPlaneEventKind::MessageAcknowledged {
                message_id: request.message_id.clone(),
                acknowledged_by: request.acknowledged_by,
                note: request.note,
            },
        );
        commit_events(&mut inner, &self.ledger_path, vec![event])?;
        Ok(inner
            .messages
            .get(&request.message_id)
            .expect("acknowledgement cannot remove its message")
            .view())
    }

    pub fn snapshot(&self, now_ms: u64) -> Result<ControlPlaneSnapshot, String> {
        self.snapshot_filtered(None, now_ms)
    }

    pub fn snapshot_filtered(
        &self,
        filter: Option<&MessageScopeFilter>,
        now_ms: u64,
    ) -> Result<ControlPlaneSnapshot, String> {
        let mut inner = self.lock()?;
        reap_expired_claims_locked(&mut inner, &self.ledger_path, now_ms)?;
        Ok(snapshot_locked(&inner, filter, now_ms))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ControlPlaneInner>, String> {
        self.inner
            .lock()
            .map_err(|_| "control-plane state lock is poisoned".to_string())
    }
}

fn claim_delivery_locked(
    inner: &mut ControlPlaneInner,
    ledger_path: &Path,
    message_id: &str,
    claimant_id: &str,
    lease_ms: u64,
    now_ms: u64,
) -> Result<DeliveryClaim, String> {
    let runtime = inner
        .messages
        .get(message_id)
        .ok_or_else(|| format!("unknown control-plane message: {message_id}"))?;
    match runtime.state() {
        DeliveryState::Unrouted => {
            return Err("delivery cannot be claimed until a route is registered".to_string())
        }
        DeliveryState::Queued => {}
        DeliveryState::Claimed => return Err("delivery already has an active claim".to_string()),
        DeliveryState::Delivered => return Err("message was already delivered".to_string()),
        DeliveryState::Acknowledged => return Err("message was already acknowledged".to_string()),
        DeliveryState::DeadLetter => return Err("message is dead-lettered".to_string()),
    }
    if runtime
        .next_attempt_at_ms
        .is_some_and(|retry_at| retry_at > now_ms)
    {
        return Err(format!(
            "delivery retry is not due until {}",
            runtime.next_attempt_at_ms.unwrap_or_default()
        ));
    }
    if runtime.attempt_count >= runtime.message.max_delivery_attempts {
        return Err("message has exhausted its delivery attempts".to_string());
    }
    let route_id = runtime
        .route_id
        .clone()
        .ok_or_else(|| "delivery route disappeared".to_string())?;
    let attempt = runtime.attempt_count.saturating_add(1);
    inner.claim_counter = inner.claim_counter.saturating_add(1);
    let claim = DeliveryClaim {
        claim_id: format!("pp-claim-{now_ms}-{}", inner.claim_counter),
        message_id: message_id.to_string(),
        route_id,
        claimant_id: claimant_id.to_string(),
        attempt,
        claimed_at_ms: now_ms,
        lease_expires_at_ms: now_ms.saturating_add(lease_ms),
    };
    let event = next_event(
        inner,
        now_ms,
        ControlPlaneEventKind::DeliveryClaimed {
            claim: claim.clone(),
        },
    );
    commit_events(inner, ledger_path, vec![event])?;
    Ok(claim)
}

fn reap_expired_claims_locked(
    inner: &mut ControlPlaneInner,
    ledger_path: &Path,
    now_ms: u64,
) -> Result<(), String> {
    let expired: Vec<_> = inner
        .messages
        .values()
        .filter_map(|runtime| {
            let claim = runtime.active_claim.as_ref()?;
            (claim.lease_expires_at_ms <= now_ms).then_some((
                claim.clone(),
                runtime.attempt_count >= runtime.message.max_delivery_attempts,
            ))
        })
        .collect();
    if expired.is_empty() {
        return Ok(());
    }
    let mut events = Vec::with_capacity(expired.len());
    for (claim, terminal) in expired {
        events.push(next_event(
            inner,
            now_ms,
            ControlPlaneEventKind::DeliveryLeaseExpired {
                message_id: claim.message_id,
                claim_id: claim.claim_id,
                claimant_id: claim.claimant_id,
                error: "delivery claim lease expired before a result was recorded".to_string(),
                retry_at_ms: (!terminal).then_some(now_ms),
                terminal,
            },
        ));
    }
    commit_events(inner, ledger_path, events)
}

fn snapshot_locked(
    inner: &ControlPlaneInner,
    filter: Option<&MessageScopeFilter>,
    now_ms: u64,
) -> ControlPlaneSnapshot {
    let mut messages: Vec<_> = inner
        .messages
        .values()
        .filter(|runtime| filter.is_none_or(|filter| filter.matches(&runtime.message.scope)))
        .map(MessageRuntime::view)
        .collect();
    messages.sort_by(|left, right| {
        left.message
            .created_at_ms
            .cmp(&right.message.created_at_ms)
            .then_with(|| left.message.id.cmp(&right.message.id))
    });
    let count = |state| {
        messages
            .iter()
            .filter(|message| message.state == state)
            .count()
    };
    ControlPlaneSnapshot {
        now_ms,
        unrouted_count: count(DeliveryState::Unrouted),
        queued_count: count(DeliveryState::Queued),
        claimed_count: count(DeliveryState::Claimed),
        delivered_count: count(DeliveryState::Delivered),
        acknowledged_count: count(DeliveryState::Acknowledged),
        dead_letter_count: count(DeliveryState::DeadLetter),
        messages,
        recent_events: inner
            .recent_events
            .iter()
            .filter(|event| {
                filter.is_none_or(|filter| {
                    event_message_id(event)
                        .and_then(|message_id| inner.messages.get(message_id))
                        .is_some_and(|runtime| filter.matches(&runtime.message.scope))
                })
            })
            .cloned()
            .collect(),
    }
}

fn event_message_id(event: &ControlPlaneEvent) -> Option<&str> {
    match &event.payload {
        ControlPlaneEventKind::MessageCreated { message } => Some(&message.id),
        ControlPlaneEventKind::MessageRouted { message_id, .. }
        | ControlPlaneEventKind::DeliverySucceeded { message_id, .. }
        | ControlPlaneEventKind::DeliveryFailed { message_id, .. }
        | ControlPlaneEventKind::DeliveryLeaseExpired { message_id, .. }
        | ControlPlaneEventKind::MessageAcknowledged { message_id, .. } => Some(message_id),
        ControlPlaneEventKind::DeliveryClaimed { claim } => Some(&claim.message_id),
    }
}

fn next_event(
    inner: &mut ControlPlaneInner,
    at_ms: u64,
    payload: ControlPlaneEventKind,
) -> ControlPlaneEvent {
    inner.event_counter = inner.event_counter.saturating_add(1);
    ControlPlaneEvent {
        id: format!("pp-control-event-{at_ms}-{}", inner.event_counter),
        at_ms,
        payload,
    }
}

fn commit_events(
    inner: &mut ControlPlaneInner,
    ledger_path: &Path,
    events: Vec<ControlPlaneEvent>,
) -> Result<(), String> {
    // Validate and commit one event at a time. Durable append always precedes the
    // matching live-state change, and an error on a later event cannot hide an
    // earlier event that was already flushed successfully.
    for event in events {
        let mut projected = clone_inner_for_validation(inner);
        apply_event(&mut projected, event.clone())?;
        append_events(ledger_path, std::slice::from_ref(&event))?;
        apply_event(inner, event)?;
    }
    Ok(())
}

fn clone_inner_for_validation(inner: &ControlPlaneInner) -> ControlPlaneInner {
    ControlPlaneInner {
        messages: inner.messages.clone(),
        idempotency: inner.idempotency.clone(),
        event_ids: inner.event_ids.clone(),
        recent_events: inner.recent_events.clone(),
        event_counter: inner.event_counter,
        message_counter: inner.message_counter,
        claim_counter: inner.claim_counter,
    }
}

fn apply_event(inner: &mut ControlPlaneInner, event: ControlPlaneEvent) -> Result<(), String> {
    if !inner.event_ids.insert(event.id.clone()) {
        return Err(format!("duplicate control-plane event id: {}", event.id));
    }
    inner.event_counter = inner
        .event_counter
        .max(trailing_counter(&event.id).unwrap_or(inner.event_counter));
    match &event.payload {
        ControlPlaneEventKind::MessageCreated { message } => {
            validate_message(message)?;
            if inner.messages.contains_key(&message.id) {
                return Err(format!(
                    "duplicate control-plane message id: {}",
                    message.id
                ));
            }
            let key = idempotency_index(&message.scope, &message.idempotency_key);
            if inner.idempotency.contains_key(&key) {
                return Err(format!(
                    "duplicate idempotency key while replaying message: {}",
                    message.idempotency_key
                ));
            }
            inner.idempotency.insert(key, message.id.clone());
            inner
                .messages
                .insert(message.id.clone(), MessageRuntime::new(message.clone()));
            inner.message_counter = inner
                .message_counter
                .max(trailing_counter(&message.id).unwrap_or(inner.message_counter));
        }
        ControlPlaneEventKind::MessageRouted {
            message_id,
            route_id,
            routed_by,
        } => {
            validate_id("messageId", message_id)?;
            validate_id("routeId", route_id)?;
            validate_id("routedBy", routed_by)?;
            let runtime = message_runtime_mut(inner, message_id)?;
            if runtime.route_id.is_some() {
                return Err(format!("message already has a route: {message_id}"));
            }
            if runtime.active_claim.is_some()
                || runtime.delivered_at_ms.is_some()
                || runtime.dead_letter
            {
                return Err(format!(
                    "message cannot be routed in its current state: {message_id}"
                ));
            }
            runtime.route_id = Some(route_id.clone());
        }
        ControlPlaneEventKind::DeliveryClaimed { claim } => {
            validate_id("claimId", &claim.claim_id)?;
            validate_id("messageId", &claim.message_id)?;
            validate_id("routeId", &claim.route_id)?;
            validate_id("claimantId", &claim.claimant_id)?;
            if claim.lease_expires_at_ms <= claim.claimed_at_ms
                || claim
                    .lease_expires_at_ms
                    .saturating_sub(claim.claimed_at_ms)
                    > MAX_DELIVERY_LEASE_MS
            {
                return Err(format!(
                    "delivery claim lease is invalid: {}",
                    claim.claim_id
                ));
            }
            let runtime = message_runtime_mut(inner, &claim.message_id)?;
            if runtime.state() != DeliveryState::Queued {
                return Err(format!(
                    "message cannot be claimed in its current state: {}",
                    claim.message_id
                ));
            }
            if runtime.route_id.as_ref() != Some(&claim.route_id) {
                return Err(format!(
                    "delivery claim route mismatch: {}",
                    claim.message_id
                ));
            }
            if claim.attempt != runtime.attempt_count.saturating_add(1)
                || claim.attempt > runtime.message.max_delivery_attempts
            {
                return Err(format!(
                    "delivery claim attempt is invalid: {}",
                    claim.message_id
                ));
            }
            if runtime
                .next_attempt_at_ms
                .is_some_and(|retry_at| retry_at > claim.claimed_at_ms)
            {
                return Err(format!(
                    "delivery claim precedes retry time: {}",
                    claim.message_id
                ));
            }
            runtime.attempt_count = claim.attempt;
            runtime.active_claim = Some(claim.clone());
            runtime.next_attempt_at_ms = None;
            inner.claim_counter = inner
                .claim_counter
                .max(trailing_counter(&claim.claim_id).unwrap_or(inner.claim_counter));
        }
        ControlPlaneEventKind::DeliverySucceeded {
            message_id,
            claim_id,
            claimant_id,
            receipt,
        } => {
            validate_id("messageId", message_id)?;
            validate_id("claimId", claim_id)?;
            validate_id("claimantId", claimant_id)?;
            if let Some(receipt) = receipt {
                validate_optional_text("receipt", receipt, MAX_BODY_LENGTH)?;
            }
            let runtime = message_runtime_mut(inner, message_id)?;
            require_claim(runtime, claim_id, claimant_id)?;
            runtime.active_claim = None;
            runtime.last_error = None;
            runtime.next_attempt_at_ms = None;
            runtime.delivered_at_ms = Some(event.at_ms);
            runtime.delivery_receipt = receipt.clone();
        }
        ControlPlaneEventKind::DeliveryFailed {
            message_id,
            claim_id,
            claimant_id,
            error,
            retry_at_ms,
            terminal,
        }
        | ControlPlaneEventKind::DeliveryLeaseExpired {
            message_id,
            claim_id,
            claimant_id,
            error,
            retry_at_ms,
            terminal,
        } => {
            validate_id("messageId", message_id)?;
            validate_id("claimId", claim_id)?;
            validate_id("claimantId", claimant_id)?;
            validate_required_text("error", error, MAX_BODY_LENGTH)?;
            let runtime = message_runtime_mut(inner, message_id)?;
            require_claim(runtime, claim_id, claimant_id)?;
            if *terminal && retry_at_ms.is_some() {
                return Err(format!(
                    "terminal delivery event cannot retry: {message_id}"
                ));
            }
            if !*terminal && retry_at_ms.is_none() {
                return Err(format!(
                    "retryable delivery event requires retry time: {message_id}"
                ));
            }
            runtime.active_claim = None;
            runtime.last_error = Some(error.clone());
            runtime.next_attempt_at_ms = *retry_at_ms;
            runtime.dead_letter = *terminal;
        }
        ControlPlaneEventKind::MessageAcknowledged {
            message_id,
            acknowledged_by,
            note,
        } => {
            validate_id("messageId", message_id)?;
            validate_id("acknowledgedBy", acknowledged_by)?;
            if let Some(note) = note {
                validate_optional_text("note", note, MAX_BODY_LENGTH)?;
            }
            let runtime = message_runtime_mut(inner, message_id)?;
            if runtime.delivered_at_ms.is_none() || runtime.acknowledged_at_ms.is_some() {
                return Err(format!(
                    "message cannot be acknowledged in its current state: {message_id}"
                ));
            }
            runtime.acknowledged_at_ms = Some(event.at_ms);
            runtime.acknowledged_by = Some(acknowledged_by.clone());
            runtime.acknowledgement_note = note.clone();
        }
    }
    inner.recent_events.push_back(event);
    while inner.recent_events.len() > MAX_RECENT_EVENTS {
        inner.recent_events.pop_front();
    }
    Ok(())
}

fn trailing_counter(value: &str) -> Option<u64> {
    value.rsplit_once('-')?.1.parse().ok()
}

fn require_claim(
    runtime: &MessageRuntime,
    claim_id: &str,
    claimant_id: &str,
) -> Result<(), String> {
    let claim = runtime.active_claim.as_ref().ok_or_else(|| {
        format!(
            "message has no active delivery claim: {}",
            runtime.message.id
        )
    })?;
    if claim.claim_id != claim_id || claim.claimant_id != claimant_id {
        return Err(format!(
            "delivery claim ownership mismatch: {}",
            runtime.message.id
        ));
    }
    Ok(())
}

fn message_runtime_mut<'a>(
    inner: &'a mut ControlPlaneInner,
    message_id: &str,
) -> Result<&'a mut MessageRuntime, String> {
    inner
        .messages
        .get_mut(message_id)
        .ok_or_else(|| format!("event references unknown message: {message_id}"))
}

fn append_events(path: &Path, events: &[ControlPlaneEvent]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("cannot open control-plane ledger: {error}"))?;
    for event in events {
        serde_json::to_writer(&mut file, event)
            .map_err(|error| format!("cannot serialize control-plane event: {error}"))?;
        file.write_all(b"\n")
            .map_err(|error| format!("cannot append control-plane event: {error}"))?;
    }
    file.sync_data()
        .map_err(|error| format!("cannot flush control-plane ledger: {error}"))
}

fn load_ledger(path: &Path, inner: &mut ControlPlaneInner) -> Result<(), String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot read control-plane ledger: {error}")),
    };
    let ends_with_newline = contents.ends_with('\n');
    let lines: Vec<_> = contents.split('\n').collect();
    let mut truncate_at = None;
    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event = match serde_json::from_str::<ControlPlaneEvent>(line) {
            Ok(event) => event,
            Err(_) if index + 1 == lines.len() && !ends_with_newline => {
                // A process can die after a partial final append. Remove only that
                // unterminated tail so future appends remain valid, and reject
                // corruption anywhere else.
                truncate_at = Some(
                    contents
                        .rfind('\n')
                        .map(|position| position.saturating_add(1))
                        .unwrap_or(0),
                );
                break;
            }
            Err(error) => {
                return Err(format!(
                    "invalid control-plane ledger event on line {}: {error}",
                    index + 1
                ))
            }
        };
        apply_event(inner, event)?;
    }
    if let Some(length) = truncate_at {
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|error| format!("cannot repair torn control-plane ledger tail: {error}"))?;
        file.set_len(length as u64)
            .map_err(|error| format!("cannot truncate torn control-plane ledger tail: {error}"))?;
        file.sync_data()
            .map_err(|error| format!("cannot flush repaired control-plane ledger: {error}"))?;
    } else if !contents.is_empty() && !ends_with_newline {
        // The JSON body can be fully durable while the final newline is torn. Keep the
        // valid event and repair the separator before the next append.
        let mut file = OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(|error| format!("cannot repair control-plane ledger separator: {error}"))?;
        file.write_all(b"\n")
            .map_err(|error| format!("cannot append control-plane ledger separator: {error}"))?;
        file.sync_data()
            .map_err(|error| format!("cannot flush repaired control-plane ledger: {error}"))?;
    }
    Ok(())
}

fn validate_post_request(request: &PostMessageRequest) -> Result<(), String> {
    validate_scope(&request.scope)?;
    validate_actor(&request.sender)?;
    validate_destination(&request.destination)?;
    validate_required_text("body", &request.body, MAX_BODY_LENGTH)?;
    validate_optional_text("subject", &request.subject, MAX_SUBJECT_LENGTH)?;
    validate_id("idempotencyKey", &request.idempotency_key)?;
    if let Some(value) = &request.correlation_id {
        validate_id("correlationId", value)?;
    }
    if let Some(value) = &request.reply_to_message_id {
        validate_id("replyToMessageId", value)?;
    }
    if request.max_delivery_attempts == 0 || request.max_delivery_attempts > MAX_DELIVERY_ATTEMPTS {
        return Err(format!(
            "maxDeliveryAttempts must be between 1 and {MAX_DELIVERY_ATTEMPTS}"
        ));
    }
    Ok(())
}

fn validate_message(message: &ControlMessage) -> Result<(), String> {
    validate_id("message.id", &message.id)?;
    validate_post_request(&PostMessageRequest {
        scope: message.scope.clone(),
        kind: message.kind.clone(),
        sender: message.sender.clone(),
        destination: message.destination.clone(),
        subject: message.subject.clone(),
        body: message.body.clone(),
        idempotency_key: message.idempotency_key.clone(),
        correlation_id: message.correlation_id.clone(),
        reply_to_message_id: message.reply_to_message_id.clone(),
        max_delivery_attempts: message.max_delivery_attempts,
    })
}

fn validate_scope(scope: &MessageScope) -> Result<(), String> {
    validate_id("scope.organizationId", &scope.organization_id)?;
    validate_id("scope.repositoryId", &scope.repository_id)?;
    validate_id("scope.repositoryRoot", &scope.repository_root)?;
    validate_id("scope.worktreePath", &scope.worktree_path)?;
    validate_id("scope.branchName", &scope.branch_name)?;
    validate_id("scope.planId", &scope.plan_id)?;
    validate_id("scope.planPath", &scope.plan_path)?;
    validate_id("scope.nodeId", &scope.node_id)?;
    if let Some(value) = &scope.item_id {
        validate_id("scope.itemId", value)?;
    }
    validate_id("scope.workerId", &scope.worker_id)?;
    if let Some(value) = &scope.orchestrator_id {
        validate_id("scope.orchestratorId", value)?;
    }
    Ok(())
}

fn validate_actor(actor: &MessageActor) -> Result<(), String> {
    validate_id("sender.actorId", &actor.actor_id)
}

fn validate_destination(destination: &MessageDestination) -> Result<(), String> {
    validate_id("destination.targetId", &destination.target_id)?;
    if let Some(value) = &destination.connector_id {
        validate_id("destination.connectorId", value)?;
    }
    if let Some(value) = &destination.route_id {
        validate_id("destination.routeId", value)?;
    }
    if matches!(
        destination.kind,
        DestinationKind::Chat | DestinationKind::Ide
    ) && destination.connector_id.is_none()
    {
        return Err("CHAT and IDE destinations require a connectorId".to_string());
    }
    Ok(())
}

fn validate_lease(lease_ms: u64) -> Result<(), String> {
    if lease_ms == 0 || lease_ms > MAX_DELIVERY_LEASE_MS {
        return Err(format!(
            "leaseMs must be between 1 and {MAX_DELIVERY_LEASE_MS}"
        ));
    }
    Ok(())
}

fn validate_id(field: &str, value: &str) -> Result<(), String> {
    validate_required_text(field, value, MAX_ID_LENGTH)?;
    if value.contains('\0') {
        return Err(format!("{field} cannot contain a null character"));
    }
    Ok(())
}

fn validate_required_text(field: &str, value: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} is required"));
    }
    validate_optional_text(field, value, max)
}

fn validate_optional_text(field: &str, value: &str, max: usize) -> Result<(), String> {
    if value.len() > max {
        return Err(format!("{field} exceeds the {max}-byte limit"));
    }
    Ok(())
}

fn message_matches_request(message: &ControlMessage, request: &PostMessageRequest) -> bool {
    message.scope == request.scope
        && message.kind == request.kind
        && message.sender == request.sender
        && message.destination == request.destination
        && message.subject == request.subject
        && message.body == request.body
        && message.idempotency_key == request.idempotency_key
        && message.correlation_id == request.correlation_id
        && message.reply_to_message_id == request.reply_to_message_id
        && message.max_delivery_attempts == request.max_delivery_attempts
}

fn idempotency_index(scope: &MessageScope, key: &str) -> String {
    format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
        scope.organization_id.to_lowercase(),
        scope.repository_id.to_lowercase(),
        normalize_path(&scope.worktree_path),
        scope.branch_name,
        scope.plan_id.to_lowercase(),
        scope.node_id.to_lowercase(),
        scope.worker_id.to_lowercase(),
        key
    )
}

fn normalize_path(value: &str) -> String {
    value
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

fn option_matches(filter: &Option<String>, actual: &str, insensitive: bool) -> bool {
    filter.as_ref().is_none_or(|value| {
        if insensitive {
            value.eq_ignore_ascii_case(actual)
        } else {
            value == actual
        }
    })
}

fn option_matches_path(filter: &Option<String>, actual: &str) -> bool {
    filter
        .as_ref()
        .is_none_or(|value| normalize_path(value) == normalize_path(actual))
}

const fn default_max_delivery_attempts() -> u32 {
    DEFAULT_MAX_DELIVERY_ATTEMPTS
}

const fn default_delivery_lease_ms() -> u64 {
    DEFAULT_DELIVERY_LEASE_MS
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "perfect-planner-control-{name}-{}-{}-{}.jsonl",
            std::process::id(),
            unix_ms(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn scope(repository: &str, worker: &str) -> MessageScope {
        MessageScope {
            organization_id: "org-looplet".to_string(),
            repository_id: repository.to_string(),
            repository_root: format!("C:/repos/{repository}"),
            worktree_path: format!("C:/repos/{repository}-worktree"),
            branch_name: "feature/control-plane".to_string(),
            plan_id: "PP-001".to_string(),
            plan_path: format!(
                "C:/repos/{repository}-worktree/.claude/scratch/perfect-plan/plan.json"
            ),
            node_id: "A01".to_string(),
            item_id: None,
            worker_id: worker.to_string(),
            orchestrator_id: Some("head-orchestrator-01".to_string()),
        }
    }

    fn request(repository: &str, worker: &str, key: &str) -> PostMessageRequest {
        PostMessageRequest {
            scope: scope(repository, worker),
            kind: MessageKind::WorkerNote,
            sender: MessageActor {
                kind: ActorKind::Worker,
                actor_id: worker.to_string(),
            },
            destination: MessageDestination {
                kind: DestinationKind::Orchestrator,
                target_id: "head-orchestrator-01".to_string(),
                connector_id: Some("perfect-planner-local".to_string()),
                route_id: Some("local-orchestrator-inbox".to_string()),
                label: "Perfect Planner local inbox".to_string(),
                requires_acknowledgement: true,
                retry_base_ms: 5_000,
                registered_at_ms: Some(1),
                metadata: BTreeMap::new(),
            },
            subject: "Need a decision".to_string(),
            body: "The provider requires a partner account.".to_string(),
            idempotency_key: key.to_string(),
            correlation_id: Some("decision-provider-access".to_string()),
            reply_to_message_id: None,
            max_delivery_attempts: 3,
        }
    }

    fn claim(store: &ControlPlaneStore, message_id: &str, now_ms: u64) -> DeliveryClaim {
        store
            .claim_delivery(
                ClaimDeliveryRequest {
                    message_id: message_id.to_string(),
                    claimant_id: "connector-local-01".to_string(),
                    lease_ms: 100,
                },
                now_ms,
            )
            .unwrap()
    }

    #[test]
    fn persists_messages_and_rebuilds_state_after_restart() {
        let path = test_path("restart");
        let store = ControlPlaneStore::open(path.clone()).unwrap();
        let posted = store
            .post_message(request("repo-a", "worker-a", "note-1"), 10)
            .unwrap();
        assert!(!posted.duplicate);
        assert_eq!(posted.message.state, DeliveryState::Queued);
        drop(store);

        let reopened = ControlPlaneStore::open(path.clone()).unwrap();
        let snapshot = reopened.snapshot(20).unwrap();
        assert_eq!(snapshot.queued_count, 1);
        assert_eq!(
            snapshot.messages[0].message.body,
            "The provider requires a partner account."
        );
        assert_eq!(snapshot.recent_events.len(), 1);
        fs::remove_file(path).ok();
    }

    #[test]
    fn restart_continues_all_id_sequences_without_collisions() {
        let path = test_path("restart-sequences");
        let store = ControlPlaneStore::open(path.clone()).unwrap();
        let first = store
            .post_message(request("repo-a", "worker-a", "note-1"), 10)
            .unwrap();
        let first_claim = claim(&store, &first.message.message.id, 11);
        store
            .record_delivery_result(
                RecordDeliveryResultRequest {
                    message_id: first.message.message.id,
                    claim_id: first_claim.claim_id,
                    claimant_id: first_claim.claimant_id,
                    succeeded: false,
                    receipt: None,
                    error: Some("retry".to_string()),
                    retry_at_ms: Some(12),
                    terminal: false,
                },
                12,
            )
            .unwrap();
        drop(store);

        let reopened = ControlPlaneStore::open(path.clone()).unwrap();
        let second = reopened
            .post_message(request("repo-a", "worker-a", "note-2"), 13)
            .unwrap();
        let second_claim = claim(&reopened, &second.message.message.id, 14);
        assert_ne!(second.message.message.id, "pp-message-10-1");
        assert_ne!(second_claim.claim_id, "pp-claim-11-1");
        assert_eq!(reopened.snapshot(15).unwrap().messages.len(), 2);
        fs::remove_file(path).ok();
    }

    #[test]
    fn rejects_incomplete_scope_and_null_delimited_ids() {
        let path = test_path("scope");
        let store = ControlPlaneStore::open(path.clone()).unwrap();
        let mut missing = request("repo-a", "worker-a", "note-1");
        missing.scope.repository_id.clear();
        assert!(store
            .post_message(missing, 10)
            .unwrap_err()
            .contains("repositoryId"));

        let mut malicious = request("repo-a", "worker-a", "note-2");
        malicious.scope.organization_id = "org\0other".to_string();
        assert!(store
            .post_message(malicious, 11)
            .unwrap_err()
            .contains("null"));
        assert_eq!(store.snapshot(12).unwrap().messages.len(), 0);
        fs::remove_file(path).ok();
    }

    #[test]
    fn idempotency_is_scope_local_and_conflicting_reuse_is_rejected() {
        let path = test_path("idempotency");
        let store = ControlPlaneStore::open(path.clone()).unwrap();
        let first = store
            .post_message(request("repo-a", "worker-a", "same-key"), 10)
            .unwrap();
        let duplicate = store
            .post_message(request("repo-a", "worker-a", "same-key"), 11)
            .unwrap();
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.message.message.id, first.message.message.id);
        assert_eq!(store.snapshot(12).unwrap().recent_events.len(), 1);

        let mut conflict = request("repo-a", "worker-a", "same-key");
        conflict.body = "different content".to_string();
        assert!(store
            .post_message(conflict, 13)
            .unwrap_err()
            .contains("different message"));

        let other = store
            .post_message(request("repo-b", "worker-b", "same-key"), 14)
            .unwrap();
        assert_ne!(other.message.message.id, first.message.message.id);
        assert_eq!(store.snapshot(15).unwrap().messages.len(), 2);
        fs::remove_file(path).ok();
    }

    #[test]
    fn unrouted_message_cannot_be_claimed_until_a_route_event_exists() {
        let path = test_path("route");
        let store = ControlPlaneStore::open(path.clone()).unwrap();
        let mut unrouted = request("repo-a", "worker-a", "note-1");
        unrouted.destination.route_id = None;
        let posted = store.post_message(unrouted, 10).unwrap();
        assert_eq!(posted.message.state, DeliveryState::Unrouted);
        assert!(store
            .claim_delivery(
                ClaimDeliveryRequest {
                    message_id: posted.message.message.id.clone(),
                    claimant_id: "connector".to_string(),
                    lease_ms: 100,
                },
                11,
            )
            .unwrap_err()
            .contains("route"));

        let routed = store
            .route_message(
                RouteMessageRequest {
                    message_id: posted.message.message.id.clone(),
                    route_id: "chat-route-01".to_string(),
                    routed_by: "head-orchestrator-01".to_string(),
                },
                12,
            )
            .unwrap();
        assert_eq!(routed.state, DeliveryState::Queued);
        assert_eq!(claim(&store, &posted.message.message.id, 13).attempt, 1);
        fs::remove_file(path).ok();
    }

    #[test]
    fn delivery_and_acknowledgement_require_separate_durable_events() {
        let path = test_path("delivery-ack");
        let store = ControlPlaneStore::open(path.clone()).unwrap();
        let posted = store
            .post_message(request("repo-a", "worker-a", "note-1"), 10)
            .unwrap();
        assert!(store
            .acknowledge_message(
                AcknowledgeMessageRequest {
                    message_id: posted.message.message.id.clone(),
                    acknowledged_by: "head-orchestrator-01".to_string(),
                    note: None,
                },
                11,
            )
            .unwrap_err()
            .contains("before durable delivery"));

        let delivery_claim = claim(&store, &posted.message.message.id, 12);
        let delivered = store
            .record_delivery_result(
                RecordDeliveryResultRequest {
                    message_id: posted.message.message.id.clone(),
                    claim_id: delivery_claim.claim_id,
                    claimant_id: delivery_claim.claimant_id,
                    succeeded: true,
                    receipt: Some("local-receipt-001".to_string()),
                    error: None,
                    retry_at_ms: None,
                    terminal: false,
                },
                13,
            )
            .unwrap();
        assert_eq!(delivered.state, DeliveryState::Delivered);
        assert_eq!(
            delivered.delivery_receipt.as_deref(),
            Some("local-receipt-001")
        );

        let acknowledged = store
            .acknowledge_message(
                AcknowledgeMessageRequest {
                    message_id: posted.message.message.id,
                    acknowledged_by: "head-orchestrator-01".to_string(),
                    note: Some("Decision received".to_string()),
                },
                14,
            )
            .unwrap();
        assert_eq!(acknowledged.state, DeliveryState::Acknowledged);
        let event_types: Vec<_> = store
            .snapshot(15)
            .unwrap()
            .recent_events
            .into_iter()
            .map(|event| match event.payload {
                ControlPlaneEventKind::MessageCreated { .. } => "created",
                ControlPlaneEventKind::DeliveryClaimed { .. } => "claimed",
                ControlPlaneEventKind::DeliverySucceeded { .. } => "delivered",
                ControlPlaneEventKind::MessageAcknowledged { .. } => "acknowledged",
                _ => "other",
            })
            .collect();
        assert_eq!(
            event_types,
            vec!["created", "claimed", "delivered", "acknowledged"]
        );
        fs::remove_file(path).ok();
    }

    #[test]
    fn failures_retry_only_when_due_and_dead_letter_at_the_attempt_limit() {
        let path = test_path("retry");
        let store = ControlPlaneStore::open(path.clone()).unwrap();
        let mut post = request("repo-a", "worker-a", "note-1");
        post.max_delivery_attempts = 2;
        let posted = store.post_message(post, 10).unwrap();
        let first_claim = claim(&store, &posted.message.message.id, 11);
        let failed = store
            .record_delivery_result(
                RecordDeliveryResultRequest {
                    message_id: posted.message.message.id.clone(),
                    claim_id: first_claim.claim_id,
                    claimant_id: first_claim.claimant_id,
                    succeeded: false,
                    receipt: None,
                    error: Some("chat endpoint unavailable".to_string()),
                    retry_at_ms: Some(50),
                    terminal: false,
                },
                12,
            )
            .unwrap();
        assert_eq!(failed.state, DeliveryState::Queued);
        assert!(store
            .claim_delivery(
                ClaimDeliveryRequest {
                    message_id: posted.message.message.id.clone(),
                    claimant_id: "connector-local-01".to_string(),
                    lease_ms: 100,
                },
                49,
            )
            .unwrap_err()
            .contains("not due"));

        let second_claim = claim(&store, &posted.message.message.id, 50);
        let terminal = store
            .record_delivery_result(
                RecordDeliveryResultRequest {
                    message_id: posted.message.message.id,
                    claim_id: second_claim.claim_id,
                    claimant_id: second_claim.claimant_id,
                    succeeded: false,
                    receipt: None,
                    error: Some("chat endpoint still unavailable".to_string()),
                    retry_at_ms: Some(60),
                    terminal: false,
                },
                51,
            )
            .unwrap();
        assert_eq!(terminal.state, DeliveryState::DeadLetter);
        assert_eq!(terminal.next_attempt_at_ms, None);
        fs::remove_file(path).ok();
    }

    #[test]
    fn expired_claims_are_recorded_and_eventually_dead_lettered() {
        let path = test_path("expired");
        let store = ControlPlaneStore::open(path.clone()).unwrap();
        let mut post = request("repo-a", "worker-a", "note-1");
        post.max_delivery_attempts = 2;
        let posted = store.post_message(post, 10).unwrap();
        let first = claim(&store, &posted.message.message.id, 20);
        let after_first_expiry = store.snapshot(first.lease_expires_at_ms).unwrap();
        assert_eq!(after_first_expiry.queued_count, 1);
        assert!(after_first_expiry.messages[0]
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("lease expired"));

        let second = claim(
            &store,
            &posted.message.message.id,
            first.lease_expires_at_ms,
        );
        let after_second_expiry = store.snapshot(second.lease_expires_at_ms).unwrap();
        assert_eq!(after_second_expiry.dead_letter_count, 1);
        assert!(after_second_expiry
            .recent_events
            .iter()
            .any(|event| matches!(
                event.payload,
                ControlPlaneEventKind::DeliveryLeaseExpired { terminal: true, .. }
            )));
        fs::remove_file(path).ok();
    }

    #[test]
    fn rejects_results_without_the_exact_live_claim() {
        let path = test_path("claim-owner");
        let store = ControlPlaneStore::open(path.clone()).unwrap();
        let posted = store
            .post_message(request("repo-a", "worker-a", "note-1"), 10)
            .unwrap();
        let no_claim = store.record_delivery_result(
            RecordDeliveryResultRequest {
                message_id: posted.message.message.id.clone(),
                claim_id: "invented-claim".to_string(),
                claimant_id: "connector-local-01".to_string(),
                succeeded: true,
                receipt: None,
                error: None,
                retry_at_ms: None,
                terminal: false,
            },
            11,
        );
        assert!(no_claim.unwrap_err().contains("no active claim"));

        let live = claim(&store, &posted.message.message.id, 12);
        let wrong_owner = store.record_delivery_result(
            RecordDeliveryResultRequest {
                message_id: posted.message.message.id,
                claim_id: live.claim_id,
                claimant_id: "different-connector".to_string(),
                succeeded: true,
                receipt: None,
                error: None,
                retry_at_ms: None,
                terminal: false,
            },
            13,
        );
        assert!(wrong_owner.unwrap_err().contains("does not own"));
        fs::remove_file(path).ok();
    }

    #[test]
    fn filtered_snapshots_keep_repositories_and_workers_isolated() {
        let path = test_path("filter");
        let store = ControlPlaneStore::open(path.clone()).unwrap();
        store
            .post_message(request("repo-a", "worker-a", "note-a"), 10)
            .unwrap();
        store
            .post_message(request("repo-b", "worker-b", "note-b"), 11)
            .unwrap();
        let filter = MessageScopeFilter {
            repository_id: Some("REPO-A".to_string()),
            worker_id: Some("WORKER-A".to_string()),
            ..MessageScopeFilter::default()
        };
        let snapshot = store.snapshot_filtered(Some(&filter), 12).unwrap();
        assert_eq!(snapshot.messages.len(), 1);
        assert_eq!(snapshot.messages[0].message.scope.repository_id, "repo-a");
        assert_eq!(snapshot.recent_events.len(), 1);
        fs::remove_file(path).ok();
    }

    #[test]
    fn claim_next_is_route_scoped_and_oldest_first() {
        let path = test_path("claim-next");
        let store = ControlPlaneStore::open(path.clone()).unwrap();
        let first = store
            .post_message(request("repo-a", "worker-a", "note-a"), 10)
            .unwrap();
        let mut other_route = request("repo-a", "worker-b", "note-b");
        other_route.destination.route_id = Some("chat-route".to_string());
        store.post_message(other_route, 11).unwrap();
        let claimed = store
            .claim_next_delivery(
                ClaimNextDeliveryRequest {
                    route_id: "local-orchestrator-inbox".to_string(),
                    claimant_id: "local-ui".to_string(),
                    lease_ms: 100,
                },
                12,
            )
            .unwrap()
            .unwrap();
        assert_eq!(claimed.message_id, first.message.message.id);
        fs::remove_file(path).ok();
    }

    #[test]
    fn malformed_complete_lines_fail_but_a_torn_final_append_is_ignored() {
        let good_path = test_path("torn");
        let store = ControlPlaneStore::open(good_path.clone()).unwrap();
        store
            .post_message(request("repo-a", "worker-a", "note-a"), 10)
            .unwrap();
        drop(store);
        {
            let mut file = OpenOptions::new().append(true).open(&good_path).unwrap();
            file.write_all(b"{\"id\":\"torn").unwrap();
        }
        let reopened = ControlPlaneStore::open(good_path.clone()).unwrap();
        assert_eq!(reopened.snapshot(20).unwrap().messages.len(), 1);
        reopened
            .post_message(request("repo-a", "worker-a", "note-b"), 21)
            .unwrap();
        drop(reopened);
        assert_eq!(
            ControlPlaneStore::open(good_path.clone())
                .unwrap()
                .snapshot(22)
                .unwrap()
                .messages
                .len(),
            2
        );

        let corrupt_path = test_path("corrupt");
        fs::write(&corrupt_path, b"not-json\n").unwrap();
        assert!(ControlPlaneStore::open(corrupt_path.clone())
            .err()
            .unwrap()
            .contains("line 1"));
        fs::remove_file(good_path).ok();
        fs::remove_file(corrupt_path).ok();
    }
}
