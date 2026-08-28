use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryChange {
    pub desired: String,
    pub actual_commit: Option<String>,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeftoverItem {
    pub id: String,
    pub what: String,
    pub location: String,
    pub severity: String,
    pub suggested_next_action: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryRequest {
    pub run_id: String,
    pub plan_id: String,
    pub title: String,
    pub branch: String,
    pub commit_sha: String,
    pub pull_request_url: Option<String>,
    pub merge_sha: Option<String>,
    pub finished_at: String,
    pub changes: Vec<DeliveryChange>,
    pub leftovers: Vec<LeftoverItem>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryOutcome {
    pub handover_dir: PathBuf,
    pub archive_dir: PathBuf,
    pub checklist_line: String,
    pub leftovers_count: usize,
}

pub fn deliver_run(
    repo_root: &Path,
    run_dir: &Path,
    checklist_path: &Path,
    request: &DeliveryRequest,
) -> Result<DeliveryOutcome, String> {
    validate_identity(&request.run_id, "runId")?;
    validate_identity(&request.plan_id, "planId")?;
    for leftover in &request.leftovers {
        validate_identity(&leftover.id, "leftover.id")?;
        if leftover.what.trim().is_empty()
            || leftover.location.trim().is_empty()
            || leftover.severity.trim().is_empty()
            || leftover.suggested_next_action.trim().is_empty()
        {
            return Err(format!("leftover {} is incomplete", leftover.id));
        }
    }
    let repo_root = repo_root
        .canonicalize()
        .map_err(|error| format!("cannot resolve repository root: {error}"))?;
    let orchestrator_root = repo_root
        .join(".claude")
        .join("scratch")
        .join("orchestrator");
    let expected_run_dir = orchestrator_root.join(&request.run_id);
    let supplied_run_dir = canonicalize_with_missing_tail(run_dir)
        .map_err(|error| format!("cannot resolve run directory: {error}"))?;
    if !same_path(&supplied_run_dir, &expected_run_dir) {
        return Err(
            "run directory does not exactly match the repository-scoped run ID".to_string(),
        );
    }
    let checklist_parent = checklist_path
        .parent()
        .ok_or_else(|| "checklist path has no parent".to_string())?
        .canonicalize()
        .map_err(|error| format!("cannot resolve checklist parent: {error}"))?;
    if checklist_parent != repo_root {
        return Err("completion checklist must be repository-local".to_string());
    }

    let handover_dir = repo_root
        .join("docs")
        .join("handovers")
        .join(&request.run_id);
    let recover = format!(
        "git show {}:docs/handovers/{}/LEFTOVERS.md",
        request.commit_sha, request.run_id
    );
    let reference = request
        .pull_request_url
        .as_deref()
        .or(request.merge_sha.as_deref())
        .unwrap_or(&request.commit_sha);
    let checklist_line = format!(
        "- [x] {} — {} — refs: {} — recover: {}",
        request.finished_at, request.title, reference, recover
    );
    let archive_root = orchestrator_root.join("archive");
    let archive_dir = archive_root.join(&request.run_id);
    if !expected_run_dir.exists() && archive_dir.exists() {
        verify_delivered_state(&handover_dir, checklist_path, &checklist_line)?;
        return Ok(DeliveryOutcome {
            handover_dir,
            archive_dir,
            checklist_line,
            leftovers_count: request.leftovers.len(),
        });
    }
    if !expected_run_dir.is_dir() {
        return Err("repository-scoped run directory does not exist".to_string());
    }

    let completion = completion_report(request);
    let changes = changes_report(request);
    let leftovers = leftovers_report(request);
    write_exact_or_verify(
        &expected_run_dir.join("COMPLETION-REPORT.md"),
        completion.as_bytes(),
    )?;
    write_exact_or_verify(&expected_run_dir.join("changes.md"), changes.as_bytes())?;
    write_exact_or_verify(&expected_run_dir.join("LEFTOVERS.md"), leftovers.as_bytes())?;

    fs::create_dir_all(&handover_dir)
        .map_err(|error| format!("cannot create handover directory: {error}"))?;
    copy_exact_or_verify(
        &expected_run_dir.join("COMPLETION-REPORT.md"),
        &handover_dir.join("COMPLETION-REPORT.md"),
    )?;
    copy_exact_or_verify(
        &expected_run_dir.join("LEFTOVERS.md"),
        &handover_dir.join("LEFTOVERS.md"),
    )?;
    copy_exact_or_verify(
        &expected_run_dir.join("changes.md"),
        &handover_dir.join("changes.md"),
    )?;

    append_without_rewrite(checklist_path, &checklist_line)?;

    append_run_done(&expected_run_dir.join("events.jsonl"), request)?;
    fs::create_dir_all(&archive_root)
        .map_err(|error| format!("cannot create archive directory: {error}"))?;
    if archive_dir.exists() {
        return Err("run archive already exists; refusing to overwrite it".to_string());
    }
    fs::rename(&expected_run_dir, &archive_dir)
        .map_err(|error| format!("cannot archive completed run: {error}"))?;

    Ok(DeliveryOutcome {
        handover_dir,
        archive_dir,
        checklist_line,
        leftovers_count: request.leftovers.len(),
    })
}

fn completion_report(request: &DeliveryRequest) -> String {
    format!(
        "# Completion report — {}\n\n- Run: `{}`\n- Plan: `{}`\n- Branch: `{}`\n- Commit: `{}`\n- Pull request: {}\n- Merge SHA: {}\n- Finished: {}\n- Leftovers: {}\n",
        request.title,
        request.run_id,
        request.plan_id,
        request.branch,
        request.commit_sha,
        request.pull_request_url.as_deref().unwrap_or("not opened"),
        request.merge_sha.as_deref().unwrap_or("not merged"),
        request.finished_at,
        request.leftovers.len()
    )
}

fn changes_report(request: &DeliveryRequest) -> String {
    let mut output = String::from(
        "# Changes — desired vs actual\n\n| Desired | Actual commit | Status |\n|---|---|---|\n",
    );
    for change in &request.changes {
        output.push_str(&format!(
            "| {} | {} | {} |\n",
            escape_table(&change.desired),
            change.actual_commit.as_deref().unwrap_or("missing"),
            escape_table(&change.status)
        ));
    }
    output
}

fn leftovers_report(request: &DeliveryRequest) -> String {
    let mut output = format!("# Leftovers — {}\n\n", request.run_id);
    if request.leftovers.is_empty() {
        output.push_str("No known outstanding items.\n");
    } else {
        for item in &request.leftovers {
            output.push_str(&format!(
                "- `{}` · **{}** — {} — where: `{}` — next: {}\n",
                item.id,
                item.severity.trim(),
                item.what.trim(),
                item.location.trim(),
                item.suggested_next_action.trim()
            ));
        }
    }
    output
}

fn append_run_done(path: &Path, request: &DeliveryRequest) -> Result<(), String> {
    let event = json!({
        "ts": request.finished_at,
        "runId": request.run_id,
        "nodeId": null,
        "worker": "head-orchestrator",
        "type": "run-done",
        "msg": "delivery complete",
        "data": { "commit": request.commit_sha, "leftovers": request.leftovers.len() }
    });
    if path.exists() {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("cannot inspect run event bus: {error}"))?;
        if let Some(last) = text.lines().last() {
            let last_event: serde_json::Value = serde_json::from_str(last)
                .map_err(|error| format!("final run event is malformed: {error}"))?;
            if last_event.get("type").and_then(|value| value.as_str()) == Some("run-done") {
                if last_event.get("runId").and_then(|value| value.as_str())
                    == Some(request.run_id.as_str())
                {
                    return Ok(());
                }
                return Err("event bus already ends with another run's completion".to_string());
            }
        }
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("cannot open run event bus: {error}"))?;
    writeln!(file, "{event}").map_err(|error| format!("cannot append run-done event: {error}"))
}

fn append_without_rewrite(path: &Path, line: &str) -> Result<(), String> {
    let before = if path.exists() {
        fs::read(path).map_err(|error| format!("cannot read completion checklist: {error}"))?
    } else {
        Vec::new()
    };
    if String::from_utf8_lossy(&before)
        .lines()
        .any(|existing| existing == line)
    {
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("cannot open completion checklist: {error}"))?;
    if !before.is_empty() && !before.ends_with(b"\n") {
        writeln!(file).map_err(|error| format!("cannot terminate checklist line: {error}"))?;
    }
    writeln!(file, "{line}")
        .map_err(|error| format!("cannot append completion checklist: {error}"))?;
    drop(file);
    let after =
        fs::read(path).map_err(|error| format!("cannot verify completion checklist: {error}"))?;
    if !after.starts_with(&before) {
        return Err("completion checklist history changed during append".to_string());
    }
    Ok(())
}

fn write_exact_or_verify(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.exists() {
        let existing = fs::read(path).map_err(|error| format!("cannot verify report: {error}"))?;
        if existing == bytes {
            return Ok(());
        }
        return Err(format!(
            "existing report differs from the requested delivery: {}",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create report directory: {error}"))?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| format!("cannot write report: {error}"))?;
    fs::rename(&temporary, path).map_err(|error| format!("cannot publish report: {error}"))
}

fn copy_exact_or_verify(source: &Path, destination: &Path) -> Result<(), String> {
    let source_bytes = fs::read(source)
        .map_err(|error| format!("cannot read delivery source {}: {error}", source.display()))?;
    write_exact_or_verify(destination, &source_bytes)
}

fn verify_delivered_state(
    handover_dir: &Path,
    checklist_path: &Path,
    checklist_line: &str,
) -> Result<(), String> {
    for file in ["COMPLETION-REPORT.md", "changes.md", "LEFTOVERS.md"] {
        if !handover_dir.join(file).is_file() {
            return Err(format!(
                "archived run is missing durable handover file {file}"
            ));
        }
    }
    let checklist = fs::read_to_string(checklist_path)
        .map_err(|error| format!("cannot verify completion checklist: {error}"))?;
    if !checklist.lines().any(|line| line == checklist_line) {
        return Err("archived run has no matching completion checklist pointer".to_string());
    }
    Ok(())
}

fn validate_identity(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 120
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(format!("invalid {field}"));
    }
    if Path::new(value)
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("invalid {field}"));
    }
    Ok(())
}

fn canonicalize_with_missing_tail(path: &Path) -> std::io::Result<PathBuf> {
    let mut existing = path;
    let mut missing = Vec::new();
    while !existing.exists() {
        let name = existing.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "path has no existing ancestor",
            )
        })?;
        missing.push(name.to_os_string());
        existing = existing.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "path has no existing ancestor",
            )
        })?;
    }
    let mut resolved = existing.canonicalize()?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

#[cfg(not(windows))]
fn same_path(left: &Path, right: &Path) -> bool {
    left == right
}

#[cfg(windows)]
fn same_path(left: &Path, right: &Path) -> bool {
    fn normalized(path: &Path) -> String {
        path.to_string_lossy()
            .trim_start_matches(r"\\?\")
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    }
    normalized(left) == normalized(right)
}

fn escape_table(value: &str) -> String {
    value.replace('|', "\\|").replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_repo() -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("pp-delivery-{id}"));
        fs::create_dir_all(root.join(".claude/scratch/orchestrator/run-1")).expect("run dir");
        root
    }

    #[test]
    fn delivery_writes_handover_appends_and_archives_without_rewriting() {
        let root = temp_repo();
        let run_dir = root.join(".claude/scratch/orchestrator/run-1");
        fs::write(run_dir.join("events.jsonl"), "{\"type\":\"gate-pass\"}\n").expect("event");
        let checklist = root.join("COMPLETE-CHECKLIST.md");
        fs::write(&checklist, "# History\n- [x] old\n").expect("checklist");
        let request = DeliveryRequest {
            run_id: "run-1".to_string(),
            plan_id: "PP-002".to_string(),
            title: "Toy run".to_string(),
            branch: "feature/toy".to_string(),
            commit_sha: "abc123".to_string(),
            pull_request_url: None,
            merge_sha: None,
            finished_at: "2026-08-22 01:02".to_string(),
            changes: vec![DeliveryChange {
                desired: "add gate".to_string(),
                actual_commit: Some("abc123".to_string()),
                status: "SUCCEEDED".to_string(),
            }],
            leftovers: Vec::new(),
        };
        let outcome = deliver_run(&root, &run_dir, &checklist, &request).expect("deliver");
        let history = fs::read_to_string(&checklist).expect("history");
        assert!(history.starts_with("# History\n- [x] old\n"));
        assert!(outcome.handover_dir.join("LEFTOVERS.md").exists());
        assert!(outcome.archive_dir.exists());
        let events = fs::read_to_string(outcome.archive_dir.join("events.jsonl")).expect("events");
        assert_eq!(
            events.lines().last().unwrap(),
            serde_json::to_string(&json!({
                "ts": request.finished_at,
                "runId": request.run_id,
                "nodeId": null,
                "worker": "head-orchestrator",
                "type": "run-done",
                "msg": "delivery complete",
                "data": { "commit": request.commit_sha, "leftovers": 0 }
            }))
            .unwrap()
        );
        let repeated = deliver_run(&root, &run_dir, &checklist, &request)
            .expect("delivery retry should resume from the archive");
        assert_eq!(repeated.archive_dir, outcome.archive_dir);
        assert_eq!(
            fs::read_to_string(&checklist)
                .expect("checklist")
                .lines()
                .filter(|line| *line == outcome.checklist_line)
                .count(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }
}
