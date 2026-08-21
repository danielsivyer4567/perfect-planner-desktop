//! Strict metadata-only protocol for the native collision census helper.
//!
//! Paths exist only in the parent-to-child request assembled from a validated registry snapshot.
//! They are never accepted from the renderer and never appear in the child response.

use super::identity::{
    physical_path_identity, PhysicalPathIdentity, PhysicalPathKind, RestrictedAuthorityHandle,
};
use super::registry::{
    inventory_attestation, opaque_identity_from_parts, planner_manifest_digest,
    CensusInputSnapshot, DiscoveryCensus, DiscoveryRootCensus, PlannerCensusMetadata,
    PlannerNodeManifest, PlannerRegistration, RootInventoryAttestation,
    ValidatedPlannerRegistration, MAX_PLAN_DIRECTORY_ENTRIES,
};
use crate::supervisor::unix_ms;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf, Prefix};

pub(crate) const PROTOCOL_VERSION: u32 = 1;
pub(crate) const PROTOCOL_DOMAIN: &str = "perfect-planner-collision-census-v1";
pub(crate) const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_PLAN_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ROOTS: usize = 128;
const MAX_PLANNERS: usize = 512;
const MAX_NODES: usize = 2_048;
const MAX_FILES: usize = 8_192;
const MAX_RESOURCES: usize = 2_048;
const MAX_TEXT_BYTES: usize = 4_096;
const PLANNER_DIRECTORY: [&str; 3] = [".claude", "scratch", "perfect-plan"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CensusHelperRequest {
    pub protocol_version: u32,
    pub protocol_domain: String,
    pub nonce: String,
    pub registry_generation: u64,
    pub input_digest: String,
    pub capability_deadline_ms: u64,
    pub roots: Vec<RootInput>,
    pub planners: Vec<PlannerInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RootInput {
    pub root_id: String,
    pub authority: PathAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PlannerInput {
    pub registration: PlannerRegistration,
    pub repository_root: PathAuthority,
    pub worktree_root: PathAuthority,
    pub plan: PathAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PathAuthority {
    pub path: String,
    pub volume_id: u64,
    pub file_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CensusHelperResponse {
    pub protocol_version: u32,
    pub protocol_domain: String,
    pub nonce: String,
    pub registry_generation: u64,
    pub input_digest: String,
    pub captured_at_ms: u64,
    pub expires_at_ms: u64,
    pub roots: Vec<DiscoveryRootCensus>,
    pub planners: Vec<PlannerCensusMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiscoveryError {
    Unavailable,
    Timeout,
    Malformed,
    LimitExceeded,
    IdentityChanged,
    Failed,
}

pub(crate) fn request_from_snapshot(
    input: &CensusInputSnapshot,
    nonce: String,
    capability_deadline_ms: u64,
) -> Result<CensusHelperRequest, DiscoveryError> {
    validate_nonce(&nonce)?;
    let roots = input
        .configured_roots
        .iter()
        .map(|root| {
            Ok(RootInput {
                root_id: bounded(&root.root_id)?.to_string(),
                authority: authority(&root.path, &root.identity)?,
            })
        })
        .collect::<Result<Vec<_>, DiscoveryError>>()?;
    let planners = input
        .registrations
        .iter()
        .map(planner_input)
        .collect::<Result<Vec<_>, DiscoveryError>>()?;
    bounded_count(roots.len(), MAX_ROOTS)?;
    bounded_count(planners.len(), MAX_PLANNERS)?;
    Ok(CensusHelperRequest {
        protocol_version: PROTOCOL_VERSION,
        protocol_domain: PROTOCOL_DOMAIN.to_string(),
        nonce,
        registry_generation: input.attestation.registry_generation,
        input_digest: input.attestation.digest_hex(),
        capability_deadline_ms,
        roots,
        planners,
    })
}

pub(crate) fn execute_request(
    request: CensusHelperRequest,
) -> Result<CensusHelperResponse, DiscoveryError> {
    validate_request(&request)?;
    let started_at_ms = unix_ms();
    if started_at_ms >= request.capability_deadline_ms {
        return Err(DiscoveryError::Timeout);
    }

    let mut root_ids = BTreeSet::new();
    let mut roots = Vec::with_capacity(request.roots.len());
    for root in &request.roots {
        if !root_ids.insert(root.root_id.clone()) {
            return Err(DiscoveryError::Malformed);
        }
        validate_authority(&root.authority, PhysicalPathKind::Directory)?;
        roots.push((root, Vec::<String>::new()));
    }

    let mut planner_ids = BTreeSet::new();
    let mut plan_identities = BTreeSet::new();
    let mut metadata = Vec::with_capacity(request.planners.len());
    for planner in &request.planners {
        if unix_ms() >= request.capability_deadline_ms {
            return Err(DiscoveryError::Timeout);
        }
        validate_planner_input(planner)?;
        let registration = &planner.registration;
        if !planner_ids.insert(registration.identity.planner_id.clone()) {
            return Err(DiscoveryError::Malformed);
        }
        let plan_identity = identity_key(&planner.plan)?;
        if !plan_identities.insert(plan_identity) {
            return Err(DiscoveryError::IdentityChanged);
        }
        let worktree_identity = identity_key(&planner.worktree_root)?;
        let matching_roots = roots
            .iter()
            .enumerate()
            .filter_map(|(index, (root, _))| {
                identity_key(&root.authority)
                    .is_ok_and(|root_identity| root_identity == worktree_identity)
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        if matching_roots.len() != 1 {
            return Err(DiscoveryError::IdentityChanged);
        }
        let fixed_plan_directory =
            planner_directory(Path::new(&roots[matching_roots[0]].0.authority.path));
        let plan_parent = Path::new(&planner.plan.path)
            .parent()
            .ok_or(DiscoveryError::IdentityChanged)?;
        validate_local_path(&fixed_plan_directory, PhysicalPathKind::Directory)?;
        let fixed_identity =
            physical_path_identity(&fixed_plan_directory, PhysicalPathKind::Directory)
                .map_err(|_| DiscoveryError::IdentityChanged)?;
        let parent_identity = physical_path_identity(plan_parent, PhysicalPathKind::Directory)
            .map_err(|_| DiscoveryError::IdentityChanged)?;
        if fixed_identity.volume_id != parent_identity.volume_id
            || fixed_identity.file_id != parent_identity.file_id
        {
            return Err(DiscoveryError::IdentityChanged);
        }

        let bytes = read_bounded_plan(&planner.plan)?;
        let plan_digest = hex_sha256(&bytes);
        let projected = project_plan(&bytes)?;
        validate_projection(registration, &projected)?;
        roots[matching_roots[0]]
            .1
            .push(registration.identity.planner_id.clone());
        metadata.push(metadata_from(planner, plan_digest)?);

        validate_authority(&planner.repository_root, PhysicalPathKind::Directory)?;
        validate_authority(&planner.worktree_root, PhysicalPathKind::Directory)?;
        validate_authority(&planner.plan, PhysicalPathKind::RegularFile)?;
    }

    let mut root_census = Vec::with_capacity(roots.len());
    for (root, planner_ids) in &roots {
        validate_authority(&root.authority, PhysicalPathKind::Directory)?;
        if planner_ids.is_empty() {
            return Err(DiscoveryError::IdentityChanged);
        }
        let inventory = enumerate_root_plans(root, planner_ids, &request.planners)?;
        let mut planner_ids = planner_ids.clone();
        planner_ids.sort();
        root_census.push(DiscoveryRootCensus {
            root_id: root.root_id.clone(),
            reachable: true,
            planner_ids,
            plan_file_count: inventory.plan_file_count,
            inventory_digest: inventory.inventory_digest,
            failure: None,
        });
    }
    let captured_at_ms = unix_ms();
    if captured_at_ms < started_at_ms || captured_at_ms >= request.capability_deadline_ms {
        return Err(DiscoveryError::Timeout);
    }
    metadata.sort();
    Ok(CensusHelperResponse {
        protocol_version: request.protocol_version,
        protocol_domain: request.protocol_domain,
        nonce: request.nonce,
        registry_generation: request.registry_generation,
        input_digest: request.input_digest,
        captured_at_ms,
        expires_at_ms: request.capability_deadline_ms,
        roots: root_census,
        planners: metadata,
    })
}

pub(crate) fn validate_response(
    request: &CensusHelperRequest,
    response: CensusHelperResponse,
) -> Result<DiscoveryCensus, DiscoveryError> {
    if response.protocol_version != request.protocol_version
        || response.protocol_domain != request.protocol_domain
        || response.nonce != request.nonce
        || response.registry_generation != request.registry_generation
        || response.input_digest != request.input_digest
        || response.expires_at_ms != request.capability_deadline_ms
        || response.captured_at_ms >= response.expires_at_ms
    {
        return Err(DiscoveryError::Malformed);
    }
    let mut expected = response
        .planners
        .iter()
        .zip(&request.planners)
        .map(|(actual, planner)| metadata_from(planner, actual.plan_content_digest.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    expected.sort();
    if response.planners.len() != expected.len() {
        return Err(DiscoveryError::Malformed);
    }
    for (actual, expected) in response.planners.iter().zip(expected.iter()) {
        if actual != expected || !is_sha256(&actual.plan_content_digest) {
            return Err(DiscoveryError::Malformed);
        }
    }
    validate_root_response(request, &response.roots, &response.planners)?;
    Ok(DiscoveryCensus {
        registry_generation: response.registry_generation,
        input_digest: response.input_digest,
        captured_at_ms: response.captured_at_ms,
        expires_at_ms: response.expires_at_ms,
        roots: response.roots,
        planners: response.planners,
    })
}

fn planner_input(planner: &ValidatedPlannerRegistration) -> Result<PlannerInput, DiscoveryError> {
    Ok(PlannerInput {
        registration: planner.registration.clone(),
        repository_root: authority(
            Path::new(&planner.registration.identity.repository_root),
            &planner.repository_root_identity,
        )?,
        worktree_root: authority(
            Path::new(&planner.registration.identity.worktree_root),
            &planner.worktree_root_identity,
        )?,
        plan: authority(
            Path::new(&planner.registration.identity.plan_path),
            &planner.plan_identity,
        )?,
    })
}

fn authority(
    path: &Path,
    identity: &PhysicalPathIdentity,
) -> Result<PathAuthority, DiscoveryError> {
    let path = path.to_str().ok_or(DiscoveryError::Malformed)?;
    bounded(path)?;
    Ok(PathAuthority {
        path: path.to_string(),
        volume_id: identity.volume_id,
        file_id: hex_bytes(&identity.file_id),
    })
}

fn validate_request(request: &CensusHelperRequest) -> Result<(), DiscoveryError> {
    if request.protocol_version != PROTOCOL_VERSION
        || request.protocol_domain != PROTOCOL_DOMAIN
        || request.registry_generation == 0
        || !is_sha256(&request.input_digest)
    {
        return Err(DiscoveryError::Malformed);
    }
    validate_nonce(&request.nonce)?;
    bounded_count(request.roots.len(), MAX_ROOTS)?;
    bounded_count(request.planners.len(), MAX_PLANNERS)?;
    if request.roots.is_empty() {
        return Err(DiscoveryError::Malformed);
    }
    Ok(())
}

fn validate_planner_input(planner: &PlannerInput) -> Result<(), DiscoveryError> {
    let registration = &planner.registration;
    for value in [
        &registration.identity.planner_id,
        &registration.identity.repository_id,
        &registration.identity.branch,
        &registration.identity.plan_id,
    ] {
        bounded(value)?;
    }
    bounded_count(registration.identity.nodes.len(), MAX_NODES)?;
    bounded_count(registration.identity.files.len(), MAX_FILES)?;
    bounded_count(registration.identity.resources.len(), MAX_RESOURCES)?;
    if registration.lease_generation == 0
        || registration.lease_expires_at_ms <= registration.heartbeat_at_ms
    {
        return Err(DiscoveryError::Malformed);
    }
    for node in &registration.identity.nodes {
        bounded(&node.node_id)?;
        bounded_count(node.files.len(), MAX_FILES)?;
        bounded_count(node.resources.len(), MAX_RESOURCES)?;
        validate_manifest_values(&node.files, false)?;
        validate_manifest_values(&node.resources, true)?;
    }
    validate_manifest_values(&registration.identity.files, false)?;
    validate_manifest_values(&registration.identity.resources, true)?;
    validate_authority(&planner.repository_root, PhysicalPathKind::Directory)?;
    validate_authority(&planner.worktree_root, PhysicalPathKind::Directory)?;
    validate_authority(&planner.plan, PhysicalPathKind::RegularFile)?;
    Ok(())
}

fn validate_manifest_values(values: &[String], resource: bool) -> Result<(), DiscoveryError> {
    let mut unique = BTreeSet::new();
    for value in values {
        bounded(value)?;
        if value.contains('\0') || (!resource && !safe_relative_manifest(value)) {
            return Err(DiscoveryError::Malformed);
        }
        if !unique.insert(value) {
            return Err(DiscoveryError::Malformed);
        }
    }
    Ok(())
}

fn safe_relative_manifest(value: &str) -> bool {
    let path = Path::new(value);
    !path.is_absolute()
        && !value.contains(':')
        && !value.contains('*')
        && !value.contains('?')
        && !path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn validate_authority(
    authority: &PathAuthority,
    kind: PhysicalPathKind,
) -> Result<(), DiscoveryError> {
    bounded(&authority.path)?;
    if authority.volume_id == 0 || authority.file_id.len() != 32 {
        return Err(DiscoveryError::Malformed);
    }
    let expected_file_id = decode_file_id(&authority.file_id)?;
    let path = Path::new(&authority.path);
    validate_local_path(path, kind)?;
    let current =
        physical_path_identity(path, kind).map_err(|_| DiscoveryError::IdentityChanged)?;
    if current.volume_id != authority.volume_id || current.file_id != expected_file_id {
        return Err(DiscoveryError::IdentityChanged);
    }
    Ok(())
}

fn read_bounded_plan(authority: &PathAuthority) -> Result<Vec<u8>, DiscoveryError> {
    let path = Path::new(&authority.path);
    let expected = authority_identity(authority)?;
    let mut handle =
        RestrictedAuthorityHandle::open(path, PhysicalPathKind::RegularFile, &expected)
            .map_err(|_| DiscoveryError::IdentityChanged)?;
    handle
        .read_bounded(MAX_PLAN_BYTES)
        .map_err(|_| DiscoveryError::IdentityChanged)
}

#[derive(Debug, PartialEq, Eq)]
struct PlanProjection {
    plan_id: String,
    branch: String,
    nodes: Vec<PlannerNodeManifest>,
}

fn project_plan(bytes: &[u8]) -> Result<PlanProjection, DiscoveryError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| DiscoveryError::Malformed)?;
    let root = value.as_object().ok_or(DiscoveryError::Malformed)?;
    let meta = root
        .get("meta")
        .and_then(Value::as_object)
        .ok_or(DiscoveryError::Malformed)?;
    let plan_id = bounded_json_string(meta.get("number"))?;
    let branch = bounded_json_string(meta.get("branch"))?;
    let entries = root
        .get("vertebrae")
        .and_then(Value::as_array)
        .ok_or(DiscoveryError::Malformed)?;
    bounded_count(entries.len(), MAX_NODES)?;
    let mut nodes = Vec::with_capacity(entries.len());
    let mut ids = BTreeSet::new();
    for entry in entries {
        let entry = entry.as_object().ok_or(DiscoveryError::Malformed)?;
        let node_id = bounded_json_string(entry.get("id"))?;
        if !ids.insert(node_id.clone()) {
            return Err(DiscoveryError::Malformed);
        }
        let files = json_string_list(entry.get("files"), MAX_FILES)?;
        let resources = json_string_list(entry.get("resources"), MAX_RESOURCES)?;
        validate_manifest_values(&files, false)?;
        validate_manifest_values(&resources, true)?;
        nodes.push(PlannerNodeManifest {
            node_id,
            files,
            resources,
        });
    }
    nodes.sort_by(|left, right| left.node_id.cmp(&right.node_id));
    Ok(PlanProjection {
        plan_id,
        branch,
        nodes,
    })
}

fn validate_projection(
    registration: &PlannerRegistration,
    projection: &PlanProjection,
) -> Result<(), DiscoveryError> {
    let mut expected_nodes = registration.identity.nodes.clone();
    expected_nodes.sort_by(|left, right| left.node_id.cmp(&right.node_id));
    let projected_files = projection
        .nodes
        .iter()
        .flat_map(|node| node.files.iter().cloned())
        .collect::<BTreeSet<_>>();
    let projected_resources = projection
        .nodes
        .iter()
        .flat_map(|node| node.resources.iter().cloned())
        .collect::<BTreeSet<_>>();
    let registered_files = registration
        .identity
        .files
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let registered_resources = registration
        .identity
        .resources
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if projection.plan_id != registration.identity.plan_id
        || projection.branch != registration.identity.branch
        || projection.nodes != expected_nodes
        || projected_files != registered_files
        || projected_resources != registered_resources
    {
        return Err(DiscoveryError::IdentityChanged);
    }
    Ok(())
}

fn metadata_from(
    planner: &PlannerInput,
    plan_content_digest: String,
) -> Result<PlannerCensusMetadata, DiscoveryError> {
    let registration = &planner.registration;
    let mut nodes = registration.identity.nodes.clone();
    nodes.sort_by(|left, right| left.node_id.cmp(&right.node_id));
    let repository_file_id = decode_file_id(&planner.repository_root.file_id)?;
    let worktree_file_id = decode_file_id(&planner.worktree_root.file_id)?;
    let mut metadata = PlannerCensusMetadata {
        planner_id: registration.identity.planner_id.clone(),
        repository_id: registration.identity.repository_id.clone(),
        repository_identity: opaque_identity_from_parts(
            b"repository",
            planner.repository_root.volume_id,
            &repository_file_id,
        ),
        worktree_identity: opaque_identity_from_parts(
            b"worktree",
            planner.worktree_root.volume_id,
            &worktree_file_id,
        ),
        branch: registration.identity.branch.clone(),
        plan_id: registration.identity.plan_id.clone(),
        plan_content_digest,
        manifest_digest: String::new(),
        files: registration.identity.files.clone(),
        resources: registration.identity.resources.clone(),
        nodes,
        lease_generation: registration.lease_generation,
        registered_at_ms: registration.registered_at_ms,
        updated_at_ms: registration.updated_at_ms,
        heartbeat_at_ms: registration.heartbeat_at_ms,
        lease_expires_at_ms: registration.lease_expires_at_ms,
    };
    metadata.manifest_digest = planner_manifest_digest(&metadata);
    Ok(metadata)
}

fn enumerate_root_plans(
    root: &RootInput,
    assigned_planner_ids: &[String],
    planners: &[PlannerInput],
) -> Result<RootInventoryAttestation, DiscoveryError> {
    let root_path = Path::new(&root.authority.path);
    let root_identity = authority_identity(&root.authority)?;
    let root_guard =
        RestrictedAuthorityHandle::open(root_path, PhysicalPathKind::Directory, &root_identity)
            .map_err(|_| DiscoveryError::IdentityChanged)?;
    let plan_directory = planner_directory(root_path);
    let expected = planners
        .iter()
        .filter(|planner| assigned_planner_ids.contains(&planner.registration.identity.planner_id))
        .map(|planner| identity_key(&planner.plan))
        .collect::<Result<BTreeSet<_>, _>>()?;

    if !plan_directory.exists() {
        return if expected.is_empty() {
            Err(DiscoveryError::IdentityChanged)
        } else {
            Err(DiscoveryError::IdentityChanged)
        };
    }
    validate_local_path(&plan_directory, PhysicalPathKind::Directory)?;
    let plan_directory_identity =
        physical_path_identity(&plan_directory, PhysicalPathKind::Directory)
            .map_err(|_| DiscoveryError::IdentityChanged)?;
    let plan_directory_guard = RestrictedAuthorityHandle::open(
        &plan_directory,
        PhysicalPathKind::Directory,
        &plan_directory_identity,
    )
    .map_err(|_| DiscoveryError::IdentityChanged)?;
    let mut observed = BTreeSet::new();
    let mut inventory_entries = Vec::new();
    let mut entries = fs::read_dir(&plan_directory).map_err(map_io)?;
    for index in 0..=MAX_PLAN_DIRECTORY_ENTRIES {
        let Some(entry) = entries.next() else {
            break;
        };
        if index == MAX_PLAN_DIRECTORY_ENTRIES {
            return Err(DiscoveryError::LimitExceeded);
        }
        let entry = entry.map_err(map_io)?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(map_io)?;
        if metadata.file_type().is_symlink() {
            return Err(DiscoveryError::IdentityChanged);
        }
        if !metadata.is_file()
            || path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| !extension.eq_ignore_ascii_case("json"))
                .unwrap_or(true)
        {
            continue;
        }
        validate_local_path(&path, PhysicalPathKind::RegularFile)?;
        let identity = physical_path_identity(&path, PhysicalPathKind::RegularFile)
            .map_err(|_| DiscoveryError::IdentityChanged)?;
        let physical_key = (identity.volume_id, identity.file_id);
        if !expected.contains(&physical_key) {
            return Err(DiscoveryError::IdentityChanged);
        }
        observed.insert(physical_key);
        let bytes = read_bounded_path(&path, &identity)?;
        inventory_entries.push((identity.volume_id, identity.file_id, hex_sha256(&bytes)));
    }
    plan_directory_guard
        .revalidate()
        .map_err(|_| DiscoveryError::IdentityChanged)?;
    root_guard
        .revalidate()
        .map_err(|_| DiscoveryError::IdentityChanged)?;
    if observed != expected {
        return Err(DiscoveryError::IdentityChanged);
    }
    inventory_attestation(&inventory_entries).map_err(|_| DiscoveryError::IdentityChanged)
}

fn read_bounded_path(
    path: &Path,
    expected: &PhysicalPathIdentity,
) -> Result<Vec<u8>, DiscoveryError> {
    let mut handle = RestrictedAuthorityHandle::open(path, PhysicalPathKind::RegularFile, expected)
        .map_err(|_| DiscoveryError::IdentityChanged)?;
    handle
        .read_bounded(MAX_PLAN_BYTES)
        .map_err(|_| DiscoveryError::IdentityChanged)
}

fn planner_directory(root: &Path) -> PathBuf {
    PLANNER_DIRECTORY
        .iter()
        .fold(root.to_path_buf(), |path, component| path.join(component))
}

fn validate_root_response(
    request: &CensusHelperRequest,
    actual: &[DiscoveryRootCensus],
    actual_metadata: &[PlannerCensusMetadata],
) -> Result<(), DiscoveryError> {
    if actual.len() != request.roots.len() {
        return Err(DiscoveryError::Malformed);
    }
    let expected_roots = request
        .roots
        .iter()
        .map(|root| root.root_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut actual_roots = BTreeSet::new();
    let mut actual_planners = BTreeSet::new();
    for root in actual {
        if !root.reachable
            || root.failure.is_some()
            || !expected_roots.contains(root.root_id.as_str())
            || !actual_roots.insert(root.root_id.as_str())
        {
            return Err(DiscoveryError::Malformed);
        }
        for planner in &root.planner_ids {
            if !actual_planners.insert(planner.as_str()) {
                return Err(DiscoveryError::Malformed);
            }
        }
        let mut inventory_entries = Vec::with_capacity(root.planner_ids.len());
        for planner_id in &root.planner_ids {
            let request_planner = request
                .planners
                .iter()
                .find(|planner| planner.registration.identity.planner_id == *planner_id)
                .ok_or(DiscoveryError::Malformed)?;
            let response_planner = actual_metadata
                .iter()
                .find(|planner| planner.planner_id == *planner_id)
                .ok_or(DiscoveryError::Malformed)?;
            inventory_entries.push((
                request_planner.plan.volume_id,
                decode_file_id(&request_planner.plan.file_id)?,
                response_planner.plan_content_digest.clone(),
            ));
        }
        let expected_inventory =
            inventory_attestation(&inventory_entries).map_err(|_| DiscoveryError::Malformed)?;
        if root.plan_file_count != expected_inventory.plan_file_count
            || root.inventory_digest != expected_inventory.inventory_digest
        {
            return Err(DiscoveryError::Malformed);
        }
    }
    let expected_planners = request
        .planners
        .iter()
        .map(|planner| planner.registration.identity.planner_id.as_str())
        .collect::<BTreeSet<_>>();
    if actual_planners != expected_planners {
        return Err(DiscoveryError::Malformed);
    }
    Ok(())
}

pub(crate) fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, DiscoveryError> {
    let payload = serde_json::to_vec(value).map_err(|_| DiscoveryError::Malformed)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(DiscoveryError::LimitExceeded);
    }
    let length = u32::try_from(payload.len()).map_err(|_| DiscoveryError::LimitExceeded)?;
    let mut frame = Vec::with_capacity(12 + payload.len());
    frame.extend_from_slice(b"PPCENS1\0");
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub(crate) fn decode_frame<T: for<'de> Deserialize<'de>>(
    frame: &[u8],
) -> Result<T, DiscoveryError> {
    if frame.len() < 12 || &frame[..8] != b"PPCENS1\0" {
        return Err(DiscoveryError::Malformed);
    }
    let length = u32::from_be_bytes(frame[8..12].try_into().unwrap()) as usize;
    if length > MAX_FRAME_BYTES || frame.len() != 12 + length {
        return Err(if length > MAX_FRAME_BYTES {
            DiscoveryError::LimitExceeded
        } else {
            DiscoveryError::Malformed
        });
    }
    serde_json::from_slice(&frame[12..]).map_err(|_| DiscoveryError::Malformed)
}

pub(crate) fn validate_local_path(
    path: &Path,
    kind: PhysicalPathKind,
) -> Result<(), DiscoveryError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(DiscoveryError::IdentityChanged);
    }
    #[cfg(windows)]
    validate_windows_local_path(path, kind)?;
    #[cfg(not(windows))]
    {
        let metadata = fs::symlink_metadata(path).map_err(map_io)?;
        if metadata.file_type().is_symlink()
            || (kind == PhysicalPathKind::Directory && !metadata.is_dir())
            || (kind == PhysicalPathKind::RegularFile && !metadata.is_file())
        {
            return Err(DiscoveryError::IdentityChanged);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn validate_windows_local_path(path: &Path, kind: PhysicalPathKind) -> Result<(), DiscoveryError> {
    use std::os::windows::fs::MetadataExt;

    let mut components = path.components();
    let drive = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) => letter,
            _ => return Err(DiscoveryError::IdentityChanged),
        },
        _ => return Err(DiscoveryError::IdentityChanged),
    };
    if !matches!(components.next(), Some(Component::RootDir)) || !fixed_local_drive(drive) {
        return Err(DiscoveryError::IdentityChanged);
    }
    let mut cursor = PathBuf::from(format!("{}:\\", drive as char));
    for component in components {
        let Component::Normal(part) = component else {
            return Err(DiscoveryError::IdentityChanged);
        };
        cursor.push(part);
        let metadata = fs::symlink_metadata(&cursor).map_err(map_io)?;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(DiscoveryError::IdentityChanged);
        }
    }
    let metadata = fs::symlink_metadata(path).map_err(map_io)?;
    if (kind == PhysicalPathKind::Directory && !metadata.is_dir())
        || (kind == PhysicalPathKind::RegularFile && !metadata.is_file())
    {
        return Err(DiscoveryError::IdentityChanged);
    }

    let root = format!("{}:\\", drive as char)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(root_path_name: *const u16) -> u32;
    }
    const DRIVE_FIXED: u32 = 3;
    if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_FIXED {
        return Err(DiscoveryError::IdentityChanged);
    }
    Ok(())
}

#[cfg(windows)]
fn fixed_local_drive(letter: u8) -> bool {
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn QueryDosDeviceW(device_name: *const u16, target_path: *mut u16, max: u32) -> u32;
    }
    let name = format!("{}:", letter as char)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut target = [0_u16; 512];
    let length =
        unsafe { QueryDosDeviceW(name.as_ptr(), target.as_mut_ptr(), target.len() as u32) };
    if length == 0 {
        return false;
    }
    let target = String::from_utf16_lossy(&target[..length as usize])
        .trim_end_matches('\0')
        .to_ascii_lowercase();
    target.starts_with("\\device\\harddiskvolume")
}

fn bounded(value: &str) -> Result<&str, DiscoveryError> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES || value.contains('\0') {
        Err(DiscoveryError::LimitExceeded)
    } else {
        Ok(value)
    }
}

fn bounded_count(value: usize, maximum: usize) -> Result<(), DiscoveryError> {
    if value > maximum {
        Err(DiscoveryError::LimitExceeded)
    } else {
        Ok(())
    }
}

fn bounded_json_string(value: Option<&Value>) -> Result<String, DiscoveryError> {
    bounded(
        value
            .and_then(Value::as_str)
            .ok_or(DiscoveryError::Malformed)?,
    )
    .map(str::to_string)
}

fn json_string_list(value: Option<&Value>, maximum: usize) -> Result<Vec<String>, DiscoveryError> {
    let entries = value
        .and_then(Value::as_array)
        .ok_or(DiscoveryError::Malformed)?;
    bounded_count(entries.len(), maximum)?;
    entries
        .iter()
        .map(|entry| bounded_json_string(Some(entry)))
        .collect()
}

fn identity_key(authority: &PathAuthority) -> Result<(u64, [u8; 16]), DiscoveryError> {
    Ok((authority.volume_id, decode_file_id(&authority.file_id)?))
}

fn authority_identity(authority: &PathAuthority) -> Result<PhysicalPathIdentity, DiscoveryError> {
    Ok(PhysicalPathIdentity {
        canonical_path: PathBuf::from(&authority.path),
        volume_id: authority.volume_id,
        file_id: decode_file_id(&authority.file_id)?,
    })
}

fn decode_file_id(value: &str) -> Result<[u8; 16], DiscoveryError> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DiscoveryError::Malformed);
    }
    let mut decoded = [0_u8; 16];
    for (index, output) in decoded.iter_mut().enumerate() {
        let offset = index * 2;
        *output = u8::from_str_radix(&value[offset..offset + 2], 16)
            .map_err(|_| DiscoveryError::Malformed)?;
    }
    Ok(decoded)
}

fn validate_nonce(value: &str) -> Result<(), DiscoveryError> {
    if is_sha256(value) {
        Ok(())
    } else {
        Err(DiscoveryError::Malformed)
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn map_io(error: io::Error) -> DiscoveryError {
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => {
            DiscoveryError::IdentityChanged
        }
        _ => DiscoveryError::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::super::registry::{
        CensusInputAttestation, PlannerRegistrationSeed, ValidatedDiscoveryRoot,
    };
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    struct Fixture {
        root: PathBuf,
        repository: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let base = std::env::temp_dir().join(format!(
                "pp-b04-discovery-{label}-{}-{sequence}",
                std::process::id()
            ));
            let root = base.join("worktree");
            let repository = base.join("repository");
            fs::create_dir_all(root.join(".claude/scratch/perfect-plan")).unwrap();
            fs::create_dir_all(&repository).unwrap();
            Self { root, repository }
        }

        fn snapshot(&self, plan_ids: &[&str]) -> CensusInputSnapshot {
            let registrations = plan_ids
                .iter()
                .enumerate()
                .map(|(index, planner_id)| {
                    let plan_path = self
                        .root
                        .join(".claude/scratch/perfect-plan")
                        .join(format!("{planner_id}.json"));
                    let file = format!("src/{planner_id}.rs");
                    let resource = format!("mutex:{planner_id}");
                    let plan_id = format!("PP-{:03}", index + 1);
                    let branch = format!("feature/{planner_id}");
                    fs::write(
                        &plan_path,
                        serde_json::to_vec(&serde_json::json!({
                            "meta": {"number": plan_id, "branch": branch},
                            "vertebrae": [{
                                "id": "B04",
                                "files": [file],
                                "resources": [resource]
                            }],
                            "notes": "SENTINEL-RAW-PLAN-NOTES",
                            "commands": ["SENTINEL-DO-NOT-LEAK"]
                        }))
                        .unwrap(),
                    )
                    .unwrap();
                    ValidatedPlannerRegistration {
                        registration: PlannerRegistration {
                            identity: PlannerRegistrationSeed {
                                planner_id: (*planner_id).into(),
                                repository_id: format!("repo-{planner_id}"),
                                repository_root: self.repository.to_string_lossy().into_owned(),
                                worktree_root: self.root.to_string_lossy().into_owned(),
                                branch,
                                plan_id,
                                plan_path: plan_path.to_string_lossy().into_owned(),
                                files: vec![file.clone()],
                                resources: vec![resource.clone()],
                                nodes: vec![PlannerNodeManifest {
                                    node_id: "B04".into(),
                                    files: vec![file],
                                    resources: vec![resource],
                                }],
                            },
                            lease_generation: 1,
                            registered_at_ms: 1,
                            updated_at_ms: 2,
                            heartbeat_at_ms: 3,
                            lease_expires_at_ms: u64::MAX - 1,
                        },
                        repository_root_identity: physical_path_identity(
                            &self.repository,
                            PhysicalPathKind::Directory,
                        )
                        .unwrap(),
                        worktree_root_identity: physical_path_identity(
                            &self.root,
                            PhysicalPathKind::Directory,
                        )
                        .unwrap(),
                        plan_identity: physical_path_identity(
                            &plan_path,
                            PhysicalPathKind::RegularFile,
                        )
                        .unwrap(),
                    }
                })
                .collect();
            CensusInputSnapshot {
                attestation: CensusInputAttestation {
                    registry_generation: 1,
                    input_digest: [0xab; 32],
                },
                configured_roots: vec![ValidatedDiscoveryRoot {
                    root_id: "root-a".into(),
                    path: self.root.clone(),
                    identity: physical_path_identity(&self.root, PhysicalPathKind::Directory)
                        .unwrap(),
                }],
                registrations,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(parent) = self.root.parent() {
                let _ = fs::remove_dir_all(parent);
            }
        }
    }

    #[test]
    fn frames_reject_trailing_truncated_unknown_and_oversized_input() {
        let value = serde_json::json!({"value": 1});
        let frame = encode_frame(&value).unwrap();
        let decoded: Value = decode_frame(&frame).unwrap();
        assert_eq!(decoded, value);
        assert!(decode_frame::<Value>(&frame[..frame.len() - 1]).is_err());
        let mut trailing = frame.clone();
        trailing.push(0);
        assert!(decode_frame::<Value>(&trailing).is_err());
        let mut oversized = b"PPCENS1\0".to_vec();
        oversized.extend_from_slice(&((MAX_FRAME_BYTES as u32) + 1).to_be_bytes());
        assert_eq!(
            decode_frame::<Value>(&oversized),
            Err(DiscoveryError::LimitExceeded)
        );
    }

    #[test]
    fn strict_request_rejects_unknown_fields_and_nonce_rebinding() {
        let json = serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "protocolDomain": PROTOCOL_DOMAIN,
            "nonce": "a".repeat(64),
            "registryGeneration": 1,
            "inputDigest": "b".repeat(64),
            "capabilityDeadlineMs": 9,
            "roots": [],
            "planners": [],
            "path": "caller-controlled"
        });
        assert!(serde_json::from_value::<CensusHelperRequest>(json).is_err());
    }

    #[test]
    fn exact_worktree_root_supports_multiple_registered_plans_and_leaks_no_raw_content() {
        let fixture = Fixture::new("multi");
        let snapshot = fixture.snapshot(&["planner-a", "planner-b"]);
        let request = request_from_snapshot(&snapshot, "c".repeat(64), unix_ms() + 30_000).unwrap();
        let response = execute_request(request.clone()).unwrap();
        assert_eq!(
            response.roots[0].planner_ids,
            vec!["planner-a", "planner-b"]
        );
        let serialized = serde_json::to_string(&response).unwrap();
        for forbidden in [
            fixture.root.to_string_lossy().as_ref(),
            fixture.repository.to_string_lossy().as_ref(),
            "SENTINEL-RAW-PLAN-NOTES",
            "SENTINEL-DO-NOT-LEAK",
        ] {
            assert!(!serialized.contains(forbidden));
        }
        let census = validate_response(&request, response).unwrap();
        assert_eq!(census.planners.len(), 2);
    }

    #[test]
    fn unregistered_extra_plan_in_fixed_directory_is_unknown() {
        let fixture = Fixture::new("extra");
        let snapshot = fixture.snapshot(&["planner-a"]);
        fs::write(
            fixture.root.join(".claude/scratch/perfect-plan/extra.json"),
            b"{}",
        )
        .unwrap();
        let request = request_from_snapshot(&snapshot, "d".repeat(64), unix_ms() + 30_000).unwrap();
        assert_eq!(
            execute_request(request),
            Err(DiscoveryError::IdentityChanged)
        );
    }

    #[test]
    fn nested_or_sibling_configured_root_is_unknown() {
        let fixture = Fixture::new("nested");
        let snapshot = fixture.snapshot(&["planner-a"]);
        let mut request =
            request_from_snapshot(&snapshot, "e".repeat(64), unix_ms() + 30_000).unwrap();
        let parent = fixture.root.parent().unwrap();
        let parent_identity = physical_path_identity(parent, PhysicalPathKind::Directory).unwrap();
        request.roots[0].authority = authority(parent, &parent_identity).unwrap();
        assert_eq!(
            execute_request(request),
            Err(DiscoveryError::IdentityChanged)
        );
    }

    #[test]
    fn configured_root_with_zero_registered_planners_is_unknown() {
        let fixture = Fixture::new("zero-root");
        let snapshot = fixture.snapshot(&["planner-a"]);
        let sibling = fixture.root.parent().unwrap().join("sibling-worktree");
        fs::create_dir_all(&sibling).unwrap();
        let sibling_identity =
            physical_path_identity(&sibling, PhysicalPathKind::Directory).unwrap();
        let mut request =
            request_from_snapshot(&snapshot, "4".repeat(64), unix_ms() + 30_000).unwrap();
        request.roots.push(RootInput {
            root_id: "root-empty".into(),
            authority: authority(&sibling, &sibling_identity).unwrap(),
        });
        assert_eq!(
            execute_request(request),
            Err(DiscoveryError::IdentityChanged)
        );
    }

    #[test]
    fn registration_aggregate_must_equal_exact_node_union() {
        let fixture = Fixture::new("union");
        let snapshot = fixture.snapshot(&["planner-a"]);
        let mut request =
            request_from_snapshot(&snapshot, "f".repeat(64), unix_ms() + 30_000).unwrap();
        request.planners[0]
            .registration
            .identity
            .files
            .push("src/fabricated.rs".into());
        assert_eq!(
            execute_request(request),
            Err(DiscoveryError::IdentityChanged)
        );
    }

    #[cfg(windows)]
    #[test]
    fn stable_hardlink_alias_of_registered_plan_is_same_physical_identity() {
        let fixture = Fixture::new("hardlink");
        let snapshot = fixture.snapshot(&["planner-a"]);
        let original = PathBuf::from(&snapshot.registrations[0].registration.identity.plan_path);
        let alias = fixture.root.join(".claude/scratch/perfect-plan/alias.json");
        fs::hard_link(original, alias).unwrap();
        let request = request_from_snapshot(&snapshot, "1".repeat(64), unix_ms() + 30_000).unwrap();
        assert!(execute_request(request).is_ok());
    }

    #[test]
    fn replayed_or_rebound_response_nonce_is_rejected() {
        let fixture = Fixture::new("nonce");
        let snapshot = fixture.snapshot(&["planner-a"]);
        let request = request_from_snapshot(&snapshot, "2".repeat(64), unix_ms() + 30_000).unwrap();
        let mut response = execute_request(request.clone()).unwrap();
        response.nonce = "3".repeat(64);
        assert_eq!(
            validate_response(&request, response),
            Err(DiscoveryError::Malformed)
        );
    }

    #[test]
    fn missing_malformed_duplicate_node_and_duplicate_manifest_are_unknown() {
        let fixture = Fixture::new("invalid-plan");
        let snapshot = fixture.snapshot(&["planner-a"]);
        let plan = PathBuf::from(&snapshot.registrations[0].registration.identity.plan_path);
        let request = request_from_snapshot(&snapshot, "5".repeat(64), unix_ms() + 30_000).unwrap();

        fs::remove_file(&plan).unwrap();
        assert_eq!(
            execute_request(request.clone()),
            Err(DiscoveryError::IdentityChanged)
        );

        fs::write(&plan, b"{not-json").unwrap();
        let mut malformed = request.clone();
        malformed.planners[0].plan = authority(
            &plan,
            &physical_path_identity(&plan, PhysicalPathKind::RegularFile).unwrap(),
        )
        .unwrap();
        assert_eq!(
            execute_request(malformed.clone()),
            Err(DiscoveryError::Malformed)
        );

        let base = serde_json::json!({
            "meta": {"number": "PP-001", "branch": "feature/planner-a"},
            "vertebrae": [
                {"id": "B04", "files": ["src/planner-a.rs"], "resources": ["mutex:planner-a"]},
                {"id": "B04", "files": ["src/planner-a.rs"], "resources": ["mutex:planner-a"]}
            ]
        });
        fs::write(&plan, serde_json::to_vec(&base).unwrap()).unwrap();
        malformed.planners[0].plan = authority(
            &plan,
            &physical_path_identity(&plan, PhysicalPathKind::RegularFile).unwrap(),
        )
        .unwrap();
        assert_eq!(
            execute_request(malformed.clone()),
            Err(DiscoveryError::Malformed)
        );

        let duplicate_manifest = serde_json::json!({
            "meta": {"number": "PP-001", "branch": "feature/planner-a"},
            "vertebrae": [{
                "id": "B04",
                "files": ["src/planner-a.rs", "src/planner-a.rs"],
                "resources": ["mutex:planner-a"]
            }]
        });
        fs::write(&plan, serde_json::to_vec(&duplicate_manifest).unwrap()).unwrap();
        malformed.planners[0].plan = authority(
            &plan,
            &physical_path_identity(&plan, PhysicalPathKind::RegularFile).unwrap(),
        )
        .unwrap();
        assert_eq!(execute_request(malformed), Err(DiscoveryError::Malformed));
    }

    #[test]
    fn forged_response_duplicate_or_missing_root_and_planner_is_rejected() {
        let fixture = Fixture::new("forged-response");
        let snapshot = fixture.snapshot(&["planner-a", "planner-b"]);
        let request = request_from_snapshot(&snapshot, "6".repeat(64), unix_ms() + 30_000).unwrap();
        let good = execute_request(request.clone()).unwrap();

        let mut duplicate_root = good.clone();
        duplicate_root.roots.push(duplicate_root.roots[0].clone());
        assert_eq!(
            validate_response(&request, duplicate_root),
            Err(DiscoveryError::Malformed)
        );

        let mut missing_planner = good.clone();
        missing_planner.planners.pop();
        assert_eq!(
            validate_response(&request, missing_planner),
            Err(DiscoveryError::Malformed)
        );

        let mut duplicate_planner = good;
        duplicate_planner.planners[1] = duplicate_planner.planners[0].clone();
        assert_eq!(
            validate_response(&request, duplicate_planner),
            Err(DiscoveryError::Malformed)
        );
    }
}
