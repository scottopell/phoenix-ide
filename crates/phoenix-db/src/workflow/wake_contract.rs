use super::{
    CommitTransitionPlanCas, LocalBarrierDecl, LocalBarrierMemberDecl, LocalCodec,
    LocalDeliveryDecl, LocalDeliveryPayloadKind, LocalEffectDecl, WorkflowRepository,
};
use crate::{DbError, DbResult};
use phoenix_workflow::wake_contract::{
    transition, WakeCommand, WakeDisposition, WakeEffectRole, WakeRejection, WakeState,
};
use phoenix_workflow::{
    BarrierId, BarrierStatus, CommitOutcome, DeliveryId, EffectId, EffectRole, EffectStatus,
    ExecutionCapability, Generation, ReceiptFamily, ReceiptOrigin, RuntimeAcceptanceStatus,
    TransitionId, Version, WorkflowId, WorkflowStatus,
};

const WAKE_CONTRACT_CODEC_FAMILY: &str = "wake.contract";
const WAKE_CONTRACT_CODEC_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitWakeCommandInput {
    pub workflow_id: WorkflowId,
    pub command: WakeCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitWakeCommandOutcome {
    Applied {
        state: WakeState,
        transition_id: TransitionId,
    },
    Replayed {
        state: WakeState,
        transition_id: TransitionId,
    },
    Rejected(WakeRejection),
    VersionConflict,
}

#[derive(Clone)]
pub struct WakeContractRepository {
    workflow_repo: WorkflowRepository,
    #[cfg(test)]
    fail_after_transition_once: std::sync::Arc<std::sync::Mutex<Option<WorkflowId>>>,
}

impl WakeContractRepository {
    #[must_use]
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self {
            workflow_repo: WorkflowRepository::new(pool),
            #[cfg(test)]
            fail_after_transition_once: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub async fn commit_wake_command(
        &self,
        input: &CommitWakeCommandInput,
    ) -> DbResult<CommitWakeCommandOutcome> {
        let mut tx = self.workflow_repo.begin_immediate_tx().await?;
        let current = self.load_state_tx(&mut tx, input.workflow_id).await?;
        let result = transition(&current, input.command.clone());

        match result.disposition {
            WakeDisposition::Rejected(rejection) => {
                tx.rollback().await?;
                Ok(CommitWakeCommandOutcome::Rejected(rejection))
            }
            WakeDisposition::Replayed { transition_id, .. } => {
                validate_replay_artifacts_tx(&mut tx, input.workflow_id, &current).await?;
                tx.rollback().await?;
                Ok(CommitWakeCommandOutcome::Replayed {
                    state: current,
                    transition_id,
                })
            }
            WakeDisposition::Applied { ref event } => {
                let expected_version = match &current {
                    WakeState::Absent => Version(0),
                    WakeState::Present(contract) => contract.version,
                };
                let generation = match &result.new_state {
                    WakeState::Absent => Generation(0),
                    WakeState::Present(contract) => contract.generation,
                };
                let next_status = match &result.new_state {
                    WakeState::Present(contract)
                        if matches!(
                            contract.lifecycle,
                            phoenix_workflow::wake_contract::WakeLifecycle::Closed(_)
                        ) =>
                    {
                        WorkflowStatus::Completed
                    }
                    WakeState::Absent | WakeState::Present(_) => WorkflowStatus::Active,
                };
                let codec = codec();
                let event_payload = encode(event)?;
                let snapshot_payload = encode(&result.new_state)?;
                let effects = result
                    .owed_effects
                    .iter()
                    .map(|effect| local_effect(effect, contract_version(&result.new_state)))
                    .collect::<DbResult<Vec<_>>>()?;
                let terminal_bundle = terminal_bundle(event, &result.new_state)?;
                let committed_at = committed_at(&result.new_state);

                if current == WakeState::Absent {
                    self.insert_initial_head(
                        &mut tx,
                        input.workflow_id,
                        generation,
                        &codec,
                        committed_at,
                    )
                    .await?;
                }
                if let WakeState::Present(contract) = &result.new_state {
                    if current == WakeState::Absent {
                        insert_contract_identity_tx(&mut tx, input.workflow_id, &contract.id)
                            .await?;
                    }
                }
                if is_fence_finalization(event) {
                    validate_fence_receipt_tx(&mut tx, input.workflow_id, &current).await?;
                }
                if should_revoke_observation_authority(event) {
                    revoke_observation_authority_tx(&mut tx, input.workflow_id).await?;
                }
                if terminal_bundle.is_some() {
                    invalidate_prior_required_effects_tx(&mut tx, input.workflow_id).await?;
                }
                let committed = tx
                    .commit_transition_plan(&CommitTransitionPlanCas {
                        workflow_id: input.workflow_id,
                        expected_version,
                        transition_id: event.transition_id,
                        generation,
                        next_status,
                        event_codec: codec.clone(),
                        event_payload,
                        next_snapshot_codec: codec,
                        next_snapshot_payload: snapshot_payload,
                        committed_at,
                        effects,
                        dependencies: vec![],
                        barriers: terminal_bundle
                            .as_ref()
                            .map_or_else(Vec::new, |bundle| vec![bundle.barrier.clone()]),
                        barrier_members: terminal_bundle
                            .as_ref()
                            .map_or_else(Vec::new, |bundle| vec![bundle.barrier_member.clone()]),
                        deliveries: terminal_bundle
                            .as_ref()
                            .map_or_else(Vec::new, |bundle| vec![bundle.delivery.clone()]),
                        schedules: vec![],
                    })
                    .await?;
                if committed != CommitOutcome::Committed {
                    tx.rollback().await?;
                    return Ok(CommitWakeCommandOutcome::VersionConflict);
                }
                if let Some(bundle) = &terminal_bundle {
                    insert_terminal_receipt_tx(
                        &mut tx,
                        input.workflow_id,
                        generation,
                        contract_version(&result.new_state),
                        bundle,
                    )
                    .await?;
                }

                #[cfg(test)]
                if self
                    .fail_after_transition_once
                    .lock()
                    .expect("wake contract failpoint lock")
                    .take()
                    == Some(input.workflow_id)
                {
                    return Err(DbError::Serialization(
                        "injected failure after wake transition".into(),
                    ));
                }

                tx.commit().await?;
                Ok(CommitWakeCommandOutcome::Applied {
                    state: result.new_state,
                    transition_id: event.transition_id,
                })
            }
        }
    }

    pub async fn load_state(&self, workflow_id: WorkflowId) -> DbResult<WakeState> {
        let mut tx = self.workflow_repo.begin_tx().await?;
        let state = self.load_state_tx(&mut tx, workflow_id).await?;
        tx.rollback().await?;
        Ok(state)
    }

    async fn load_state_tx(
        &self,
        tx: &mut super::WorkflowTx<'_>,
        workflow_id: WorkflowId,
    ) -> DbResult<WakeState> {
        let Some(head) = tx.fetch_workflow_head(workflow_id).await? else {
            return Ok(WakeState::Absent);
        };
        if head.binding.profile.profile_kind != "wake.contract" {
            return Err(DbError::Serialization(format!(
                "workflow {} uses profile {}, not wake.contract",
                workflow_id.0, head.binding.profile.profile_kind
            )));
        }
        decode(&head.snapshot_payload)
    }

    async fn insert_initial_head(
        &self,
        tx: &mut super::WorkflowTx<'_>,
        workflow_id: WorkflowId,
        generation: Generation,
        codec: &LocalCodec,
        created_at: phoenix_workflow::Timestamp,
    ) -> DbResult<()> {
        let inserted = sqlx::query(
            "INSERT INTO workflows
             (workflow_id, profile_kind, profile_version, runtime_acceptance_enabled,
              external_acceptance_enabled, version, generation, status, snapshot_codec_family,
              snapshot_codec_version, snapshot_payload, created_at, updated_at)
             VALUES (?1, 'wake.contract', 1, 1, 0, 0, ?2, 'Active', ?3, ?4, ?5, ?6, ?6)",
        )
        .bind(super::to_i64(workflow_id.0, "workflow_id")?)
        .bind(super::to_i64(generation.0, "generation")?)
        .bind(&codec.family)
        .bind(i64::from(codec.version))
        .bind(encode(&WakeState::Absent)?)
        .bind(super::to_i64(created_at.0, "created_at")?)
        .execute(&mut *tx.tx)
        .await?;
        if inserted.rows_affected() != 1 {
            return Err(DbError::Serialization(
                "wake contract head insert failed".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO workflow_supported_codecs
             (workflow_id, codec_family, codec_version) VALUES (?1, ?2, ?3)",
        )
        .bind(super::to_i64(workflow_id.0, "workflow_id")?)
        .bind(&codec.family)
        .bind(i64::from(codec.version))
        .execute(&mut *tx.tx)
        .await?;
        Ok(())
    }

    #[cfg(test)]
    fn fail_after_transition_once(&self, workflow_id: WorkflowId) {
        *self
            .fail_after_transition_once
            .lock()
            .expect("wake contract failpoint lock") = Some(workflow_id);
    }
}

async fn insert_contract_identity_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
    contract_id: &phoenix_workflow::wake_contract::WakeContractId,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO wake_contract_identity_bindings (contract_id, workflow_id)
         VALUES (?1, ?2)",
    )
    .bind(contract_id.as_str())
    .bind(super::to_i64(workflow_id.0, "workflow_id")?)
    .execute(&mut *tx.tx)
    .await?;
    Ok(())
}

async fn validate_replay_artifacts_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
    state: &WakeState,
) -> DbResult<()> {
    let WakeState::Present(contract) = state else {
        return Err(DbError::Serialization(
            "cannot replay an absent wake contract".into(),
        ));
    };
    let event_payload: Vec<u8> = sqlx::query_scalar(
        "SELECT event_payload FROM workflow_transitions
         WHERE workflow_id = ?1 AND transition_id = ?2",
    )
    .bind(super::to_i64(workflow_id.0, "workflow_id")?)
    .bind(super::to_i64(
        contract.head_transition_id.0,
        "transition_id",
    )?)
    .fetch_one(&mut *tx.tx)
    .await?;
    let event: phoenix_workflow::wake_contract::WakeContractEvent = decode(&event_payload)?;
    let expected_effect_id = Some(effect_id(
        contract.head_transition_id,
        match event.kind {
            phoenix_workflow::wake_contract::WakeEventKind::Registered { .. } => {
                WakeEffectRole::BeginObservation
            }
            phoenix_workflow::wake_contract::WakeEventKind::DeliveryOwnerTransferred { .. } => {
                WakeEffectRole::TransferDeliveryOwner
            }
            phoenix_workflow::wake_contract::WakeEventKind::TerminalProposed { .. } => {
                WakeEffectRole::FenceObservationAuthority
            }
            phoenix_workflow::wake_contract::WakeEventKind::Terminalized { .. } => {
                WakeEffectRole::CommitTerminalization
            }
        },
    ));
    if let Some(effect_id) = expected_effect_id {
        let effect_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_effects
             WHERE workflow_id = ?1 AND effect_id = ?2",
        )
        .bind(super::to_i64(workflow_id.0, "workflow_id")?)
        .bind(super::to_i64(effect_id, "effect_id")?)
        .fetch_one(&mut *tx.tx)
        .await?;
        if effect_count != 1 {
            return Err(DbError::Serialization(format!(
                "wake replay missing effect {effect_id}"
            )));
        }
    }
    if matches!(
        contract.lifecycle,
        phoenix_workflow::wake_contract::WakeLifecycle::Closed(_)
    ) {
        let terminal_rows: i64 = sqlx::query_scalar(
            "SELECT
                 (SELECT COUNT(*) FROM workflow_receipts WHERE workflow_id = ?1 AND receipt_id = ?2) +
                 (SELECT COUNT(*) FROM workflow_barriers WHERE workflow_id = ?1 AND barrier_id = ?2) +
                 (SELECT COUNT(*) FROM workflow_barrier_members WHERE workflow_id = ?1 AND barrier_id = ?2) +
                 (SELECT COUNT(*) FROM workflow_deliveries WHERE workflow_id = ?1 AND delivery_id = ?2)",
        )
        .bind(super::to_i64(workflow_id.0, "workflow_id")?)
        .bind(super::to_i64(contract.head_transition_id.0, "terminal_id")?)
        .fetch_one(&mut *tx.tx)
        .await?;
        if terminal_rows != 4 {
            return Err(DbError::Serialization(
                "wake replay terminal bundle is incomplete".into(),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct TerminalBundle {
    effect_id: EffectId,
    receipt_payload: Vec<u8>,
    origin: ReceiptOrigin,
    barrier: LocalBarrierDecl,
    barrier_member: LocalBarrierMemberDecl,
    delivery: LocalDeliveryDecl,
}

fn terminal_bundle(
    event: &phoenix_workflow::wake_contract::WakeContractEvent,
    _state: &WakeState,
) -> DbResult<Option<TerminalBundle>> {
    let phoenix_workflow::wake_contract::WakeEventKind::Terminalized {
        terminal,
        delivery_owner,
        resume_policy,
    } = &event.kind
    else {
        return Ok(None);
    };
    let terminal_effect = EffectId(effect_id(
        event.transition_id,
        WakeEffectRole::CommitTerminalization,
    ));
    let receipt_payload = encode(event)?;
    let origin = match terminal {
        phoenix_workflow::wake_contract::CanonicalTerminal::Expired { .. } => {
            ReceiptOrigin::DeadlineExpiration
        }
        phoenix_workflow::wake_contract::CanonicalTerminal::Cancelled { .. } => {
            ReceiptOrigin::CancellationArbitration
        }
        phoenix_workflow::wake_contract::CanonicalTerminal::Forgotten { .. } => {
            ReceiptOrigin::ForgottenInterruption
        }
        phoenix_workflow::wake_contract::CanonicalTerminal::Fired { .. } => {
            ReceiptOrigin::Reconciliation
        }
    };
    let requires_runtime_acceptance = matches!(
        resume_policy,
        phoenix_workflow::wake_contract::WakeResumePolicy::RequestWhenIdle
    );
    let barrier_id = BarrierId(event.transition_id.0);
    let delivery_id = DeliveryId(event.transition_id.0);
    let payload = receipt_payload.clone();
    Ok(Some(TerminalBundle {
        effect_id: terminal_effect,
        receipt_payload: receipt_payload.clone(),
        origin,
        barrier: LocalBarrierDecl {
            barrier_id,
            status: BarrierStatus::Satisfied,
            reducer_event_codec: codec(),
            reducer_event_payload: receipt_payload,
        },
        barrier_member: LocalBarrierMemberDecl {
            barrier_id,
            effect_id: terminal_effect,
            receipt_family: ReceiptFamily::CurrentGenerationEffect,
        },
        delivery: LocalDeliveryDecl {
            delivery_id,
            effect_id: None,
            barrier_id: Some(barrier_id),
            consumer_kind: delivery_owner.0.clone(),
            event_codec: codec(),
            payload_kind: LocalDeliveryPayloadKind::Barrier,
            payload_blob: payload,
            requires_runtime_acceptance,
            runtime_acceptance_status: requires_runtime_acceptance
                .then_some(RuntimeAcceptanceStatus::Owed),
        },
    }))
}

fn is_fence_finalization(event: &phoenix_workflow::wake_contract::WakeContractEvent) -> bool {
    matches!(
        event.kind,
        phoenix_workflow::wake_contract::WakeEventKind::Terminalized { .. }
    )
}

async fn validate_fence_receipt_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
    current: &WakeState,
) -> DbResult<()> {
    let WakeState::Present(contract) = current else {
        return Ok(());
    };
    let phoenix_workflow::wake_contract::WakeLifecycle::Open(
        phoenix_workflow::wake_contract::OpenWakeLifecycle::TerminalProposed(proposal),
    ) = &contract.lifecycle
    else {
        return Ok(());
    };
    let fence_effect_id = effect_id(
        proposal.transition_id,
        WakeEffectRole::FenceObservationAuthority,
    );
    let receipted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_effects e
         JOIN workflow_receipts r
           ON r.workflow_id = e.workflow_id AND r.effect_id = e.effect_id
         WHERE e.workflow_id = ?1 AND e.effect_id = ?2 AND e.status = 'Receipted'",
    )
    .bind(super::to_i64(workflow_id.0, "workflow_id")?)
    .bind(super::to_i64(fence_effect_id, "effect_id")?)
    .fetch_one(&mut *tx.tx)
    .await?;
    if receipted != 1 {
        return Err(DbError::Serialization(
            "terminal proposal requires a durable fence-effect receipt".into(),
        ));
    }
    Ok(())
}

async fn invalidate_prior_required_effects_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE workflow_effects SET status = 'Invalidated'
         WHERE workflow_id = ?1 AND role = 'Required'
           AND status IN ('Blocked', 'Eligible', 'Executing', 'RetryWait', 'AmbiguityWait')",
    )
    .bind(super::to_i64(workflow_id.0, "workflow_id")?)
    .execute(&mut *tx.tx)
    .await?;
    Ok(())
}

fn should_revoke_observation_authority(
    event: &phoenix_workflow::wake_contract::WakeContractEvent,
) -> bool {
    matches!(
        event.kind,
        phoenix_workflow::wake_contract::WakeEventKind::TerminalProposed { .. }
            | phoenix_workflow::wake_contract::WakeEventKind::Terminalized { .. }
    )
}

async fn revoke_observation_authority_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE workflow_attempts
         SET status = 'AuthorityLost'
         WHERE workflow_id = ?1 AND status IN ('Begun', 'ObservationRecorded')",
    )
    .bind(super::to_i64(workflow_id.0, "workflow_id")?)
    .execute(&mut *tx.tx)
    .await?;
    sqlx::query("DELETE FROM workflow_reclaimable_leases WHERE workflow_id = ?1")
        .bind(super::to_i64(workflow_id.0, "workflow_id")?)
        .execute(&mut *tx.tx)
        .await?;
    Ok(())
}

async fn insert_terminal_receipt_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
    generation: Generation,
    declared_workflow_version: Version,
    bundle: &TerminalBundle,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO workflow_receipts
         (workflow_id, receipt_id, effect_id, generation, declared_workflow_version,
          process_incarnation, attempt_id, origin, receipt_codec_family,
          receipt_codec_version, receipt_payload)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, NULL, ?6, ?7, ?8, ?9)",
    )
    .bind(super::to_i64(workflow_id.0, "workflow_id")?)
    .bind(super::to_i64(bundle.delivery.delivery_id.0, "receipt_id")?)
    .bind(super::to_i64(bundle.effect_id.0, "effect_id")?)
    .bind(super::to_i64(generation.0, "generation")?)
    .bind(super::to_i64(
        declared_workflow_version.0,
        "declared_workflow_version",
    )?)
    .bind(receipt_origin_str(bundle.origin))
    .bind(WAKE_CONTRACT_CODEC_FAMILY)
    .bind(i64::from(WAKE_CONTRACT_CODEC_VERSION))
    .bind(&bundle.receipt_payload)
    .execute(&mut *tx.tx)
    .await?;
    sqlx::query(
        "UPDATE workflow_effects SET status = 'Receipted'
         WHERE workflow_id = ?1 AND effect_id = ?2",
    )
    .bind(super::to_i64(workflow_id.0, "workflow_id")?)
    .bind(super::to_i64(bundle.effect_id.0, "effect_id")?)
    .execute(&mut *tx.tx)
    .await?;
    Ok(())
}

fn receipt_origin_str(origin: ReceiptOrigin) -> &'static str {
    match origin {
        ReceiptOrigin::Execution => "Execution",
        ReceiptOrigin::Adoption => "Adoption",
        ReceiptOrigin::Reconciliation => "Reconciliation",
        ReceiptOrigin::Manual => "Manual",
        ReceiptOrigin::CancellationArbitration => "CancellationArbitration",
        ReceiptOrigin::DeadlineExpiration => "DeadlineExpiration",
        ReceiptOrigin::ForgottenInterruption => "ForgottenInterruption",
        ReceiptOrigin::ScheduleCollapse => "ScheduleCollapse",
    }
}

fn local_effect(
    effect: &phoenix_workflow::wake_contract::WakeOwedEffect,
    declared_workflow_version: Version,
) -> DbResult<LocalEffectDecl> {
    let role = match effect.key.role {
        WakeEffectRole::BeginObservation => "begin_observation",
        WakeEffectRole::FenceObservationAuthority => "fence_observation_authority",
        WakeEffectRole::CommitTerminalization => "commit_terminalization",
        WakeEffectRole::TransferDeliveryOwner => "transfer_delivery_owner",
    };
    let effect_id = EffectId(effect_id(effect.key.transition_id, effect.key.role));
    Ok(LocalEffectDecl {
        effect_id,
        declared_workflow_version,
        family: "wake.contract".into(),
        kind: role.into(),
        intent_codec: codec(),
        intent_payload: encode(effect)?,
        generation: effect.key.generation,
        role: EffectRole::Required,
        capability: ExecutionCapability::SafelyRepeatable,
        next_eligible_at: None,
        destructive_resource: None,
        status: EffectStatus::Eligible,
    })
}

fn codec() -> LocalCodec {
    LocalCodec {
        family: WAKE_CONTRACT_CODEC_FAMILY.into(),
        version: WAKE_CONTRACT_CODEC_VERSION,
    }
}

fn encode<T: serde::Serialize>(value: &T) -> DbResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| DbError::Serialization(error.to_string()))
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> DbResult<T> {
    serde_json::from_slice(bytes).map_err(|error| DbError::Serialization(error.to_string()))
}

fn contract_version(state: &WakeState) -> Version {
    match state {
        WakeState::Absent => Version(0),
        WakeState::Present(contract) => contract.version,
    }
}

fn effect_id(transition_id: TransitionId, role: WakeEffectRole) -> u64 {
    let role = match role {
        WakeEffectRole::BeginObservation => 1,
        WakeEffectRole::FenceObservationAuthority => 2,
        WakeEffectRole::CommitTerminalization => 3,
        WakeEffectRole::TransferDeliveryOwner => 4,
    };
    transition_id.0.saturating_mul(8).saturating_add(role)
}

fn committed_at(state: &WakeState) -> phoenix_workflow::Timestamp {
    match state {
        WakeState::Absent => phoenix_workflow::Timestamp(0),
        WakeState::Present(contract) => match &contract.lifecycle {
            phoenix_workflow::wake_contract::WakeLifecycle::Closed(terminal) => {
                terminal.occurred_at()
            }
            phoenix_workflow::wake_contract::WakeLifecycle::Open(_) => contract.registered_at,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::run_pending_migrations;
    use phoenix_workflow::wake_contract::{
        CancellationCause, EncodedWakeValue, WakeCodecFamily, WakeCodecRef, WakeCodecVersion,
        WakeCommandKind, WakeCondition, WakeContractId, WakeOwner, WakePayload, WakeProfileKind,
        WakeProfileRef, WakeProfileVersion, WakeSubject,
    };
    use phoenix_workflow::Timestamp;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    async fn open_repo_pair() -> (
        tempfile::TempDir,
        WakeContractRepository,
        WakeContractRepository,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wake-contract.db");
        let url = format!("sqlite://{}", path.display());
        let open = || async {
            let options = SqliteConnectOptions::from_str(&url)
                .unwrap()
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(std::time::Duration::from_secs(5));
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await
                .unwrap();
            sqlx::query("PRAGMA foreign_keys = ON")
                .execute(&pool)
                .await
                .unwrap();
            if sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='conversations'",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
                == 0
            {
                sqlx::raw_sql(crate::ddl::SCHEMA)
                    .execute(&pool)
                    .await
                    .unwrap();
                crate::Database {
                    pool: pool.clone(),
                    path: String::new(),
                }
                .run_migrations()
                .await
                .unwrap();
                run_pending_migrations(&pool).await.unwrap();
            }
            WakeContractRepository::new(pool)
        };
        (dir, open().await, open().await)
    }

    fn subject() -> WakeSubject {
        WakeSubject {
            profile: WakeProfileRef {
                kind: WakeProfileKind("bash".into()),
                version: WakeProfileVersion(1),
            },
            resource: EncodedWakeValue {
                codec: WakeCodecRef {
                    family: WakeCodecFamily("bash.handle".into()),
                    version: WakeCodecVersion(1),
                },
                payload: WakePayload(b"handle".to_vec()),
            },
        }
    }

    fn register(workflow_id: WorkflowId) -> CommitWakeCommandInput {
        CommitWakeCommandInput {
            workflow_id,
            command: WakeCommand {
                transition_id: TransitionId(1),
                kind: WakeCommandKind::Register {
                    id: WakeContractId::new("contract").unwrap(),
                    registration_owner: WakeOwner("conversation".into()),
                    subject: subject(),
                    condition: WakeCondition::Terminal,
                    registered_at: Timestamp(10),
                    deadline: Timestamp(100),
                },
            },
        }
    }

    fn cancel(state: &WakeState, workflow_id: WorkflowId) -> CommitWakeCommandInput {
        let WakeState::Present(contract) = state else {
            panic!("expected present contract")
        };
        CommitWakeCommandInput {
            workflow_id,
            command: WakeCommand {
                transition_id: TransitionId(2),
                kind: WakeCommandKind::Cancel {
                    expected_head: contract.head(),
                    cause: CancellationCause::UserRequested,
                    occurred_at: Timestamp(20),
                },
            },
        }
    }

    fn propose_cancel(
        state: &WakeState,
        workflow_id: WorkflowId,
    ) -> (
        CommitWakeCommandInput,
        phoenix_workflow::wake_contract::ObservationFenceProof,
    ) {
        let input = cancel(state, workflow_id);
        let modeled = transition(state, input.command.clone());
        let [phoenix_workflow::wake_contract::WakeOwedEffect {
            kind:
                phoenix_workflow::wake_contract::WakeOwedEffectKind::FenceObservationAuthority { proof },
            ..
        }] = modeled.owed_effects.as_slice()
        else {
            panic!("cancellation proposal must owe a fence")
        };
        (input, proof.clone())
    }

    fn finalize_proposal(
        state: &WakeState,
        workflow_id: WorkflowId,
        proof: phoenix_workflow::wake_contract::ObservationFenceProof,
    ) -> CommitWakeCommandInput {
        let WakeState::Present(contract) = state else {
            panic!("expected present contract")
        };
        CommitWakeCommandInput {
            workflow_id,
            command: WakeCommand {
                transition_id: TransitionId(3),
                kind: WakeCommandKind::Reconcile {
                    expected_head: contract.head(),
                    observation:
                        phoenix_workflow::wake_contract::ReconcileObservation::ObservationAuthorityFenced(
                            proof,
                        ),
                },
            },
        }
    }

    fn observe(state: &WakeState, workflow_id: WorkflowId) -> CommitWakeCommandInput {
        let WakeState::Present(contract) = state else {
            panic!("expected present contract")
        };
        CommitWakeCommandInput {
            workflow_id,
            command: WakeCommand {
                transition_id: TransitionId(2),
                kind: WakeCommandKind::ObserveTerminal {
                    expected_head: contract.head(),
                    evidence: phoenix_workflow::wake_contract::TerminalEvidence {
                        occurred_at: Timestamp(20),
                        value: EncodedWakeValue {
                            codec: WakeCodecRef {
                                family: WakeCodecFamily("bash.terminal".into()),
                                version: WakeCodecVersion(1),
                            },
                            payload: WakePayload(b"done".to_vec()),
                        },
                    },
                },
            },
        }
    }

    async fn table_count(
        repo: &WakeContractRepository,
        table: &'static str,
        workflow_id: u64,
    ) -> i64 {
        let sql = match table {
            "transitions" => "SELECT COUNT(*) FROM workflow_transitions WHERE workflow_id = ?1",
            "effects" => "SELECT COUNT(*) FROM workflow_effects WHERE workflow_id = ?1",
            "receipts" => "SELECT COUNT(*) FROM workflow_receipts WHERE workflow_id = ?1",
            "barriers" => "SELECT COUNT(*) FROM workflow_barriers WHERE workflow_id = ?1",
            "barrier_members" => {
                "SELECT COUNT(*) FROM workflow_barrier_members WHERE workflow_id = ?1"
            }
            "deliveries" => "SELECT COUNT(*) FROM workflow_deliveries WHERE workflow_id = ?1",
            _ => panic!("unknown table selector"),
        };
        sqlx::query_scalar(sql)
            .bind(i64::try_from(workflow_id).unwrap())
            .fetch_one(&repo.workflow_repo.pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn command_commit_reloads_and_exact_replay_is_side_effect_free() {
        let (_dir, first, restarted) = open_repo_pair().await;
        let input = register(WorkflowId(1));
        let applied = first.commit_wake_command(&input).await.unwrap();
        assert!(matches!(applied, CommitWakeCommandOutcome::Applied { .. }));
        assert_eq!(
            restarted.load_state(WorkflowId(1)).await.unwrap(),
            match applied {
                CommitWakeCommandOutcome::Applied { state, .. } => state,
                CommitWakeCommandOutcome::Replayed { .. }
                | CommitWakeCommandOutcome::Rejected(_)
                | CommitWakeCommandOutcome::VersionConflict => unreachable!(),
            }
        );
        assert!(matches!(
            restarted.commit_wake_command(&input).await.unwrap(),
            CommitWakeCommandOutcome::Replayed { .. }
        ));
        let transitions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_transitions WHERE workflow_id = 1")
                .fetch_one(&first.workflow_repo.pool)
                .await
                .unwrap();
        assert_eq!(transitions, 1);
    }

    #[tokio::test]
    async fn replay_rejects_a_head_with_missing_owed_artifacts() {
        let (_dir, repo, _) = open_repo_pair().await;
        let input = register(WorkflowId(7));
        assert!(matches!(
            repo.commit_wake_command(&input).await.unwrap(),
            CommitWakeCommandOutcome::Applied { .. }
        ));
        sqlx::query("DELETE FROM workflow_effects WHERE workflow_id = 7")
            .execute(&repo.workflow_repo.pool)
            .await
            .unwrap();
        assert!(matches!(
            repo.commit_wake_command(&input).await,
            Err(DbError::Serialization(message)) if message.contains("missing effect")
        ));
    }

    #[tokio::test]
    async fn crash_cut_rolls_back_head_transition_and_effect_together() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        repo.fail_after_transition_once(WorkflowId(2));
        assert!(repo
            .commit_wake_command(&register(WorkflowId(2)))
            .await
            .is_err());
        assert_eq!(
            restarted.load_state(WorkflowId(2)).await.unwrap(),
            WakeState::Absent
        );
        let workflow_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflows WHERE workflow_id = 2")
                .fetch_one(&repo.workflow_repo.pool)
                .await
                .unwrap();
        let transition_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_transitions WHERE workflow_id = 2")
                .fetch_one(&repo.workflow_repo.pool)
                .await
                .unwrap();
        let effect_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workflow_effects WHERE workflow_id = 2")
                .fetch_one(&repo.workflow_repo.pool)
                .await
                .unwrap();
        assert_eq!((workflow_count, transition_count, effect_count), (0, 0, 0));
    }

    #[tokio::test]
    async fn stale_same_generation_head_cannot_commit() {
        let (_dir, repo, _) = open_repo_pair().await;
        let registered = repo
            .commit_wake_command(&register(WorkflowId(3)))
            .await
            .unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = registered else {
            panic!("registration should apply")
        };
        let cancellation = cancel(&state, WorkflowId(3));
        assert!(matches!(
            repo.commit_wake_command(&cancellation).await.unwrap(),
            CommitWakeCommandOutcome::Applied { .. }
        ));
        let stale = cancel(&state, WorkflowId(3));
        let stale = CommitWakeCommandInput {
            command: WakeCommand {
                transition_id: TransitionId(3),
                ..stale.command
            },
            ..stale
        };
        assert!(matches!(
            repo.commit_wake_command(&stale).await.unwrap(),
            CommitWakeCommandOutcome::Rejected(WakeRejection::StaleHead { .. })
        ));
    }

    #[tokio::test]
    async fn contract_identity_is_unique_across_workflows() {
        let (_dir, repo, _) = open_repo_pair().await;
        assert!(matches!(
            repo.commit_wake_command(&register(WorkflowId(8)))
                .await
                .unwrap(),
            CommitWakeCommandOutcome::Applied { .. }
        ));
        assert!(repo
            .commit_wake_command(&register(WorkflowId(9)))
            .await
            .is_err());
        assert_eq!(
            repo.load_state(WorkflowId(9)).await.unwrap(),
            WakeState::Absent
        );
    }

    #[tokio::test]
    async fn terminalization_requires_a_durable_fence_receipt() {
        let (_dir, repo, _) = open_repo_pair().await;
        let registered = repo
            .commit_wake_command(&register(WorkflowId(10)))
            .await
            .unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = registered else {
            panic!("registration should apply")
        };
        let (proposal, proof) = propose_cancel(&state, WorkflowId(10));
        let proposed = repo.commit_wake_command(&proposal).await.unwrap();
        let CommitWakeCommandOutcome::Applied {
            state: proposed_state,
            ..
        } = proposed
        else {
            panic!("proposal should apply")
        };
        let finalization = finalize_proposal(&proposed_state, WorkflowId(10), proof.clone());
        assert!(matches!(
            repo.commit_wake_command(&finalization).await,
            Err(DbError::Serialization(message)) if message.contains("fence-effect receipt")
        ));
        let fence_effect_id: i64 = sqlx::query_scalar(
            "SELECT effect_id FROM workflow_effects
             WHERE workflow_id = 10 AND kind = 'fence_observation_authority'",
        )
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO workflow_receipts
             (workflow_id, receipt_id, effect_id, generation, declared_workflow_version,
              process_incarnation, attempt_id, origin, receipt_codec_family,
              receipt_codec_version, receipt_payload)
             VALUES (10, 99, ?1, 0, 2, 0, NULL, 'Reconciliation', 'wake.contract', 1, X'00')",
        )
        .bind(fence_effect_id)
        .execute(&repo.workflow_repo.pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE workflow_effects SET status = 'Receipted'
             WHERE workflow_id = 10 AND effect_id = ?1",
        )
        .bind(fence_effect_id)
        .execute(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert!(matches!(
            repo.commit_wake_command(&finalization).await.unwrap(),
            CommitWakeCommandOutcome::Applied { .. }
        ));
    }

    #[tokio::test]
    async fn terminalization_atomically_creates_canonical_bundle() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let registered = repo
            .commit_wake_command(&register(WorkflowId(5)))
            .await
            .unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = registered else {
            panic!("registration should apply")
        };
        let terminal = observe(&state, WorkflowId(5));
        assert!(matches!(
            repo.commit_wake_command(&terminal).await.unwrap(),
            CommitWakeCommandOutcome::Applied { .. }
        ));
        assert!(matches!(
            restarted.load_state(WorkflowId(5)).await.unwrap(),
            WakeState::Present(phoenix_workflow::wake_contract::WakeContract {
                lifecycle: phoenix_workflow::wake_contract::WakeLifecycle::Closed(_),
                ..
            })
        ));
        assert_eq!(table_count(&repo, "transitions", 5).await, 2);
        assert_eq!(table_count(&repo, "effects", 5).await, 2);
        assert_eq!(table_count(&repo, "receipts", 5).await, 1);
        assert_eq!(table_count(&repo, "barriers", 5).await, 1);
        assert_eq!(table_count(&repo, "barrier_members", 5).await, 1);
        assert_eq!(table_count(&repo, "deliveries", 5).await, 1);
        let runtime_acceptance_enabled: i64 = sqlx::query_scalar(
            "SELECT runtime_acceptance_enabled FROM workflows WHERE workflow_id = 5",
        )
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert_eq!(runtime_acceptance_enabled, 1);
        let open_required_effects: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_effects
             WHERE workflow_id = 5 AND role = 'Required'
               AND status NOT IN ('Receipted', 'Invalidated')",
        )
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert_eq!(open_required_effects, 0);
        let receipt_payload: Vec<u8> = sqlx::query_scalar(
            "SELECT receipt_payload FROM workflow_receipts WHERE workflow_id = 5",
        )
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        let delivery_payload: Vec<u8> = sqlx::query_scalar(
            "SELECT payload_blob FROM workflow_deliveries WHERE workflow_id = 5",
        )
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert_eq!(delivery_payload, receipt_payload);
    }

    #[tokio::test]
    async fn terminalization_crash_cut_rolls_back_the_entire_bundle() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let registered = repo
            .commit_wake_command(&register(WorkflowId(6)))
            .await
            .unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = registered else {
            panic!("registration should apply")
        };
        repo.fail_after_transition_once(WorkflowId(6));
        assert!(repo
            .commit_wake_command(&observe(&state, WorkflowId(6)))
            .await
            .is_err());
        assert_eq!(restarted.load_state(WorkflowId(6)).await.unwrap(), state);
        assert_eq!(table_count(&repo, "transitions", 6).await, 1);
        assert_eq!(table_count(&repo, "effects", 6).await, 1);
        assert_eq!(table_count(&repo, "receipts", 6).await, 0);
        assert_eq!(table_count(&repo, "barriers", 6).await, 0);
        assert_eq!(table_count(&repo, "barrier_members", 6).await, 0);
        assert_eq!(table_count(&repo, "deliveries", 6).await, 0);
    }

    #[tokio::test]
    async fn concurrent_same_command_has_one_commit_and_one_replay() {
        let (_dir, first, second) = open_repo_pair().await;
        let input = register(WorkflowId(4));
        let (left, right) = tokio::join!(
            first.commit_wake_command(&input),
            second.commit_wake_command(&input)
        );
        let outcomes = [left.unwrap(), right.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, CommitWakeCommandOutcome::Applied { .. }))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, CommitWakeCommandOutcome::Replayed { .. }))
                .count(),
            1
        );
    }
}
