//! Cross-module contract tests for the B19 collision-claim trust boundary.
//!
//! Behavioral race and native-identity tests live beside the crate-private implementation. These
//! integration checks prevent a later refactor from accidentally widening the Tauri surface or
//! removing one of the independently required fail-closed gates.

use std::{fs, path::PathBuf};

fn source(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn authority_issuance_is_native_only_and_aggregate_scoped() {
    let registry = source("src/collision_assessor/registry.rs");
    assert!(registry.contains("struct MachineAuthoritySetReceipt"));
    assert!(registry.contains("machine-authority-set-scope:v1"));
    assert!(registry.contains("verify_authority_set_receipt"));
    assert!(registry.contains("authority_scope_high_water"));
    assert!(registry.contains("authority.registry_generation == document.generation"));
    assert!(!registry.contains("#[tauri::command]\n    pub(crate) fn publish_machine_claim"));
    assert!(!registry.contains("authority_process_key"));
}

#[test]
fn native_authorities_survive_until_every_acceptance_boundary() {
    let registry = source("src/collision_assessor/registry.rs");
    let discovery = source("src/collision_assessor/discovery.rs");
    assert!(registry.contains("claim_authority_attestation: Option<NativeClaimAuthorityBundle>"));
    assert!(registry.contains("revalidate_snapshot_identities(&current)?;"));
    assert!(registry.contains("census native claim authority changed during restart validation"));
    assert!(discovery.contains("for attestation in &claim_authority_attestations"));
    assert!(discovery.contains("attestation\n            .revalidate()"));
}

#[test]
fn public_claim_snapshot_is_opaque_typed_and_globs_fail_closed() {
    let model = source("src/collision_assessor/model.rs");
    let identity = source("src/collision_assessor/identity.rs");
    assert!(model.contains("enum CanonicalClaimKind"));
    assert!(model.contains("ExactFile"));
    assert!(model.contains("DirectoryTree"));
    assert!(model.contains("Resource"));
    assert!(model.contains("pub participant_id: String"));
    assert!(!model.contains("pub worktree_path"));
    assert!(!model.contains("pub absolute_path"));
    assert!(identity.contains("IdentityError::AmbiguousGlob"));
    assert!(identity.contains("expected_absent: Vec<PathBuf>"));
}
