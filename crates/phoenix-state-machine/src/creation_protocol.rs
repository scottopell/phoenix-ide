//! Pure durable conversation-creation protocol.

use phoenix_core::domain::creation_protocol::{
    CreationClaim, CreationClaimToken, CreationError, CreationProtocolState, CreationStage,
    CreationStatus, CreationTime, CreationWorkerId,
};
use thiserror::Error;

pub const MAX_CREATION_ATTEMPTS: u32 = 4;
pub const CREATION_RETRY_DELAYS_MS: [CreationTime; 3] = [2_000, 10_000, 30_000];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreationProtocolEvent {
    ClaimRequested {
        worker_id: CreationWorkerId,
        token: CreationClaimToken,
        now: CreationTime,
        lease_duration: CreationTime,
    },
    LeaseRenewed {
        claim: CreationClaim,
        now: CreationTime,
        lease_duration: CreationTime,
    },
    StageSucceeded {
        claim: CreationClaim,
        now: CreationTime,
    },
    StageFailedRetryable {
        claim: CreationClaim,
        now: CreationTime,
        error: CreationError,
    },
    StageFailedPermanent {
        claim: CreationClaim,
        now: CreationTime,
        error: CreationError,
    },
    CancelRequested,
    DeleteRequested,
    ReconciliationSucceeded,
    ReconciliationFailedRetryable {
        error: CreationError,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreationProtocolEffect {
    RunStage {
        claim: CreationClaim,
        stage: CreationStage,
    },
    ReconcileStage {
        claim: CreationClaim,
        stage: CreationStage,
    },
    ReconcileForCancellation,
    ReconcileForDeletion,
    DeleteTombstone,
    WakeAt(CreationTime),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreationDecision {
    pub state: CreationProtocolState,
    pub effects: Vec<CreationProtocolEffect>,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CreationProtocolError {
    #[error("event is not legal while creation is {0}")]
    InvalidStatus(&'static str),
    #[error("creation claim is stale or does not own the current generation")]
    ClaimLost,
    #[error("creation claim lease has expired")]
    LeaseExpired,
    #[error("lease duration must be positive")]
    InvalidLeaseDuration,
}

/// Decide one durable creation transition without performing I/O.
///
/// # Errors
///
/// Returns [`CreationProtocolError`] when the event lacks current authority,
/// its lease has expired, or it is not legal in the current lifecycle state.
#[allow(clippy::too_many_lines, clippy::wildcard_enum_match_arm)]
pub fn decide_creation(
    current: &CreationProtocolState,
    event: CreationProtocolEvent,
) -> Result<CreationDecision, CreationProtocolError> {
    let mut state = current.clone();
    let mut effects = Vec::new();

    match event {
        CreationProtocolEvent::ClaimRequested {
            worker_id,
            token,
            now,
            lease_duration,
        } => {
            if lease_duration == 0 {
                return Err(CreationProtocolError::InvalidLeaseDuration);
            }
            let takeover = match &state.status {
                CreationStatus::Accepted => {
                    state.attempt = 1;
                    false
                }
                CreationStatus::RetryScheduled {
                    next_attempt_at, ..
                } if *next_attempt_at <= now => {
                    state.attempt = state.attempt.saturating_add(1);
                    false
                }
                CreationStatus::Claimed(claim) if claim.lease_until <= now => true,
                status => return Err(CreationProtocolError::InvalidStatus(status_name(status))),
            };
            state.generation = state.generation.saturating_add(1);
            let claim = CreationClaim {
                worker_id,
                generation: state.generation,
                token,
                lease_until: now.saturating_add(lease_duration),
            };
            state.status = CreationStatus::Claimed(claim.clone());
            effects.push(if takeover {
                CreationProtocolEffect::ReconcileStage {
                    claim,
                    stage: state.stage,
                }
            } else {
                CreationProtocolEffect::RunStage {
                    claim,
                    stage: state.stage,
                }
            });
        }
        CreationProtocolEvent::LeaseRenewed {
            claim,
            now,
            lease_duration,
        } => {
            if lease_duration == 0 {
                return Err(CreationProtocolError::InvalidLeaseDuration);
            }
            authorize(&state, &claim, now)?;
            let mut renewed = claim;
            renewed.lease_until = now.saturating_add(lease_duration);
            state.status = CreationStatus::Claimed(renewed);
        }
        CreationProtocolEvent::StageSucceeded { claim, now } => {
            authorize(&state, &claim, now)?;
            if let Some(next) = state.stage.next() {
                state.stage = next;
                effects.push(CreationProtocolEffect::RunStage { claim, stage: next });
            } else {
                state.status = CreationStatus::Ready;
            }
        }
        CreationProtocolEvent::StageFailedRetryable { claim, now, error } => {
            authorize(&state, &claim, now)?;
            if state.attempt >= MAX_CREATION_ATTEMPTS {
                state.status = CreationStatus::Failed(error);
            } else {
                let delay = CREATION_RETRY_DELAYS_MS[(state.attempt - 1) as usize];
                let next_attempt_at = now.saturating_add(delay);
                state.status = CreationStatus::RetryScheduled {
                    next_attempt_at,
                    last_error: error,
                };
                effects.push(CreationProtocolEffect::WakeAt(next_attempt_at));
            }
        }
        CreationProtocolEvent::StageFailedPermanent { claim, now, error } => {
            authorize(&state, &claim, now)?;
            state.status = CreationStatus::Failed(error);
        }
        CreationProtocolEvent::CancelRequested => match state.status {
            CreationStatus::Accepted
            | CreationStatus::Claimed(_)
            | CreationStatus::RetryScheduled { .. } => {
                state.generation = state.generation.saturating_add(1);
                state.status = CreationStatus::Cancelling;
                effects.push(CreationProtocolEffect::ReconcileForCancellation);
            }
            _ => {
                return Err(CreationProtocolError::InvalidStatus(status_name(
                    &state.status,
                )))
            }
        },
        CreationProtocolEvent::DeleteRequested => match state.status {
            CreationStatus::Accepted
            | CreationStatus::Claimed(_)
            | CreationStatus::RetryScheduled { .. }
            | CreationStatus::Cancelling
            | CreationStatus::Cancelled
            | CreationStatus::Failed(_) => {
                state.generation = state.generation.saturating_add(1);
                state.status = CreationStatus::DeletionPending;
                effects.push(CreationProtocolEffect::ReconcileForDeletion);
            }
            _ => {
                return Err(CreationProtocolError::InvalidStatus(status_name(
                    &state.status,
                )))
            }
        },
        CreationProtocolEvent::ReconciliationSucceeded => match state.status {
            CreationStatus::Cancelling => state.status = CreationStatus::Cancelled,
            CreationStatus::DeletionPending => {
                effects.push(CreationProtocolEffect::DeleteTombstone);
            }
            _ => {
                return Err(CreationProtocolError::InvalidStatus(status_name(
                    &state.status,
                )))
            }
        },
        CreationProtocolEvent::ReconciliationFailedRetryable { .. } => match state.status {
            CreationStatus::Cancelling => {
                effects.push(CreationProtocolEffect::ReconcileForCancellation);
            }
            CreationStatus::DeletionPending => {
                effects.push(CreationProtocolEffect::ReconcileForDeletion);
            }
            _ => {
                return Err(CreationProtocolError::InvalidStatus(status_name(
                    &state.status,
                )))
            }
        },
    }

    Ok(CreationDecision { state, effects })
}

fn authorize(
    state: &CreationProtocolState,
    supplied: &CreationClaim,
    now: CreationTime,
) -> Result<(), CreationProtocolError> {
    let CreationStatus::Claimed(current) = &state.status else {
        return Err(CreationProtocolError::ClaimLost);
    };
    if current.worker_id != supplied.worker_id
        || current.generation != supplied.generation
        || current.token != supplied.token
        || state.generation != supplied.generation
    {
        return Err(CreationProtocolError::ClaimLost);
    }
    if current.lease_until <= now {
        return Err(CreationProtocolError::LeaseExpired);
    }
    Ok(())
}

const fn status_name(status: &CreationStatus) -> &'static str {
    match status {
        CreationStatus::Accepted => "accepted",
        CreationStatus::Claimed(_) => "claimed",
        CreationStatus::RetryScheduled { .. } => "retry_scheduled",
        CreationStatus::Cancelling => "cancelling",
        CreationStatus::Cancelled => "cancelled",
        CreationStatus::DeletionPending => "deletion_pending",
        CreationStatus::Ready => "ready",
        CreationStatus::Failed(_) => "failed",
    }
}
