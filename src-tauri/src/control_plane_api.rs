use crate::control_plane::{
    self, AcknowledgeMessageRequest, ActorKind, ClaimDeliveryRequest, ControlPlaneStore,
    DeliveryState, DestinationKind, MessageActor, MessageDestination, MessageKind, MessageScope,
    MessageScopeFilter, MessageView, PostMessageRequest, RecordDeliveryResultRequest,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const REPOSITORY_SENTINEL: &str = "__repository__";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendScope {
    organization_id: String,
    repository_id: String,
    repository_root: String,
    worktree_path: String,
    branch: String,
    plan_id: String,
    plan_path: String,
    node_id: Option<String>,
    item_id: Option<String>,
    worker_id: Option<String>,
    orchestrator_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendDestination {
    registration_id: Option<String>,
    kind: String,
    label: String,
    address: Option<String>,
    enabled: bool,
    requires_acknowledgement: bool,
    max_attempts: u32,
    retry_base_ms: u64,
    registered_at_ms: Option<u64>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostControlMessageRequest {
    idempotency_key: String,
    correlation_id: Option<String>,
    kind: String,
    scope: FrontendScope,
    author_id: String,
    body: String,
    destination: FrontendDestination,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendAttempt {
    attempt_id: String,
    message_id: String,
    consumer_id: String,
    state: String,
    attempt_number: u32,
    claimed_at_ms: u64,
    lease_expires_at_ms: u64,
    completed_at_ms: Option<u64>,
    retry_at_ms: Option<u64>,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendAcknowledgement {
    acknowledged_by: String,
    acknowledged_at_ms: u64,
    note: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendMessage {
    id: String,
    idempotency_key: String,
    correlation_id: String,
    kind: String,
    scope: FrontendScope,
    author_id: String,
    body: String,
    destination: FrontendDestination,
    state: String,
    attempts: Vec<FrontendAttempt>,
    acknowledgement: Option<FrontendAcknowledgement>,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendStateCounts {
    unrouted: usize,
    queued: usize,
    claimed: usize,
    delivered: usize,
    acknowledged: usize,
    dead_letter: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendSnapshot {
    repository_id: String,
    organization_id: Option<String>,
    now_ms: u64,
    messages: Vec<FrontendMessage>,
    registrations: Vec<FrontendDestination>,
    state_counts: FrontendStateCounts,
    pending_acknowledgement_count: usize,
    failed_attempt_count: usize,
    next_retry_at_ms: Option<u64>,
    last_updated_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPlaneSnapshotRequest {
    repository_id: String,
    organization_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimControlDeliveriesRequest {
    repository_id: String,
    organization_id: Option<String>,
    consumer_id: String,
    #[serde(default)]
    destination_kinds: Vec<String>,
    limit: Option<usize>,
    lease_ms: Option<u64>,
    filter: Option<FrontendMessageFilter>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendMessageFilter {
    branch: Option<String>,
    plan_id: Option<String>,
    node_id: Option<String>,
    worker_id: Option<String>,
    orchestrator_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordControlDeliveryRequest {
    repository_id: String,
    message_id: String,
    attempt_id: String,
    consumer_id: String,
    outcome: String,
    error: Option<String>,
    retry_after_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcknowledgeControlMessageRequest {
    repository_id: String,
    message_id: String,
    acknowledged_by: String,
    note: Option<String>,
}

fn require_text(value: &str, field: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} is required"));
    }
    Ok(trimmed.to_string())
}

fn core_kind(value: &str) -> Result<MessageKind, String> {
    match value {
        "workerNote" => Ok(MessageKind::WorkerNote),
        "handoff" | "orchestratorInstruction" => Ok(MessageKind::Handoff),
        "decisionRequest" => Ok(MessageKind::DecisionRequest),
        "decisionResponse" => Ok(MessageKind::DecisionResponse),
        "alert" => Ok(MessageKind::Alert),
        "status" | "workerReport" => Ok(MessageKind::Status),
        _ => Err(format!("unsupported control message kind: {value}")),
    }
}

fn frontend_kind(value: &MessageKind) -> &'static str {
    match value {
        MessageKind::WorkerNote => "workerNote",
        MessageKind::Handoff => "handoff",
        MessageKind::DecisionRequest => "decisionRequest",
        MessageKind::DecisionResponse => "decisionResponse",
        MessageKind::Alert => "alert",
        MessageKind::Status => "status",
    }
}

fn core_destination_kind(value: &str) -> Result<DestinationKind, String> {
    match value {
        "localUi" | "orchestrator" => Ok(DestinationKind::Orchestrator),
        "worker" => Ok(DestinationKind::Worker),
        "codexChat" => Ok(DestinationKind::Chat),
        "ide" => Ok(DestinationKind::Ide),
        _ => Err(format!("unsupported destination kind: {value}")),
    }
}

fn frontend_destination_kind(value: &DestinationKind) -> &'static str {
    match value {
        DestinationKind::Orchestrator => "localUi",
        DestinationKind::Worker => "worker",
        DestinationKind::Chat => "codexChat",
        DestinationKind::Ide => "ide",
    }
}

fn actor_kind(scope: &FrontendScope, author_id: &str) -> ActorKind {
    if scope.worker_id.as_deref() == Some(author_id) {
        ActorKind::Worker
    } else if scope.orchestrator_id.as_deref() == Some(author_id) {
        ActorKind::Orchestrator
    } else {
        ActorKind::User
    }
}

fn to_core_scope(scope: &FrontendScope) -> Result<MessageScope, String> {
    Ok(MessageScope {
        organization_id: require_text(&scope.organization_id, "scope.organizationId")?,
        repository_id: require_text(&scope.repository_id, "scope.repositoryId")?,
        repository_root: require_text(&scope.repository_root, "scope.repositoryRoot")?,
        worktree_path: require_text(&scope.worktree_path, "scope.worktreePath")?,
        branch_name: require_text(&scope.branch, "scope.branch")?,
        plan_id: require_text(&scope.plan_id, "scope.planId")?,
        plan_path: require_text(&scope.plan_path, "scope.planPath")?,
        node_id: scope
            .node_id
            .clone()
            .unwrap_or_else(|| REPOSITORY_SENTINEL.to_string()),
        item_id: scope.item_id.clone(),
        worker_id: scope
            .worker_id
            .clone()
            .unwrap_or_else(|| REPOSITORY_SENTINEL.to_string()),
        orchestrator_id: scope.orchestrator_id.clone(),
    })
}

fn to_frontend_scope(scope: &MessageScope) -> FrontendScope {
    FrontendScope {
        organization_id: scope.organization_id.clone(),
        repository_id: scope.repository_id.clone(),
        repository_root: scope.repository_root.clone(),
        worktree_path: scope.worktree_path.clone(),
        branch: scope.branch_name.clone(),
        plan_id: scope.plan_id.clone(),
        plan_path: scope.plan_path.clone(),
        node_id: (scope.node_id != REPOSITORY_SENTINEL).then(|| scope.node_id.clone()),
        item_id: scope.item_id.clone(),
        worker_id: (scope.worker_id != REPOSITORY_SENTINEL).then(|| scope.worker_id.clone()),
        orchestrator_id: scope.orchestrator_id.clone(),
    }
}

fn to_core_destination(destination: &FrontendDestination) -> Result<MessageDestination, String> {
    let route_is_complete = destination.enabled
        && destination
            .registration_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        && destination
            .address
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
    Ok(MessageDestination {
        kind: core_destination_kind(&destination.kind)?,
        target_id: destination
            .address
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unregistered".to_string()),
        connector_id: Some(destination.kind.clone()),
        route_id: route_is_complete
            .then(|| destination.registration_id.clone())
            .flatten(),
        label: destination.label.clone(),
        requires_acknowledgement: destination.requires_acknowledgement,
        retry_base_ms: destination.retry_base_ms,
        registered_at_ms: destination.registered_at_ms,
        metadata: destination.metadata.clone(),
    })
}

fn to_frontend_destination(view: &MessageView) -> FrontendDestination {
    let destination = &view.message.destination;
    FrontendDestination {
        registration_id: view.route_id.clone(),
        kind: frontend_destination_kind(&destination.kind).to_string(),
        label: destination.label.clone(),
        address: (destination.target_id != "unregistered").then(|| destination.target_id.clone()),
        enabled: view.route_id.is_some(),
        requires_acknowledgement: destination.requires_acknowledgement,
        max_attempts: view.message.max_delivery_attempts,
        retry_base_ms: destination.retry_base_ms,
        registered_at_ms: destination.registered_at_ms,
        metadata: destination.metadata.clone(),
    }
}

fn state_name(state: &DeliveryState) -> &'static str {
    match state {
        DeliveryState::Unrouted => "unrouted",
        DeliveryState::Queued => "queued",
        DeliveryState::Claimed => "claimed",
        DeliveryState::Delivered => "delivered",
        DeliveryState::Acknowledged => "acknowledged",
        DeliveryState::DeadLetter => "deadLetter",
    }
}

fn to_frontend_message(view: &MessageView) -> FrontendMessage {
    let attempts = view
        .active_claim
        .iter()
        .map(|claim| FrontendAttempt {
            attempt_id: claim.claim_id.clone(),
            message_id: claim.message_id.clone(),
            consumer_id: claim.claimant_id.clone(),
            state: "claimed".to_string(),
            attempt_number: claim.attempt,
            claimed_at_ms: claim.claimed_at_ms,
            lease_expires_at_ms: claim.lease_expires_at_ms,
            completed_at_ms: None,
            retry_at_ms: view.next_attempt_at_ms,
            error: view.last_error.clone(),
        })
        .collect();
    let acknowledgement = view.acknowledged_at_ms.map(|at| FrontendAcknowledgement {
        acknowledged_by: view.acknowledged_by.clone().unwrap_or_default(),
        acknowledged_at_ms: at,
        note: view.acknowledgement_note.clone(),
    });
    let updated_at_ms = view
        .acknowledged_at_ms
        .or(view.delivered_at_ms)
        .or_else(|| view.active_claim.as_ref().map(|claim| claim.claimed_at_ms))
        .unwrap_or(view.message.created_at_ms);
    FrontendMessage {
        id: view.message.id.clone(),
        idempotency_key: view.message.idempotency_key.clone(),
        correlation_id: view
            .message
            .correlation_id
            .clone()
            .unwrap_or_else(|| view.message.id.clone()),
        kind: frontend_kind(&view.message.kind).to_string(),
        scope: to_frontend_scope(&view.message.scope),
        author_id: view.message.sender.actor_id.clone(),
        body: view.message.body.clone(),
        destination: to_frontend_destination(view),
        state: state_name(&view.state).to_string(),
        attempts,
        acknowledgement,
        created_at_ms: view.message.created_at_ms,
        updated_at_ms,
    }
}

fn scope_filter(request: &ControlPlaneSnapshotRequest) -> Result<MessageScopeFilter, String> {
    Ok(MessageScopeFilter {
        organization_id: request.organization_id.clone(),
        repository_id: Some(require_text(&request.repository_id, "repositoryId")?),
        ..MessageScopeFilter::default()
    })
}

fn build_snapshot(
    request: &ControlPlaneSnapshotRequest,
    now_ms: u64,
    views: Vec<MessageView>,
) -> FrontendSnapshot {
    let mut registrations = Vec::new();
    let mut seen_routes = BTreeSet::new();
    let mut counts = FrontendStateCounts::default();
    let mut pending_acknowledgement_count = 0;
    let mut failed_attempt_count = 0;
    let mut next_retry_at_ms: Option<u64> = None;
    let mut last_updated_at_ms: Option<u64> = None;
    let mut messages = Vec::with_capacity(views.len());
    for view in views {
        match view.state {
            DeliveryState::Unrouted => counts.unrouted += 1,
            DeliveryState::Queued => counts.queued += 1,
            DeliveryState::Claimed => counts.claimed += 1,
            DeliveryState::Delivered => {
                counts.delivered += 1;
                if view.message.destination.requires_acknowledgement {
                    pending_acknowledgement_count += 1;
                }
            }
            DeliveryState::Acknowledged => counts.acknowledged += 1,
            DeliveryState::DeadLetter => counts.dead_letter += 1,
        }
        if view.last_error.is_some() {
            failed_attempt_count += 1;
        }
        if let Some(retry_at) = view.next_attempt_at_ms {
            next_retry_at_ms = Some(next_retry_at_ms.map_or(retry_at, |old| old.min(retry_at)));
        }
        let message = to_frontend_message(&view);
        last_updated_at_ms = Some(
            last_updated_at_ms.map_or(message.updated_at_ms, |old| old.max(message.updated_at_ms)),
        );
        if let Some(registration_id) = &message.destination.registration_id {
            if seen_routes.insert(registration_id.clone()) {
                registrations.push(message.destination.clone());
            }
        }
        messages.push(message);
    }
    FrontendSnapshot {
        repository_id: request.repository_id.clone(),
        organization_id: request.organization_id.clone(),
        now_ms,
        messages,
        registrations,
        state_counts: counts,
        pending_acknowledgement_count,
        failed_attempt_count,
        next_retry_at_ms,
        last_updated_at_ms,
    }
}

fn ensure_repository_message(
    store: &ControlPlaneStore,
    repository_id: &str,
    message_id: &str,
    now_ms: u64,
) -> Result<MessageView, String> {
    let filter = MessageScopeFilter {
        repository_id: Some(require_text(repository_id, "repositoryId")?),
        ..MessageScopeFilter::default()
    };
    store
        .snapshot_filtered(Some(&filter), now_ms)?
        .messages
        .into_iter()
        .find(|view| view.message.id == message_id)
        .ok_or_else(|| "control message not found in repository".to_string())
}

#[tauri::command]
pub fn post_control_message(
    state: tauri::State<'_, ControlPlaneStore>,
    request: PostControlMessageRequest,
) -> Result<FrontendMessage, String> {
    let sender = MessageActor {
        kind: actor_kind(&request.scope, &request.author_id),
        actor_id: require_text(&request.author_id, "authorId")?,
    };
    let destination = to_core_destination(&request.destination)?;
    let core = PostMessageRequest {
        scope: to_core_scope(&request.scope)?,
        kind: core_kind(&request.kind)?,
        sender,
        destination,
        subject: String::new(),
        body: request.body,
        idempotency_key: request.idempotency_key,
        correlation_id: request.correlation_id,
        reply_to_message_id: None,
        max_delivery_attempts: request.destination.max_attempts.max(1),
    };
    let outcome = state.post_message(core, control_plane::unix_ms())?;
    Ok(to_frontend_message(&outcome.message))
}

#[tauri::command]
pub fn control_plane_snapshot(
    state: tauri::State<'_, ControlPlaneStore>,
    request: ControlPlaneSnapshotRequest,
) -> Result<FrontendSnapshot, String> {
    let now_ms = control_plane::unix_ms();
    let filter = scope_filter(&request)?;
    let snapshot = state.snapshot_filtered(Some(&filter), now_ms)?;
    Ok(build_snapshot(&request, now_ms, snapshot.messages))
}

#[tauri::command]
pub fn claim_control_deliveries(
    state: tauri::State<'_, ControlPlaneStore>,
    request: ClaimControlDeliveriesRequest,
) -> Result<Vec<FrontendMessage>, String> {
    let now_ms = control_plane::unix_ms();
    let repository_id = require_text(&request.repository_id, "repositoryId")?;
    let consumer_id = require_text(&request.consumer_id, "consumerId")?;
    let destination_kinds: BTreeSet<_> = request.destination_kinds.iter().cloned().collect();
    let filter = MessageScopeFilter {
        organization_id: request.organization_id.clone(),
        repository_id: Some(repository_id.clone()),
        branch_name: request
            .filter
            .as_ref()
            .and_then(|value| value.branch.clone()),
        plan_id: request
            .filter
            .as_ref()
            .and_then(|value| value.plan_id.clone()),
        node_id: request
            .filter
            .as_ref()
            .and_then(|value| value.node_id.clone()),
        worker_id: request
            .filter
            .as_ref()
            .and_then(|value| value.worker_id.clone()),
        ..MessageScopeFilter::default()
    };
    let mut candidates = state.snapshot_filtered(Some(&filter), now_ms)?.messages;
    candidates.retain(|view| {
        if view.state != DeliveryState::Queued {
            return false;
        }
        if let Some(orchestrator_id) = request
            .filter
            .as_ref()
            .and_then(|value| value.orchestrator_id.as_ref())
        {
            if view.message.scope.orchestrator_id.as_ref() != Some(orchestrator_id) {
                return false;
            }
        }
        destination_kinds.is_empty()
            || destination_kinds.contains(frontend_destination_kind(&view.message.destination.kind))
    });
    candidates.sort_by_key(|view| (view.message.created_at_ms, view.message.id.clone()));
    let limit = request.limit.unwrap_or(20).clamp(1, 100);
    let lease_ms = request
        .lease_ms
        .unwrap_or(control_plane::DEFAULT_DELIVERY_LEASE_MS);
    let mut claimed_ids = BTreeSet::new();
    for view in candidates.into_iter().take(limit) {
        state.claim_delivery(
            ClaimDeliveryRequest {
                message_id: view.message.id.clone(),
                claimant_id: consumer_id.clone(),
                lease_ms,
            },
            now_ms,
        )?;
        claimed_ids.insert(view.message.id);
    }
    if claimed_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(state
        .snapshot_filtered(Some(&filter), now_ms)?
        .messages
        .into_iter()
        .filter(|view| claimed_ids.contains(&view.message.id))
        .map(|view| to_frontend_message(&view))
        .collect())
}

#[tauri::command]
pub fn record_control_delivery(
    state: tauri::State<'_, ControlPlaneStore>,
    request: RecordControlDeliveryRequest,
) -> Result<FrontendMessage, String> {
    let now_ms = control_plane::unix_ms();
    ensure_repository_message(&state, &request.repository_id, &request.message_id, now_ms)?;
    let succeeded = match request.outcome.as_str() {
        "delivered" => true,
        "failed" => false,
        value => return Err(format!("unsupported delivery outcome: {value}")),
    };
    let retry_at_ms = request
        .retry_after_ms
        .map(|delay| now_ms.saturating_add(delay));
    let view = state.record_delivery_result(
        RecordDeliveryResultRequest {
            message_id: request.message_id,
            claim_id: request.attempt_id,
            claimant_id: request.consumer_id,
            succeeded,
            receipt: succeeded.then(|| "adapter-confirmed".to_string()),
            error: request.error,
            retry_at_ms,
            terminal: false,
        },
        now_ms,
    )?;
    Ok(to_frontend_message(&view))
}

#[tauri::command]
pub fn acknowledge_control_message(
    state: tauri::State<'_, ControlPlaneStore>,
    request: AcknowledgeControlMessageRequest,
) -> Result<FrontendMessage, String> {
    let now_ms = control_plane::unix_ms();
    ensure_repository_message(&state, &request.repository_id, &request.message_id, now_ms)?;
    let view = state.acknowledge_message(
        AcknowledgeMessageRequest {
            message_id: request.message_id,
            acknowledged_by: request.acknowledged_by,
            note: request.note,
        },
        now_ms,
    )?;
    Ok(to_frontend_message(&view))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_external_routes_are_never_claimable() {
        let destination = FrontendDestination {
            registration_id: None,
            kind: "codexChat".to_string(),
            label: "Codex".to_string(),
            address: None,
            enabled: true,
            requires_acknowledgement: true,
            max_attempts: 3,
            retry_base_ms: 5_000,
            registered_at_ms: None,
            metadata: BTreeMap::new(),
        };
        let core = to_core_destination(&destination).unwrap();
        assert_eq!(core.kind, DestinationKind::Chat);
        assert!(core.route_id.is_none());
        assert_eq!(core.target_id, "unregistered");
    }

    #[test]
    fn scope_round_trip_preserves_repository_plan_and_actor_fences() {
        let scope = FrontendScope {
            organization_id: "org-a".to_string(),
            repository_id: "repo-a".to_string(),
            repository_root: "C:/repos/a".to_string(),
            worktree_path: "C:/repos/a-work".to_string(),
            branch: "feature/a".to_string(),
            plan_id: "pp-plan-a".to_string(),
            plan_path: "C:/repos/a-work/.claude/scratch/perfect-plan/a.json".to_string(),
            node_id: Some("A01".to_string()),
            item_id: Some("A01:0".to_string()),
            worker_id: Some("worker-a".to_string()),
            orchestrator_id: Some("orch-a".to_string()),
        };
        let round_trip = to_frontend_scope(&to_core_scope(&scope).unwrap());
        assert_eq!(round_trip.repository_id, scope.repository_id);
        assert_eq!(round_trip.plan_path, scope.plan_path);
        assert_eq!(round_trip.item_id, scope.item_id);
        assert_eq!(round_trip.orchestrator_id, scope.orchestrator_id);
    }
}
