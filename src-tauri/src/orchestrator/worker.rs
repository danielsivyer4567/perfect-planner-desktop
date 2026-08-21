use super::evidence::{
    validate_evidence, EvidenceArtifact, EvidenceGateResult, EvidenceProfile, VerificationResult,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerManifest {
    pub run_id: String,
    pub plan_id: String,
    pub node_id: String,
    pub allowed_files: Vec<String>,
    pub profile: EvidenceProfile,
    pub verification_commands: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerSubmission {
    pub lease_token: String,
    pub changed_files: Vec<String>,
    pub artifacts: Vec<EvidenceArtifact>,
    pub verification: Vec<VerificationResult>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerGateResult {
    pub passed: bool,
    pub manifest_escapes: Vec<String>,
    pub evidence: EvidenceGateResult,
}

pub fn validate_manifest(manifest: &WorkerManifest) -> Result<(), String> {
    if manifest.run_id.trim().is_empty()
        || manifest.plan_id.trim().is_empty()
        || manifest.node_id.trim().is_empty()
    {
        return Err("worker manifest identity is incomplete".to_string());
    }
    if manifest.allowed_files.is_empty() || manifest.verification_commands.is_empty() {
        return Err("worker manifest requires files and verification commands".to_string());
    }
    for path in &manifest.allowed_files {
        validate_relative_path(path)?;
    }
    Ok(())
}

pub fn validate_submission(
    manifest: &WorkerManifest,
    submission: &WorkerSubmission,
) -> Result<WorkerGateResult, String> {
    validate_manifest(manifest)?;
    if submission.lease_token.trim().is_empty() {
        return Err("worker submission has no lease token".to_string());
    }
    let allowed = manifest
        .allowed_files
        .iter()
        .map(|path| normalize(path))
        .collect::<BTreeSet<_>>();
    let mut escapes = Vec::new();
    for path in &submission.changed_files {
        validate_relative_path(path)?;
        if !allowed.contains(&normalize(path)) {
            escapes.push(path.clone());
        }
    }
    let evidence = validate_evidence(
        &manifest.profile,
        &submission.artifacts,
        &submission.verification,
    );
    Ok(WorkerGateResult {
        passed: escapes.is_empty() && evidence.passed,
        manifest_escapes: escapes,
        evidence,
    })
}

fn normalize(value: &str) -> String {
    value.replace('\\', "/").to_ascii_lowercase()
}

fn validate_relative_path(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("manifest path is empty".to_string());
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("manifest path is not repository-relative: {value}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::evidence::{EvidenceArtifact, EvidenceKind};
    use std::path::PathBuf;

    fn artifact(kind: EvidenceKind) -> EvidenceArtifact {
        EvidenceArtifact {
            kind,
            path: PathBuf::from("proof"),
            sha256: "hash".to_string(),
            bytes: 1,
        }
    }

    #[test]
    fn rejects_changes_outside_the_node_manifest() {
        let manifest = WorkerManifest {
            run_id: "run-1".to_string(),
            plan_id: "PP-002".to_string(),
            node_id: "B04".to_string(),
            allowed_files: vec!["src/allowed.rs".to_string()],
            profile: EvidenceProfile::Headless,
            verification_commands: vec!["cargo test".to_string()],
        };
        let result = validate_submission(
            &manifest,
            &WorkerSubmission {
                lease_token: "lease".to_string(),
                changed_files: vec!["src/allowed.rs".to_string(), "src/escape.rs".to_string()],
                artifacts: vec![
                    artifact(EvidenceKind::CommandOutput),
                    artifact(EvidenceKind::ExitCode),
                    artifact(EvidenceKind::GitDiff),
                ],
                verification: vec![VerificationResult {
                    command_id: "cargo".to_string(),
                    exit_code: 0,
                    output_artifact: "cargo.log".to_string(),
                }],
            },
        )
        .expect("valid submission shape");
        assert!(!result.passed);
        assert_eq!(result.manifest_escapes, vec!["src/escape.rs"]);
    }
}
