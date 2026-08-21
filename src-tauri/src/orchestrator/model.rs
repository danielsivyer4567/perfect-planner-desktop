use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

pub const RUN_STATE_SCHEMA_VERSION: u32 = 1;
pub const MAX_NODE_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunStatus {
    Pending,
    Preflight,
    Running,
    DecisionRequired,
    ReleaseGate,
    Blocked,
    Failed,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeStatus {
    Pending,
    Ready,
    Claimed,
    Running,
    GatePassed,
    GateFailed,
    Reassigning,
    Blocked,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidencePhase {
    Before,
    After,
    Verification,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceKind {
    Screenshot,
    CommandOutput,
    GitDiff,
    Ocr,
    BrowserConsole,
    NetworkLog,
    File,
    Log,
}

/// A fencing lease. `token` is the authority checked immediately before commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    pub node_id: String,
    pub worker: String,
    pub token: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRecord {
    pub evidence_id: String,
    pub node_id: String,
    pub kind: EvidenceKind,
    pub phase: EvidencePhase,
    pub path: String,
    pub sha256: String,
    pub captured_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub data: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeState {
    pub node_id: String,
    pub title: String,
    pub status: NodeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<String>,
    pub attempts: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<Lease>,
    #[serde(default)]
    pub evidence: Vec<EvidenceRecord>,
    #[serde(default)]
    pub allowed_files: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    pub schema_version: u32,
    pub run_id: String,
    pub plan_id: String,
    pub status: RunStatus,
    #[serde(default)]
    pub nodes: BTreeMap<String, NodeState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<Value>,
    pub created_at: String,
    pub updated_at: String,
}

impl RunState {
    pub fn new(
        run_id: impl Into<String>,
        plan_id: impl Into<String>,
        now: impl Into<String>,
    ) -> Self {
        let now = now.into();
        Self {
            schema_version: RUN_STATE_SCHEMA_VERSION,
            run_id: run_id.into(),
            plan_id: plan_id.into(),
            status: RunStatus::Pending,
            nodes: BTreeMap::new(),
            baseline: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationIssue {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationErrors {
    issues: Vec<ValidationIssue>,
}

impl ValidationErrors {
    pub(crate) fn new(issues: Vec<ValidationIssue>) -> Self {
        debug_assert!(!issues.is_empty());
        Self { issues }
    }

    pub fn issues(&self) -> &[ValidationIssue] {
        &self.issues
    }

    fn from_issues(issues: Vec<ValidationIssue>) -> Result<(), Self> {
        if issues.is_empty() {
            Ok(())
        } else {
            Err(Self::new(issues))
        }
    }
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, issue) in self.issues.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            write!(formatter, "{}: {}", issue.field, issue.message)?;
        }
        Ok(())
    }
}

impl Error for ValidationErrors {}

pub trait Validate {
    fn validate(&self) -> Result<(), ValidationErrors>;
}

impl Validate for Lease {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut issues = Vec::new();
        require_text(&mut issues, "nodeId", &self.node_id);
        require_text(&mut issues, "worker", &self.worker);
        require_text(&mut issues, "token", &self.token);
        require_text(&mut issues, "expiresAt", &self.expires_at);
        ValidationErrors::from_issues(issues)
    }
}

impl Validate for EvidenceRecord {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut issues = Vec::new();
        require_text(&mut issues, "evidenceId", &self.evidence_id);
        require_text(&mut issues, "nodeId", &self.node_id);
        require_text(&mut issues, "path", &self.path);
        require_text(&mut issues, "capturedAt", &self.captured_at);
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            issue(
                &mut issues,
                "sha256",
                "must be a 64-character hexadecimal digest",
            );
        }
        if self
            .command
            .as_deref()
            .is_some_and(|command| command.trim().is_empty())
        {
            issue(&mut issues, "command", "must not be empty when set");
        }
        ValidationErrors::from_issues(issues)
    }
}

impl Validate for NodeState {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut issues = Vec::new();
        require_text(&mut issues, "nodeId", &self.node_id);
        require_text(&mut issues, "title", &self.title);
        if self.attempts > MAX_NODE_ATTEMPTS {
            issue(
                &mut issues,
                "attempts",
                "must not exceed the initial attempt plus two automatic retries",
            );
        }
        if self
            .worker
            .as_deref()
            .is_some_and(|worker| worker.trim().is_empty())
        {
            issue(&mut issues, "worker", "must not be empty when set");
        }

        collect_unique_text_issues(&mut issues, "allowedFiles", &self.allowed_files);
        collect_unique_text_issues(&mut issues, "dependsOn", &self.depends_on);

        if self
            .depends_on
            .iter()
            .any(|dependency| dependency == &self.node_id)
        {
            issue(&mut issues, "dependsOn", "must not contain the node itself");
        }

        if let Some(lease) = &self.lease {
            collect_nested(&mut issues, "lease", lease.validate());
            if lease.node_id != self.node_id {
                issue(&mut issues, "lease.nodeId", "must match nodeId");
            }
            if self
                .worker
                .as_deref()
                .is_some_and(|worker| worker != lease.worker)
            {
                issue(&mut issues, "lease.worker", "must match the node worker");
            }
        }

        let mut evidence_ids = BTreeSet::new();
        for (index, evidence) in self.evidence.iter().enumerate() {
            collect_nested(
                &mut issues,
                &format!("evidence[{index}]"),
                evidence.validate(),
            );
            if evidence.node_id != self.node_id {
                issue(
                    &mut issues,
                    &format!("evidence[{index}].nodeId"),
                    "must match nodeId",
                );
            }
            if !evidence_ids.insert(&evidence.evidence_id) {
                issue(
                    &mut issues,
                    &format!("evidence[{index}].evidenceId"),
                    "must be unique within the node",
                );
            }
        }

        if self.status == NodeStatus::Done && self.evidence.is_empty() {
            issue(&mut issues, "evidence", "a done node must retain evidence");
        }
        if self.status == NodeStatus::Done && self.lease.is_some() {
            issue(&mut issues, "lease", "a done node must not retain a lease");
        }

        ValidationErrors::from_issues(issues)
    }
}

impl Validate for RunState {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut issues = Vec::new();
        if self.schema_version != RUN_STATE_SCHEMA_VERSION {
            issue(
                &mut issues,
                "schemaVersion",
                &format!("must equal {RUN_STATE_SCHEMA_VERSION}"),
            );
        }
        require_text(&mut issues, "runId", &self.run_id);
        require_text(&mut issues, "planId", &self.plan_id);
        require_text(&mut issues, "createdAt", &self.created_at);
        require_text(&mut issues, "updatedAt", &self.updated_at);

        for (key, node) in &self.nodes {
            if key.trim().is_empty() {
                issue(&mut issues, "nodes", "node map keys must not be empty");
            }
            collect_nested(&mut issues, &format!("nodes.{key}"), node.validate());
            if key != &node.node_id {
                issue(
                    &mut issues,
                    &format!("nodes.{key}.nodeId"),
                    "must match its node map key",
                );
            }
            for dependency in &node.depends_on {
                if !self.nodes.contains_key(dependency) {
                    issue(
                        &mut issues,
                        &format!("nodes.{key}.dependsOn"),
                        &format!("references unknown node {dependency}"),
                    );
                }
            }
        }

        if self.status == RunStatus::Completed {
            for (key, node) in &self.nodes {
                if node.status != NodeStatus::Done {
                    issue(
                        &mut issues,
                        &format!("nodes.{key}.status"),
                        "must be done when the run is completed",
                    );
                }
            }
        }

        ValidationErrors::from_issues(issues)
    }
}

fn require_text(issues: &mut Vec<ValidationIssue>, field: &str, value: &str) {
    if value.trim().is_empty() {
        issue(issues, field, "must not be empty");
    }
}

fn collect_unique_text_issues(issues: &mut Vec<ValidationIssue>, field: &str, values: &[String]) {
    let mut seen = BTreeSet::new();
    for (index, value) in values.iter().enumerate() {
        if value.trim().is_empty() {
            issue(issues, &format!("{field}[{index}]"), "must not be empty");
        } else if !seen.insert(value) {
            issue(
                issues,
                &format!("{field}[{index}]"),
                "must not be duplicated",
            );
        }
    }
}

fn collect_nested(
    issues: &mut Vec<ValidationIssue>,
    prefix: &str,
    result: Result<(), ValidationErrors>,
) {
    if let Err(errors) = result {
        issues.extend(errors.issues.into_iter().map(|nested| ValidationIssue {
            field: format!("{prefix}.{}", nested.field),
            message: nested.message,
        }));
    }
}

fn issue(issues: &mut Vec<ValidationIssue>, field: &str, message: &str) {
    issues.push(ValidationIssue {
        field: field.to_owned(),
        message: message.to_owned(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(node_id: &str) -> EvidenceRecord {
        EvidenceRecord {
            evidence_id: "ev-1".into(),
            node_id: node_id.into(),
            kind: EvidenceKind::CommandOutput,
            phase: EvidencePhase::Verification,
            path: "evidence/test.txt".into(),
            sha256: "a".repeat(64),
            captured_at: "2026-08-22T00:00:00Z".into(),
            command: Some("cargo test".into()),
            exit_code: Some(0),
            data: BTreeMap::new(),
        }
    }

    fn valid_node() -> NodeState {
        NodeState {
            node_id: "TO-01".into(),
            title: "Durable event model".into(),
            status: NodeStatus::Running,
            worker: Some("worker-a".into()),
            attempts: 1,
            lease: Some(Lease {
                node_id: "TO-01".into(),
                worker: "worker-a".into(),
                token: "fence-token".into(),
                expires_at: "2026-08-22T00:05:00Z".into(),
            }),
            evidence: vec![evidence("TO-01")],
            allowed_files: vec!["src-tauri/src/orchestrator/model.rs".into()],
            depends_on: Vec::new(),
            last_error: None,
        }
    }

    #[test]
    fn stable_model_round_trips_with_camel_case_keys() {
        let mut run = RunState::new("ORCH-20260822-001", "PP-001", "2026-08-22T00:00:00Z");
        run.status = RunStatus::Running;
        run.nodes.insert("TO-01".into(), valid_node());
        run.validate().expect("valid run state");

        let json = serde_json::to_value(&run).expect("serialize run state");
        assert_eq!(json["schemaVersion"], RUN_STATE_SCHEMA_VERSION);
        assert_eq!(json["runId"], "ORCH-20260822-001");
        assert_eq!(json["nodes"]["TO-01"]["lease"]["nodeId"], "TO-01");

        let decoded: RunState = serde_json::from_value(json).expect("deserialize run state");
        assert_eq!(decoded, run);
    }

    #[test]
    fn validation_fails_closed_on_broken_fence_and_retry_budget() {
        let mut node = valid_node();
        node.attempts = MAX_NODE_ATTEMPTS + 1;
        node.lease.as_mut().expect("lease").node_id = "OTHER".into();

        let errors = node.validate().expect_err("invalid node must fail");
        assert!(errors
            .issues()
            .iter()
            .any(|error| error.field == "attempts"));
        assert!(errors
            .issues()
            .iter()
            .any(|error| error.field == "lease.nodeId"));
    }

    #[test]
    fn completed_run_rejects_non_done_nodes() {
        let mut run = RunState::new("ORCH-20260822-001", "PP-001", "2026-08-22T00:00:00Z");
        run.status = RunStatus::Completed;
        run.nodes.insert("TO-01".into(), valid_node());

        let errors = run.validate().expect_err("incomplete node must fail");
        assert!(errors
            .issues()
            .iter()
            .any(|error| error.field == "nodes.TO-01.status"));
    }

    #[test]
    fn done_node_requires_evidence_and_released_lease() {
        let mut node = valid_node();
        node.status = NodeStatus::Done;
        node.evidence.clear();

        let errors = node.validate().expect_err("unproven done node must fail");
        assert!(errors
            .issues()
            .iter()
            .any(|error| error.field == "evidence"));
        assert!(errors.issues().iter().any(|error| error.field == "lease"));
    }
}
