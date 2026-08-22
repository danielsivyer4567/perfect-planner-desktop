//! Native scheduler authority issuance for collision-safe reservations.
//!
//! This module deliberately has no Tauri command and accepts no renderer-shaped input. The
//! scheduler must first reduce its native registry, lease, plan and node observations to bounded
//! SHA-256 digests. Only the single native owner below can turn that binding into signed authority.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;

const AUTHORITY_VERSION: u8 = 1;
const DIGEST_BYTES: usize = 32;
const SIGNATURE_BYTES: usize = 64;
const MAX_ACTIVE_RESERVATIONS: usize = 4_096;
const MAX_RESERVATION_TTL_MS: u64 = 300_000;
const ENTROPY_ATTEMPTS: usize = 4;

/// Native-only, fixed-size reservation scope.
///
/// The private fields prevent callers from partially constructing a binding. Raw repository
/// paths, branch names, node labels, worker tokens and renderer values do not belong here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReservationBinding {
    scheduler_instance_digest: [u8; DIGEST_BYTES],
    repository_identity_digest: [u8; DIGEST_BYTES],
    planner_identity_digest: [u8; DIGEST_BYTES],
    plan_content_digest: [u8; DIGEST_BYTES],
    node_set_digest: [u8; DIGEST_BYTES],
    registry_generation: u64,
    lease_generation: u64,
    authority_generation: u64,
}

impl ReservationBinding {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_native_digests(
        scheduler_instance_digest: [u8; DIGEST_BYTES],
        repository_identity_digest: [u8; DIGEST_BYTES],
        planner_identity_digest: [u8; DIGEST_BYTES],
        plan_content_digest: [u8; DIGEST_BYTES],
        node_set_digest: [u8; DIGEST_BYTES],
        registry_generation: u64,
        lease_generation: u64,
        authority_generation: u64,
    ) -> Result<Self, AuthorityError> {
        for digest in [
            &scheduler_instance_digest,
            &repository_identity_digest,
            &planner_identity_digest,
            &plan_content_digest,
            &node_set_digest,
        ] {
            if digest.iter().all(|byte| *byte == 0) {
                return Err(AuthorityError::InvalidDigest);
            }
        }
        if registry_generation == 0 || lease_generation == 0 || authority_generation == 0 {
            return Err(AuthorityError::InvalidGeneration);
        }
        Ok(Self {
            scheduler_instance_digest,
            repository_identity_digest,
            planner_identity_digest,
            plan_content_digest,
            node_set_digest,
            registry_generation,
            lease_generation,
            authority_generation,
        })
    }

    pub(crate) fn digest(&self) -> [u8; DIGEST_BYTES] {
        let mut encoder = DigestEncoder::new(b"perfect-planner:scheduler-reservation-binding:v1");
        encoder.fixed(&self.scheduler_instance_digest);
        encoder.fixed(&self.repository_identity_digest);
        encoder.fixed(&self.planner_identity_digest);
        encoder.fixed(&self.plan_content_digest);
        encoder.fixed(&self.node_set_digest);
        encoder.u64(self.registry_generation);
        encoder.u64(self.lease_generation);
        encoder.u64(self.authority_generation);
        encoder.finish()
    }
}

/// Public verification material. It contains no signing capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthorityVerificationMaterial {
    pub issuer_epoch: u64,
    pub verifying_key: String,
    pub key_fingerprint: String,
}

impl AuthorityVerificationMaterial {
    pub(crate) fn verify(
        &self,
        envelope: &SignedAuthorityEnvelope,
        now_ms: u64,
    ) -> Result<(), AuthorityError> {
        if envelope.version != AUTHORITY_VERSION {
            return Err(AuthorityError::UnsupportedVersion);
        }
        if self.issuer_epoch == 0 || envelope.issuer_epoch != self.issuer_epoch {
            return Err(AuthorityError::StaleEpoch);
        }
        if envelope.issued_at_ms > now_ms {
            return Err(AuthorityError::ClockRollback);
        }
        if now_ms >= envelope.expires_at_ms {
            return Err(AuthorityError::ReservationExpired);
        }
        if envelope.expires_at_ms.saturating_sub(envelope.issued_at_ms) > MAX_RESERVATION_TTL_MS {
            return Err(AuthorityError::InvalidLifetime);
        }
        let verifying_key_bytes = decode_fixed_hex::<DIGEST_BYTES>(&self.verifying_key)
            .map_err(|_| AuthorityError::InvalidVerificationMaterial)?;
        if fingerprint(&verifying_key_bytes) != self.key_fingerprint
            || envelope.issuer_fingerprint != self.key_fingerprint
        {
            return Err(AuthorityError::InvalidVerificationMaterial);
        }
        let expected_digest = envelope_digest(envelope);
        if hex(&expected_digest) != envelope.authority_digest {
            return Err(AuthorityError::SignatureInvalid);
        }
        let signature_bytes = decode_fixed_hex::<SIGNATURE_BYTES>(&envelope.signature)
            .map_err(|_| AuthorityError::SignatureInvalid)?;
        let verifying_key = VerifyingKey::from_bytes(&verifying_key_bytes)
            .map_err(|_| AuthorityError::InvalidVerificationMaterial)?;
        verifying_key
            .verify_strict(
                &authority_signature_message(&expected_digest),
                &Signature::from_bytes(&signature_bytes),
            )
            .map_err(|_| AuthorityError::SignatureInvalid)
    }
}

/// A signed, serializable authority receipt. Every field is integrity-covered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignedAuthorityEnvelope {
    pub version: u8,
    pub issuer_epoch: u64,
    pub issuer_fingerprint: String,
    pub reservation_id: String,
    pub binding_digest: String,
    pub payload_digest: String,
    pub registry_generation: u64,
    pub lease_generation: u64,
    pub authority_generation: u64,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub authority_digest: String,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveReservation {
    binding_digest: [u8; DIGEST_BYTES],
    expires_at_ms: u64,
}

/// The one native signing owner for a scheduler process.
///
/// This type intentionally implements neither `Clone` nor `Serialize`. The `SigningKey` remains
/// private and ed25519-dalek is compiled with its `zeroize` feature, so replacing or dropping the
/// owner zeroizes the library's secret key storage. Temporary seed arrays are also cleared here.
pub(crate) struct SchedulerAuthorityIssuer {
    issuer_epoch: u64,
    signing_key: SigningKey,
    active: BTreeMap<[u8; DIGEST_BYTES], ActiveReservation>,
    max_active: usize,
}

impl SchedulerAuthorityIssuer {
    /// Construct the process owner from operating-system entropy. No secret or verifier is
    /// accepted from IPC or configuration.
    pub(crate) fn new_process(issuer_epoch: u64) -> Result<Self, AuthorityError> {
        if issuer_epoch == 0 {
            return Err(AuthorityError::InvalidEpoch);
        }
        let signing_key = generate_signing_key(fill_os_random)?;
        Ok(Self {
            issuer_epoch,
            signing_key,
            active: BTreeMap::new(),
            max_active: MAX_ACTIVE_RESERVATIONS,
        })
    }

    pub(crate) fn verification_material(&self) -> AuthorityVerificationMaterial {
        let verifying_key = self.signing_key.verifying_key().to_bytes();
        AuthorityVerificationMaterial {
            issuer_epoch: self.issuer_epoch,
            verifying_key: hex(&verifying_key),
            key_fingerprint: fingerprint(&verifying_key),
        }
    }

    /// Rotate to the immediately following process epoch and invalidate every outstanding
    /// reservation. Assignment drops (and therefore zeroizes) the previous dalek signing key.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn rotate_process_epoch(
        &mut self,
        next_epoch: u64,
    ) -> Result<AuthorityVerificationMaterial, AuthorityError> {
        let expected = self
            .issuer_epoch
            .checked_add(1)
            .ok_or(AuthorityError::InvalidEpoch)?;
        if next_epoch != expected {
            return Err(AuthorityError::InvalidEpoch);
        }
        let replacement = generate_signing_key(fill_os_random)?;
        self.signing_key = replacement;
        self.issuer_epoch = next_epoch;
        self.active.clear();
        Ok(self.verification_material())
    }

    /// Mint one authority envelope from already-verified native scheduler state.
    pub(crate) fn issue_reservation_authority(
        &mut self,
        binding: &ReservationBinding,
        payload_digest: [u8; DIGEST_BYTES],
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<SignedAuthorityEnvelope, AuthorityError> {
        self.issue_with_entropy(binding, payload_digest, now_ms, ttl_ms, fill_os_random)
    }

    fn issue_with_entropy<F>(
        &mut self,
        binding: &ReservationBinding,
        payload_digest: [u8; DIGEST_BYTES],
        now_ms: u64,
        ttl_ms: u64,
        mut fill: F,
    ) -> Result<SignedAuthorityEnvelope, AuthorityError>
    where
        F: FnMut(&mut [u8]) -> Result<(), AuthorityError>,
    {
        if payload_digest.iter().all(|byte| *byte == 0) {
            return Err(AuthorityError::InvalidDigest);
        }
        if ttl_ms == 0 || ttl_ms > MAX_RESERVATION_TTL_MS {
            return Err(AuthorityError::InvalidLifetime);
        }
        let expires_at_ms = now_ms
            .checked_add(ttl_ms)
            .ok_or(AuthorityError::ExpiryOverflow)?;
        self.active
            .retain(|_, reservation| reservation.expires_at_ms > now_ms);
        let binding_digest = binding.digest();
        if self
            .active
            .values()
            .any(|reservation| reservation.binding_digest == binding_digest)
        {
            return Err(AuthorityError::DuplicateReservation);
        }
        if self.active.len() >= self.max_active {
            return Err(AuthorityError::CapacityExceeded);
        }

        for _ in 0..ENTROPY_ATTEMPTS {
            let mut nonce = [0_u8; DIGEST_BYTES];
            fill(&mut nonce)?;
            if nonce.iter().all(|byte| *byte == 0) {
                nonce.fill(0);
                return Err(AuthorityError::EntropyUnavailable);
            }
            let reservation_id = reservation_id(self.issuer_epoch, &binding_digest, &nonce);
            nonce.fill(0);
            if self.active.contains_key(&reservation_id) {
                continue;
            }
            let material = self.verification_material();
            let mut envelope = SignedAuthorityEnvelope {
                version: AUTHORITY_VERSION,
                issuer_epoch: self.issuer_epoch,
                issuer_fingerprint: material.key_fingerprint,
                reservation_id: hex(&reservation_id),
                binding_digest: hex(&binding_digest),
                payload_digest: hex(&payload_digest),
                registry_generation: binding.registry_generation,
                lease_generation: binding.lease_generation,
                authority_generation: binding.authority_generation,
                issued_at_ms: now_ms,
                expires_at_ms,
                authority_digest: String::new(),
                signature: String::new(),
            };
            let digest = envelope_digest(&envelope);
            envelope.authority_digest = hex(&digest);
            envelope.signature = hex(&self
                .signing_key
                .sign(&authority_signature_message(&digest))
                .to_bytes());
            self.active.insert(
                reservation_id,
                ActiveReservation {
                    binding_digest,
                    expires_at_ms,
                },
            );
            return Ok(envelope);
        }
        Err(AuthorityError::EntropyCollision)
    }

    /// Retire only an authority issued by this live owner. A stale epoch, altered receipt, or
    /// binding mismatch fails closed and cannot release another reservation.
    #[cfg(test)]
    pub(crate) fn retire_reservation(
        &mut self,
        envelope: &SignedAuthorityEnvelope,
        now_ms: u64,
    ) -> Result<(), AuthorityError> {
        self.verification_material().verify(envelope, now_ms)?;
        let reservation_id = decode_fixed_hex::<DIGEST_BYTES>(&envelope.reservation_id)
            .map_err(|_| AuthorityError::UnknownReservation)?;
        let binding_digest = decode_fixed_hex::<DIGEST_BYTES>(&envelope.binding_digest)
            .map_err(|_| AuthorityError::ReservationMismatch)?;
        let active = self
            .active
            .get(&reservation_id)
            .ok_or(AuthorityError::UnknownReservation)?;
        if active.binding_digest != binding_digest || active.expires_at_ms != envelope.expires_at_ms
        {
            return Err(AuthorityError::ReservationMismatch);
        }
        self.active.remove(&reservation_id);
        Ok(())
    }

    #[cfg(test)]
    fn new_for_test(issuer_epoch: u64, secret: [u8; 32], max_active: usize) -> Self {
        assert!(issuer_epoch > 0);
        assert!(secret != [0; 32]);
        assert!(max_active > 0);
        Self {
            issuer_epoch,
            signing_key: SigningKey::from_bytes(&secret),
            active: BTreeMap::new(),
            max_active,
        }
    }

    #[cfg(test)]
    fn rotate_with_secret_for_test(
        &mut self,
        next_epoch: u64,
        secret: [u8; 32],
    ) -> Result<AuthorityVerificationMaterial, AuthorityError> {
        let expected = self
            .issuer_epoch
            .checked_add(1)
            .ok_or(AuthorityError::InvalidEpoch)?;
        if next_epoch != expected || secret == [0; 32] {
            return Err(AuthorityError::InvalidEpoch);
        }
        self.signing_key = SigningKey::from_bytes(&secret);
        self.issuer_epoch = next_epoch;
        self.active.clear();
        Ok(self.verification_material())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityError {
    InvalidEpoch,
    InvalidDigest,
    InvalidGeneration,
    InvalidLifetime,
    ExpiryOverflow,
    CapacityExceeded,
    DuplicateReservation,
    #[cfg(test)]
    UnknownReservation,
    #[cfg(test)]
    ReservationMismatch,
    ReservationExpired,
    StaleEpoch,
    ClockRollback,
    EntropyUnavailable,
    EntropyCollision,
    InvalidVerificationMaterial,
    UnsupportedVersion,
    SignatureInvalid,
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidEpoch => "scheduler authority epoch is invalid or non-monotonic",
            Self::InvalidDigest => "scheduler authority requires non-zero fixed SHA-256 digests",
            Self::InvalidGeneration => "scheduler authority generations must be non-zero",
            Self::InvalidLifetime => "reservation authority lifetime is outside the bounded limit",
            Self::ExpiryOverflow => "reservation authority expiry overflowed",
            Self::CapacityExceeded => "scheduler authority reservation capacity is exhausted",
            Self::DuplicateReservation => "the exact native reservation binding is already active",
            #[cfg(test)]
            Self::UnknownReservation => "reservation authority is unknown to this process owner",
            #[cfg(test)]
            Self::ReservationMismatch => "reservation authority does not match the active binding",
            Self::ReservationExpired => "reservation authority expired",
            Self::StaleEpoch => "reservation authority belongs to another process epoch",
            Self::ClockRollback => "trusted time is earlier than reservation issuance",
            Self::EntropyUnavailable => "operating-system entropy is unavailable",
            Self::EntropyCollision => "operating-system entropy repeated a live reservation ID",
            Self::InvalidVerificationMaterial => "authority verification material is malformed",
            Self::UnsupportedVersion => "authority envelope version is unsupported",
            Self::SignatureInvalid => "authority envelope signature or digest is invalid",
        })
    }
}

impl std::error::Error for AuthorityError {}

fn envelope_digest(envelope: &SignedAuthorityEnvelope) -> [u8; DIGEST_BYTES] {
    let mut encoder = DigestEncoder::new(b"perfect-planner:scheduler-authority-envelope:v1");
    encoder.u8(envelope.version);
    encoder.u64(envelope.issuer_epoch);
    encoder.text(&envelope.issuer_fingerprint);
    encoder.text(&envelope.reservation_id);
    encoder.text(&envelope.binding_digest);
    encoder.text(&envelope.payload_digest);
    encoder.u64(envelope.registry_generation);
    encoder.u64(envelope.lease_generation);
    encoder.u64(envelope.authority_generation);
    encoder.u64(envelope.issued_at_ms);
    encoder.u64(envelope.expires_at_ms);
    encoder.finish()
}

fn authority_signature_message(digest: &[u8; DIGEST_BYTES]) -> [u8; DIGEST_BYTES] {
    let mut encoder = DigestEncoder::new(b"perfect-planner:scheduler-authority-signature:v1");
    encoder.fixed(digest);
    encoder.finish()
}

fn reservation_id(
    issuer_epoch: u64,
    binding_digest: &[u8; DIGEST_BYTES],
    nonce: &[u8; DIGEST_BYTES],
) -> [u8; DIGEST_BYTES] {
    let mut encoder = DigestEncoder::new(b"perfect-planner:opaque-reservation-id:v1");
    encoder.u64(issuer_epoch);
    encoder.fixed(binding_digest);
    encoder.fixed(nonce);
    encoder.finish()
}

fn fingerprint(verifying_key: &[u8; DIGEST_BYTES]) -> String {
    let mut encoder = DigestEncoder::new(b"perfect-planner:scheduler-verifying-key:v1");
    encoder.fixed(verifying_key);
    hex(&encoder.finish())
}

fn generate_signing_key<F>(mut fill: F) -> Result<SigningKey, AuthorityError>
where
    F: FnMut(&mut [u8]) -> Result<(), AuthorityError>,
{
    let mut secret = [0_u8; DIGEST_BYTES];
    fill(&mut secret)?;
    if secret.iter().all(|byte| *byte == 0) {
        secret.fill(0);
        return Err(AuthorityError::EntropyUnavailable);
    }
    let signing_key = SigningKey::from_bytes(&secret);
    secret.fill(0);
    Ok(signing_key)
}

#[cfg(windows)]
fn fill_os_random(bytes: &mut [u8]) -> Result<(), AuthorityError> {
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    #[link(name = "Bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> i32;
    }
    let length = u32::try_from(bytes.len()).map_err(|_| AuthorityError::EntropyUnavailable)?;
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            length,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status >= 0 {
        Ok(())
    } else {
        Err(AuthorityError::EntropyUnavailable)
    }
}

#[cfg(unix)]
fn fill_os_random(bytes: &mut [u8]) -> Result<(), AuthorityError> {
    use std::fs::File;
    use std::io::Read;
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(bytes))
        .map_err(|_| AuthorityError::EntropyUnavailable)
}

#[cfg(not(any(windows, unix)))]
fn fill_os_random(_bytes: &mut [u8]) -> Result<(), AuthorityError> {
    Err(AuthorityError::EntropyUnavailable)
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn decode_fixed_hex<const N: usize>(value: &str) -> Result<[u8; N], ()> {
    if value.len() != N * 2 {
        return Err(());
    }
    let mut output = [0_u8; N];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = (chunk[0] as char).to_digit(16).ok_or(())? as u8;
        let low = (chunk[1] as char).to_digit(16).ok_or(())? as u8;
        output[index] = (high << 4) | low;
    }
    Ok(output)
}

struct DigestEncoder(Sha256);

impl DigestEncoder {
    fn new(domain: &[u8]) -> Self {
        let mut digest = Sha256::new();
        digest.update((domain.len() as u64).to_be_bytes());
        digest.update(domain);
        Self(digest)
    }

    fn u8(&mut self, value: u8) {
        self.0.update([value]);
    }

    fn u64(&mut self, value: u64) {
        self.0.update(value.to_be_bytes());
    }

    fn fixed(&mut self, value: &[u8; DIGEST_BYTES]) {
        self.0.update(value);
    }

    fn text(&mut self, value: &str) {
        self.0.update((value.len() as u64).to_be_bytes());
        self.0.update(value.as_bytes());
    }

    fn finish(self) -> [u8; DIGEST_BYTES] {
        self.0.finalize().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn binding(seed: u8) -> ReservationBinding {
        ReservationBinding::from_native_digests(
            digest(seed),
            digest(seed.wrapping_add(1)),
            digest(seed.wrapping_add(2)),
            digest(seed.wrapping_add(3)),
            digest(seed.wrapping_add(4)),
            7,
            11,
            13,
        )
        .unwrap()
    }

    fn fill(byte: u8) -> impl FnMut(&mut [u8]) -> Result<(), AuthorityError> {
        move |output| {
            output.fill(byte);
            Ok(())
        }
    }

    #[test]
    fn signed_authority_verifies_and_every_binding_field_is_covered() {
        let mut issuer = SchedulerAuthorityIssuer::new_for_test(41, digest(9), 4);
        let verifier = issuer.verification_material();
        let envelope = issuer
            .issue_with_entropy(&binding(20), digest(80), 1_000, 10_000, fill(44))
            .unwrap();
        assert!(verifier.verify(&envelope, 1_001).is_ok());

        let mut altered = envelope.clone();
        altered.lease_generation += 1;
        assert_eq!(
            verifier.verify(&altered, 1_001),
            Err(AuthorityError::SignatureInvalid)
        );
        let mut altered = envelope.clone();
        altered.payload_digest = hex(&digest(81));
        assert_eq!(
            verifier.verify(&altered, 1_001),
            Err(AuthorityError::SignatureInvalid)
        );
    }

    #[test]
    fn exact_live_binding_cannot_be_reserved_twice_and_capacity_is_bounded() {
        let mut issuer = SchedulerAuthorityIssuer::new_for_test(4, digest(2), 2);
        issuer
            .issue_with_entropy(&binding(10), digest(90), 1_000, 5_000, fill(30))
            .unwrap();
        assert_eq!(
            issuer.issue_with_entropy(&binding(10), digest(90), 1_001, 5_000, fill(31)),
            Err(AuthorityError::DuplicateReservation)
        );
        issuer
            .issue_with_entropy(&binding(20), digest(91), 1_001, 5_000, fill(32))
            .unwrap();
        assert_eq!(
            issuer.issue_with_entropy(&binding(30), digest(92), 1_002, 5_000, fill(33)),
            Err(AuthorityError::CapacityExceeded)
        );
    }

    #[test]
    fn monotonic_epoch_rotation_revokes_live_state_and_changes_public_key() {
        let mut issuer = SchedulerAuthorityIssuer::new_for_test(8, digest(3), 4);
        let old_verifier = issuer.verification_material();
        let old = issuer
            .issue_with_entropy(&binding(10), digest(99), 1_000, 5_000, fill(45))
            .unwrap();
        assert_eq!(
            issuer.rotate_with_secret_for_test(10, digest(4)),
            Err(AuthorityError::InvalidEpoch)
        );
        let new_verifier = issuer.rotate_with_secret_for_test(9, digest(4)).unwrap();
        assert_ne!(old_verifier.verifying_key, new_verifier.verifying_key);
        assert!(old_verifier.verify(&old, 1_001).is_ok());
        assert_eq!(
            new_verifier.verify(&old, 1_001),
            Err(AuthorityError::StaleEpoch)
        );
        assert!(issuer.active.is_empty());
    }

    #[test]
    fn retirement_requires_the_exact_signed_live_reservation() {
        let mut issuer = SchedulerAuthorityIssuer::new_for_test(5, digest(8), 4);
        let first = issuer
            .issue_with_entropy(&binding(10), digest(70), 1_000, 5_000, fill(11))
            .unwrap();
        let mut forged = first.clone();
        forged.binding_digest = hex(&digest(71));
        assert_eq!(
            issuer.retire_reservation(&forged, 1_001),
            Err(AuthorityError::SignatureInvalid)
        );
        issuer.retire_reservation(&first, 1_001).unwrap();
        assert_eq!(
            issuer.retire_reservation(&first, 1_002),
            Err(AuthorityError::UnknownReservation)
        );
    }

    #[test]
    fn invalid_native_scope_lifetime_entropy_and_time_fail_closed() {
        assert_eq!(
            ReservationBinding::from_native_digests(
                [0; 32],
                digest(2),
                digest(3),
                digest(4),
                digest(5),
                1,
                1,
                1,
            ),
            Err(AuthorityError::InvalidDigest)
        );
        let mut issuer = SchedulerAuthorityIssuer::new_for_test(2, digest(7), 2);
        assert_eq!(
            issuer.issue_with_entropy(&binding(10), digest(20), 1, 0, fill(1)),
            Err(AuthorityError::InvalidLifetime)
        );
        assert_eq!(
            issuer.issue_with_entropy(&binding(10), digest(20), 1, 10, fill(0)),
            Err(AuthorityError::EntropyUnavailable)
        );
        let envelope = issuer
            .issue_with_entropy(&binding(10), digest(20), 100, 10, fill(1))
            .unwrap();
        let verifier = issuer.verification_material();
        assert_eq!(
            verifier.verify(&envelope, 99),
            Err(AuthorityError::ClockRollback)
        );
        assert_eq!(
            verifier.verify(&envelope, 110),
            Err(AuthorityError::ReservationExpired)
        );
    }

    #[test]
    fn public_serialization_contains_no_private_signing_material() {
        let issuer = SchedulerAuthorityIssuer::new_for_test(7, digest(55), 2);
        assert!(std::mem::needs_drop::<SchedulerAuthorityIssuer>());
        let json = serde_json::to_string(&issuer.verification_material()).unwrap();
        assert!(json.contains("verifyingKey"));
        assert!(json.contains("keyFingerprint"));
        assert!(!json.contains(&hex(&digest(55))));
    }
}
