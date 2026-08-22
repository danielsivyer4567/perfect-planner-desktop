//! Source-level security contracts for the B20 scheduler authority issuer.
//!
//! Behavioral cryptography and transition tests belong beside the crate-private modules. These
//! integration checks guard the production boundary: the renderer cannot issue admission, the
//! signing owner cannot be copied or selected by a test toggle, and projection failure remains
//! fail-closed.

use std::{fs, path::PathBuf};

fn source(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn item_body<'a>(source: &'a str, marker: &str) -> &'a str {
    let marker_at = source
        .find(marker)
        .unwrap_or_else(|| panic!("missing source contract marker `{marker}`"));
    let open = source[marker_at..]
        .find('{')
        .map(|offset| marker_at + offset)
        .unwrap_or_else(|| panic!("missing body for `{marker}`"));
    let mut depth = 0_u32;
    for (offset, character) in source[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated body for `{marker}`");
}

fn signature<'a>(source: &'a str, marker: &str) -> &'a str {
    let marker_at = source
        .find(marker)
        .unwrap_or_else(|| panic!("missing source contract marker `{marker}`"));
    let end = source[marker_at..]
        .find('{')
        .map(|offset| marker_at + offset)
        .unwrap_or_else(|| panic!("missing signature terminator for `{marker}`"));
    &source[marker_at..end]
}

fn attributes_before<'a>(source: &'a str, marker: &str) -> &'a str {
    let marker_at = source
        .find(marker)
        .unwrap_or_else(|| panic!("missing source contract marker `{marker}`"));
    let start = source[..marker_at]
        .rfind("\n\n")
        .map_or(0, |offset| offset + 2);
    &source[start..marker_at]
}

#[test]
fn issuer_and_admission_are_not_tauri_commands() {
    let issuer = source("src/collision_assessor/authority.rs");
    let projection = source("src/orchestrator/authority_projection.rs");
    let api = source("src/orchestrator/api.rs");

    assert!(!issuer.contains("#[tauri::command]"));
    assert!(!projection.contains("#[tauri::command]"));
    for function in [
        "pub fn orchestrator_claim_node",
        "pub fn orchestrator_heartbeat",
    ] {
        assert!(
            !attributes_before(&api, function).contains("#[tauri::command]"),
            "legacy admission function `{function}` must remain native-only"
        );
    }
}

#[test]
fn production_manifest_permissions_and_handler_expose_only_brokered_admission() {
    let build = source("build.rs");
    let permission = source("permissions/orchestrator-pipeline.toml");
    let lib = source("src/lib.rs");
    let handler = lib
        .split(".invoke_handler(tauri::generate_handler![")
        .nth(1)
        .and_then(|tail| tail.split("])").next())
        .expect("production Tauri invoke handler");

    for command in ["orchestrator_claim_node", "orchestrator_heartbeat"] {
        assert!(!build.contains(&format!("\"{command}\"")));
        assert!(!permission.contains(&format!("\"{command}\"")));
        assert!(
            !handler.contains(command),
            "renderer invoke handler must not expose `{command}`"
        );
    }
    for command in ["orchestrator_admit_worker", "orchestrator_worker_heartbeat"] {
        assert!(build.contains(&format!("\"{command}\"")));
        assert!(permission.contains(&format!("\"{command}\"")));
        assert!(handler.contains(command));
    }
}

#[test]
fn scheduler_authority_owner_is_non_clone_and_keeps_its_signing_secret_private() {
    let issuer = source("src/collision_assessor/authority.rs");
    let owner = "pub(crate) struct SchedulerAuthorityIssuer";
    let owner_attributes = attributes_before(&issuer, owner);
    let derive_attributes = owner_attributes
        .lines()
        .filter(|line| line.trim_start().starts_with("#[derive("))
        .collect::<Vec<_>>()
        .join("\n");
    let owner_body = item_body(&issuer, owner);
    let signing_key_line = owner_body
        .lines()
        .find(|line| line.contains("signing_key:") && line.contains("SigningKey"))
        .expect("issuer owns an Ed25519 signing key");

    assert!(
        !derive_attributes.contains("Clone"),
        "the process signing owner must not derive Clone"
    );
    assert!(!issuer.contains("impl Clone for SchedulerAuthorityIssuer"));
    assert!(
        signing_key_line.trim_start().starts_with("signing_key:"),
        "signing material must be a private field"
    );
    assert!(
        !issuer.contains("Deserialize"),
        "native authority material must not be deserializable from renderer-shaped input"
    );
}

#[test]
fn production_authority_cannot_be_enabled_by_a_test_boolean() {
    let issuer = source("src/collision_assessor/authority.rs");
    let production_constructor = signature(&issuer, "pub(crate) fn new_process");

    for forbidden in [
        "enabled: bool",
        "test_enabled",
        "authority_enabled",
        "enable_authority",
        "cfg!(test)",
    ] {
        assert!(
            !issuer.contains(forbidden),
            "test or boolean switch `{forbidden}` must not become production authority"
        );
    }
    assert!(production_constructor.contains("epoch: u64"));
    assert!(!production_constructor.contains("bool"));

    let test_constructor = "fn new_for_test";
    assert!(
        attributes_before(&issuer, test_constructor).contains("#[cfg(test)]"),
        "deterministic authority injection must stay test-only"
    );
}

#[test]
fn authority_projection_enforces_the_ordered_fail_closed_admission_chain() {
    let projection = source("src/orchestrator/authority_projection.rs");
    let transitions = [
        (
            "fn reserve",
            "ProjectionStatus::Unknown",
            "ProjectionStatus::Reserved",
        ),
        (
            "fn publish_authority",
            "ProjectionStatus::Reserved",
            "ProjectionStatus::AuthorityPublished",
        ),
        (
            "fn accept_clear_census",
            "ProjectionStatus::AuthorityPublished",
            "ProjectionStatus::CensusClear",
        ),
        (
            "fn consume_clearance",
            "ProjectionStatus::CensusClear",
            "ProjectionStatus::ClaimAuthorized",
        ),
    ];

    for (method, required, next) in transitions {
        let body = item_body(&projection, method);
        assert!(
            body.contains(required),
            "`{method}` must require the prior state `{required}`"
        );
        assert!(
            body.contains(next),
            "`{method}` must be the only transition to `{next}`"
        );
        assert!(
            body.contains("fail_closed"),
            "`{method}` must route invalid order or evidence through fail_closed"
        );
    }

    let fail_closed = item_body(&projection, "fn fail_closed");
    assert!(fail_closed.contains("ProjectionStatus::Unknown"));
    assert!(projection.contains("ProjectionError"));
    assert!(projection.contains("fn binding_matches"));
    assert!(projection.contains("fn receipt_link_matches"));
    assert!(projection.contains("receipt chain does not match"));
}

#[test]
fn renderer_claim_and_heartbeat_clients_invoke_only_brokered_native_admission() {
    let renderer = source("../src/services/orchestratorPipeline.ts");
    let claim = item_body(&renderer, "export async function orchestratorClaim");
    let heartbeat = item_body(&renderer, "export async function orchestratorHeartbeat");

    assert!(claim.contains("orchestrator_admit_worker"));
    assert!(heartbeat.contains("orchestrator_worker_heartbeat"));
    for body in [claim, heartbeat] {
        assert!(body.contains("invokePipeline"));
        assert!(!body.contains("leaseToken"));
        assert!(!body.contains("workerId"));
        assert!(!body.contains("nowMs"));
    }
}

#[test]
fn sealed_signed_grant_is_the_only_authority_backed_scheduler_claim_path() {
    let runtime = source("src/orchestrator/authority_runtime.rs");
    let scheduler = source("src/orchestrator/scheduler.rs");
    let grant_marker = "pub(crate) struct AuthorizedLeaseGrant";
    let grant_attributes = attributes_before(&runtime, grant_marker);
    let grant_body = item_body(&runtime, grant_marker);
    assert!(!grant_attributes.contains("Deserialize"));
    assert!(!runtime.contains("#[tauri::command]"));
    for field in ["authorization:", "signed_authority:", "verification:"] {
        let line = grant_body
            .lines()
            .find(|line| line.contains(field))
            .unwrap_or_else(|| panic!("missing sealed grant field {field}"));
        assert!(!line.trim_start().starts_with("pub"));
    }

    let authorized_claim = item_body(&scheduler, "fn claim_authorized");
    for required in [
        "grant.verify(now_ms)",
        "consumed_authorization_ids",
        "authorization.binding.fence",
        "authority_epoch: Some",
        "authorization_id: Some",
    ] {
        assert!(
            authorized_claim.contains(required),
            "authorized scheduler claim must bind `{required}`"
        );
    }
    assert!(!attributes_before(&scheduler, "fn claim_authorized").contains("#[tauri::command]"));
}
