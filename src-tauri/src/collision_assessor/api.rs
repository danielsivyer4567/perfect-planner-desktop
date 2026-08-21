use super::capability::{CapabilityStore, DiscoveryScope, IssuedDiscoveryCapability};
use crate::supervisor::unix_ms;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IssueDiscoveryCapabilityRequest {
    pub run_id: String,
    pub registry_generation: u64,
    pub repository_census_hash: String,
    pub ttl_ms: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevokeDiscoveryCapabilityRequest {
    pub token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeDiscoveryCapabilityResponse {
    pub revoked: bool,
}

#[tauri::command]
pub fn collision_assessor_issue_discovery_capability(
    state: tauri::State<'_, CapabilityStore>,
    request: IssueDiscoveryCapabilityRequest,
) -> Result<IssuedDiscoveryCapability, String> {
    state
        .issue(
            DiscoveryScope {
                run_id: request.run_id,
                registry_generation: request.registry_generation,
                repository_census_hash: request.repository_census_hash,
            },
            unix_ms(),
            request.ttl_ms,
        )
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn collision_assessor_revoke_discovery_capability(
    state: tauri::State<'_, CapabilityStore>,
    request: RevokeDiscoveryCapabilityRequest,
) -> Result<RevokeDiscoveryCapabilityResponse, String> {
    state
        .revoke(&request.token)
        .map_err(|error| error.to_string())?;
    Ok(RevokeDiscoveryCapabilityResponse { revoked: true })
}

#[cfg(test)]
mod tests {
    #[test]
    fn permission_surface_contains_only_named_assessor_commands() {
        let permission = include_str!("../../permissions/collision-assessor.toml");
        let expected = [
            "collision_assessor_issue_discovery_capability",
            "collision_assessor_revoke_discovery_capability",
        ];
        for command in expected {
            assert!(permission.contains(command), "missing command {command}");
        }
        for forbidden in ["path", "shell", "process", "kill", "read_file", "command:"] {
            assert!(
                !permission.to_ascii_lowercase().contains(forbidden),
                "permission surface contains forbidden authority: {forbidden}"
            );
        }
        assert_eq!(permission.matches("collision_assessor_").count(), 2);
    }
}
