//! Typed persistence adapter for the wake workflow profile.
//!
//! This module translates wake-domain values into the normalized workflow repository. It does
//! not introduce a second scheduler or authority model.

#![allow(
    clippy::items_after_statements,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls
)]

use chrono::{DateTime, TimeZone, Utc};
use phoenix_workflow::{
    wake_profile::{
        self, BashTerminalStatus, ObserveHandleIntent, TmuxTerminalStatus, WakeCancellationReason,
        WakeForgottenReason, WakeRegistrationIntent, WakeResourceIdentity, WakeTerminalEvidence,
        WakeTerminalPayload, WorkScopeKind,
    },
    BarrierStatus, EffectAmbiguity, EffectRole, EffectStatus, ReceiptFamily, SemanticAuthority,
    Timestamp, WorkflowStatus,
};
use serde_json::{json, Value};
use sqlx::{Row, Sqlite, Transaction};

use super::{
    AcceptReceiptResult, ClaimEffectResult, DueEffect, DurableAcceptReceiptRequest,
    DurableBarrierMemberRecord, DurableBarrierRecord, DurableClaimAuthority, DurableClaimRequest,
    DurableCodecRef, DurableEffectRecord, DurableObservationRecord, DurablePayload,
    DurableProtocolSelectionRegistration, DurableReceiptOrigin, DurableWakeTerminalProjection,
    DurableWorkflowTransitionCommit, ExternalAcceptanceResult, ReconcileEffectResult,
    RecordObservationResult, WorkflowRepository, WorkflowRepositoryError, WorkflowRepositoryResult,
};

pub const SELECTION_ID: &str = "wake-v1";
pub const SELECTOR_IDENTITY: &str = "phoenix.wake";
pub const EXECUTOR_KIND: &str = "wake";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeRegistrationFailpoint {
    AfterExternalAcceptance,
    AfterInitialTransition,
    AfterTypedBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRegistrationRequest {
    pub idempotency_key: String,
    pub intent_fingerprint: String,
    pub workflow_id: String,
    pub transition_id: String,
    pub binding_id: String,
    pub authority_scope: String,
    pub intent: WakeRegistrationIntent,
    pub fence_version: u64,
    pub accepted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeRegistrationResult {
    New {
        workflow_id: String,
        receipt: DurablePayload,
    },
    Replay {
        workflow_id: String,
        receipt: DurablePayload,
    },
    Conflict,
    Retryable,
    NotAccepting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedWakeEffect {
    pub authority: DurableClaimAuthority,
    pub attempt_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeBinding {
    pub contract_id: String,
    pub resource: WakeResourceIdentity,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeObservationRequest {
    pub observation_id: String,
    pub authority: DurableClaimAuthority,
    pub attempt_id: String,
    pub evidence: WakeTerminalEvidence,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeTerminalReceiptRequest {
    pub receipt_id: String,
    pub reducer_inbox_id: String,
    pub authority: DurableClaimAuthority,
    pub attempt_id: String,
    pub terminal: WakeTerminalPayload,
    pub accepted_at: DateTime<Utc>,
    pub origin: DurableReceiptOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeCancellationRequest {
    pub workflow_id: String,
    pub observe_effect_id: String,
    pub reducer_inbox_id: String,
    pub transition_id: String,
    pub expected_version: u64,
    pub expected_generation: u64,
    pub contract_id: &'static str,
    pub resource: WakeResourceIdentity,
    pub resolved_at: Timestamp,
    pub committed_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct WakeWorkflowAdapter<'a> {
    repository: &'a WorkflowRepository,
}

impl<'a> WakeWorkflowAdapter<'a> {
    #[must_use]
    pub const fn new(repository: &'a WorkflowRepository) -> Self {
        Self { repository }
    }

    /// Ensure the externally retryable wake protocol selection exists.
    pub async fn ensure_protocol_selection(
        &self,
        registered_at: DateTime<Utc>,
    ) -> WorkflowRepositoryResult<()> {
        let existing: Option<(String, i64, i64)> = sqlx::query_as(
            "SELECT profile_id, protocol_version, external_acceptance_enabled \
             FROM workflow_protocol_selections WHERE id = ?1",
        )
        .bind(SELECTION_ID)
        .fetch_optional(self.repository.pool())
        .await?;
        if let Some((profile, version, external)) = existing {
            if profile == wake_profile::PROFILE_ID
                && version == i64::from(wake_profile::PROTOCOL_VERSION)
                && external == 1
            {
                return Ok(());
            }
            return Err(WorkflowRepositoryError::CorruptState(
                "wake protocol selection id is bound to incompatible capabilities".to_owned(),
            ));
        }

        let codecs = [
            wake_profile::snapshot_codec(),
            wake_profile::event_codec(),
            wake_profile::intent_codec(),
            wake_profile::barrier_codec(),
            wake_profile::terminal_codec(),
        ]
        .into_iter()
        .map(|codec| DurableCodecRef {
            family: codec.family.to_owned(),
            version: codec.version,
        })
        .collect();
        let registration = DurableProtocolSelectionRegistration {
            selection_id: SELECTION_ID.to_owned(),
            profile_id: wake_profile::PROFILE_ID.to_owned(),
            selector_identity: SELECTOR_IDENTITY.to_owned(),
            selector_version: 1,
            protocol_version: wake_profile::PROTOCOL_VERSION,
            authority: SemanticAuthority::EngineProtocol,
            accepting: true,
            runtime_acceptance_enabled: true,
            external_acceptance_enabled: true,
            registered_at,
            drained_at: None,
            supported_codecs: codecs,
            executor_kinds: vec![EXECUTOR_KIND.to_owned()],
        };
        match self
            .repository
            .register_protocol_selection(&registration)
            .await
        {
            Ok(()) => Ok(()),
            Err(WorkflowRepositoryError::Sqlx(error)) if is_unique_constraint(&error) => {
                // Another registrar may have won. Re-read and verify rather than assuming parity.
                Box::pin(self.ensure_protocol_selection(registered_at)).await
            }
            Err(error) => Err(error),
        }
    }

    /// Atomically accept registration and install its complete initial durable graph.
    pub async fn register(
        &self,
        request: &WakeRegistrationRequest,
    ) -> WorkflowRepositoryResult<WakeRegistrationResult> {
        match self.register_with_failpoint(request, None).await {
            Err(WorkflowRepositoryError::Sqlx(error))
                if super::is_unique_constraint(&error) || super::is_busy_or_locked(&error) =>
            {
                match self.register_with_failpoint(request, None).await {
                    Err(WorkflowRepositoryError::Sqlx(retry_error))
                        if super::is_busy_or_locked(&retry_error) =>
                    {
                        Ok(WakeRegistrationResult::Retryable)
                    }
                    result => result,
                }
            }
            result => result,
        }
    }

    pub async fn register_with_failpoint(
        &self,
        request: &WakeRegistrationRequest,
        failpoint: Option<WakeRegistrationFailpoint>,
    ) -> WorkflowRepositoryResult<WakeRegistrationResult> {
        self.ensure_protocol_selection(request.accepted_at).await?;
        let receipt = registration_receipt_payload(&request.intent, &request.idempotency_key)?;
        let snapshot = payload(
            wake_profile::snapshot_codec(),
            registration_snapshot_json(&request.intent, request.fence_version),
        )?;
        let commit = registration_transition(request)?;
        let mut tx = self.repository.pool().begin().await?;

        if let Some(existing) = lookup_wake_registration(&mut tx, request).await? {
            tx.rollback().await?;
            return match existing {
                ExternalAcceptanceResult::Replay {
                    workflow_id,
                    handle_receipt,
                } => {
                    validate_registration_invariant(self.repository, request, &snapshot, &commit)
                        .await?;
                    Ok(WakeRegistrationResult::Replay {
                        workflow_id,
                        receipt: handle_receipt,
                    })
                }
                ExternalAcceptanceResult::Conflict => Ok(WakeRegistrationResult::Conflict),
                ExternalAcceptanceResult::New { .. }
                | ExternalAcceptanceResult::Retryable
                | ExternalAcceptanceResult::NotAccepting => {
                    unreachable!("lookup only returns replay or conflict")
                }
            };
        }

        let accepting: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workflow_protocol_selections s \
             JOIN workflow_profile_codecs c ON c.selection_id = s.id \
             JOIN workflow_profile_executors e ON e.selection_id = s.id \
             WHERE s.id = ?1 AND s.profile_id = ?2 AND s.protocol_version = ?3 \
               AND s.authority = 'engine_protocol' AND s.accepting = 1 \
               AND s.external_acceptance_enabled = 1 AND c.codec_family = ?4 \
               AND c.codec_version = ?5 AND e.executor_kind = ?6)",
        )
        .bind(SELECTION_ID)
        .bind(wake_profile::PROFILE_ID)
        .bind(i64::from(wake_profile::PROTOCOL_VERSION))
        .bind(&snapshot.codec.family)
        .bind(i64::from(snapshot.codec.version))
        .bind(EXECUTOR_KIND)
        .fetch_one(&mut *tx)
        .await?;
        if !accepting {
            tx.rollback().await?;
            return Ok(WakeRegistrationResult::NotAccepting);
        }

        insert_registration_core(&mut tx, request, &receipt, &snapshot, &commit).await?;
        fail_registration(
            &mut tx,
            failpoint,
            WakeRegistrationFailpoint::AfterExternalAcceptance,
        )
        .await?;
        insert_registration_transition(&mut tx, &commit).await?;
        fail_registration(
            &mut tx,
            failpoint,
            WakeRegistrationFailpoint::AfterInitialTransition,
        )
        .await?;
        insert_wake_binding(&mut tx, request).await?;
        fail_registration(
            &mut tx,
            failpoint,
            WakeRegistrationFailpoint::AfterTypedBinding,
        )
        .await?;
        tx.commit().await?;
        Ok(WakeRegistrationResult::New {
            workflow_id: request.workflow_id.clone(),
            receipt,
        })
    }

    pub async fn due(&self, now: DateTime<Utc>) -> WorkflowRepositoryResult<Vec<DueEffect>> {
        Ok(self
            .repository
            .discover_due_effects(now)
            .await?
            .into_iter()
            .filter(|due| due_effect_id(due).starts_with("wake-observe:"))
            .collect())
    }

    pub async fn load_binding(&self, workflow_id: &str) -> WorkflowRepositoryResult<WakeBinding> {
        let row = sqlx::query(
            "SELECT contract_id, expires_at FROM wake_workflow_bindings WHERE workflow_id = ?1",
        )
        .bind(workflow_id)
        .fetch_optional(self.repository.pool())
        .await?
        .ok_or_else(|| {
            WorkflowRepositoryError::CorruptState("wake workflow has no typed binding".to_owned())
        })?;
        Ok(WakeBinding {
            contract_id: row.get("contract_id"),
            resource: self.load_resource(workflow_id).await?,
            expires_at: parse_datetime(&row.get::<String, _>("expires_at"))?,
        })
    }

    pub async fn next_deadline(&self) -> WorkflowRepositoryResult<Option<DateTime<Utc>>> {
        let value: Option<String> = sqlx::query_scalar(
            "SELECT MIN(b.expires_at) FROM wake_workflow_bindings b \
             JOIN workflows w ON w.id = b.workflow_id \
             WHERE w.status = 'active' AND w.authority = 'engine_protocol' \
               AND w.execution_mode = 'authoritative'",
        )
        .fetch_one(self.repository.pool())
        .await?;
        value.map(|value| parse_datetime(&value)).transpose()
    }

    pub async fn claim(
        &self,
        due: &DueEffect,
        claim_token: String,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> WorkflowRepositoryResult<Option<ClaimedWakeEffect>> {
        let (workflow_id, effect_id) = match due {
            DueEffect::Eligible {
                workflow_id,
                effect_id,
                ..
            } => (workflow_id, effect_id),
            DueEffect::RetryWait { .. } => return Ok(None),
            DueEffect::ExpiredClaim { authority } => {
                return match self
                    .repository
                    .take_over_expired_claim(&super::DurableClaimTakeover {
                        authority: authority.clone(),
                        replacement_claim_token: claim_token,
                        replacement_worker_id: worker_id,
                        lease_until,
                        now,
                    })
                    .await?
                {
                    super::TakeOverExpiredClaimResult::Claimed { authority, attempt } => {
                        Ok(Some(ClaimedWakeEffect {
                            authority,
                            attempt_id: attempt.attempt_id,
                        }))
                    }
                    super::TakeOverExpiredClaimResult::Ineligible
                    | super::TakeOverExpiredClaimResult::StaleAuthority => Ok(None),
                };
            }
        };
        match self
            .repository
            .claim_effect(&DurableClaimRequest {
                workflow_id: workflow_id.clone(),
                effect_id: effect_id.clone(),
                claim_token,
                worker_id,
                lease_until,
                now,
            })
            .await?
        {
            ClaimEffectResult::Claimed { authority, attempt } => Ok(Some(ClaimedWakeEffect {
                authority,
                attempt_id: attempt.attempt_id,
            })),
            ClaimEffectResult::Ineligible | ClaimEffectResult::Contended => Ok(None),
        }
    }

    pub async fn record_terminal_evidence(
        &self,
        observation: &WakeObservationRequest,
        receipt: &WakeTerminalReceiptRequest,
    ) -> WorkflowRepositoryResult<AcceptReceiptResult> {
        let WakeTerminalPayload::Fired {
            resource, evidence, ..
        } = &receipt.terminal
        else {
            return Ok(AcceptReceiptResult::StaleAuthority);
        };
        if observation.authority != receipt.authority
            || observation.attempt_id != receipt.attempt_id
            || &observation.evidence != evidence
            || observation.evidence.identity() != *resource
        {
            return Ok(AcceptReceiptResult::StaleAuthority);
        }
        let binding = self
            .load_binding(&observation.authority.workflow_id)
            .await?;
        if observation.evidence.identity() != binding.resource {
            return Ok(AcceptReceiptResult::StaleAuthority);
        }
        self.repository
            .record_observation_and_accept_receipt(
                &DurableObservationRecord {
                    observation_id: observation.observation_id.clone(),
                    authority: observation.authority.clone(),
                    attempt_id: observation.attempt_id.clone(),
                    payload: payload(
                        wake_profile::terminal_codec(),
                        evidence_json(&observation.evidence),
                    )?,
                    observed_at: timestamp(observation.evidence.occurred_at())?,
                    recorded_at: observation.recorded_at,
                },
                &DurableAcceptReceiptRequest {
                    receipt_id: receipt.receipt_id.clone(),
                    reducer_inbox_id: receipt.reducer_inbox_id.clone(),
                    authority: receipt.authority.clone(),
                    now: receipt.accepted_at,
                    attempt_id: Some(receipt.attempt_id.clone()),
                    origin: receipt.origin,
                    receipt: payload(
                        wake_profile::terminal_codec(),
                        terminal_json(&receipt.terminal),
                    )?,
                    reducer_event: payload(
                        wake_profile::terminal_codec(),
                        terminal_json(&receipt.terminal),
                    )?,
                    wake_terminal_projection: Some(terminal_projection(&receipt.terminal)?),
                },
            )
            .await
    }

    pub async fn record_observation(
        &self,
        request: &WakeObservationRequest,
    ) -> WorkflowRepositoryResult<RecordObservationResult> {
        if request.evidence.identity() != self.load_resource(&request.authority.workflow_id).await?
        {
            return Ok(RecordObservationResult::StaleAuthority);
        }
        self.repository
            .record_observation(&DurableObservationRecord {
                observation_id: request.observation_id.clone(),
                authority: request.authority.clone(),
                attempt_id: request.attempt_id.clone(),
                payload: payload(
                    wake_profile::terminal_codec(),
                    evidence_json(&request.evidence),
                )?,
                observed_at: timestamp(request.evidence.occurred_at())?,
                recorded_at: request.recorded_at,
            })
            .await
    }

    pub async fn accept_terminal_receipt(
        &self,
        request: &WakeTerminalReceiptRequest,
    ) -> WorkflowRepositoryResult<AcceptReceiptResult> {
        let binding_resource = self.load_resource(&request.authority.workflow_id).await?;
        if request.terminal.resource() != &binding_resource {
            return Ok(AcceptReceiptResult::StaleAuthority);
        }
        self.repository
            .accept_receipt(&DurableAcceptReceiptRequest {
                receipt_id: request.receipt_id.clone(),
                reducer_inbox_id: request.reducer_inbox_id.clone(),
                authority: request.authority.clone(),
                now: request.accepted_at,
                attempt_id: Some(request.attempt_id.clone()),
                origin: request.origin,
                receipt: payload(
                    wake_profile::terminal_codec(),
                    terminal_json(&request.terminal),
                )?,
                reducer_event: payload(
                    wake_profile::terminal_codec(),
                    terminal_json(&request.terminal),
                )?,
                wake_terminal_projection: Some(terminal_projection(&request.terminal)?),
            })
            .await
    }

    pub async fn schedule_retry(
        &self,
        authority: &DurableClaimAuthority,
        now: DateTime<Utc>,
        exact_deadline: DateTime<Utc>,
    ) -> WorkflowRepositoryResult<ReconcileEffectResult> {
        self.repository
            .schedule_retry(authority, now, exact_deadline)
            .await
    }

    pub async fn promote_exact_deadline(
        &self,
        due: &DueEffect,
        now: DateTime<Utc>,
    ) -> WorkflowRepositoryResult<bool> {
        self.repository.promote_retry_due(due, now).await
    }

    /// Cancel by generation fencing and emit a reducer-only event. No owed acceptance is created.
    pub async fn cancel(
        &self,
        request: &WakeCancellationRequest,
    ) -> WorkflowRepositoryResult<bool> {
        let terminal = WakeTerminalPayload::Cancelled {
            contract_id: request.contract_id.to_owned(),
            resource: request.resource.clone(),
            reason: WakeCancellationReason::ExplicitCancel,
            resolved_at: request.resolved_at,
        };
        let mut tx = self.repository.pool().begin().await?;
        let next_generation = request.expected_generation.checked_add(1).ok_or(
            WorkflowRepositoryError::GenerationOutOfRange(request.expected_generation),
        )?;
        let next_version = request.expected_version.checked_add(1).ok_or(
            WorkflowRepositoryError::VersionOutOfRange(request.expected_version),
        )?;
        let updated =
            sqlx::query(
                "UPDATE workflows SET version = ?1, generation = ?2, status = 'cancelled', \
             snapshot_codec_family = ?3, snapshot_codec_version = ?4, snapshot_payload = ?5 \
             WHERE id = ?6 AND version = ?7 AND generation = ?8 AND status = 'active' \
               AND authority = 'engine_protocol' AND execution_mode = 'authoritative'",
            )
            .bind(
                i64::try_from(next_version)
                    .map_err(|_| WorkflowRepositoryError::VersionOutOfRange(next_version))?,
            )
            .bind(
                i64::try_from(next_generation)
                    .map_err(|_| WorkflowRepositoryError::GenerationOutOfRange(next_generation))?,
            )
            .bind(wake_profile::snapshot_codec().family)
            .bind(i64::from(wake_profile::snapshot_codec().version))
            .bind(terminal_json(&terminal).to_string())
            .bind(&request.workflow_id)
            .bind(i64::try_from(request.expected_version).map_err(|_| {
                WorkflowRepositoryError::VersionOutOfRange(request.expected_version)
            })?)
            .bind(i64::try_from(request.expected_generation).map_err(|_| {
                WorkflowRepositoryError::GenerationOutOfRange(request.expected_generation)
            })?)
            .execute(&mut *tx)
            .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        sqlx::query(
            "UPDATE workflow_effects SET status = 'invalidated' WHERE id = ?1 AND workflow_id = ?2 \
             AND generation = ?3 AND status NOT IN ('receipted', 'invalidated')",
        )
        .bind(&request.observe_effect_id)
        .bind(&request.workflow_id)
        .bind(i64::try_from(request.expected_generation).map_err(|_| WorkflowRepositoryError::GenerationOutOfRange(request.expected_generation))?)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO workflow_transitions \
             (id, workflow_id, from_version, to_version, generation, event_codec_family, \
              event_codec_version, event_payload, committed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(&request.transition_id)
        .bind(&request.workflow_id)
        .bind(i64::try_from(request.expected_version).map_err(|_| WorkflowRepositoryError::VersionOutOfRange(request.expected_version))?)
        .bind(i64::try_from(next_version).map_err(|_| WorkflowRepositoryError::VersionOutOfRange(next_version))?)
        .bind(i64::try_from(next_generation).map_err(|_| WorkflowRepositoryError::GenerationOutOfRange(next_generation))?)
        .bind(wake_profile::event_codec().family)
        .bind(i64::from(wake_profile::event_codec().version))
        .bind(json!({"type":"cancel_requested"}).to_string())
        .bind(request.committed_at.to_rfc3339())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO workflow_reducer_inbox \
             (id, workflow_id, receipt_id, barrier_id, event_codec_family, event_codec_version, \
              event_payload, requires_runtime_acceptance, delivery_status, consumed_by_transition_id) \
             VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?5, 0, 'pending', NULL)",
        )
        .bind(&request.reducer_inbox_id)
        .bind(&request.workflow_id)
        .bind(wake_profile::terminal_codec().family)
        .bind(i64::from(wake_profile::terminal_codec().version))
        .bind(terminal_json(&terminal).to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn load_resource(
        &self,
        workflow_id: &str,
    ) -> WorkflowRepositoryResult<WakeResourceIdentity> {
        let row = sqlx::query(
            "SELECT resource_kind, bash_work_scope_kind, bash_work_scope_stable_key, bash_handle_id, \
                    tmux_work_scope_kind, tmux_work_scope_stable_key, tmux_server_generation, tmux_window_id \
             FROM wake_workflow_bindings WHERE workflow_id = ?1",
        )
        .bind(workflow_id)
        .fetch_optional(self.repository.pool())
        .await?
        .ok_or_else(|| WorkflowRepositoryError::CorruptState("wake workflow has no typed binding".to_owned()))?;
        let parse_scope = |kind: String, stable_key: String| {
            let kind = match kind.as_str() {
                "conversation" => WorkScopeKind::Conversation,
                "worktree" => WorkScopeKind::Worktree,
                _ => {
                    return Err(WorkflowRepositoryError::CorruptState(
                        "wake binding has unknown work scope kind".to_owned(),
                    ));
                }
            };
            Ok(phoenix_workflow::wake_profile::WorkScopeIdentity { kind, stable_key })
        };
        match row.get::<String, _>("resource_kind").as_str() {
            "bash" => Ok(WakeResourceIdentity::Bash(
                phoenix_workflow::wake_profile::BashResourceIdentity {
                    work_scope: parse_scope(
                        row.get("bash_work_scope_kind"),
                        row.get("bash_work_scope_stable_key"),
                    )?,
                    handle_id: row.get("bash_handle_id"),
                },
            )),
            "tmux_window" => Ok(WakeResourceIdentity::TmuxWindow(
                phoenix_workflow::wake_profile::TmuxResourceIdentity {
                    work_scope: parse_scope(
                        row.get("tmux_work_scope_kind"),
                        row.get("tmux_work_scope_stable_key"),
                    )?,
                    server_generation: row.get("tmux_server_generation"),
                    window_id: row.get("tmux_window_id"),
                },
            )),
            _ => Err(WorkflowRepositoryError::CorruptState(
                "wake binding has unknown resource kind".to_owned(),
            )),
        }
    }
}

async fn lookup_wake_registration(
    tx: &mut Transaction<'_, Sqlite>,
    request: &WakeRegistrationRequest,
) -> WorkflowRepositoryResult<Option<ExternalAcceptanceResult>> {
    let row = sqlx::query(
        "SELECT intent_fingerprint, workflow_id, receipt_codec_family, receipt_codec_version, receipt_payload \
         FROM external_acceptance_bindings WHERE profile_id = ?1 AND protocol_version = ?2 \
           AND authority = 'engine_protocol' AND authority_scope = ?3 AND idempotency_key = ?4",
    )
    .bind(wake_profile::PROFILE_ID)
    .bind(i64::from(wake_profile::PROTOCOL_VERSION))
    .bind(&request.authority_scope)
    .bind(&request.idempotency_key)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|row| {
        if row.get::<String, _>("intent_fingerprint") == request.intent_fingerprint {
            ExternalAcceptanceResult::Replay {
                workflow_id: row.get("workflow_id"),
                handle_receipt: DurablePayload {
                    codec: DurableCodecRef {
                        family: row.get("receipt_codec_family"),
                        version: row
                            .get::<i64, _>("receipt_codec_version")
                            .try_into()
                            .expect("stored wake receipt codec version fits u32"),
                    },
                    payload: row.get("receipt_payload"),
                },
            }
        } else {
            ExternalAcceptanceResult::Conflict
        }
    }))
}

async fn insert_registration_core(
    tx: &mut Transaction<'_, Sqlite>,
    request: &WakeRegistrationRequest,
    receipt: &DurablePayload,
    snapshot: &DurablePayload,
    _commit: &DurableWorkflowTransitionCommit,
) -> WorkflowRepositoryResult<()> {
    sqlx::query(
        "INSERT INTO workflows \
         (id, profile_id, protocol_version, authority, execution_mode, authoritative_workflow_id, \
          protocol_selection_id, version, generation, status, snapshot_codec_family, \
          snapshot_codec_version, snapshot_payload, accepted_at) \
         VALUES (?1, ?2, ?3, 'engine_protocol', 'authoritative', NULL, ?4, 1, 0, 'active', ?5, ?6, ?7, ?8)",
    )
    .bind(&request.workflow_id)
    .bind(wake_profile::PROFILE_ID)
    .bind(i64::from(wake_profile::PROTOCOL_VERSION))
    .bind(SELECTION_ID)
    .bind(&snapshot.codec.family)
    .bind(i64::from(snapshot.codec.version))
    .bind(&snapshot.payload)
    .bind(request.accepted_at.to_rfc3339())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO external_acceptance_bindings \
         (id, selection_id, profile_id, protocol_version, authority, authority_scope, \
          idempotency_key, intent_fingerprint, workflow_id, receipt_codec_family, \
          receipt_codec_version, receipt_payload, accepted_at) \
         VALUES (?1, ?2, ?3, ?4, 'engine_protocol', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )
    .bind(&request.binding_id)
    .bind(SELECTION_ID)
    .bind(wake_profile::PROFILE_ID)
    .bind(i64::from(wake_profile::PROTOCOL_VERSION))
    .bind(&request.authority_scope)
    .bind(&request.idempotency_key)
    .bind(&request.intent_fingerprint)
    .bind(&request.workflow_id)
    .bind(&receipt.codec.family)
    .bind(i64::from(receipt.codec.version))
    .bind(&receipt.payload)
    .bind(request.accepted_at.to_rfc3339())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_registration_transition(
    tx: &mut Transaction<'_, Sqlite>,
    commit: &DurableWorkflowTransitionCommit,
) -> WorkflowRepositoryResult<()> {
    let effect = &commit.effects[0];
    let barrier = &commit.barriers[0];
    let member = &commit.barrier_members[0];
    sqlx::query(
        "INSERT INTO workflow_transitions \
         (id, workflow_id, from_version, to_version, generation, event_codec_family, \
          event_codec_version, event_payload, committed_at) VALUES (?1, ?2, 0, 1, 0, ?3, ?4, ?5, ?6)",
    )
    .bind(&commit.transition_id)
    .bind(&commit.workflow_id)
    .bind(&commit.event.codec.family)
    .bind(i64::from(commit.event.codec.version))
    .bind(&commit.event.payload)
    .bind(commit.committed_at.to_rfc3339())
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO workflow_effects \
         (id, workflow_id, declaring_transition_id, declared_workflow_version, generation, family, \
          kind, codec_family, codec_version, role, ambiguity_policy, intent_payload, status, \
          pending_reconciliation, next_eligible_at, destructive_resource) \
         VALUES (?1, ?2, ?3, 1, 0, ?4, ?5, ?6, ?7, 'required', \
                 'observable_reconciliation', ?8, 'eligible', 0, NULL, NULL)",
    )
    .bind(&effect.effect_id)
    .bind(&commit.workflow_id)
    .bind(&commit.transition_id)
    .bind(&effect.family)
    .bind(&effect.kind)
    .bind(&effect.codec.family)
    .bind(i64::from(effect.codec.version))
    .bind(&effect.intent_payload)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO workflow_barriers \
         (id, workflow_id, declaring_transition_id, declaring_workflow_version, status, satisfied_at, \
          event_codec_family, event_codec_version, event_payload) \
         VALUES (?1, ?2, ?3, 1, 'waiting', NULL, ?4, ?5, ?6)",
    )
    .bind(&barrier.barrier_id)
    .bind(&commit.workflow_id)
    .bind(&commit.transition_id)
    .bind(&barrier.barrier_event.codec.family)
    .bind(i64::from(barrier.barrier_event.codec.version))
    .bind(&barrier.barrier_event.payload)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO workflow_barrier_members (barrier_id, effect_id, receipt_family) \
         VALUES (?1, ?2, 'current_generation_effect')",
    )
    .bind(&member.barrier_id)
    .bind(&member.effect_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_wake_binding(
    tx: &mut Transaction<'_, Sqlite>,
    request: &WakeRegistrationRequest,
) -> WorkflowRepositoryResult<()> {
    let fence_version = i64::try_from(request.fence_version)
        .map_err(|_| WorkflowRepositoryError::VersionOutOfRange(request.fence_version))?;
    sqlx::query(
        "INSERT OR IGNORE INTO wake_registration_fences (conversation_id, version, status) \
         VALUES (?1, ?2, 'open')",
    )
    .bind(&request.intent.conversation_id)
    .bind(fence_version)
    .execute(&mut **tx)
    .await?;
    let (
        resource_kind,
        bash_scope_kind,
        bash_scope_key,
        bash_handle,
        tmux_scope_kind,
        tmux_scope_key,
        tmux_generation,
        tmux_window,
    ) = resource_columns(&request.intent.resource);
    sqlx::query(
        "INSERT INTO wake_workflow_bindings \
         (contract_id, workflow_id, conversation_id, registration_scope_kind, registration_scope_stable_key, \
          resource_kind, bash_work_scope_kind, bash_work_scope_stable_key, bash_handle_id, tmux_work_scope_kind, \
          tmux_work_scope_stable_key, tmux_server_generation, tmux_window_id, registering_tool_use_id, registered_at, \
          expires_at, registration_fence_version, observe_effect_id, lifecycle_fence_status) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, 'open')",
    )
    .bind(&request.intent.contract_id)
    .bind(&request.workflow_id)
    .bind(&request.intent.conversation_id)
    .bind(scope_kind(request.intent.registration_scope.kind))
    .bind(&request.intent.registration_scope.stable_key)
    .bind(resource_kind)
    .bind(bash_scope_kind).bind(bash_scope_key).bind(bash_handle)
    .bind(tmux_scope_kind).bind(tmux_scope_key).bind(tmux_generation).bind(tmux_window)
    .bind(&request.intent.registering_tool_use_id)
    .bind(timestamp(request.intent.registered_at)?.to_rfc3339())
    .bind(timestamp(request.intent.expires_at)?.to_rfc3339())
    .bind(fence_version)
    .bind(effect_id(&request.workflow_id))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn validate_registration_invariant(
    repository: &WorkflowRepository,
    request: &WakeRegistrationRequest,
    snapshot: &DurablePayload,
    commit: &DurableWorkflowTransitionCommit,
) -> WorkflowRepositoryResult<()> {
    let effect = &commit.effects[0];
    let barrier = &commit.barriers[0];
    let receipt = registration_receipt_payload(&request.intent, &request.idempotency_key)?;
    let acceptance_valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM external_acceptance_bindings \
         WHERE id = ?1 AND selection_id = ?2 AND profile_id = ?3 AND protocol_version = ?4 \
           AND authority = 'engine_protocol' AND authority_scope = ?5 AND idempotency_key = ?6 \
           AND intent_fingerprint = ?7 AND workflow_id = ?8 AND receipt_codec_family = ?9 \
           AND receipt_codec_version = ?10 AND receipt_payload = ?11 AND accepted_at = ?12)",
    )
    .bind(&request.binding_id)
    .bind(SELECTION_ID)
    .bind(wake_profile::PROFILE_ID)
    .bind(i64::from(wake_profile::PROTOCOL_VERSION))
    .bind(&request.authority_scope)
    .bind(&request.idempotency_key)
    .bind(&request.intent_fingerprint)
    .bind(&request.workflow_id)
    .bind(&receipt.codec.family)
    .bind(i64::from(receipt.codec.version))
    .bind(&receipt.payload)
    .bind(request.accepted_at.to_rfc3339())
    .fetch_one(repository.pool())
    .await?;
    if !acceptance_valid {
        return Err(WorkflowRepositoryError::CorruptState(
            "wake registration replay has a mismatched external acceptance".to_owned(),
        ));
    }
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workflows w \
         JOIN workflow_transitions t ON t.workflow_id = w.id \
         JOIN workflow_effects e ON e.workflow_id = w.id AND e.declaring_transition_id = t.id \
         JOIN workflow_barriers b ON b.workflow_id = w.id AND b.declaring_transition_id = t.id \
         JOIN workflow_barrier_members m ON m.barrier_id = b.id AND m.effect_id = e.id \
         JOIN wake_workflow_bindings wb ON wb.workflow_id = w.id AND wb.observe_effect_id = e.id \
         JOIN wake_registration_fences f ON f.conversation_id = wb.conversation_id \
              AND f.version = wb.registration_fence_version \
         WHERE w.id = ?1 AND w.profile_id = ?2 AND w.protocol_version = ?3 \
           AND w.authority = 'engine_protocol' AND w.execution_mode = 'authoritative' \
           AND w.protocol_selection_id = ?4 AND w.version = 1 AND w.generation = 0 AND w.status = 'active' \
           AND w.snapshot_codec_family = ?5 AND w.snapshot_codec_version = ?6 AND w.snapshot_payload = ?7 \
           AND t.id = ?8 AND t.from_version = 0 AND t.to_version = 1 AND t.generation = 0 \
           AND t.event_codec_family = ?9 AND t.event_codec_version = ?10 AND t.event_payload = ?11 \
           AND e.id = ?12 AND e.declared_workflow_version = 1 AND e.generation = 0 \
           AND e.family = ?13 AND e.kind = ?14 AND e.codec_family = ?15 AND e.codec_version = ?16 \
           AND e.role = 'required' AND e.ambiguity_policy = 'observable_reconciliation' \
           AND e.intent_payload = ?17 AND e.status = 'eligible' AND e.next_eligible_at IS NULL \
           AND b.id = ?18 AND b.declaring_workflow_version = 1 AND b.status = 'waiting' \
           AND b.event_codec_family = ?19 AND b.event_codec_version = ?20 AND b.event_payload = ?21 \
           AND m.receipt_family = 'current_generation_effect' AND wb.contract_id = ?22 \
           AND wb.conversation_id = ?23 AND wb.registration_scope_kind = ?24 \
           AND wb.registration_scope_stable_key = ?25 AND wb.registering_tool_use_id = ?26 \
           AND wb.registered_at = ?27 AND wb.expires_at = ?28 AND wb.registration_fence_version = ?29 \
           AND wb.lifecycle_fence_status = 'open' AND f.status = 'open')",
    )
    .bind(&request.workflow_id).bind(wake_profile::PROFILE_ID)
    .bind(i64::from(wake_profile::PROTOCOL_VERSION)).bind(SELECTION_ID)
    .bind(&snapshot.codec.family).bind(i64::from(snapshot.codec.version)).bind(&snapshot.payload)
    .bind(&commit.transition_id).bind(&commit.event.codec.family).bind(i64::from(commit.event.codec.version)).bind(&commit.event.payload)
    .bind(&effect.effect_id).bind(&effect.family).bind(&effect.kind).bind(&effect.codec.family)
    .bind(i64::from(effect.codec.version)).bind(&effect.intent_payload)
    .bind(&barrier.barrier_id).bind(&barrier.barrier_event.codec.family)
    .bind(i64::from(barrier.barrier_event.codec.version)).bind(&barrier.barrier_event.payload)
    .bind(&request.intent.contract_id).bind(&request.intent.conversation_id)
    .bind(scope_kind(request.intent.registration_scope.kind)).bind(&request.intent.registration_scope.stable_key)
    .bind(&request.intent.registering_tool_use_id)
    .bind(timestamp(request.intent.registered_at)?.to_rfc3339())
    .bind(timestamp(request.intent.expires_at)?.to_rfc3339())
    .bind(i64::try_from(request.fence_version).map_err(|_| WorkflowRepositoryError::VersionOutOfRange(request.fence_version))?)
    .fetch_one(repository.pool()).await?;
    if !valid {
        return Err(WorkflowRepositoryError::CorruptState(
            "wake registration replay violates the complete registration invariant".to_owned(),
        ));
    }
    validate_binding_resource(repository, request).await
}

async fn validate_binding_resource(
    repository: &WorkflowRepository,
    request: &WakeRegistrationRequest,
) -> WorkflowRepositoryResult<()> {
    let stored = WakeWorkflowAdapter::new(repository)
        .load_resource(&request.workflow_id)
        .await?;
    if stored != request.intent.resource {
        return Err(WorkflowRepositoryError::CorruptState(
            "wake registration replay has a mismatched typed resource binding".to_owned(),
        ));
    }
    Ok(())
}

async fn fail_registration(
    tx: &mut Transaction<'_, Sqlite>,
    configured: Option<WakeRegistrationFailpoint>,
    here: WakeRegistrationFailpoint,
) -> WorkflowRepositoryResult<()> {
    if configured == Some(here) {
        sqlx::query("ROLLBACK").execute(&mut **tx).await?;
        return Err(WorkflowRepositoryError::CorruptState(format!(
            "wake registration rollback test failpoint triggered at {here:?}"
        )));
    }
    Ok(())
}

fn registration_transition(
    request: &WakeRegistrationRequest,
) -> WorkflowRepositoryResult<DurableWorkflowTransitionCommit> {
    let observe = ObserveHandleIntent {
        contract_id: request.intent.contract_id.clone(),
        resource: request.intent.resource.clone(),
        expires_at: request.intent.expires_at,
    };
    let observe_id = effect_id(&request.workflow_id);
    Ok(DurableWorkflowTransitionCommit {
        transition_id: request.transition_id.clone(),
        workflow_id: request.workflow_id.clone(),
        expected_from_version: 0,
        next_version: 1,
        next_generation: 0,
        committed_at: request.accepted_at,
        workflow_status: WorkflowStatus::Active,
        snapshot: payload(
            wake_profile::snapshot_codec(),
            registration_snapshot_json(&request.intent, request.fence_version),
        )?,
        event: payload(wake_profile::event_codec(), json!({"type":"registered"}))?,
        effects: vec![DurableEffectRecord {
            effect_id: observe_id.clone(),
            family: wake_profile::PROFILE_ID.to_owned(),
            kind: wake_profile::OBSERVE_HANDLE_KIND.to_owned(),
            codec: codec(wake_profile::intent_codec()),
            role: EffectRole::Required,
            ambiguity_policy: EffectAmbiguity::ObservableReconciliation,
            intent_payload: observe_intent_json(&observe).to_string(),
            next_eligible_at: None,
            destructive_resource: None,
            generation: 0,
            status: EffectStatus::Eligible,
        }],
        dependencies: vec![],
        barriers: vec![DurableBarrierRecord {
            barrier_id: barrier_id(&request.workflow_id),
            status: BarrierStatus::Waiting,
            satisfied_at: None,
            barrier_event: payload(
                wake_profile::barrier_codec(),
                json!({"type":"registration_observed","receipt":registration_receipt_json(&request.intent)}),
            )?,
        }],
        barrier_members: vec![DurableBarrierMemberRecord {
            barrier_id: barrier_id(&request.workflow_id),
            effect_id: observe_id,
            receipt_family: ReceiptFamily::CurrentGenerationEffect,
        }],
        invalidations: vec![],
        owed_acceptances: vec![],
    })
}

fn codec(value: phoenix_workflow::CodecRef) -> DurableCodecRef {
    DurableCodecRef {
        family: value.family.to_owned(),
        version: value.version,
    }
}

fn payload(
    codec_ref: phoenix_workflow::CodecRef,
    value: Value,
) -> WorkflowRepositoryResult<DurablePayload> {
    Ok(DurablePayload {
        codec: codec(codec_ref),
        payload: serde_json::to_string(&value)
            .map_err(|error| WorkflowRepositoryError::CorruptState(error.to_string()))?,
    })
}

fn registration_receipt_payload(
    intent: &WakeRegistrationIntent,
    key: &str,
) -> WorkflowRepositoryResult<DurablePayload> {
    payload(
        wake_profile::barrier_codec(),
        json!({"idempotency_key":key,"receipt":registration_receipt_json(intent)}),
    )
}

fn registration_receipt_json(intent: &WakeRegistrationIntent) -> Value {
    json!({"contract_id":intent.contract_id,"resource":resource_json(&intent.resource),"expires_at":intent.expires_at.0,"registering_tool_use_id":intent.registering_tool_use_id})
}

fn registration_snapshot_json(intent: &WakeRegistrationIntent, fence_version: u64) -> Value {
    json!({"contract_id":intent.contract_id,"conversation_id":intent.conversation_id,"registration_scope":scope_json(intent.registration_scope.kind, &intent.registration_scope.stable_key),"resource":resource_json(&intent.resource),"registering_tool_use_id":intent.registering_tool_use_id,"registered_at":intent.registered_at.0,"expires_at":intent.expires_at.0,"registration_fence_version":fence_version,"runtime_availability":"busy","continuation":null,"terminal":null,"cancelled":false})
}

fn observe_intent_json(intent: &ObserveHandleIntent) -> Value {
    json!({"contract_id":intent.contract_id,"resource":resource_json(&intent.resource),"expires_at":intent.expires_at.0})
}

fn evidence_json(evidence: &WakeTerminalEvidence) -> Value {
    match evidence {
        WakeTerminalEvidence::Bash(value) => {
            json!({"type":"bash","identity":resource_json(&WakeResourceIdentity::Bash(value.identity.clone())),"status":bash_status(value.status),"occurred_at":value.occurred_at.0,"exit_code":value.exit_code,"duration_ms":value.duration_ms,"signal_number":value.signal_number,"kill_signal_sent":value.kill_signal_sent,"final_tail":value.final_tail})
        }
        WakeTerminalEvidence::TmuxWindow(value) => {
            json!({"type":"tmux_window","identity":resource_json(&WakeResourceIdentity::TmuxWindow(value.identity.clone())),"status":tmux_status(value.status),"occurred_at":value.occurred_at.0,"exit_code":value.exit_code,"duration_ms":value.duration_ms,"final_tail":value.final_tail})
        }
    }
}

fn terminal_projection(
    terminal: &WakeTerminalPayload,
) -> WorkflowRepositoryResult<DurableWakeTerminalProjection> {
    let resolved_at = timestamp(match terminal {
        WakeTerminalPayload::Fired { resolved_at, .. }
        | WakeTerminalPayload::Expired { resolved_at, .. }
        | WakeTerminalPayload::Cancelled { resolved_at, .. }
        | WakeTerminalPayload::Forgotten { resolved_at, .. } => *resolved_at,
    })?;
    let mut projection = DurableWakeTerminalProjection {
        contract_id: terminal.contract_id().to_owned(),
        resource_kind: match terminal.resource() {
            WakeResourceIdentity::Bash(_) => "bash".to_owned(),
            WakeResourceIdentity::TmuxWindow(_) => "tmux_window".to_owned(),
        },
        status: match terminal {
            WakeTerminalPayload::Fired { .. } => "fired",
            WakeTerminalPayload::Expired { .. } => "expired",
            WakeTerminalPayload::Cancelled { .. } => "cancelled",
            WakeTerminalPayload::Forgotten { .. } => "forgotten",
        }
        .to_owned(),
        resolved_at,
        bash_status: None,
        bash_occurred_at: None,
        bash_exit_code: None,
        bash_duration_ms: None,
        bash_signal_number: None,
        bash_kill_signal_sent: None,
        bash_tail: vec![],
        tmux_status: None,
        tmux_occurred_at: None,
        tmux_server_generation: None,
        tmux_exit_code: None,
        tmux_duration_ms: None,
        tmux_tail: vec![],
        forgotten_reason: None,
        cancellation_reason: None,
    };
    match terminal {
        WakeTerminalPayload::Fired { evidence, .. } => match evidence {
            WakeTerminalEvidence::Bash(value) => {
                projection.bash_status = Some(bash_status(value.status).to_owned());
                projection.bash_occurred_at = Some(timestamp(value.occurred_at)?);
                projection.bash_exit_code = value.exit_code;
                projection.bash_duration_ms = value.duration_ms;
                projection.bash_signal_number = value.signal_number;
                projection
                    .bash_kill_signal_sent
                    .clone_from(&value.kill_signal_sent);
                projection.bash_tail.clone_from(&value.final_tail);
            }
            WakeTerminalEvidence::TmuxWindow(value) => {
                projection.tmux_status = Some(tmux_status(value.status).to_owned());
                projection.tmux_occurred_at = Some(timestamp(value.occurred_at)?);
                projection.tmux_server_generation = Some(value.identity.server_generation.clone());
                projection.tmux_exit_code = value.exit_code;
                projection.tmux_duration_ms = value.duration_ms;
                projection.tmux_tail.clone_from(&value.final_tail);
            }
        },
        WakeTerminalPayload::Cancelled { .. } => {
            projection.cancellation_reason = Some("explicit_cancel".to_owned());
        }
        WakeTerminalPayload::Forgotten { reason, .. } => {
            projection.forgotten_reason = Some(
                match reason {
                    WakeForgottenReason::HandleMissing => "handle_missing",
                    WakeForgottenReason::RuntimeUnrecoverableAfterRestart => {
                        "runtime_unrecoverable_after_restart"
                    }
                }
                .to_owned(),
            );
        }
        WakeTerminalPayload::Expired { .. } => {}
    }
    Ok(projection)
}

fn terminal_json(terminal: &WakeTerminalPayload) -> Value {
    match terminal {
        WakeTerminalPayload::Fired {
            contract_id,
            resource,
            evidence,
            resolved_at,
        } => {
            json!({"type":"fired","contract_id":contract_id,"resource":resource_json(resource),"evidence":evidence_json(evidence),"resolved_at":resolved_at.0})
        }
        WakeTerminalPayload::Expired {
            contract_id,
            resource,
            resolved_at,
        } => {
            json!({"type":"expired","contract_id":contract_id,"resource":resource_json(resource),"resolved_at":resolved_at.0})
        }
        WakeTerminalPayload::Cancelled {
            contract_id,
            resource,
            reason,
            resolved_at,
        } => {
            json!({"type":"cancelled","contract_id":contract_id,"resource":resource_json(resource),"reason":match reason { WakeCancellationReason::ExplicitCancel => "explicit_cancel" },"resolved_at":resolved_at.0})
        }
        WakeTerminalPayload::Forgotten {
            contract_id,
            resource,
            reason,
            resolved_at,
        } => {
            json!({"type":"forgotten","contract_id":contract_id,"resource":resource_json(resource),"reason":match reason { WakeForgottenReason::HandleMissing => "handle_missing", WakeForgottenReason::RuntimeUnrecoverableAfterRestart => "runtime_unrecoverable_after_restart" },"resolved_at":resolved_at.0})
        }
    }
}

fn resource_json(resource: &WakeResourceIdentity) -> Value {
    match resource {
        WakeResourceIdentity::Bash(value) => {
            json!({"type":"bash","work_scope":scope_json(value.work_scope.kind, &value.work_scope.stable_key),"handle_id":value.handle_id})
        }
        WakeResourceIdentity::TmuxWindow(value) => {
            json!({"type":"tmux_window","work_scope":scope_json(value.work_scope.kind, &value.work_scope.stable_key),"server_generation":value.server_generation,"window_id":value.window_id})
        }
    }
}

fn scope_json(kind: WorkScopeKind, stable_key: &str) -> Value {
    json!({"kind":scope_kind(kind),"stable_key":stable_key})
}
fn scope_kind(kind: WorkScopeKind) -> &'static str {
    match kind {
        WorkScopeKind::Conversation => "conversation",
        WorkScopeKind::Worktree => "worktree",
    }
}
fn bash_status(status: BashTerminalStatus) -> &'static str {
    match status {
        BashTerminalStatus::Exited => "exited",
        BashTerminalStatus::Killed => "killed",
        BashTerminalStatus::KillPendingKernel => "kill_pending_kernel",
    }
}
fn tmux_status(status: TmuxTerminalStatus) -> &'static str {
    match status {
        TmuxTerminalStatus::ExitMarkerObserved => "exit_marker_observed",
        TmuxTerminalStatus::WindowKilled => "window_killed",
    }
}
fn effect_id(workflow_id: &str) -> String {
    format!("wake-observe:{workflow_id}")
}
fn barrier_id(workflow_id: &str) -> String {
    format!("wake-registration:{workflow_id}")
}
fn due_effect_id(due: &DueEffect) -> &str {
    match due {
        DueEffect::Eligible { effect_id, .. } | DueEffect::RetryWait { effect_id, .. } => effect_id,
        DueEffect::ExpiredClaim { authority } => &authority.effect_id,
    }
}

fn parse_datetime(value: &str) -> WorkflowRepositoryResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| WorkflowRepositoryError::CorruptState("invalid wake timestamp".to_owned()))
}

fn timestamp(value: Timestamp) -> WorkflowRepositoryResult<DateTime<Utc>> {
    let seconds = i64::try_from(value.0).map_err(|_| {
        WorkflowRepositoryError::CorruptState("wake timestamp exceeds chrono range".to_owned())
    })?;
    Utc.timestamp_opt(seconds, 0).single().ok_or_else(|| {
        WorkflowRepositoryError::CorruptState("wake timestamp is outside chrono range".to_owned())
    })
}

#[allow(clippy::type_complexity)]
fn resource_columns(
    resource: &WakeResourceIdentity,
) -> (
    &'static str,
    Option<&str>,
    Option<&str>,
    Option<&str>,
    Option<&str>,
    Option<&str>,
    Option<&str>,
    Option<&str>,
) {
    match resource {
        WakeResourceIdentity::Bash(value) => (
            "bash",
            Some(scope_kind(value.work_scope.kind)),
            Some(value.work_scope.stable_key.as_str()),
            Some(value.handle_id.as_str()),
            None,
            None,
            None,
            None,
        ),
        WakeResourceIdentity::TmuxWindow(value) => (
            "tmux_window",
            None,
            None,
            None,
            Some(scope_kind(value.work_scope.kind)),
            Some(value.work_scope.stable_key.as_str()),
            Some(value.server_generation.as_str()),
            Some(value.window_id.as_str()),
        ),
    }
}

fn is_unique_constraint(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|db| db.code())
        .is_some_and(|code| code == "1555" || code == "2067")
}

#[cfg(test)]
mod tests;
