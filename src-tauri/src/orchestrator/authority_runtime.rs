//! Single-process owner for B20 scheduler admission authority.
//!
//! This state is constructed exactly once by Tauri startup and is never exposed as a command.
//! A held operating-system lock prevents a second desktop process from becoming an issuer for
//! the same app-data scope. Every restart advances the durable epoch before creating a new key,
//! so an in-flight projection from an earlier owner can only recover as UNKNOWN.

#![allow(clippy::items_after_test_module)]

use super::authority_projection::{
    AuthorityProjection, AuthorityProjectionCheckpoint, AuthorityPublicationReceipt,
    CensusClearReceipt, ClaimAuthorization, ClaimRequest, PreclaimReservation, ProjectionPolicy,
};
use crate::collision_assessor::authority::{
    AuthorityVerificationMaterial, ReservationBinding, SchedulerAuthorityIssuer,
    SignedAuthorityEnvelope,
};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const OWNER_DIRECTORY: &str = "scheduler-authority";
const OWNER_LOCK_FILE: &str = "owner.lock";
const EPOCH_FILE: &str = "epoch";
static EPOCH_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) struct SchedulerAuthorityRuntime {
    _owner_lock: File,
    #[cfg(not(windows))]
    owner_lock_path: PathBuf,
    epoch: u64,
    issuer: Mutex<SchedulerAuthorityIssuer>,
    projections: Mutex<BTreeMap<String, AuthorityProjection>>,
}

/// Native-only proof that one exact CLEAR receipt was consumed and signed by the live owner.
/// Private fields prevent renderer-shaped or deserialized input from constructing a lease grant.
pub(crate) struct AuthorizedLeaseGrant {
    scope_id: String,
    authorization: ClaimAuthorization,
    signed_authority: SignedAuthorityEnvelope,
    verification: AuthorityVerificationMaterial,
}

impl AuthorizedLeaseGrant {
    pub(crate) fn scope_id(&self) -> &str {
        &self.scope_id
    }

    pub(crate) fn authorization(&self) -> &ClaimAuthorization {
        &self.authorization
    }

    pub(crate) fn verify(&self, now_ms: u64) -> Result<(), String> {
        self.verification
            .verify(&self.signed_authority, now_ms)
            .map_err(|error| format!("signed scheduler authority rejected: {error}"))?;
        if self.authorization.binding.epoch != self.verification.issuer_epoch
            || self.authorization.expires_at_ms != self.signed_authority.expires_at_ms
            || self.authorization.issued_at_ms != self.signed_authority.issued_at_ms
        {
            return Err("signed scheduler authority does not match the claim window".to_string());
        }
        Ok(())
    }
}

impl SchedulerAuthorityRuntime {
    pub(crate) fn open(app_data_dir: &Path) -> Result<Self, String> {
        let owner_dir = app_data_dir.join(OWNER_DIRECTORY);
        fs::create_dir_all(&owner_dir)
            .map_err(|error| format!("cannot create scheduler authority directory: {error}"))?;
        let owner_dir = owner_dir
            .canonicalize()
            .map_err(|error| format!("cannot resolve scheduler authority directory: {error}"))?;
        let lock_path = owner_dir.join(OWNER_LOCK_FILE);
        let owner_lock = open_exclusive_owner_lock(&lock_path).map_err(|error| {
            format!("scheduler authority already owned or unavailable: {error}")
        })?;
        let epoch = advance_epoch(&owner_dir.join(EPOCH_FILE))?;
        let issuer = SchedulerAuthorityIssuer::new_process(epoch)
            .map_err(|error| format!("cannot initialize scheduler authority issuer: {error}"))?;
        Ok(Self {
            _owner_lock: owner_lock,
            #[cfg(not(windows))]
            owner_lock_path: lock_path,
            epoch,
            issuer: Mutex::new(issuer),
            projections: Mutex::new(BTreeMap::new()),
        })
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(crate) fn verification_material(&self) -> Result<AuthorityVerificationMaterial, String> {
        self.issuer
            .lock()
            .map_err(|_| "scheduler authority issuer lock is poisoned".to_string())
            .map(|issuer| issuer.verification_material())
    }

    pub(crate) fn issue_reservation_authority(
        &self,
        binding: &ReservationBinding,
        payload_digest: [u8; 32],
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<SignedAuthorityEnvelope, String> {
        self.issuer
            .lock()
            .map_err(|_| "scheduler authority issuer lock is poisoned".to_string())?
            .issue_reservation_authority(binding, payload_digest, now_ms, ttl_ms)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn reserve(
        &self,
        scope_id: &str,
        reservation: PreclaimReservation,
        now_ms: u64,
    ) -> Result<AuthorityProjectionCheckpoint, String> {
        self.mutate_projection(scope_id, |projection| {
            projection.reserve(reservation, now_ms)?;
            Ok(projection.checkpoint())
        })
    }

    pub(crate) fn publish_authority(
        &self,
        scope_id: &str,
        publication: AuthorityPublicationReceipt,
        now_ms: u64,
    ) -> Result<AuthorityProjectionCheckpoint, String> {
        self.mutate_projection(scope_id, |projection| {
            projection.publish_authority(publication, now_ms)?;
            Ok(projection.checkpoint())
        })
    }

    pub(crate) fn accept_clear_census(
        &self,
        scope_id: &str,
        clearance: CensusClearReceipt,
        now_ms: u64,
    ) -> Result<AuthorityProjectionCheckpoint, String> {
        self.mutate_projection(scope_id, |projection| {
            projection.accept_clear_census(clearance, now_ms)?;
            Ok(projection.checkpoint())
        })
    }

    pub(crate) fn consume_clearance(
        &self,
        scope_id: &str,
        request: ClaimRequest,
        now_ms: u64,
    ) -> Result<(ClaimAuthorization, AuthorityProjectionCheckpoint), String> {
        self.mutate_projection(scope_id, |projection| {
            let authorization = projection.consume_clearance(request, now_ms)?;
            Ok((authorization, projection.checkpoint()))
        })
    }

    /// Consume CLEAR and sign the resulting authorization while the one process owner is held.
    /// The returned value is not a worker token; only `SchedulerStore::claim_authorized` may turn
    /// it into a persisted lease.
    pub(crate) fn consume_and_sign_clearance(
        &self,
        scope_id: &str,
        request: ClaimRequest,
        native_binding: &ReservationBinding,
        payload_digest: [u8; 32],
        now_ms: u64,
    ) -> Result<(AuthorizedLeaseGrant, AuthorityProjectionCheckpoint), String> {
        if request.requested_at_ms != now_ms {
            return Err(
                "claim authorization must be issued at the scheduler transaction time".to_string(),
            );
        }
        let (authorization, checkpoint) = self.consume_clearance(scope_id, request, now_ms)?;
        let ttl_ms = authorization
            .expires_at_ms
            .checked_sub(now_ms)
            .filter(|ttl| *ttl > 0)
            .ok_or_else(|| "claim authorization already expired".to_string())?;
        let signed_authority =
            self.issue_reservation_authority(native_binding, payload_digest, now_ms, ttl_ms)?;
        let verification = self.verification_material()?;
        let grant = AuthorizedLeaseGrant {
            scope_id: scope_id.to_string(),
            authorization,
            signed_authority,
            verification,
        };
        grant.verify(now_ms)?;
        Ok((grant, checkpoint))
    }

    pub(crate) fn invalidate(&self, scope_id: &str) -> Result<(), String> {
        self.projections
            .lock()
            .map_err(|_| "scheduler authority projection lock is poisoned".to_string())?
            .remove(scope_id);
        Ok(())
    }

    fn mutate_projection<T>(
        &self,
        scope_id: &str,
        operation: impl FnOnce(
            &mut AuthorityProjection,
        ) -> Result<T, super::authority_projection::ProjectionError>,
    ) -> Result<T, String> {
        if scope_id.trim().is_empty() || scope_id.len() > 512 {
            return Err("scheduler authority scope is empty or oversized".to_string());
        }
        let mut projections = self
            .projections
            .lock()
            .map_err(|_| "scheduler authority projection lock is poisoned".to_string())?;
        let projection = projections
            .entry(scope_id.to_string())
            .or_insert_with(|| AuthorityProjection::new(ProjectionPolicy::default()));
        operation(projection).map_err(|error| error.to_string())
    }
}

#[cfg(not(windows))]
impl Drop for SchedulerAuthorityRuntime {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.owner_lock_path);
    }
}

fn advance_epoch(path: &Path) -> Result<u64, String> {
    let current = if path.exists() {
        let mut text = String::new();
        File::open(path)
            .and_then(|mut file| file.read_to_string(&mut text))
            .map_err(|error| format!("cannot read scheduler authority epoch: {error}"))?;
        text.trim()
            .parse::<u64>()
            .map_err(|_| "scheduler authority epoch is malformed".to_string())?
    } else {
        0
    };
    let next = current
        .checked_add(1)
        .filter(|value| *value > 0)
        .ok_or_else(|| "scheduler authority epoch exhausted".to_string())?;
    let sequence = EPOCH_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("tmp-{}-{sequence}", std::process::id()));
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("cannot create scheduler authority epoch temp: {error}"))?;
        writeln!(file, "{next}")
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("cannot flush scheduler authority epoch: {error}"))?;
        replace_file(&temporary, path)
            .map_err(|error| format!("cannot publish scheduler authority epoch: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map(|_| next)
}

#[cfg(windows)]
fn open_exclusive_owner_lock(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
}

#[cfg(not(windows))]
fn open_exclusive_owner_lock(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create_new(true).write(true).open(path)
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::authority_projection::{
        AuthorityBinding, CensusVerdict, ProjectionStatus,
    };
    use crate::orchestrator::scheduler::{NodeStatus, ScheduledNode, SchedulerStore};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pp-authority-runtime-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("temp app data");
        path
    }

    #[test]
    fn one_process_owner_wins_and_restart_rotates_epoch_and_key() {
        let root = temp_dir();
        let first = SchedulerAuthorityRuntime::open(&root).expect("first owner");
        let first_material = first.verification_material().expect("first verifier");
        assert_eq!(first.epoch(), 1);
        assert!(SchedulerAuthorityRuntime::open(&root).is_err());
        drop(first);

        let restarted = SchedulerAuthorityRuntime::open(&root).expect("restart owner");
        let restarted_material = restarted.verification_material().expect("restart verifier");
        assert_eq!(restarted.epoch(), 2);
        assert_eq!(restarted_material.issuer_epoch, 2);
        assert_ne!(
            first_material.key_fingerprint,
            restarted_material.key_fingerprint
        );
        drop(restarted);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn signed_clearance_grant_and_replay_consumption_persist_with_the_lease() {
        let root = temp_dir();
        let runtime = SchedulerAuthorityRuntime::open(&root).expect("authority owner");
        let binding = AuthorityBinding {
            organization_id: "org-a".to_string(),
            repository_id: "repo-a".to_string(),
            plan_id: "PP-002".to_string(),
            node_id: "B20".to_string(),
            epoch: runtime.epoch(),
            generation: 1,
            fence: 1,
            plan_digest: "a".repeat(64),
            manifest_digest: "b".repeat(64),
            collision_digest: "c".repeat(64),
        };
        let scope = "org-a/repo-a/PP-002/B20/g1";
        runtime
            .reserve(
                scope,
                PreclaimReservation {
                    receipt_id: "reservation-1".to_string(),
                    binding: binding.clone(),
                    issued_at_ms: 100,
                    expires_at_ms: 10_000,
                },
                100,
            )
            .expect("reserve");
        runtime
            .publish_authority(
                scope,
                AuthorityPublicationReceipt {
                    receipt_id: "publication-1".to_string(),
                    reservation_receipt_id: "reservation-1".to_string(),
                    binding: binding.clone(),
                    published_at_ms: 101,
                    expires_at_ms: 9_000,
                },
                101,
            )
            .expect("publish");
        runtime
            .accept_clear_census(
                scope,
                CensusClearReceipt {
                    receipt_id: "clearance-1".to_string(),
                    reservation_receipt_id: "reservation-1".to_string(),
                    publication_receipt_id: "publication-1".to_string(),
                    binding: binding.clone(),
                    census_digest: "d".repeat(64),
                    verdict: CensusVerdict::Clear,
                    observed_at_ms: 102,
                    expires_at_ms: 8_000,
                },
                102,
            )
            .expect("clear census");
        let native_binding = ReservationBinding::from_native_digests(
            [1; 32], [2; 32], [3; 32], [4; 32], [5; 32], 1, 1, 1,
        )
        .expect("native binding");
        let (grant, checkpoint) = runtime
            .consume_and_sign_clearance(
                scope,
                ClaimRequest {
                    request_id: "authorization-1".to_string(),
                    worker_id: "worker-1".to_string(),
                    clearance_receipt_id: "clearance-1".to_string(),
                    binding,
                    requested_at_ms: 103,
                    expires_at_ms: 5_000,
                },
                &native_binding,
                [6; 32],
                103,
            )
            .expect("signed claim grant");
        assert_eq!(checkpoint.status, ProjectionStatus::ClaimAuthorized);

        let scheduler = SchedulerStore::open(
            root.join("scheduler.json"),
            root.clone(),
            vec![ScheduledNode {
                id: "B20".to_string(),
                wave: 1,
                depends_on: Vec::new(),
                attempts: 0,
                status: NodeStatus::Ready,
                lease: None,
                stall_alarm_fence: None,
            }],
        )
        .expect("scheduler");
        let lease = scheduler
            .claim_authorized(
                &grant,
                super::super::scheduler::AdmissionGitBaseline {
                    head_commit: "a".repeat(40),
                    outside_manifest_digest: "b".repeat(64),
                },
                103,
            )
            .expect("authorized lease");
        assert_eq!(lease.authority_epoch, Some(runtime.epoch()));
        assert_eq!(lease.authorization_id.as_deref(), Some("authorization-1"));
        assert!(scheduler
            .claim_authorized(
                &grant,
                super::super::scheduler::AdmissionGitBaseline {
                    head_commit: "a".repeat(40),
                    outside_manifest_digest: "b".repeat(64),
                },
                103,
            )
            .is_err());
        let persisted = scheduler.snapshot().expect("snapshot");
        assert!(persisted
            .consumed_authorization_ids
            .contains("authorization-1"));
        drop(scheduler);
        drop(runtime);
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both buffers are valid NUL-terminated UTF-16 paths for this call.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
