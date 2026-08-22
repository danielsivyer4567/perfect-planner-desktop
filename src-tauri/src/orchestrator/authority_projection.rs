//! Scheduler-owned projection for the B20 preclaim authority protocol.
//!
//! This module deliberately performs no I/O and exposes no renderer command.
//! The scheduler is expected to persist each successful transition and its used
//! receipt set atomically before allowing the next transition.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityBinding {
    pub organization_id: String,
    pub repository_id: String,
    pub plan_id: String,
    pub node_id: String,
    pub epoch: u64,
    pub generation: u64,
    pub fence: u64,
    pub plan_digest: String,
    pub manifest_digest: String,
    pub collision_digest: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionStatus {
    Unknown,
    Reserved,
    AuthorityPublished,
    CensusClear,
    ClaimAuthorized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CensusVerdict {
    Clear,
    Conflict,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreclaimReservation {
    pub receipt_id: String,
    pub binding: AuthorityBinding,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityPublicationReceipt {
    pub receipt_id: String,
    pub reservation_receipt_id: String,
    pub binding: AuthorityBinding,
    pub published_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CensusClearReceipt {
    pub receipt_id: String,
    pub reservation_receipt_id: String,
    pub publication_receipt_id: String,
    pub binding: AuthorityBinding,
    pub census_digest: String,
    pub verdict: CensusVerdict,
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimRequest {
    pub request_id: String,
    pub worker_id: String,
    pub clearance_receipt_id: String,
    pub binding: AuthorityBinding,
    pub requested_at_ms: u64,
    pub expires_at_ms: u64,
}

/// This is scheduler authorization data, not a renderer capability.
///
/// Integration must sign it with the scheduler-owned issuer and consume it in
/// the same durable transaction that creates the worker lease.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimAuthorization {
    pub authorization_id: String,
    pub worker_id: String,
    pub reservation_receipt_id: String,
    pub publication_receipt_id: String,
    pub clearance_receipt_id: String,
    pub binding: AuthorityBinding,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectionPolicy {
    pub max_clock_skew_ms: u64,
    pub max_receipt_age_ms: u64,
    pub max_claim_ttl_ms: u64,
}

impl Default for ProjectionPolicy {
    fn default() -> Self {
        Self {
            max_clock_skew_ms: 5_000,
            max_receipt_age_ms: 60_000,
            max_claim_ttl_ms: 30_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionErrorKind {
    WrongOrder,
    InvalidIdentity,
    BindingDrift,
    ReceiptChainDrift,
    Replay,
    StaleTimestamp,
    FutureTimestamp,
    PartialState,
    RestartInvalidated,
    CensusNotClear,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionError {
    pub kind: ProjectionErrorKind,
    pub message: String,
}

impl ProjectionError {
    fn new(kind: ProjectionErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ProjectionError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityProjectionCheckpoint {
    pub status: ProjectionStatus,
    pub reservation: Option<PreclaimReservation>,
    pub publication: Option<AuthorityPublicationReceipt>,
    pub clearance: Option<CensusClearReceipt>,
    pub authorization: Option<ClaimAuthorization>,
    pub used_receipt_ids: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityProjection {
    policy: ProjectionPolicy,
    status: ProjectionStatus,
    reservation: Option<PreclaimReservation>,
    publication: Option<AuthorityPublicationReceipt>,
    clearance: Option<CensusClearReceipt>,
    authorization: Option<ClaimAuthorization>,
    used_receipt_ids: BTreeSet<String>,
}

impl Default for AuthorityProjection {
    fn default() -> Self {
        Self::new(ProjectionPolicy::default())
    }
}

impl AuthorityProjection {
    pub fn new(policy: ProjectionPolicy) -> Self {
        Self {
            policy,
            status: ProjectionStatus::Unknown,
            reservation: None,
            publication: None,
            clearance: None,
            authorization: None,
            used_receipt_ids: BTreeSet::new(),
        }
    }

    pub fn status(&self) -> ProjectionStatus {
        self.status
    }

    pub fn checkpoint(&self) -> AuthorityProjectionCheckpoint {
        AuthorityProjectionCheckpoint {
            status: self.status,
            reservation: self.reservation.clone(),
            publication: self.publication.clone(),
            clearance: self.clearance.clone(),
            authorization: self.authorization.clone(),
            used_receipt_ids: self.used_receipt_ids.clone(),
        }
    }

    /// Restore only a complete, internally coherent, still-fresh checkpoint.
    /// Partial or stale restart state is discarded and remains `Unknown`.
    pub fn restore(
        &mut self,
        checkpoint: AuthorityProjectionCheckpoint,
        now_ms: u64,
    ) -> Result<(), ProjectionError> {
        let validated = Self::validate_checkpoint(self.policy, &checkpoint, now_ms);
        if let Err(error) = validated {
            self.used_receipt_ids.extend(checkpoint.used_receipt_ids);
            return self.fail_closed(error);
        }
        if checkpoint.status != ProjectionStatus::Unknown {
            self.used_receipt_ids.extend(checkpoint.used_receipt_ids);
            return self.fail_closed(ProjectionError::new(
                ProjectionErrorKind::RestartInvalidated,
                "scheduler restart invalidated the in-flight authority chain",
            ));
        }

        self.status = checkpoint.status;
        self.reservation = checkpoint.reservation;
        self.publication = checkpoint.publication;
        self.clearance = checkpoint.clearance;
        self.authorization = checkpoint.authorization;
        self.used_receipt_ids = checkpoint.used_receipt_ids;
        Ok(())
    }

    pub fn reserve(
        &mut self,
        reservation: PreclaimReservation,
        now_ms: u64,
    ) -> Result<(), ProjectionError> {
        if self.status != ProjectionStatus::Unknown || self.has_live_state() {
            return self.fail_closed(ProjectionError::new(
                ProjectionErrorKind::WrongOrder,
                "preclaim reservation was not the first authority transition",
            ));
        }
        if let Err(error) = validate_binding(&reservation.binding)
            .and_then(|_| validate_receipt_id(&reservation.receipt_id, "reservation"))
            .and_then(|_| {
                validate_window(
                    "reservation",
                    reservation.issued_at_ms,
                    reservation.expires_at_ms,
                    now_ms,
                    self.policy,
                )
            })
            .and_then(|_| self.reject_replay(&reservation.receipt_id))
        {
            return self.fail_closed(error);
        }

        self.used_receipt_ids.insert(reservation.receipt_id.clone());
        self.reservation = Some(reservation);
        self.status = ProjectionStatus::Reserved;
        Ok(())
    }

    pub fn publish_authority(
        &mut self,
        publication: AuthorityPublicationReceipt,
        now_ms: u64,
    ) -> Result<(), ProjectionError> {
        if self.status != ProjectionStatus::Reserved {
            return self.fail_closed(ProjectionError::new(
                ProjectionErrorKind::WrongOrder,
                "authority publication did not follow exactly one reservation",
            ));
        }
        let Some(reservation) = self.reservation.as_ref() else {
            return self.fail_closed(ProjectionError::new(
                ProjectionErrorKind::PartialState,
                "reserved projection lost its reservation receipt",
            ));
        };
        let validation = validate_receipt_id(&publication.receipt_id, "publication")
            .and_then(|_| self.reject_replay(&publication.receipt_id))
            .and_then(|_| binding_matches(&reservation.binding, &publication.binding))
            .and_then(|_| {
                receipt_link_matches(
                    "publication reservation",
                    &reservation.receipt_id,
                    &publication.reservation_receipt_id,
                )
            })
            .and_then(|_| {
                validate_window(
                    "publication",
                    publication.published_at_ms,
                    publication.expires_at_ms,
                    now_ms,
                    self.policy,
                )
            })
            .and_then(|_| {
                require_monotonic(
                    "publication",
                    reservation.issued_at_ms,
                    publication.published_at_ms,
                )
            })
            .and_then(|_| {
                require_not_after(
                    "publication expiry",
                    publication.expires_at_ms,
                    reservation.expires_at_ms,
                )
            });
        if let Err(error) = validation {
            return self.fail_closed(error);
        }

        self.used_receipt_ids.insert(publication.receipt_id.clone());
        self.publication = Some(publication);
        self.status = ProjectionStatus::AuthorityPublished;
        Ok(())
    }

    pub fn accept_clear_census(
        &mut self,
        clearance: CensusClearReceipt,
        now_ms: u64,
    ) -> Result<(), ProjectionError> {
        if self.status != ProjectionStatus::AuthorityPublished {
            return self.fail_closed(ProjectionError::new(
                ProjectionErrorKind::WrongOrder,
                "collision census did not follow authority publication",
            ));
        }
        let (Some(reservation), Some(publication)) =
            (self.reservation.as_ref(), self.publication.as_ref())
        else {
            return self.fail_closed(ProjectionError::new(
                ProjectionErrorKind::PartialState,
                "published projection is missing its receipt chain",
            ));
        };

        let validation = validate_receipt_id(&clearance.receipt_id, "census")
            .and_then(|_| validate_digest(&clearance.census_digest, "censusDigest"))
            .and_then(|_| self.reject_replay(&clearance.receipt_id))
            .and_then(|_| binding_matches(&reservation.binding, &clearance.binding))
            .and_then(|_| {
                receipt_link_matches(
                    "census reservation",
                    &reservation.receipt_id,
                    &clearance.reservation_receipt_id,
                )
            })
            .and_then(|_| {
                receipt_link_matches(
                    "census publication",
                    &publication.receipt_id,
                    &clearance.publication_receipt_id,
                )
            })
            .and_then(|_| {
                if clearance.verdict == CensusVerdict::Clear {
                    Ok(())
                } else {
                    Err(ProjectionError::new(
                        ProjectionErrorKind::CensusNotClear,
                        "collision census is not CLEAR",
                    ))
                }
            })
            .and_then(|_| {
                validate_window(
                    "census",
                    clearance.observed_at_ms,
                    clearance.expires_at_ms,
                    now_ms,
                    self.policy,
                )
            })
            .and_then(|_| {
                require_monotonic(
                    "census",
                    publication.published_at_ms,
                    clearance.observed_at_ms,
                )
            })
            .and_then(|_| {
                require_not_after(
                    "census expiry",
                    clearance.expires_at_ms,
                    publication.expires_at_ms.min(reservation.expires_at_ms),
                )
            });
        if let Err(error) = validation {
            return self.fail_closed(error);
        }

        self.used_receipt_ids.insert(clearance.receipt_id.clone());
        self.clearance = Some(clearance);
        self.status = ProjectionStatus::CensusClear;
        Ok(())
    }

    /// Atomically consumes the CLEAR receipt in memory and yields claim data.
    ///
    /// The scheduler integration must persist this transition and create the
    /// lease in one durable transaction. A returned value alone is not proof
    /// that a worker owns a claim.
    pub fn consume_clearance(
        &mut self,
        request: ClaimRequest,
        now_ms: u64,
    ) -> Result<ClaimAuthorization, ProjectionError> {
        if self.status != ProjectionStatus::CensusClear {
            return self.fail_closed(ProjectionError::new(
                ProjectionErrorKind::WrongOrder,
                "claim authorization attempted without an unconsumed CLEAR receipt",
            ));
        }
        let (Some(reservation), Some(publication), Some(clearance)) = (
            self.reservation.as_ref(),
            self.publication.as_ref(),
            self.clearance.as_ref(),
        ) else {
            return self.fail_closed(ProjectionError::new(
                ProjectionErrorKind::PartialState,
                "CLEAR projection is missing part of its authority chain",
            ));
        };

        let validation = validate_receipt_id(&request.request_id, "claim request")
            .and_then(|_| require_text(&request.worker_id, "workerId"))
            .and_then(|_| self.reject_replay(&request.request_id))
            .and_then(|_| binding_matches(&reservation.binding, &request.binding))
            .and_then(|_| {
                receipt_link_matches(
                    "claim clearance",
                    &clearance.receipt_id,
                    &request.clearance_receipt_id,
                )
            })
            .and_then(|_| {
                validate_window(
                    "claim request",
                    request.requested_at_ms,
                    request.expires_at_ms,
                    now_ms,
                    self.policy,
                )
            })
            .and_then(|_| {
                require_monotonic(
                    "claim request",
                    clearance.observed_at_ms,
                    request.requested_at_ms,
                )
            })
            .and_then(|_| {
                require_not_after(
                    "claim expiry",
                    request.expires_at_ms,
                    clearance.expires_at_ms,
                )
            })
            .and_then(|_| {
                let ttl = request
                    .expires_at_ms
                    .saturating_sub(request.requested_at_ms);
                if ttl == 0 || ttl > self.policy.max_claim_ttl_ms {
                    Err(ProjectionError::new(
                        ProjectionErrorKind::StaleTimestamp,
                        "claim lifetime exceeds the scheduler policy",
                    ))
                } else {
                    Ok(())
                }
            });
        if let Err(error) = validation {
            return self.fail_closed(error);
        }

        let authorization = ClaimAuthorization {
            authorization_id: request.request_id.clone(),
            worker_id: request.worker_id,
            reservation_receipt_id: reservation.receipt_id.clone(),
            publication_receipt_id: publication.receipt_id.clone(),
            clearance_receipt_id: clearance.receipt_id.clone(),
            binding: request.binding,
            issued_at_ms: request.requested_at_ms,
            expires_at_ms: request.expires_at_ms,
        };
        self.used_receipt_ids.insert(request.request_id);
        self.authorization = Some(authorization.clone());
        self.status = ProjectionStatus::ClaimAuthorized;
        Ok(authorization)
    }

    fn has_live_state(&self) -> bool {
        self.reservation.is_some()
            || self.publication.is_some()
            || self.clearance.is_some()
            || self.authorization.is_some()
    }

    fn reject_replay(&self, receipt_id: &str) -> Result<(), ProjectionError> {
        if self.used_receipt_ids.contains(receipt_id) {
            Err(ProjectionError::new(
                ProjectionErrorKind::Replay,
                format!("receipt {receipt_id} has already been consumed"),
            ))
        } else {
            Ok(())
        }
    }

    fn fail_closed<T>(&mut self, error: ProjectionError) -> Result<T, ProjectionError> {
        self.status = ProjectionStatus::Unknown;
        self.reservation = None;
        self.publication = None;
        self.clearance = None;
        self.authorization = None;
        Err(error)
    }

    fn validate_checkpoint(
        policy: ProjectionPolicy,
        checkpoint: &AuthorityProjectionCheckpoint,
        now_ms: u64,
    ) -> Result<(), ProjectionError> {
        let shape_matches = match checkpoint.status {
            ProjectionStatus::Unknown => {
                checkpoint.reservation.is_none()
                    && checkpoint.publication.is_none()
                    && checkpoint.clearance.is_none()
                    && checkpoint.authorization.is_none()
            }
            ProjectionStatus::Reserved => {
                checkpoint.reservation.is_some()
                    && checkpoint.publication.is_none()
                    && checkpoint.clearance.is_none()
                    && checkpoint.authorization.is_none()
            }
            ProjectionStatus::AuthorityPublished => {
                checkpoint.reservation.is_some()
                    && checkpoint.publication.is_some()
                    && checkpoint.clearance.is_none()
                    && checkpoint.authorization.is_none()
            }
            ProjectionStatus::CensusClear => {
                checkpoint.reservation.is_some()
                    && checkpoint.publication.is_some()
                    && checkpoint.clearance.is_some()
                    && checkpoint.authorization.is_none()
            }
            ProjectionStatus::ClaimAuthorized => {
                checkpoint.reservation.is_some()
                    && checkpoint.publication.is_some()
                    && checkpoint.clearance.is_some()
                    && checkpoint.authorization.is_some()
            }
        };
        if !shape_matches {
            return Err(ProjectionError::new(
                ProjectionErrorKind::PartialState,
                "authority checkpoint is partial or contradicts its status",
            ));
        }
        if checkpoint.status == ProjectionStatus::Unknown {
            return Ok(());
        }

        let mut chain_receipt_ids = BTreeSet::new();
        let reservation = checkpoint.reservation.as_ref().ok_or_else(|| {
            ProjectionError::new(ProjectionErrorKind::PartialState, "reservation is absent")
        })?;
        validate_binding(&reservation.binding)?;
        validate_receipt_id(&reservation.receipt_id, "restored reservation")?;
        require_unique_checkpoint_id(&mut chain_receipt_ids, &reservation.receipt_id)?;
        validate_window(
            "restored reservation",
            reservation.issued_at_ms,
            reservation.expires_at_ms,
            now_ms,
            policy,
        )?;
        require_used(checkpoint, &reservation.receipt_id)?;

        if let Some(publication) = &checkpoint.publication {
            validate_receipt_id(&publication.receipt_id, "restored publication")?;
            require_unique_checkpoint_id(&mut chain_receipt_ids, &publication.receipt_id)?;
            binding_matches(&reservation.binding, &publication.binding)?;
            receipt_link_matches(
                "restored publication reservation",
                &reservation.receipt_id,
                &publication.reservation_receipt_id,
            )?;
            validate_window(
                "restored publication",
                publication.published_at_ms,
                publication.expires_at_ms,
                now_ms,
                policy,
            )?;
            require_monotonic(
                "restored publication",
                reservation.issued_at_ms,
                publication.published_at_ms,
            )?;
            require_not_after(
                "restored publication expiry",
                publication.expires_at_ms,
                reservation.expires_at_ms,
            )?;
            require_used(checkpoint, &publication.receipt_id)?;
        }

        if let Some(clearance) = &checkpoint.clearance {
            let publication = checkpoint.publication.as_ref().ok_or_else(|| {
                ProjectionError::new(ProjectionErrorKind::PartialState, "publication is absent")
            })?;
            if clearance.verdict != CensusVerdict::Clear {
                return Err(ProjectionError::new(
                    ProjectionErrorKind::CensusNotClear,
                    "restored census is not CLEAR",
                ));
            }
            validate_receipt_id(&clearance.receipt_id, "restored census")?;
            require_unique_checkpoint_id(&mut chain_receipt_ids, &clearance.receipt_id)?;
            binding_matches(&reservation.binding, &clearance.binding)?;
            receipt_link_matches(
                "restored census reservation",
                &reservation.receipt_id,
                &clearance.reservation_receipt_id,
            )?;
            receipt_link_matches(
                "restored census publication",
                &publication.receipt_id,
                &clearance.publication_receipt_id,
            )?;
            validate_digest(&clearance.census_digest, "censusDigest")?;
            validate_window(
                "restored census",
                clearance.observed_at_ms,
                clearance.expires_at_ms,
                now_ms,
                policy,
            )?;
            require_monotonic(
                "restored census",
                publication.published_at_ms,
                clearance.observed_at_ms,
            )?;
            require_not_after(
                "restored census expiry",
                clearance.expires_at_ms,
                publication.expires_at_ms.min(reservation.expires_at_ms),
            )?;
            require_used(checkpoint, &clearance.receipt_id)?;
        }

        if let Some(authorization) = &checkpoint.authorization {
            let (Some(publication), Some(clearance)) =
                (&checkpoint.publication, &checkpoint.clearance)
            else {
                return Err(ProjectionError::new(
                    ProjectionErrorKind::PartialState,
                    "authorization chain is partial",
                ));
            };
            validate_receipt_id(&authorization.authorization_id, "restored authorization")?;
            require_unique_checkpoint_id(&mut chain_receipt_ids, &authorization.authorization_id)?;
            binding_matches(&reservation.binding, &authorization.binding)?;
            receipt_link_matches(
                "restored authorization reservation",
                &reservation.receipt_id,
                &authorization.reservation_receipt_id,
            )?;
            receipt_link_matches(
                "restored authorization publication",
                &publication.receipt_id,
                &authorization.publication_receipt_id,
            )?;
            receipt_link_matches(
                "restored authorization clearance",
                &clearance.receipt_id,
                &authorization.clearance_receipt_id,
            )?;
            require_text(&authorization.worker_id, "workerId")?;
            validate_window(
                "restored authorization",
                authorization.issued_at_ms,
                authorization.expires_at_ms,
                now_ms,
                policy,
            )?;
            require_monotonic(
                "restored authorization",
                clearance.observed_at_ms,
                authorization.issued_at_ms,
            )?;
            require_not_after(
                "restored authorization expiry",
                authorization.expires_at_ms,
                clearance.expires_at_ms,
            )?;
            let ttl = authorization
                .expires_at_ms
                .saturating_sub(authorization.issued_at_ms);
            if ttl == 0 || ttl > policy.max_claim_ttl_ms {
                return Err(ProjectionError::new(
                    ProjectionErrorKind::StaleTimestamp,
                    "restored claim lifetime exceeds the scheduler policy",
                ));
            }
            require_used(checkpoint, &authorization.authorization_id)?;
        }

        Ok(())
    }
}

fn validate_binding(binding: &AuthorityBinding) -> Result<(), ProjectionError> {
    require_text(&binding.organization_id, "organizationId")?;
    require_text(&binding.repository_id, "repositoryId")?;
    require_text(&binding.plan_id, "planId")?;
    require_text(&binding.node_id, "nodeId")?;
    if binding.epoch == 0 || binding.generation == 0 || binding.fence == 0 {
        return Err(ProjectionError::new(
            ProjectionErrorKind::InvalidIdentity,
            "epoch, generation and fence must all be non-zero",
        ));
    }
    validate_digest(&binding.plan_digest, "planDigest")?;
    validate_digest(&binding.manifest_digest, "manifestDigest")?;
    validate_digest(&binding.collision_digest, "collisionDigest")
}

fn require_text(value: &str, field: &str) -> Result<(), ProjectionError> {
    if value.trim().is_empty() {
        Err(ProjectionError::new(
            ProjectionErrorKind::InvalidIdentity,
            format!("{field} must not be empty"),
        ))
    } else {
        Ok(())
    }
}

fn validate_receipt_id(value: &str, label: &str) -> Result<(), ProjectionError> {
    require_text(value, &format!("{label} receipt id"))
}

fn validate_digest(value: &str, field: &str) -> Result<(), ProjectionError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Err(ProjectionError::new(
            ProjectionErrorKind::InvalidIdentity,
            format!("{field} must be a 64-character hexadecimal digest"),
        ))
    } else {
        Ok(())
    }
}

fn binding_matches(
    expected: &AuthorityBinding,
    actual: &AuthorityBinding,
) -> Result<(), ProjectionError> {
    if expected == actual {
        Ok(())
    } else {
        Err(ProjectionError::new(
            ProjectionErrorKind::BindingDrift,
            "authority binding drifted across epoch, generation, fence, digest, or scope",
        ))
    }
}

fn receipt_link_matches(label: &str, expected: &str, actual: &str) -> Result<(), ProjectionError> {
    if expected == actual {
        Ok(())
    } else {
        Err(ProjectionError::new(
            ProjectionErrorKind::ReceiptChainDrift,
            format!("{label} receipt chain does not match"),
        ))
    }
}

fn validate_window(
    label: &str,
    observed_at_ms: u64,
    expires_at_ms: u64,
    now_ms: u64,
    policy: ProjectionPolicy,
) -> Result<(), ProjectionError> {
    if observed_at_ms == 0 || expires_at_ms <= observed_at_ms {
        return Err(ProjectionError::new(
            ProjectionErrorKind::StaleTimestamp,
            format!("{label} has an invalid validity window"),
        ));
    }
    if observed_at_ms > now_ms.saturating_add(policy.max_clock_skew_ms) {
        return Err(ProjectionError::new(
            ProjectionErrorKind::FutureTimestamp,
            format!("{label} timestamp is too far in the future"),
        ));
    }
    if expires_at_ms <= now_ms || now_ms.saturating_sub(observed_at_ms) > policy.max_receipt_age_ms
    {
        return Err(ProjectionError::new(
            ProjectionErrorKind::StaleTimestamp,
            format!("{label} is stale or expired"),
        ));
    }
    Ok(())
}

fn require_monotonic(
    label: &str,
    previous_ms: u64,
    current_ms: u64,
) -> Result<(), ProjectionError> {
    if current_ms < previous_ms {
        Err(ProjectionError::new(
            ProjectionErrorKind::StaleTimestamp,
            format!("{label} predates the preceding receipt"),
        ))
    } else {
        Ok(())
    }
}

fn require_not_after(label: &str, actual_ms: u64, ceiling_ms: u64) -> Result<(), ProjectionError> {
    if actual_ms > ceiling_ms {
        Err(ProjectionError::new(
            ProjectionErrorKind::StaleTimestamp,
            format!("{label} outlives its authority chain"),
        ))
    } else {
        Ok(())
    }
}

fn require_used(
    checkpoint: &AuthorityProjectionCheckpoint,
    receipt_id: &str,
) -> Result<(), ProjectionError> {
    if checkpoint.used_receipt_ids.contains(receipt_id) {
        Ok(())
    } else {
        Err(ProjectionError::new(
            ProjectionErrorKind::PartialState,
            format!("checkpoint did not record consumed receipt {receipt_id}"),
        ))
    }
}

fn require_unique_checkpoint_id(
    seen: &mut BTreeSet<String>,
    receipt_id: &str,
) -> Result<(), ProjectionError> {
    if seen.insert(receipt_id.to_string()) {
        Ok(())
    } else {
        Err(ProjectionError::new(
            ProjectionErrorKind::Replay,
            format!("checkpoint reuses receipt id {receipt_id}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    fn binding() -> AuthorityBinding {
        AuthorityBinding {
            organization_id: "org-a".into(),
            repository_id: "repo-a".into(),
            plan_id: "PP-002".into(),
            node_id: "B20".into(),
            epoch: 7,
            generation: 3,
            fence: 41,
            plan_digest: "a".repeat(64),
            manifest_digest: "b".repeat(64),
            collision_digest: "c".repeat(64),
        }
    }

    fn reservation() -> PreclaimReservation {
        PreclaimReservation {
            receipt_id: "reservation-1".into(),
            binding: binding(),
            issued_at_ms: NOW,
            expires_at_ms: NOW + 50_000,
        }
    }

    fn publication() -> AuthorityPublicationReceipt {
        AuthorityPublicationReceipt {
            receipt_id: "publication-1".into(),
            reservation_receipt_id: "reservation-1".into(),
            binding: binding(),
            published_at_ms: NOW + 100,
            expires_at_ms: NOW + 40_000,
        }
    }

    fn clearance() -> CensusClearReceipt {
        CensusClearReceipt {
            receipt_id: "census-1".into(),
            reservation_receipt_id: "reservation-1".into(),
            publication_receipt_id: "publication-1".into(),
            binding: binding(),
            census_digest: "d".repeat(64),
            verdict: CensusVerdict::Clear,
            observed_at_ms: NOW + 200,
            expires_at_ms: NOW + 30_000,
        }
    }

    fn claim() -> ClaimRequest {
        ClaimRequest {
            request_id: "claim-1".into(),
            worker_id: "worker-a".into(),
            clearance_receipt_id: "census-1".into(),
            binding: binding(),
            requested_at_ms: NOW + 300,
            expires_at_ms: NOW + 20_000,
        }
    }

    fn advance_to_clear(projection: &mut AuthorityProjection) {
        projection.reserve(reservation(), NOW).expect("reserve");
        projection
            .publish_authority(publication(), NOW + 100)
            .expect("publish");
        projection
            .accept_clear_census(clearance(), NOW + 200)
            .expect("clear census");
    }

    #[test]
    fn happy_path_requires_every_receipt_and_consumes_clearance_once() {
        let mut projection = AuthorityProjection::default();
        advance_to_clear(&mut projection);
        let authorization = projection
            .consume_clearance(claim(), NOW + 300)
            .expect("authorize");

        assert_eq!(projection.status(), ProjectionStatus::ClaimAuthorized);
        assert_eq!(authorization.binding, binding());
        assert_eq!(authorization.worker_id, "worker-a");

        let replay = projection
            .consume_clearance(claim(), NOW + 301)
            .expect_err("clearance cannot be consumed twice");
        assert_eq!(replay.kind, ProjectionErrorKind::WrongOrder);
        assert_eq!(projection.status(), ProjectionStatus::Unknown);
    }

    #[test]
    fn wrong_order_fails_closed_to_unknown() {
        let mut projection = AuthorityProjection::default();
        let error = projection
            .publish_authority(publication(), NOW)
            .expect_err("publication without reservation must fail");
        assert_eq!(error.kind, ProjectionErrorKind::WrongOrder);
        assert_eq!(projection.status(), ProjectionStatus::Unknown);
        assert_eq!(projection.checkpoint().reservation, None);
    }

    #[test]
    fn generation_fence_and_digest_drift_each_fail_closed() {
        for mutate in [
            |value: &mut AuthorityBinding| value.generation += 1,
            |value: &mut AuthorityBinding| value.fence += 1,
            |value: &mut AuthorityBinding| value.plan_digest = "e".repeat(64),
        ] {
            let mut projection = AuthorityProjection::default();
            projection.reserve(reservation(), NOW).expect("reserve");
            let mut drifted = publication();
            mutate(&mut drifted.binding);
            let error = projection
                .publish_authority(drifted, NOW + 100)
                .expect_err("binding drift must fail");
            assert_eq!(error.kind, ProjectionErrorKind::BindingDrift);
            assert_eq!(projection.status(), ProjectionStatus::Unknown);
        }
    }

    #[test]
    fn stale_future_and_non_monotonic_receipts_fail_closed() {
        let mut stale_projection = AuthorityProjection::default();
        let mut stale = reservation();
        stale.issued_at_ms = NOW - 100_000;
        stale.expires_at_ms = NOW + 1;
        assert_eq!(
            stale_projection.reserve(stale, NOW).unwrap_err().kind,
            ProjectionErrorKind::StaleTimestamp
        );

        let mut future_projection = AuthorityProjection::default();
        let mut future = reservation();
        future.issued_at_ms = NOW + 6_000;
        future.expires_at_ms = NOW + 20_000;
        assert_eq!(
            future_projection.reserve(future, NOW).unwrap_err().kind,
            ProjectionErrorKind::FutureTimestamp
        );

        let mut backwards_projection = AuthorityProjection::default();
        backwards_projection
            .reserve(reservation(), NOW)
            .expect("reserve");
        let mut backwards = publication();
        backwards.published_at_ms = NOW - 1;
        assert_eq!(
            backwards_projection
                .publish_authority(backwards, NOW)
                .unwrap_err()
                .kind,
            ProjectionErrorKind::StaleTimestamp
        );
        assert_eq!(backwards_projection.status(), ProjectionStatus::Unknown);
    }

    #[test]
    fn conflict_or_unknown_census_never_authorizes() {
        for verdict in [CensusVerdict::Conflict, CensusVerdict::Unknown] {
            let mut projection = AuthorityProjection::default();
            projection.reserve(reservation(), NOW).expect("reserve");
            projection
                .publish_authority(publication(), NOW + 100)
                .expect("publish");
            let mut not_clear = clearance();
            not_clear.verdict = verdict;
            let error = projection
                .accept_clear_census(not_clear, NOW + 200)
                .expect_err("non-clear census must fail");
            assert_eq!(error.kind, ProjectionErrorKind::CensusNotClear);
            assert_eq!(projection.status(), ProjectionStatus::Unknown);
        }
    }

    #[test]
    fn partial_restart_state_is_discarded_as_unknown() {
        let mut used = BTreeSet::new();
        used.insert("reservation-1".to_string());
        let checkpoint = AuthorityProjectionCheckpoint {
            status: ProjectionStatus::CensusClear,
            reservation: Some(reservation()),
            publication: None,
            clearance: Some(clearance()),
            authorization: None,
            used_receipt_ids: used,
        };
        let mut restored = AuthorityProjection::default();
        let error = restored
            .restore(checkpoint, NOW + 250)
            .expect_err("partial checkpoint must fail");
        assert_eq!(error.kind, ProjectionErrorKind::PartialState);
        assert_eq!(restored.status(), ProjectionStatus::Unknown);
        assert!(!restored.has_live_state());
    }

    #[test]
    fn coherent_live_checkpoint_is_still_invalidated_by_restart() {
        let mut original = AuthorityProjection::default();
        advance_to_clear(&mut original);
        let checkpoint = original.checkpoint();

        let mut restarted = AuthorityProjection::default();
        let error = restarted
            .restore(checkpoint.clone(), NOW + 500)
            .expect_err("restart must invalidate even a coherent live checkpoint");
        assert_eq!(error.kind, ProjectionErrorKind::RestartInvalidated);
        assert_eq!(restarted.status(), ProjectionStatus::Unknown);
        assert!(!restarted.has_live_state());

        let mut stale = AuthorityProjection::default();
        let error = stale
            .restore(checkpoint, NOW + 35_000)
            .expect_err("expired checkpoint must fail");
        assert_eq!(error.kind, ProjectionErrorKind::StaleTimestamp);
        assert_eq!(stale.status(), ProjectionStatus::Unknown);
    }

    #[test]
    fn receipt_chain_mismatch_and_reused_id_fail_closed() {
        let mut chain_projection = AuthorityProjection::default();
        chain_projection
            .reserve(reservation(), NOW)
            .expect("reserve");
        let mut unlinked = publication();
        unlinked.reservation_receipt_id = "other-reservation".into();
        assert_eq!(
            chain_projection
                .publish_authority(unlinked, NOW + 100)
                .unwrap_err()
                .kind,
            ProjectionErrorKind::ReceiptChainDrift
        );
        assert_eq!(chain_projection.status(), ProjectionStatus::Unknown);

        let mut replay_projection = AuthorityProjection::default();
        replay_projection
            .reserve(reservation(), NOW)
            .expect("reserve");
        let mut replayed = publication();
        replayed.receipt_id = "reservation-1".into();
        assert_eq!(
            replay_projection
                .publish_authority(replayed, NOW + 100)
                .unwrap_err()
                .kind,
            ProjectionErrorKind::Replay
        );
        assert_eq!(replay_projection.status(), ProjectionStatus::Unknown);
    }
}
