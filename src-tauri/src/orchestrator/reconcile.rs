//! Exact, fail-closed reconciliation between a plan and already-parsed Git facts.
//!
//! The caller is responsible for collecting commits and the final-tree inventory. This module
//! deliberately performs no process execution or filesystem access so the same inputs always
//! produce the same audit result.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanNode {
    pub node_id: String,
    #[serde(default)]
    pub manifest_files: Vec<String>,
    #[serde(default)]
    pub declared_outputs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommitHunk {
    pub file: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommitRecord {
    pub commit_id: String,
    pub message: String,
    #[serde(default)]
    pub hunks: Vec<CommitHunk>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ViolationCategory {
    Unplanned,
    Unproven,
    Orphaned,
    Fatal,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Waiver {
    /// Human-meaningful, unique name retained in the audit result.
    pub name: String,
    /// Both category and exact violation ID must match before suppression is allowed.
    pub category: ViolationCategory,
    pub violation_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationInput {
    pub plan_id: String,
    #[serde(default)]
    pub nodes: Vec<PlanNode>,
    #[serde(default)]
    pub commits: Vec<CommitRecord>,
    /// Repository-relative paths that exist in the final tree.
    #[serde(default)]
    pub final_tree_files: Vec<String>,
    /// Must be true only when the caller has proven that the actual tree is clean.
    pub actual_tree_clean: bool,
    /// Supplied even when `actual_tree_clean` is false so the audit names the dirty files.
    #[serde(default)]
    pub uncommitted_files: Vec<String>,
    #[serde(default)]
    pub waivers: Vec<Waiver>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Violation {
    pub violation_id: String,
    pub category: ViolationCategory,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Waiver names remain attached to the violation instead of erasing audit history.
    #[serde(default)]
    pub waived_by: Vec<String>,
}

impl Violation {
    pub fn is_suppressed(&self) -> bool {
        self.category != ViolationCategory::Fatal && !self.waived_by.is_empty()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WaiverAudit {
    pub name: String,
    pub category: ViolationCategory,
    pub violation_id: String,
    pub applied: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationResult {
    pub passed: bool,
    /// All violations are retained. A waived violation stays in its named list with `waivedBy`.
    pub unplanned: Vec<Violation>,
    pub unproven: Vec<Violation>,
    pub orphaned: Vec<Violation>,
    pub fatal: Vec<Violation>,
    pub waivers: Vec<WaiverAudit>,
}

impl ReconciliationResult {
    pub fn active_violation_count(&self) -> usize {
        self.unplanned
            .iter()
            .chain(&self.unproven)
            .chain(&self.orphaned)
            .chain(&self.fatal)
            .filter(|violation| !violation.is_suppressed())
            .count()
    }
}

/// Reconcile supplied plan and Git facts without consulting the host environment.
pub fn reconcile(input: &ReconciliationInput) -> ReconciliationResult {
    let mut result = ReconciliationResult {
        passed: false,
        unplanned: Vec::new(),
        unproven: Vec::new(),
        orphaned: Vec::new(),
        fatal: Vec::new(),
        waivers: Vec::new(),
    };

    let plan_id = input.plan_id.trim();
    if !valid_identifier(plan_id) {
        push_fatal(
            &mut result,
            "FATAL:PLAN_ID",
            format!(
                "plan ID `{}` is empty or contains forbidden characters",
                input.plan_id
            ),
            Some(input.plan_id.clone()),
            None,
            None,
            None,
        );
    }

    let mut nodes = BTreeMap::<String, NormalizedNode>::new();
    for node in &input.nodes {
        let node_id = node.node_id.trim();
        if !valid_identifier(node_id) {
            push_fatal(
                &mut result,
                format!("FATAL:NODE_ID:{}", stable_component(node_id)),
                format!(
                    "node ID `{}` is empty or contains forbidden characters",
                    node.node_id
                ),
                Some(plan_id.to_owned()),
                Some(node.node_id.clone()),
                None,
                None,
            );
            continue;
        }
        if nodes.contains_key(node_id) {
            push_fatal(
                &mut result,
                format!("FATAL:DUPLICATE_NODE:{node_id}"),
                format!("plan contains duplicate node ID `{node_id}`"),
                Some(plan_id.to_owned()),
                Some(node_id.to_owned()),
                None,
                None,
            );
            continue;
        }

        let manifest = normalize_path_set(
            &node.manifest_files,
            &mut result,
            "MANIFEST_PATH",
            plan_id,
            node_id,
        );
        let outputs = normalize_path_set(
            &node.declared_outputs,
            &mut result,
            "OUTPUT_PATH",
            plan_id,
            node_id,
        );
        nodes.insert(node_id.to_owned(), NormalizedNode { manifest, outputs });
    }

    let final_tree = normalize_path_set(
        &input.final_tree_files,
        &mut result,
        "FINAL_TREE_PATH",
        plan_id,
        "TREE",
    );

    if !input.actual_tree_clean || !input.uncommitted_files.is_empty() {
        if input.uncommitted_files.is_empty() {
            push_fatal(
                &mut result,
                "FATAL:DIRTY_TREE",
                "actual tree is dirty; no uncommitted file inventory was supplied".to_owned(),
                Some(plan_id.to_owned()),
                None,
                None,
                None,
            );
        } else {
            for raw_file in &input.uncommitted_files {
                let file = normalize_path(raw_file).unwrap_or_else(|| raw_file.trim().to_owned());
                push_fatal(
                    &mut result,
                    format!("FATAL:UNCOMMITTED:{}", stable_component(&file)),
                    format!("uncommitted actual `{file}` makes reconciliation non-authoritative"),
                    Some(plan_id.to_owned()),
                    None,
                    None,
                    Some(file),
                );
            }
        }
    }

    let mut tagged_nodes = BTreeSet::<String>::new();
    let mut seen_commits = BTreeSet::<String>::new();

    for commit in &input.commits {
        let commit_id = commit.commit_id.trim();
        if commit_id.is_empty() || !seen_commits.insert(commit_id.to_owned()) {
            let problem = if commit_id.is_empty() {
                "commit ID is empty".to_owned()
            } else {
                format!("commit ID `{commit_id}` occurs more than once")
            };
            push_fatal(
                &mut result,
                format!("FATAL:COMMIT_ID:{}", stable_component(commit_id)),
                problem,
                Some(plan_id.to_owned()),
                None,
                Some(commit.commit_id.clone()),
                None,
            );
            continue;
        }

        let parsed_tag = match parse_exact_tag(&commit.message) {
            Ok(tag) => tag,
            Err(reason) => {
                push_fatal(
                    &mut result,
                    format!("FATAL:COMMIT_TAG:{commit_id}"),
                    format!("commit `{commit_id}` has malformed or ambiguous plan tags: {reason}"),
                    Some(plan_id.to_owned()),
                    None,
                    Some(commit_id.to_owned()),
                    None,
                );
                continue;
            }
        };

        let normalized_hunks = normalize_commit_hunks(commit, plan_id, &mut result);
        match parsed_tag {
            None => {
                for file in normalized_hunks {
                    result.unplanned.push(Violation {
                        violation_id: format!(
                            "UNPLANNED:UNTAGGED:{commit_id}:{}",
                            stable_component(&file)
                        ),
                        category: ViolationCategory::Unplanned,
                        summary: format!(
                            "commit `{commit_id}` changes `{file}` without an exact [{plan_id}/<node-id>] tag"
                        ),
                        plan_id: Some(plan_id.to_owned()),
                        node_id: None,
                        commit_id: Some(commit_id.to_owned()),
                        file: Some(file),
                        waived_by: Vec::new(),
                    });
                }
            }
            Some((tag_plan, tag_node)) if tag_plan != plan_id => {
                result.orphaned.push(Violation {
                    violation_id: format!(
                        "ORPHANED:FOREIGN_PLAN:{commit_id}:{}:{}",
                        stable_component(&tag_plan),
                        stable_component(&tag_node)
                    ),
                    category: ViolationCategory::Orphaned,
                    summary: format!(
                        "commit `{commit_id}` is tagged [{tag_plan}/{tag_node}], not plan `{plan_id}`"
                    ),
                    plan_id: Some(tag_plan),
                    node_id: Some(tag_node),
                    commit_id: Some(commit_id.to_owned()),
                    file: None,
                    waived_by: Vec::new(),
                });
            }
            Some((_, tag_node)) => {
                let Some(node) = nodes.get(&tag_node) else {
                    result.orphaned.push(Violation {
                        violation_id: format!(
                            "ORPHANED:UNKNOWN_NODE:{commit_id}:{}",
                            stable_component(&tag_node)
                        ),
                        category: ViolationCategory::Orphaned,
                        summary: format!(
                            "commit `{commit_id}` references unknown node [{plan_id}/{tag_node}]"
                        ),
                        plan_id: Some(plan_id.to_owned()),
                        node_id: Some(tag_node),
                        commit_id: Some(commit_id.to_owned()),
                        file: None,
                        waived_by: Vec::new(),
                    });
                    continue;
                };

                tagged_nodes.insert(tag_node.clone());
                for file in normalized_hunks {
                    if !node.manifest.contains(&file) {
                        result.unplanned.push(Violation {
                            violation_id: format!(
                                "UNPLANNED:OUTSIDE_MANIFEST:{commit_id}:{}:{}",
                                stable_component(&tag_node),
                                stable_component(&file)
                            ),
                            category: ViolationCategory::Unplanned,
                            summary: format!(
                                "commit `{commit_id}` changes `{file}` outside node `{tag_node}` manifest"
                            ),
                            plan_id: Some(plan_id.to_owned()),
                            node_id: Some(tag_node.clone()),
                            commit_id: Some(commit_id.to_owned()),
                            file: Some(file),
                            waived_by: Vec::new(),
                        });
                    }
                }
            }
        }
    }

    for (node_id, node) in &nodes {
        if !tagged_nodes.contains(node_id) {
            result.unproven.push(Violation {
                violation_id: format!("UNPROVEN:NO_TAGGED_COMMIT:{node_id}"),
                category: ViolationCategory::Unproven,
                summary: format!("node `{node_id}` has no commit tagged [{plan_id}/{node_id}]"),
                plan_id: Some(plan_id.to_owned()),
                node_id: Some(node_id.clone()),
                commit_id: None,
                file: None,
                waived_by: Vec::new(),
            });
        }
        for output in &node.outputs {
            if !final_tree.contains(output) {
                result.unproven.push(Violation {
                    violation_id: format!(
                        "UNPROVEN:MISSING_OUTPUT:{}:{}",
                        stable_component(node_id),
                        stable_component(output)
                    ),
                    category: ViolationCategory::Unproven,
                    summary: format!(
                        "node `{node_id}` declared output `{output}`, but it is absent from the final tree"
                    ),
                    plan_id: Some(plan_id.to_owned()),
                    node_id: Some(node_id.clone()),
                    commit_id: None,
                    file: Some(output.clone()),
                    waived_by: Vec::new(),
                });
            }
        }
    }

    validate_and_apply_waivers(input, &mut result);
    sort_result(&mut result);
    result.passed = result.active_violation_count() == 0;
    result
}

#[derive(Clone, Debug)]
struct NormalizedNode {
    manifest: BTreeSet<String>,
    outputs: BTreeSet<String>,
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
}

fn normalize_path(value: &str) -> Option<String> {
    let replaced = value.trim().replace('\\', "/");
    let mut components = Vec::new();
    let mut raw_components = replaced.split('/').peekable();
    while matches!(raw_components.peek(), Some(&".")) {
        raw_components.next();
    }
    for component in raw_components {
        if component.is_empty() || component == "." || component == ".." {
            return None;
        }
        if components.is_empty() && component.ends_with(':') {
            return None;
        }
        components.push(component);
    }
    if replaced.starts_with('/') || components.is_empty() {
        return None;
    }
    Some(components.join("/"))
}

fn normalize_path_set(
    values: &[String],
    result: &mut ReconciliationResult,
    kind: &str,
    plan_id: &str,
    node_id: &str,
) -> BTreeSet<String> {
    let mut normalized = BTreeSet::new();
    for raw in values {
        match normalize_path(raw) {
            Some(path) => {
                normalized.insert(path);
            }
            None => push_fatal(
                result,
                format!(
                    "FATAL:{kind}:{}:{}",
                    stable_component(node_id),
                    stable_component(raw)
                ),
                format!("`{raw}` is not a canonical repository-relative path"),
                Some(plan_id.to_owned()),
                Some(node_id.to_owned()),
                None,
                Some(raw.clone()),
            ),
        }
    }
    normalized
}

fn normalize_commit_hunks(
    commit: &CommitRecord,
    plan_id: &str,
    result: &mut ReconciliationResult,
) -> BTreeSet<String> {
    let mut normalized = BTreeSet::new();
    for hunk in &commit.hunks {
        match normalize_path(&hunk.file) {
            Some(file) => {
                normalized.insert(file);
            }
            None => push_fatal(
                result,
                format!(
                    "FATAL:HUNK_PATH:{}:{}",
                    stable_component(&commit.commit_id),
                    stable_component(&hunk.file)
                ),
                format!(
                    "commit `{}` contains non-canonical hunk path `{}`",
                    commit.commit_id, hunk.file
                ),
                Some(plan_id.to_owned()),
                None,
                Some(commit.commit_id.clone()),
                Some(hunk.file.clone()),
            ),
        }
    }
    normalized
}

/// Parse at most one exact `[plan-id/node-id]` tag. Bracketed text without `/` is ignored.
fn parse_exact_tag(message: &str) -> Result<Option<(String, String)>, String> {
    let mut tags = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = message[cursor..].find('[') {
        let start = cursor + relative_start;
        let after_start = start + 1;
        let Some(relative_end) = message[after_start..].find(']') else {
            if message[after_start..].contains('/') {
                return Err("unclosed bracket containing `/`".to_owned());
            }
            break;
        };
        let end = after_start + relative_end;
        let body = &message[after_start..end];
        if body.contains('/') {
            let parts = body.split('/').collect::<Vec<_>>();
            if parts.len() != 2 || !valid_identifier(parts[0]) || !valid_identifier(parts[1]) {
                return Err(format!("`[{body}]` is not an exact [plan-id/node-id] tag"));
            }
            tags.push((parts[0].to_owned(), parts[1].to_owned()));
        }
        cursor = end + 1;
    }
    match tags.len() {
        0 => Ok(None),
        1 => Ok(tags.pop()),
        count => Err(format!(
            "found {count} plan/node tags; exactly one is allowed"
        )),
    }
}

fn validate_and_apply_waivers(input: &ReconciliationInput, result: &mut ReconciliationResult) {
    let mut seen_names = BTreeSet::new();
    for waiver in &input.waivers {
        let name = waiver.name.trim();
        if name.is_empty() || !seen_names.insert(name.to_owned()) {
            push_fatal(
                result,
                format!("FATAL:WAIVER_NAME:{}", stable_component(name)),
                if name.is_empty() {
                    "waiver name is empty".to_owned()
                } else {
                    format!("waiver name `{name}` is duplicated")
                },
                Some(input.plan_id.clone()),
                None,
                None,
                None,
            );
            continue;
        }
        if waiver.category == ViolationCategory::Fatal {
            push_fatal(
                result,
                format!("FATAL:WAIVER_FATAL_TARGET:{}", stable_component(name)),
                format!("waiver `{name}` attempts to suppress a fatal violation"),
                Some(input.plan_id.clone()),
                None,
                None,
                None,
            );
        }
    }

    for waiver in &input.waivers {
        let mut applied = false;
        if waiver.category != ViolationCategory::Fatal
            && !waiver.name.trim().is_empty()
            && seen_names.contains(waiver.name.trim())
        {
            if let Some(violation) = violations_mut(result, waiver.category)
                .iter_mut()
                .find(|violation| violation.violation_id == waiver.violation_id)
            {
                violation.waived_by.push(waiver.name.trim().to_owned());
                violation.waived_by.sort();
                violation.waived_by.dedup();
                applied = true;
            }
        }
        result.waivers.push(WaiverAudit {
            name: waiver.name.clone(),
            category: waiver.category,
            violation_id: waiver.violation_id.clone(),
            applied,
        });
    }
}

fn violations_mut(
    result: &mut ReconciliationResult,
    category: ViolationCategory,
) -> &mut Vec<Violation> {
    match category {
        ViolationCategory::Unplanned => &mut result.unplanned,
        ViolationCategory::Unproven => &mut result.unproven,
        ViolationCategory::Orphaned => &mut result.orphaned,
        ViolationCategory::Fatal => &mut result.fatal,
    }
}

#[allow(clippy::too_many_arguments)]
fn push_fatal(
    result: &mut ReconciliationResult,
    violation_id: impl Into<String>,
    summary: String,
    plan_id: Option<String>,
    node_id: Option<String>,
    commit_id: Option<String>,
    file: Option<String>,
) {
    result.fatal.push(Violation {
        violation_id: violation_id.into(),
        category: ViolationCategory::Fatal,
        summary,
        plan_id,
        node_id,
        commit_id,
        file,
        waived_by: Vec::new(),
    });
}

fn stable_component(value: &str) -> String {
    let component = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if component.is_empty() {
        "EMPTY".to_owned()
    } else {
        component
    }
}

fn sort_result(result: &mut ReconciliationResult) {
    for violations in [
        &mut result.unplanned,
        &mut result.unproven,
        &mut result.orphaned,
        &mut result.fatal,
    ] {
        violations.sort_by(|left, right| left.violation_id.cmp(&right.violation_id));
    }
    result
        .waivers
        .sort_by(|left, right| left.name.cmp(&right.name));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(node_id: &str, manifest: &[&str], outputs: &[&str]) -> PlanNode {
        PlanNode {
            node_id: node_id.to_owned(),
            manifest_files: manifest.iter().map(|value| (*value).to_owned()).collect(),
            declared_outputs: outputs.iter().map(|value| (*value).to_owned()).collect(),
        }
    }

    fn commit(commit_id: &str, message: &str, hunks: &[&str]) -> CommitRecord {
        CommitRecord {
            commit_id: commit_id.to_owned(),
            message: message.to_owned(),
            hunks: hunks
                .iter()
                .map(|file| CommitHunk {
                    file: (*file).to_owned(),
                })
                .collect(),
        }
    }

    fn valid_input() -> ReconciliationInput {
        ReconciliationInput {
            plan_id: "PP-001".to_owned(),
            nodes: vec![node(
                "TO-05",
                &["src-tauri/src/orchestrator/reconcile.rs"],
                &["src-tauri/src/orchestrator/reconcile.rs"],
            )],
            commits: vec![commit(
                "abc123",
                "[PP-001/TO-05] exact reconciliation",
                &["src-tauri/src/orchestrator/reconcile.rs"],
            )],
            final_tree_files: vec!["src-tauri/src/orchestrator/reconcile.rs".to_owned()],
            actual_tree_clean: true,
            uncommitted_files: Vec::new(),
            waivers: Vec::new(),
        }
    }

    #[test]
    fn exact_tag_manifest_commit_and_output_pass() {
        let result = reconcile(&valid_input());
        assert!(result.passed, "{result:#?}");
        assert_eq!(result.active_violation_count(), 0);
    }

    #[test]
    fn windows_separators_normalize_to_repository_paths() {
        let mut input = valid_input();
        input.nodes[0].manifest_files =
            vec!["src-tauri\\src\\orchestrator\\reconcile.rs".to_owned()];
        input.commits[0].hunks[0].file = ".\\src-tauri\\src\\orchestrator\\reconcile.rs".to_owned();
        assert!(reconcile(&input).passed);
    }

    #[test]
    fn tagged_hunk_outside_manifest_is_unplanned() {
        let mut input = valid_input();
        input.commits[0].hunks.push(CommitHunk {
            file: "src-tauri/src/lib.rs".to_owned(),
        });
        let result = reconcile(&input);
        assert!(!result.passed);
        assert_eq!(result.unplanned.len(), 1);
        assert_eq!(
            result.unplanned[0].file.as_deref(),
            Some("src-tauri/src/lib.rs")
        );
    }

    #[test]
    fn untagged_actual_is_unplanned() {
        let mut input = valid_input();
        input.commits[0].message = "ordinary commit".to_owned();
        let result = reconcile(&input);
        assert!(!result.passed);
        assert_eq!(result.unplanned.len(), 1);
        assert_eq!(result.unproven.len(), 1);
    }

    #[test]
    fn every_node_requires_tagged_commit_and_outputs() {
        let mut input = valid_input();
        input
            .nodes
            .push(node("TO-06", &["src/new.rs"], &["dist/report.json"]));
        let result = reconcile(&input);
        let ids = result
            .unproven
            .iter()
            .map(|violation| violation.violation_id.as_str())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"UNPROVEN:NO_TAGGED_COMMIT:TO-06"));
        assert!(ids.contains(&"UNPROVEN:MISSING_OUTPUT:TO-06:dist_report.json"));
    }

    #[test]
    fn unknown_and_foreign_tags_are_orphaned() {
        let mut input = valid_input();
        input.commits.push(commit(
            "def456",
            "[PP-001/TO-99] unknown node",
            &["src/unknown.rs"],
        ));
        input.commits.push(commit(
            "fed987",
            "[PP-999/TO-05] foreign plan",
            &["src/foreign.rs"],
        ));
        let result = reconcile(&input);
        assert_eq!(result.orphaned.len(), 2);
        assert!(result
            .orphaned
            .iter()
            .any(|item| item.violation_id.starts_with("ORPHANED:UNKNOWN_NODE")));
        assert!(result
            .orphaned
            .iter()
            .any(|item| item.violation_id.starts_with("ORPHANED:FOREIGN_PLAN")));
    }

    #[test]
    fn exact_named_waiver_suppresses_only_matching_violation_and_stays_auditable() {
        let mut input = valid_input();
        input.commits[0].hunks.push(CommitHunk {
            file: "docs/exception.md".to_owned(),
        });
        input.waivers = vec![
            Waiver {
                name: "documented-generated-file".to_owned(),
                category: ViolationCategory::Unplanned,
                violation_id: "UNPLANNED:OUTSIDE_MANIFEST:abc123:TO-05:docs_exception.md"
                    .to_owned(),
            },
            Waiver {
                name: "wrong-category-does-not-match".to_owned(),
                category: ViolationCategory::Orphaned,
                violation_id: "UNPLANNED:OUTSIDE_MANIFEST:abc123:TO-05:docs_exception.md"
                    .to_owned(),
            },
        ];
        let result = reconcile(&input);
        assert!(result.passed, "{result:#?}");
        assert_eq!(
            result.unplanned.len(),
            1,
            "waived evidence must remain visible"
        );
        assert_eq!(result.unplanned[0].waived_by, ["documented-generated-file"]);
        assert!(result.unplanned[0].is_suppressed());
        assert!(result.waivers.iter().any(|audit| audit.applied));
        assert!(result
            .waivers
            .iter()
            .any(|audit| audit.name == "wrong-category-does-not-match" && !audit.applied));
    }

    #[test]
    fn waiver_cannot_suppress_a_different_violation() {
        let mut input = valid_input();
        input.commits[0].hunks.push(CommitHunk {
            file: "src-tauri/src/lib.rs".to_owned(),
        });
        input.waivers.push(Waiver {
            name: "narrow-waiver".to_owned(),
            category: ViolationCategory::Unplanned,
            violation_id: "UNPLANNED:some-other-violation".to_owned(),
        });
        let result = reconcile(&input);
        assert!(!result.passed);
        assert!(!result.waivers[0].applied);
        assert!(result.unplanned[0].waived_by.is_empty());
    }

    #[test]
    fn dirty_or_uncommitted_actuals_fail_closed_and_cannot_be_waived() {
        let mut input = valid_input();
        input.actual_tree_clean = false;
        input.uncommitted_files = vec!["src-tauri/src/lib.rs".to_owned()];
        input.waivers.push(Waiver {
            name: "forbidden-fatal-waiver".to_owned(),
            category: ViolationCategory::Fatal,
            violation_id: "FATAL:UNCOMMITTED:src-tauri_src_lib.rs".to_owned(),
        });
        let result = reconcile(&input);
        assert!(!result.passed);
        assert!(result
            .fatal
            .iter()
            .any(|item| item.violation_id.starts_with("FATAL:UNCOMMITTED")));
        assert!(result
            .fatal
            .iter()
            .any(|item| item.violation_id.starts_with("FATAL:WAIVER_FATAL_TARGET")));
        assert!(result.fatal.iter().all(|item| !item.is_suppressed()));
    }

    #[test]
    fn malformed_tag_fails_closed() {
        let mut input = valid_input();
        input.commits[0].message = "[PP-001/TO-05/EXTRA] invalid".to_owned();
        let result = reconcile(&input);
        assert!(!result.passed);
        assert!(result
            .fatal
            .iter()
            .any(|item| item.violation_id == "FATAL:COMMIT_TAG:abc123"));
    }

    #[test]
    fn ambiguous_tags_fail_closed() {
        let mut input = valid_input();
        input.commits[0].message = "[PP-001/TO-05] and [PP-001/TO-06]".to_owned();
        let result = reconcile(&input);
        assert!(!result.passed);
        assert!(result.fatal[0].summary.contains("exactly one is allowed"));
    }

    #[test]
    fn duplicate_node_and_noncanonical_paths_fail_closed() {
        let mut input = valid_input();
        input.nodes.push(input.nodes[0].clone());
        input.final_tree_files.push("../outside.txt".to_owned());
        let result = reconcile(&input);
        assert!(!result.passed);
        assert!(result
            .fatal
            .iter()
            .any(|item| item.violation_id.starts_with("FATAL:DUPLICATE_NODE")));
        assert!(result
            .fatal
            .iter()
            .any(|item| item.violation_id.starts_with("FATAL:FINAL_TREE_PATH")));
    }

    #[test]
    fn colon_node_ids_are_valid_exact_tags() {
        let mut input = valid_input();
        input.nodes[0].node_id = "A01:3".to_owned();
        input.commits[0].message = "[PP-001/A01:3] supported identifier".to_owned();
        assert!(reconcile(&input).passed);
    }

    #[test]
    fn serde_result_preserves_named_lists_and_waiver_audit() {
        let result = reconcile(&valid_input());
        let json = serde_json::to_value(result).expect("result should serialize");
        assert!(json.get("unplanned").is_some());
        assert!(json.get("unproven").is_some());
        assert!(json.get("orphaned").is_some());
        assert!(json.get("waivers").is_some());
    }
}
