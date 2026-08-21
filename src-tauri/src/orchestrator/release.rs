use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CiState {
    NotRun,
    Passed,
    CodeFailure,
    InfrastructureFailure,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PullRequestState {
    NotCreated,
    Open,
    Approved,
    ChangesRequested,
    Merged,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseGateInput {
    pub dirty_worktree: bool,
    pub merge_conflicts: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub unplanned: Vec<String>,
    pub unproven: Vec<String>,
    pub orphaned: Vec<String>,
    pub ci: CiState,
    pub pushed: bool,
    pub pull_request: PullRequestState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReleaseIssueKind {
    DirtyWorktree,
    MergeConflict,
    MissingEvidence,
    Reconciliation,
    CiNotRun,
    CodeFailure,
    CiInfrastructureFailure,
    NotPushed,
    ReviewRequired,
    ChangesRequested,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseIssue {
    pub kind: ReleaseIssueKind,
    pub message: String,
    pub decision_required: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseGateResult {
    pub ready_for_pr: bool,
    pub ready_to_merge: bool,
    pub merged: bool,
    pub issues: Vec<ReleaseIssue>,
}

pub fn evaluate_release(input: &ReleaseGateInput) -> ReleaseGateResult {
    let mut issues = Vec::new();
    if input.dirty_worktree {
        issues.push(issue(
            ReleaseIssueKind::DirtyWorktree,
            "working tree contains uncommitted changes",
            false,
        ));
    }
    for file in &input.merge_conflicts {
        issues.push(issue(
            ReleaseIssueKind::MergeConflict,
            format!("simulated merge conflicts in {file}"),
            false,
        ));
    }
    for item in &input.missing_evidence {
        issues.push(issue(
            ReleaseIssueKind::MissingEvidence,
            format!("evidence is incomplete for {item}"),
            false,
        ));
    }
    for item in input
        .unplanned
        .iter()
        .chain(&input.unproven)
        .chain(&input.orphaned)
    {
        issues.push(issue(
            ReleaseIssueKind::Reconciliation,
            format!("reconciliation violation: {item}"),
            true,
        ));
    }
    match input.ci {
        CiState::NotRun => issues.push(issue(
            ReleaseIssueKind::CiNotRun,
            "combined local CI has not run",
            false,
        )),
        CiState::CodeFailure => issues.push(issue(
            ReleaseIssueKind::CodeFailure,
            "combined local or hosted CI reports a code failure",
            false,
        )),
        CiState::InfrastructureFailure => issues.push(issue(
            ReleaseIssueKind::CiInfrastructureFailure,
            "CI infrastructure failure - decision required",
            true,
        )),
        CiState::Passed => {}
    }

    let ready_for_pr = issues.is_empty();
    if ready_for_pr && !input.pushed {
        issues.push(issue(
            ReleaseIssueKind::NotPushed,
            "validated branch has not been pushed",
            false,
        ));
    }
    match input.pull_request {
        PullRequestState::ChangesRequested => issues.push(issue(
            ReleaseIssueKind::ChangesRequested,
            "pull request has unresolved requested changes",
            true,
        )),
        PullRequestState::NotCreated | PullRequestState::Open if input.pushed => {
            issues.push(issue(
                ReleaseIssueKind::ReviewRequired,
                "pull request review has not passed",
                false,
            ))
        }
        _ => {}
    }
    let merged = input.pull_request == PullRequestState::Merged;
    let ready_to_merge = ready_for_pr
        && input.pushed
        && input.pull_request == PullRequestState::Approved
        && issues.is_empty();
    ReleaseGateResult {
        ready_for_pr,
        ready_to_merge,
        merged,
        issues,
    }
}

fn issue(
    kind: ReleaseIssueKind,
    message: impl Into<String>,
    decision_required: bool,
) -> ReleaseIssue {
    ReleaseIssue {
        kind,
        message: message.into(),
        decision_required,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn green() -> ReleaseGateInput {
        ReleaseGateInput {
            dirty_worktree: false,
            merge_conflicts: Vec::new(),
            missing_evidence: Vec::new(),
            unplanned: Vec::new(),
            unproven: Vec::new(),
            orphaned: Vec::new(),
            ci: CiState::Passed,
            pushed: true,
            pull_request: PullRequestState::Approved,
        }
    }

    #[test]
    fn only_a_fully_green_release_is_merge_ready() {
        assert!(evaluate_release(&green()).ready_to_merge);
        let mut conflict = green();
        conflict.merge_conflicts.push("src/App.tsx".to_string());
        assert!(!evaluate_release(&conflict).ready_for_pr);
        let mut evidence = green();
        evidence.missing_evidence.push("B04".to_string());
        assert!(!evaluate_release(&evidence).ready_for_pr);
    }

    #[test]
    fn infrastructure_failure_is_not_mislabelled_as_code_failure() {
        let mut input = green();
        input.ci = CiState::InfrastructureFailure;
        let result = evaluate_release(&input);
        assert!(!result.ready_for_pr);
        assert!(result.issues.iter().any(|entry| {
            entry.kind == ReleaseIssueKind::CiInfrastructureFailure && entry.decision_required
        }));
        assert!(!result
            .issues
            .iter()
            .any(|entry| entry.kind == ReleaseIssueKind::CodeFailure));
    }
}
