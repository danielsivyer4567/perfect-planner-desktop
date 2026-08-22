//! Single-process owner for B20 scheduler admission authority.
//!
//! This state is constructed exactly once by Tauri startup and is never exposed as a command.
//! A held operating-system lock prevents a second desktop process from becoming an issuer for
//! the same app-data scope. Every restart advances the durable epoch before creating a new key,
//! so an in-flight projection from an earlier owner can only recover as UNKNOWN.

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
use std::path::{Path, PathBuf};
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
        write!(file, "{next}\n")
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
