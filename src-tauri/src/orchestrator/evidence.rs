use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_ARTIFACT_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceProfile {
    Ui,
    Headless,
    Migration,
    Docs,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceKind {
    BeforeScreenshot,
    AfterScreenshot,
    CommandOutput,
    ExitCode,
    GitDiff,
    OcrReport,
    MigrationStatus,
    DocumentDiff,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceArtifact {
    pub kind: EvidenceKind,
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationResult {
    pub command_id: String,
    pub exit_code: i32,
    pub output_artifact: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceGateResult {
    pub passed: bool,
    pub missing: Vec<EvidenceKind>,
    pub failed_commands: Vec<String>,
    pub hashes: Vec<String>,
}

impl EvidenceProfile {
    pub fn required(&self) -> &'static [EvidenceKind] {
        use EvidenceKind::*;
        match self {
            Self::Ui => &[
                BeforeScreenshot,
                AfterScreenshot,
                CommandOutput,
                ExitCode,
                GitDiff,
            ],
            Self::Headless => &[CommandOutput, ExitCode, GitDiff],
            Self::Migration => &[CommandOutput, ExitCode, GitDiff, MigrationStatus],
            Self::Docs => &[CommandOutput, ExitCode, DocumentDiff],
        }
    }

    pub fn permits_ocr(&self) -> bool {
        matches!(self, Self::Ui)
    }
}

pub fn validate_evidence(
    profile: &EvidenceProfile,
    artifacts: &[EvidenceArtifact],
    verification: &[VerificationResult],
) -> EvidenceGateResult {
    let present = artifacts
        .iter()
        .map(|artifact| artifact.kind.clone())
        .collect::<BTreeSet<_>>();
    let missing = profile
        .required()
        .iter()
        .filter(|kind| !present.contains(kind))
        .cloned()
        .collect::<Vec<_>>();
    let failed_commands = verification
        .iter()
        .filter(|result| result.exit_code != 0 || result.output_artifact.trim().is_empty())
        .map(|result| result.command_id.clone())
        .collect::<Vec<_>>();
    let invalid_ocr = !profile.permits_ocr() && present.contains(&EvidenceKind::OcrReport);
    let hashes = artifacts
        .iter()
        .filter(|artifact| !artifact.sha256.is_empty())
        .map(|artifact| artifact.sha256.clone())
        .collect::<Vec<_>>();

    EvidenceGateResult {
        passed: missing.is_empty()
            && failed_commands.is_empty()
            && !invalid_ocr
            && !verification.is_empty()
            && hashes.len() == artifacts.len(),
        missing,
        failed_commands,
        hashes,
    }
}

pub fn capture_artifact(
    root: &Path,
    path: &Path,
    kind: EvidenceKind,
) -> Result<EvidenceArtifact, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("cannot resolve evidence root: {error}"))?;
    let path = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve evidence artifact: {error}"))?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err("evidence artifact escapes the run evidence directory".to_string());
    }
    let bytes = path
        .metadata()
        .map_err(|error| format!("cannot inspect evidence artifact: {error}"))?
        .len();
    if bytes == 0 || bytes > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "evidence artifact size {bytes} is outside the accepted range"
        ));
    }
    let mut file =
        File::open(&path).map_err(|error| format!("cannot open evidence artifact: {error}"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot hash evidence artifact: {error}"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(EvidenceArtifact {
        kind,
        path,
        sha256: format!("{:x}", digest.finalize()),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(kind: EvidenceKind) -> EvidenceArtifact {
        EvidenceArtifact {
            kind,
            path: PathBuf::from("evidence.bin"),
            sha256: "abc".to_string(),
            bytes: 3,
        }
    }

    #[test]
    fn ui_requires_before_after_and_git_diff() {
        let artifacts = vec![
            artifact(EvidenceKind::BeforeScreenshot),
            artifact(EvidenceKind::CommandOutput),
            artifact(EvidenceKind::ExitCode),
        ];
        let gate = validate_evidence(
            &EvidenceProfile::Ui,
            &artifacts,
            &[VerificationResult {
                command_id: "ui".to_string(),
                exit_code: 0,
                output_artifact: "ui.log".to_string(),
            }],
        );
        assert!(!gate.passed);
        assert!(gate.missing.contains(&EvidenceKind::AfterScreenshot));
        assert!(gate.missing.contains(&EvidenceKind::GitDiff));
    }

    #[test]
    fn headless_refuses_ocr_tax_and_accepts_command_evidence() {
        let artifacts = vec![
            artifact(EvidenceKind::CommandOutput),
            artifact(EvidenceKind::ExitCode),
            artifact(EvidenceKind::GitDiff),
        ];
        let verification = vec![VerificationResult {
            command_id: "cargo-test".to_string(),
            exit_code: 0,
            output_artifact: "cargo-test.log".to_string(),
        }];
        assert!(validate_evidence(&EvidenceProfile::Headless, &artifacts, &verification).passed);

        let mut taxed = artifacts;
        taxed.push(artifact(EvidenceKind::OcrReport));
        assert!(!validate_evidence(&EvidenceProfile::Headless, &taxed, &verification).passed);
    }
}
