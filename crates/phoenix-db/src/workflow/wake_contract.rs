use super::{
    next_global_workflow_id_tx, AcceptReceiptInput, CommitTransitionPlanCas, LocalBarrierDecl,
    LocalBarrierMemberDecl, LocalCodec, LocalDeliveryDecl, LocalDeliveryPayloadKind,
    LocalEffectDecl, ReceiptAcceptanceResult, WorkflowRepository,
};
use crate::{DbError, DbResult};
use phoenix_workflow::wake_contract::{
    transition, AuthorizedWakeSubject, RegisteringToolUseId, WakeCommand, WakeCommandKind,
    WakeCondition, WakeContractId, WakeDisposition, WakeEffectRole, WakeOwner, WakeRejection,
    WakeState,
};
use phoenix_workflow::{
    AuthorityOutcome, BarrierId, BarrierStatus, CommitOutcome, DeliveryId, EffectId, EffectRole,
    EffectStatus, ExecutionCapability, Generation, ReceiptFamily, ReceiptOrigin,
    RuntimeAcceptanceStatus, TransitionId, Version, WorkflowId, WorkflowStatus,
};

const WAKE_CONTRACT_CODEC_FAMILY: &str = "wake.contract";
const WAKE_CONTRACT_CODEC_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitWakeCommandInput {
    pub workflow_id: WorkflowId,
    pub command: WakeCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterWakeContractInput {
    pub registration_owner: WakeOwner,
    pub registering_tool_use_id: RegisteringToolUseId,
    pub subject: AuthorizedWakeSubject,
    pub condition: WakeCondition,
    pub registered_at: phoenix_workflow::Timestamp,
    pub deadline: phoenix_workflow::Timestamp,
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

    pub async fn accept_fence_receipt(
        &self,
        input: &AcceptReceiptInput,
    ) -> DbResult<ReceiptAcceptanceResult> {
        let mut tx = self.workflow_repo.begin_immediate_tx().await?;
        let is_fence: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflow_effects e
             JOIN workflows w ON w.workflow_id = e.workflow_id
             JOIN wake_contract_identity_bindings b ON b.workflow_id = e.workflow_id
             JOIN workflow_reclaimable_leases l
               ON l.workflow_id = e.workflow_id AND l.attempt_id = ?3
             WHERE e.workflow_id = ?1 AND e.effect_id = ?2
               AND e.kind = 'fence_observation_authority' AND l.lease_until > ?4
               AND w.profile_kind = 'wake.contract' AND w.profile_version = 1",
        )
        .bind(super::to_i64(input.authority.workflow_id.0, "workflow_id")?)
        .bind(super::to_i64(input.authority.effect_id.0, "effect_id")?)
        .bind(super::to_i64(input.authority.attempt_id.0, "attempt_id")?)
        .bind(super::to_i64(current_commit_time().0, "now")?)
        .fetch_one(&mut *tx.tx)
        .await?;
        if is_fence != 1 {
            tx.rollback().await?;
            return Ok(ReceiptAcceptanceResult {
                outcome: AuthorityOutcome::StaleAuthority,
                receipt: None,
                delivery: None,
            });
        }
        let result = tx.accept_receipt_without_delivery(input).await?;
        if result.outcome == AuthorityOutcome::Authorized {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(result)
    }

    pub async fn commit_wake_command(
        &self,
        input: &CommitWakeCommandInput,
    ) -> DbResult<CommitWakeCommandOutcome> {
        #[cfg(not(test))]
        if matches!(
            input.command.kind,
            phoenix_workflow::wake_contract::WakeCommandKind::Register { .. }
        ) {
            return Err(DbError::Serialization(
                "wake registration requires register_wake_contract".into(),
            ));
        }
        let mut tx = self.workflow_repo.begin_immediate_tx().await?;
        if !validate_observation_authority_tx(&mut tx, input).await? {
            tx.rollback().await?;
            return Ok(CommitWakeCommandOutcome::Rejected(
                WakeRejection::ObservationAuthorityMismatch,
            ));
        }
        self.commit_wake_command_in_tx(tx, input.workflow_id, &input.command)
            .await
    }

    pub async fn register_wake_contract(
        &self,
        input: &RegisterWakeContractInput,
    ) -> DbResult<(WorkflowId, CommitWakeCommandOutcome)> {
        let mut tx = self.workflow_repo.begin_immediate_tx().await?;
        let existing_workflow_id: Option<i64> = sqlx::query_scalar(
            "SELECT workflow_id FROM wake_contract_identity_bindings
             WHERE registration_owner = ?1 AND registering_tool_use_id = ?2",
        )
        .bind(input.registration_owner.as_str())
        .bind(input.registering_tool_use_id.as_str())
        .fetch_optional(&mut *tx.tx)
        .await?;
        let replaying = existing_workflow_id.is_some();
        let workflow_id = match existing_workflow_id {
            Some(value) => WorkflowId(super::to_u64(value, "workflow_id")?),
            None => next_global_workflow_id_tx(&mut tx).await?,
        };
        let command = WakeCommand {
            transition_id: TransitionId(1),
            kind: WakeCommandKind::Register {
                id: WakeContractId::new(format!("wake-{}", workflow_id.0))
                    .ok_or_else(|| DbError::Serialization("allocated wake ID is empty".into()))?,
                registration_owner: input.registration_owner.clone(),
                registering_tool_use_id: input.registering_tool_use_id.clone(),
                subject: input.subject.clone(),
                condition: input.condition.clone(),
                registered_at: input.registered_at,
                deadline: input.deadline,
            },
        };
        if replaying {
            let current = self.load_state_tx(&mut tx, workflow_id).await?;
            let expected = transition(&WakeState::Absent, command.clone());
            let registrations_match = match (&current, &expected.new_state) {
                (WakeState::Present(current), WakeState::Present(expected)) => {
                    current.id == expected.id
                        && current.registration_owner == expected.registration_owner
                        && current.registering_tool_use_id == expected.registering_tool_use_id
                        && current.subject == expected.subject
                        && current.delivery_transferability == expected.delivery_transferability
                        && current.condition == expected.condition
                        && current.registered_at == expected.registered_at
                        && current.deadline == expected.deadline
                }
                _ => false,
            };
            if !registrations_match {
                tx.rollback().await?;
                return Ok((
                    workflow_id,
                    CommitWakeCommandOutcome::Rejected(WakeRejection::ConflictingTransitionReuse),
                ));
            }
            validate_registration_replay_artifacts_tx(&mut tx, workflow_id).await?;
            tx.rollback().await?;
            return Ok((
                workflow_id,
                CommitWakeCommandOutcome::Replayed {
                    state: current,
                    transition_id: TransitionId(1),
                },
            ));
        }
        let outcome = self
            .commit_wake_command_in_tx(tx, workflow_id, &command)
            .await?;
        Ok((workflow_id, outcome))
    }

    async fn commit_wake_command_in_tx(
        &self,
        mut tx: super::WorkflowTx<'_>,
        workflow_id: WorkflowId,
        command: &WakeCommand,
    ) -> DbResult<CommitWakeCommandOutcome> {
        let current = self.load_state_tx(&mut tx, workflow_id).await?;
        let result = transition(&current, command.clone());

        match result.disposition {
            WakeDisposition::Rejected(rejection) => {
                tx.rollback().await?;
                Ok(CommitWakeCommandOutcome::Rejected(rejection))
            }
            WakeDisposition::Replayed { transition_id, .. } => {
                validate_replay_artifacts_tx(&mut tx, workflow_id, &current).await?;
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
                let next_status = workflow_status_after_event(event);
                let codec = codec();
                let event_payload = encode(event)?;
                let snapshot_payload = encode(&result.new_state)?;
                let effects = result
                    .owed_effects
                    .iter()
                    .map(|effect| local_effect(effect, contract_version(&result.new_state)))
                    .collect::<DbResult<Vec<_>>>()?;
                let terminal_bundle = terminal_bundle(event, &result.new_state)?;
                let committed_at = current_commit_time();

                if current == WakeState::Absent {
                    self.insert_initial_head(
                        &mut tx,
                        workflow_id,
                        generation,
                        &codec,
                        committed_at,
                    )
                    .await?;
                }
                if let WakeState::Present(contract) = &result.new_state {
                    if current == WakeState::Absent {
                        insert_contract_identity_tx(&mut tx, workflow_id, contract).await?;
                    }
                }
                if is_proposal_finalization(&current, event) {
                    validate_fence_receipt_tx(&mut tx, workflow_id, &current).await?;
                }
                if should_revoke_observation_authority(event) {
                    revoke_observation_authority_tx(&mut tx, workflow_id).await?;
                }
                if terminal_bundle.is_some() {
                    invalidate_prior_required_effects_tx(&mut tx, workflow_id).await?;
                }
                let committed = tx
                    .commit_transition_plan(&CommitTransitionPlanCas {
                        workflow_id,
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
                        workflow_id,
                        generation,
                        contract_version(&result.new_state),
                        bundle,
                    )
                    .await?;
                }
                if terminal_bundle
                    .as_ref()
                    .is_some_and(|bundle| !bundle.delivery.requires_runtime_acceptance)
                {
                    settle_non_resuming_delivery_tx(&mut tx, workflow_id, event.transition_id)
                        .await?;
                }
                if current != WakeState::Absent {
                    update_contract_identity_tx(&mut tx, workflow_id, &result.new_state).await?;
                }

                #[cfg(test)]
                if self
                    .fail_after_transition_once
                    .lock()
                    .expect("wake contract failpoint lock")
                    .take()
                    == Some(workflow_id)
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

    pub async fn finalize_receipted_proposal(
        &self,
        workflow_id: WorkflowId,
        transition_id: TransitionId,
    ) -> DbResult<CommitWakeCommandOutcome> {
        let state = self.load_state(workflow_id).await?;
        let command =
            phoenix_workflow::wake_contract::finalize_proposed_terminal(&state, transition_id)
                .ok_or_else(|| DbError::Serialization("wake has no terminal proposal".into()))?;
        self.commit_wake_command(&CommitWakeCommandInput {
            workflow_id,
            command,
        })
        .await
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
        if head.binding.profile.profile_kind != "wake.contract"
            || head.binding.profile.profile_version != 1
            || head.snapshot_codec.family != WAKE_CONTRACT_CODEC_FAMILY
            || head.snapshot_codec.version != WAKE_CONTRACT_CODEC_VERSION
        {
            return Err(DbError::Serialization(format!(
                "workflow {} has an unsupported wake contract profile or snapshot codec",
                workflow_id.0
            )));
        }
        let state: WakeState = decode(&head.snapshot_payload)?;
        let WakeState::Present(contract) = &state else {
            return Err(DbError::Serialization(
                "persisted wake workflow cannot have an absent aggregate".into(),
            ));
        };
        let expected_status = workflow_status_for_state(contract);
        if head.version != contract.version
            || head.generation != contract.generation
            || head.status != expected_status
        {
            return Err(DbError::Serialization(
                "wake workflow head disagrees with aggregate snapshot".into(),
            ));
        }
        validate_authority_projection_tx(tx, workflow_id, state).await
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

async fn validate_observation_authority_tx(
    tx: &mut super::WorkflowTx<'_>,
    input: &CommitWakeCommandInput,
) -> DbResult<bool> {
    enum PersistedObservation {
        Terminal(phoenix_workflow::wake_contract::TerminalEvidence),
        ProtocolFailure(phoenix_workflow::wake_contract::ProtocolFailureEvidence),
    }
    let (authority, recorded_evidence) = match &input.command.kind {
        WakeCommandKind::ObserveTerminal {
            authority,
            evidence,
            ..
        } => (
            authority,
            Some(PersistedObservation::Terminal(evidence.clone())),
        ),
        WakeCommandKind::Reconcile {
            observation:
                phoenix_workflow::wake_contract::ReconcileObservation::ProtocolFailure {
                    authority,
                    cause,
                    occurred_at,
                },
            ..
        } => (
            authority,
            Some(PersistedObservation::ProtocolFailure(
                phoenix_workflow::wake_contract::ProtocolFailureEvidence {
                    cause: cause.clone(),
                    occurred_at: *occurred_at,
                },
            )),
        ),
        WakeCommandKind::Register { .. }
        | WakeCommandKind::Cancel { .. }
        | WakeCommandKind::DeadlineElapsed { .. }
        | WakeCommandKind::TransferDeliveryOwner { .. }
        | WakeCommandKind::Reconcile { .. } => return Ok(true),
    };
    let workflow_id = super::to_i64(input.workflow_id.0, "workflow_id")?;
    let attempt_id = super::to_i64(authority.attempt_id().0, "attempt_id")?;
    let now = super::to_i64(current_commit_time().0, "now")?;
    let live: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_attempts a
         JOIN workflow_effects e
           ON e.workflow_id = a.workflow_id AND e.effect_id = a.effect_id
         JOIN workflow_reclaimable_leases l
           ON l.workflow_id = a.workflow_id AND l.attempt_id = a.attempt_id
         WHERE a.workflow_id = ?1 AND a.attempt_id = ?2
           AND a.status IN ('Begun', 'ObservationRecorded') AND l.lease_until > ?3
           AND e.kind = 'begin_observation'
           AND e.generation = (SELECT generation FROM workflows WHERE workflow_id = ?1)",
    )
    .bind(workflow_id)
    .bind(attempt_id)
    .bind(now)
    .fetch_one(&mut *tx.tx)
    .await?;
    if live == 1 {
        return Ok(true);
    }
    let Some(evidence) = recorded_evidence else {
        return Ok(false);
    };
    let (codec_family, codec_version, payload, observed_at) = match evidence {
        PersistedObservation::Terminal(evidence) => (
            evidence.value.codec.family.as_str().to_owned(),
            evidence.value.codec.version.get(),
            evidence.value.payload.0,
            evidence.occurred_at,
        ),
        PersistedObservation::ProtocolFailure(evidence) => (
            WAKE_CONTRACT_CODEC_FAMILY.to_owned(),
            WAKE_CONTRACT_CODEC_VERSION,
            encode(&evidence)?,
            evidence.occurred_at,
        ),
    };
    let exact_recorded: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_attempts a
         JOIN workflow_authoritative_observations o
           ON o.workflow_id = a.workflow_id AND o.attempt_id = a.attempt_id
         JOIN workflow_effects e
           ON e.workflow_id = a.workflow_id AND e.effect_id = a.effect_id
         WHERE a.workflow_id = ?1 AND a.attempt_id = ?2
           AND a.status IN ('ObservationRecorded', 'AuthorityLost')
           AND e.kind = 'begin_observation'
           AND o.observation_codec_family = ?3 AND o.observation_codec_version = ?4
           AND o.observation_payload = ?5 AND o.observed_at = ?6",
    )
    .bind(workflow_id)
    .bind(attempt_id)
    .bind(codec_family)
    .bind(i64::from(codec_version))
    .bind(payload)
    .bind(super::to_i64(observed_at.0, "observed_at")?)
    .fetch_one(&mut *tx.tx)
    .await?;
    Ok(exact_recorded > 0)
}

async fn validate_authority_projection_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
    state: WakeState,
) -> DbResult<WakeState> {
    let WakeState::Present(contract) = &state else {
        return Ok(state);
    };
    let (lifecycle_kind, terminal_at, forgotten_reason) = lifecycle_columns(&contract.lifecycle);
    let matches: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wake_contract_identity_bindings
         WHERE workflow_id = ?1 AND contract_id = ?2 AND generation = ?3 AND version = ?4
           AND registration_owner = ?5 AND delivery_owner = ?6
           AND registering_tool_use_id = ?7 AND delivery_transferability = ?8
           AND profile_kind = ?9 AND profile_version = ?10 AND resource_codec_family = ?11
           AND resource_codec_version = ?12 AND resource_payload = ?13
           AND evidence_codec_family = ?14 AND evidence_codec_version = ?15
           AND registered_at = ?16 AND deadline = ?17 AND lifecycle_kind = ?18
           AND terminal_occurred_at IS ?19 AND forgotten_reason IS ?20",
    )
    .bind(super::to_i64(workflow_id.0, "workflow_id")?)
    .bind(contract.id.as_str())
    .bind(super::to_i64(contract.generation.0, "generation")?)
    .bind(super::to_i64(contract.version.0, "version")?)
    .bind(contract.registration_owner.as_str())
    .bind(contract.delivery_owner.as_str())
    .bind(contract.registering_tool_use_id.as_str())
    .bind(transferability_str(contract.delivery_transferability))
    .bind(contract.subject.profile.kind.as_str())
    .bind(i64::from(contract.subject.profile.version.get()))
    .bind(contract.subject.resource.codec.family.as_str())
    .bind(i64::from(contract.subject.resource.codec.version.get()))
    .bind(&contract.subject.resource.payload.0)
    .bind(contract.subject.terminal_evidence_codec.family.as_str())
    .bind(i64::from(
        contract.subject.terminal_evidence_codec.version.get(),
    ))
    .bind(super::to_i64(contract.registered_at.0, "registered_at")?)
    .bind(super::to_i64(contract.deadline.0, "deadline")?)
    .bind(lifecycle_kind)
    .bind(
        terminal_at
            .map(|time| super::to_i64(time.0, "terminal_occurred_at"))
            .transpose()?,
    )
    .bind(forgotten_reason)
    .fetch_one(&mut *tx.tx)
    .await?;
    if matches != 1 {
        return Err(DbError::Serialization(
            "wake contract snapshot and normalized authority disagree".into(),
        ));
    }
    Ok(state)
}

async fn insert_contract_identity_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
    contract: &phoenix_workflow::wake_contract::WakeContract,
) -> DbResult<()> {
    let (lifecycle_kind, terminal_at, forgotten_reason) = lifecycle_columns(&contract.lifecycle);
    sqlx::query(
        "INSERT INTO wake_contract_identity_bindings
         (contract_id, workflow_id, generation, version, registration_owner, delivery_owner,
          registering_tool_use_id, delivery_transferability, profile_kind, profile_version,
          resource_codec_family, resource_codec_version, resource_payload, evidence_codec_family,
          evidence_codec_version, registered_at, deadline, lifecycle_kind, terminal_occurred_at,
          forgotten_reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
    )
    .bind(contract.id.as_str())
    .bind(super::to_i64(workflow_id.0, "workflow_id")?)
    .bind(super::to_i64(contract.generation.0, "generation")?)
    .bind(super::to_i64(contract.version.0, "version")?)
    .bind(contract.registration_owner.as_str())
    .bind(contract.delivery_owner.as_str())
    .bind(contract.registering_tool_use_id.as_str())
    .bind(transferability_str(contract.delivery_transferability))
    .bind(contract.subject.profile.kind.as_str())
    .bind(i64::from(contract.subject.profile.version.get()))
    .bind(contract.subject.resource.codec.family.as_str())
    .bind(i64::from(contract.subject.resource.codec.version.get()))
    .bind(&contract.subject.resource.payload.0)
    .bind(contract.subject.terminal_evidence_codec.family.as_str())
    .bind(i64::from(
        contract.subject.terminal_evidence_codec.version.get(),
    ))
    .bind(super::to_i64(contract.registered_at.0, "registered_at")?)
    .bind(super::to_i64(contract.deadline.0, "deadline")?)
    .bind(lifecycle_kind)
    .bind(
        terminal_at
            .map(|time| super::to_i64(time.0, "terminal_occurred_at"))
            .transpose()?,
    )
    .bind(forgotten_reason)
    .execute(&mut *tx.tx)
    .await?;
    Ok(())
}

async fn update_contract_identity_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
    state: &WakeState,
) -> DbResult<()> {
    let WakeState::Present(contract) = state else {
        return Ok(());
    };
    let (lifecycle_kind, terminal_at, forgotten_reason) = lifecycle_columns(&contract.lifecycle);
    let updated = sqlx::query(
        "UPDATE wake_contract_identity_bindings
         SET generation = ?2, version = ?3, delivery_owner = ?4,
             lifecycle_kind = ?5, terminal_occurred_at = ?6, forgotten_reason = ?7
         WHERE workflow_id = ?1",
    )
    .bind(super::to_i64(workflow_id.0, "workflow_id")?)
    .bind(super::to_i64(contract.generation.0, "generation")?)
    .bind(super::to_i64(contract.version.0, "version")?)
    .bind(contract.delivery_owner.as_str())
    .bind(lifecycle_kind)
    .bind(
        terminal_at
            .map(|time| super::to_i64(time.0, "terminal_occurred_at"))
            .transpose()?,
    )
    .bind(forgotten_reason)
    .execute(&mut *tx.tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DbError::Serialization(
            "wake contract authority projection is missing".into(),
        ));
    }
    Ok(())
}

fn transferability_str(
    transferability: phoenix_workflow::wake_contract::WakeDeliveryTransferability,
) -> &'static str {
    match transferability {
        phoenix_workflow::wake_contract::WakeDeliveryTransferability::WorkScope => "WorkScope",
        phoenix_workflow::wake_contract::WakeDeliveryTransferability::FixedOwner => "FixedOwner",
    }
}

fn lifecycle_columns(
    lifecycle: &phoenix_workflow::wake_contract::WakeLifecycle,
) -> (
    &'static str,
    Option<phoenix_workflow::Timestamp>,
    Option<&'static str>,
) {
    use phoenix_workflow::wake_contract::{CanonicalTerminal, OpenWakeLifecycle, WakeLifecycle};
    match lifecycle {
        WakeLifecycle::Open(OpenWakeLifecycle::Observing) => ("Observing", None, None),
        WakeLifecycle::Open(OpenWakeLifecycle::TerminalProposed(_)) => {
            ("TerminalProposed", None, None)
        }
        WakeLifecycle::Closed(CanonicalTerminal::Fired { evidence }) => {
            ("Fired", Some(evidence.occurred_at), None)
        }
        WakeLifecycle::Closed(CanonicalTerminal::Expired { deadline }) => {
            ("Expired", Some(*deadline), None)
        }
        WakeLifecycle::Closed(CanonicalTerminal::Cancelled { occurred_at, .. }) => {
            ("Cancelled", Some(*occurred_at), None)
        }
        WakeLifecycle::Closed(CanonicalTerminal::Forgotten { cause, occurred_at }) => (
            "Forgotten",
            Some(*occurred_at),
            Some(match cause {
                phoenix_workflow::wake_contract::ForgottenCause::PhoenixRestart => "PhoenixRestart",
                phoenix_workflow::wake_contract::ForgottenCause::CascadeDestroyedHandle => {
                    "CascadeDestroyedHandle"
                }
                phoenix_workflow::wake_contract::ForgottenCause::SubagentHandleMissing => {
                    "SubagentHandleMissing"
                }
                phoenix_workflow::wake_contract::ForgottenCause::TmuxHandleMissing => {
                    "TmuxHandleMissing"
                }
            }),
        ),
    }
}

async fn validate_registration_replay_artifacts_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
) -> DbResult<()> {
    let workflow_id = super::to_i64(workflow_id.0, "workflow_id")?;
    let registration_rows: i64 = sqlx::query_scalar(
        "SELECT
             (SELECT COUNT(*) FROM workflow_transitions
              WHERE workflow_id = ?1 AND transition_id = 1) +
             (SELECT COUNT(*) FROM workflow_effects
              WHERE workflow_id = ?1 AND effect_id = ?2 AND kind = 'begin_observation')",
    )
    .bind(workflow_id)
    .bind(super::to_i64(
        effect_id(TransitionId(1), WakeEffectRole::BeginObservation)?,
        "effect_id",
    )?)
    .fetch_one(&mut *tx.tx)
    .await?;
    if registration_rows != 2 {
        return Err(DbError::Serialization(
            "wake registration replay artifacts are incomplete".into(),
        ));
    }
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
    let codec_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workflow_supported_codecs
         WHERE workflow_id = ?1 AND codec_family = ?2 AND codec_version = ?3",
    )
    .bind(super::to_i64(workflow_id.0, "workflow_id")?)
    .bind(WAKE_CONTRACT_CODEC_FAMILY)
    .bind(i64::from(WAKE_CONTRACT_CODEC_VERSION))
    .fetch_one(&mut *tx.tx)
    .await?;
    if codec_count != 1 {
        return Err(DbError::Serialization(
            "wake contract codec/version is not supported".into(),
        ));
    }
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
    )?);
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
        let workflow_id = super::to_i64(workflow_id.0, "workflow_id")?;
        let delivery_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT delivery_id FROM workflow_deliveries WHERE workflow_id = ?1",
        )
        .bind(workflow_id)
        .fetch_all(&mut *tx.tx)
        .await?;
        let [terminal_id] = delivery_ids.as_slice() else {
            return Err(DbError::Serialization(
                "wake replay terminal bundle is incomplete".into(),
            ));
        };
        let terminal_rows: i64 = sqlx::query_scalar(
            "SELECT
                 (SELECT COUNT(*) FROM workflow_receipts WHERE workflow_id = ?1 AND receipt_id = ?2) +
                 (SELECT COUNT(*) FROM workflow_barriers WHERE workflow_id = ?1 AND barrier_id = ?2) +
                 (SELECT COUNT(*) FROM workflow_barrier_members WHERE workflow_id = ?1 AND barrier_id = ?2) +
                 (SELECT COUNT(*) FROM workflow_deliveries WHERE workflow_id = ?1 AND delivery_id = ?2)",
        )
        .bind(workflow_id)
        .bind(terminal_id)
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

fn workflow_status_for_state(
    contract: &phoenix_workflow::wake_contract::WakeContract,
) -> WorkflowStatus {
    match &contract.lifecycle {
        phoenix_workflow::wake_contract::WakeLifecycle::Closed(terminal)
            if terminal.resume_policy()
                == phoenix_workflow::wake_contract::WakeResumePolicy::SuppressAutomaticResume =>
        {
            WorkflowStatus::Completed
        }
        phoenix_workflow::wake_contract::WakeLifecycle::Open(_)
        | phoenix_workflow::wake_contract::WakeLifecycle::Closed(_) => WorkflowStatus::Active,
    }
}

fn workflow_status_after_event(
    event: &phoenix_workflow::wake_contract::WakeContractEvent,
) -> WorkflowStatus {
    match &event.kind {
        phoenix_workflow::wake_contract::WakeEventKind::Terminalized {
            resume_policy:
                phoenix_workflow::wake_contract::WakeResumePolicy::SuppressAutomaticResume,
            ..
        } => WorkflowStatus::Completed,
        phoenix_workflow::wake_contract::WakeEventKind::Registered { .. }
        | phoenix_workflow::wake_contract::WakeEventKind::DeliveryOwnerTransferred { .. }
        | phoenix_workflow::wake_contract::WakeEventKind::TerminalProposed { .. }
        | phoenix_workflow::wake_contract::WakeEventKind::Terminalized {
            resume_policy: phoenix_workflow::wake_contract::WakeResumePolicy::RequestWhenIdle,
            ..
        } => WorkflowStatus::Active,
    }
}

fn terminal_bundle(
    event: &phoenix_workflow::wake_contract::WakeContractEvent,
    _state: &WakeState,
) -> DbResult<Option<TerminalBundle>> {
    let phoenix_workflow::wake_contract::WakeEventKind::Terminalized {
        terminal,
        delivery_owner: _,
        resume_policy,
    } = &event.kind
    else {
        return Ok(None);
    };
    let terminal_effect = EffectId(effect_id(
        event.transition_id,
        WakeEffectRole::CommitTerminalization,
    )?);
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
            consumer_kind: "reducer".to_owned(),
            event_codec: codec(),
            payload_kind: LocalDeliveryPayloadKind::Barrier,
            payload_blob: payload,
            requires_runtime_acceptance,
            runtime_acceptance_status: requires_runtime_acceptance
                .then_some(RuntimeAcceptanceStatus::Owed),
        },
    }))
}

fn is_proposal_finalization(
    current: &WakeState,
    event: &phoenix_workflow::wake_contract::WakeContractEvent,
) -> bool {
    let WakeState::Present(contract) = current else {
        return false;
    };
    let phoenix_workflow::wake_contract::WakeLifecycle::Open(
        phoenix_workflow::wake_contract::OpenWakeLifecycle::TerminalProposed(proposal),
    ) = &contract.lifecycle
    else {
        return false;
    };
    matches!(
        &event.kind,
        phoenix_workflow::wake_contract::WakeEventKind::Terminalized { terminal, .. }
            if terminal == &proposal.terminal.clone().into_terminal()
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
    )?;
    let proposal_payload: Vec<u8> = sqlx::query_scalar(
        "SELECT event_payload FROM workflow_transitions
         WHERE workflow_id = ?1 AND transition_id = ?2",
    )
    .bind(super::to_i64(workflow_id.0, "workflow_id")?)
    .bind(super::to_i64(
        proposal.transition_id.0,
        "proposal_transition_id",
    )?)
    .fetch_one(&mut *tx.tx)
    .await?;
    let proposal_event: phoenix_workflow::wake_contract::WakeContractEvent =
        decode(&proposal_payload)?;
    if !matches!(
        proposal_event.kind,
        phoenix_workflow::wake_contract::WakeEventKind::TerminalProposed { proposal: ref durable }
            if durable == &proposal.terminal
                && proposal_event.contract_id == contract.id
                && proposal_event.head.generation == contract.generation
    ) {
        return Err(DbError::Serialization(
            "terminal proposal does not match its durable event".into(),
        ));
    }
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
    let workflow_id = super::to_i64(workflow_id.0, "workflow_id")?;
    sqlx::query(
        "UPDATE workflow_attempts
         SET status = 'AuthorityLost'
         WHERE workflow_id = ?1 AND status IN ('Begun', 'ObservationRecorded')",
    )
    .bind(workflow_id)
    .execute(&mut *tx.tx)
    .await?;
    sqlx::query("DELETE FROM workflow_reclaimable_leases WHERE workflow_id = ?1")
        .bind(workflow_id)
        .execute(&mut *tx.tx)
        .await?;
    sqlx::query(
        "UPDATE workflow_effects SET status = 'Invalidated'
         WHERE workflow_id = ?1 AND kind = 'begin_observation' AND status = 'Eligible'",
    )
    .bind(workflow_id)
    .execute(&mut *tx.tx)
    .await?;
    Ok(())
}

async fn settle_non_resuming_delivery_tx(
    tx: &mut super::WorkflowTx<'_>,
    workflow_id: WorkflowId,
    transition_id: TransitionId,
) -> DbResult<()> {
    let updated = sqlx::query(
        "UPDATE workflow_deliveries
         SET status = 'Accepted', accepted_by_transition_id = ?2
         WHERE workflow_id = ?1 AND delivery_id = ?2
           AND status = 'Pending' AND requires_runtime_acceptance = 0",
    )
    .bind(super::to_i64(workflow_id.0, "workflow_id")?)
    .bind(super::to_i64(transition_id.0, "transition_id")?)
    .execute(&mut *tx.tx)
    .await?
    .rows_affected();
    if updated != 1 {
        return Err(DbError::Serialization(
            "wake cancellation delivery was not settled exactly once".into(),
        ));
    }
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
    let effect_id = EffectId(effect_id(effect.key.transition_id, effect.key.role)?);
    Ok(LocalEffectDecl {
        effect_id,
        declared_workflow_version,
        family: "wake.contract".into(),
        kind: role.into(),
        intent_codec: codec(),
        intent_payload: encode(effect)?,
        generation: effect.key.generation,
        role: EffectRole::Required,
        capability: match effect.kind {
            phoenix_workflow::wake_contract::WakeOwedEffectKind::BeginObservation { .. }
            | phoenix_workflow::wake_contract::WakeOwedEffectKind::FenceObservationAuthority {
                ..
            } => ExecutionCapability::ReclaimableObservation,
            phoenix_workflow::wake_contract::WakeOwedEffectKind::CommitTerminalization {
                ..
            }
            | phoenix_workflow::wake_contract::WakeOwedEffectKind::TransferDeliveryOwner {
                ..
            } => ExecutionCapability::SafelyRepeatable,
        },
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

fn effect_id(transition_id: TransitionId, role: WakeEffectRole) -> DbResult<u64> {
    let role = match role {
        WakeEffectRole::BeginObservation => 1,
        WakeEffectRole::FenceObservationAuthority => 2,
        WakeEffectRole::CommitTerminalization => 3,
        WakeEffectRole::TransferDeliveryOwner => 4,
    };
    transition_id
        .0
        .checked_mul(8)
        .and_then(|value| value.checked_add(role))
        .filter(|value| i64::try_from(*value).is_ok())
        .ok_or_else(|| DbError::Serialization("wake effect identity overflow".into()))
}

fn current_commit_time() -> phoenix_workflow::Timestamp {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    phoenix_workflow::Timestamp(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::run_pending_migrations;
    use crate::workflow::BeginAttemptInput;
    use phoenix_workflow::wake_contract::{
        AuthorizedWakeSubject, CancellationCause, EncodedWakeValue, ForgottenCause,
        RegisteringToolUseId, WakeCodecFamily, WakeCodecRef, WakeCodecVersion, WakeCommandKind,
        WakeCondition, WakeContractId, WakeOwner, WakePayload, WakeProfileKind, WakeProfileRef,
        WakeProfileVersion, WakeSubject,
    };
    use phoenix_workflow::Timestamp;
    use phoenix_workflow::{ProcessIncarnation, ReceiptId};
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
                kind: WakeProfileKind::new("bash").unwrap(),
                version: WakeProfileVersion::new(1).unwrap(),
            },
            resource: EncodedWakeValue {
                codec: WakeCodecRef {
                    family: WakeCodecFamily::new("bash.handle").unwrap(),
                    version: WakeCodecVersion::new(1).unwrap(),
                },
                payload: WakePayload(b"handle".to_vec()),
            },
            terminal_evidence_codec: WakeCodecRef {
                family: WakeCodecFamily::new("bash.terminal").unwrap(),
                version: WakeCodecVersion::new(1).unwrap(),
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
                    registration_owner: WakeOwner::new("conversation").unwrap(),
                    registering_tool_use_id: RegisteringToolUseId::new("tool-use").unwrap(),
                    subject: AuthorizedWakeSubject::work_scope_for_test(
                        subject(),
                        WakeOwner::new("conversation").unwrap(),
                    ),
                    condition: WakeCondition::Terminal,
                    registered_at: Timestamp(10),
                    deadline: Timestamp(100),
                },
            },
        }
    }

    fn register_allocated() -> RegisterWakeContractInput {
        RegisterWakeContractInput {
            registration_owner: WakeOwner::new("conversation").unwrap(),
            registering_tool_use_id: RegisteringToolUseId::new("tool-use").unwrap(),
            subject: AuthorizedWakeSubject::work_scope_for_test(
                subject(),
                WakeOwner::new("conversation").unwrap(),
            ),
            condition: WakeCondition::Terminal,
            registered_at: Timestamp(10),
            deadline: Timestamp(100),
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
        observe_at(state, workflow_id, TransitionId(2), Timestamp(20))
    }

    fn observe_with_authority(
        state: &WakeState,
        workflow_id: WorkflowId,
        transition_id: TransitionId,
        occurred_at: Timestamp,
        authority: phoenix_workflow::wake_contract::WakeObservationAuthority,
    ) -> CommitWakeCommandInput {
        let mut input = observe_at(state, workflow_id, transition_id, occurred_at);
        let WakeCommandKind::ObserveTerminal {
            authority: command_authority,
            ..
        } = &mut input.command.kind
        else {
            unreachable!("observe_at creates a terminal observation")
        };
        *command_authority = authority;
        input
    }

    fn observe_at(
        state: &WakeState,
        workflow_id: WorkflowId,
        transition_id: TransitionId,
        occurred_at: Timestamp,
    ) -> CommitWakeCommandInput {
        let WakeState::Present(contract) = state else {
            panic!("expected present contract")
        };
        CommitWakeCommandInput {
            workflow_id,
            command: WakeCommand {
                transition_id,
                kind: WakeCommandKind::ObserveTerminal {
                    expected_head: contract.head(),
                    authority: phoenix_workflow::wake_contract::WakeObservationAuthority::for_test(
                        contract,
                        phoenix_workflow::AttemptId(1),
                    ),
                    evidence: phoenix_workflow::wake_contract::TerminalEvidence {
                        occurred_at,
                        value: EncodedWakeValue {
                            codec: WakeCodecRef {
                                family: WakeCodecFamily::new("bash.terminal").unwrap(),
                                version: WakeCodecVersion::new(1).unwrap(),
                            },
                            payload: WakePayload(b"done".to_vec()),
                        },
                    },
                },
            },
        }
    }

    fn transfer(
        state: &WakeState,
        workflow_id: WorkflowId,
        transition_id: TransitionId,
    ) -> CommitWakeCommandInput {
        let WakeState::Present(contract) = state else {
            panic!("expected present contract")
        };
        CommitWakeCommandInput {
            workflow_id,
            command: WakeCommand {
                transition_id,
                kind: WakeCommandKind::TransferDeliveryOwner {
                    expected_head: contract.head(),
                    authority:
                        phoenix_workflow::wake_contract::AuthorizedWakeOwnerTransfer::for_test(
                            contract,
                            WakeOwner::new("successor").unwrap(),
                        ),
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

    async fn start_observation_attempt(repo: &WakeContractRepository, workflow_id: WorkflowId) {
        repo.workflow_repo
            .begin_attempt(&BeginAttemptInput {
                workflow_id,
                effect_id: EffectId(9),
                attempt_id: phoenix_workflow::AttemptId(1),
                process_incarnation: ProcessIncarnation(1),
                now: Timestamp(11),
                lease_until: Some(phoenix_workflow::LeaseExpiry(i64::MAX as u64)),
            })
            .await
            .unwrap();
    }

    async fn persist_observation_before_fence(
        repo: &WakeContractRepository,
        workflow_id: WorkflowId,
    ) {
        start_observation_attempt(repo, workflow_id).await;
        let authority = crate::workflow::LocalAttemptAuthority {
            workflow_id,
            effect_id: EffectId(9),
            attempt_id: phoenix_workflow::AttemptId(1),
            declared_workflow_version: Version(1),
            generation: Generation(0),
            process_incarnation: ProcessIncarnation(1),
        };
        repo.workflow_repo
            .record_observation(&crate::workflow::RecordObservationInput {
                authority,
                observation_id: 1,
                observation_codec: LocalCodec {
                    family: "bash.terminal".into(),
                    version: 1,
                },
                observation_payload: b"done".to_vec(),
                observed_at: Timestamp(19),
                now: Timestamp(19),
            })
            .await
            .unwrap();
    }

    async fn persist_protocol_failure_before_fence(
        repo: &WakeContractRepository,
        workflow_id: WorkflowId,
        evidence: &phoenix_workflow::wake_contract::ProtocolFailureEvidence,
    ) {
        start_observation_attempt(repo, workflow_id).await;
        repo.workflow_repo
            .record_observation(&crate::workflow::RecordObservationInput {
                authority: crate::workflow::LocalAttemptAuthority {
                    workflow_id,
                    effect_id: EffectId(9),
                    attempt_id: phoenix_workflow::AttemptId(1),
                    declared_workflow_version: Version(1),
                    generation: Generation(0),
                    process_incarnation: ProcessIncarnation(1),
                },
                observation_id: 1,
                observation_codec: codec(),
                observation_payload: encode(evidence).unwrap(),
                observed_at: evidence.occurred_at,
                now: evidence.occurred_at,
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn registration_allocates_from_the_shared_workflow_sequence_atomically() {
        let (_dir, repo, _) = open_repo_pair().await;
        let (workflow_id, outcome) = repo
            .register_wake_contract(&register_allocated())
            .await
            .unwrap();
        assert_eq!(workflow_id, WorkflowId(1));
        assert!(matches!(outcome, CommitWakeCommandOutcome::Applied { .. }));
        let (replayed_workflow_id, replayed) = repo
            .register_wake_contract(&register_allocated())
            .await
            .unwrap();
        assert_eq!(replayed_workflow_id, workflow_id);
        assert!(matches!(
            replayed,
            CommitWakeCommandOutcome::Replayed { .. }
        ));
        let next: i64 = sqlx::query_scalar(
            "SELECT next_value FROM workflow_global_sequences WHERE sequence_name = 'workflow'",
        )
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert_eq!(next, 2);
        let capability: String = sqlx::query_scalar(
            "SELECT capability_kind FROM workflow_effects
             WHERE workflow_id = ?1 AND kind = 'begin_observation'",
        )
        .bind(i64::try_from(workflow_id.0).unwrap())
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert_eq!(capability, "ReclaimableObservation");
        assert!(sqlx::query(
            "UPDATE wake_contract_identity_bindings
             SET deadline = registered_at + 1801 WHERE workflow_id = ?1",
        )
        .bind(i64::try_from(workflow_id.0).unwrap())
        .execute(&repo.workflow_repo.pool)
        .await
        .is_err());
        assert!(matches!(
            repo.load_state(workflow_id).await.unwrap(),
            WakeState::Present(_)
        ));
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
    async fn registration_replays_after_later_transitions_advance_the_head() {
        let (_dir, repo, _) = open_repo_pair().await;
        let registration = register_allocated();
        let (workflow_id, registered) = repo.register_wake_contract(&registration).await.unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = registered else {
            panic!("registration should apply")
        };
        assert!(matches!(
            repo.commit_wake_command(&transfer(&state, workflow_id, TransitionId(2)))
                .await
                .unwrap(),
            CommitWakeCommandOutcome::Applied { .. }
        ));
        assert!(matches!(
            repo.register_wake_contract(&registration).await.unwrap(),
            (replayed_workflow_id, CommitWakeCommandOutcome::Replayed { .. })
                if replayed_workflow_id == workflow_id
        ));
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
    async fn wake_head_projection_drift_is_rejected() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(17);
        repo.commit_wake_command(&register(workflow_id))
            .await
            .unwrap();
        sqlx::query("UPDATE workflows SET generation = generation + 1 WHERE workflow_id = ?1")
            .bind(i64::try_from(workflow_id.0).unwrap())
            .execute(&repo.workflow_repo.pool)
            .await
            .unwrap();
        assert!(matches!(
            repo.load_state(workflow_id).await,
            Err(DbError::Serialization(message))
                if message.contains("head disagrees with aggregate")
        ));
    }

    #[tokio::test]
    async fn terminal_proposal_invalidates_unclaimed_observation_effect() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(18);
        let registered = repo
            .commit_wake_command(&register(workflow_id))
            .await
            .unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = registered else {
            panic!("registration should apply")
        };
        let (proposal, _) = propose_cancel(&state, workflow_id);
        repo.commit_wake_command(&proposal).await.unwrap();
        let status: String = sqlx::query_scalar(
            "SELECT status FROM workflow_effects
             WHERE workflow_id = ?1 AND kind = 'begin_observation'",
        )
        .bind(i64::try_from(workflow_id.0).unwrap())
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert_eq!(status, "Invalidated");
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
        start_observation_attempt(&repo, WorkflowId(10)).await;
        let (proposal, proof) = propose_cancel(&state, WorkflowId(10));
        let proposed = repo.commit_wake_command(&proposal).await.unwrap();
        let CommitWakeCommandOutcome::Applied {
            state: proposed_state,
            ..
        } = proposed
        else {
            panic!("proposal should apply")
        };
        let attempt_status: String = sqlx::query_scalar(
            "SELECT status FROM workflow_attempts
             WHERE workflow_id = 10 AND attempt_id = 1",
        )
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert_eq!(attempt_status, "AuthorityLost");
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
        let begun = repo
            .workflow_repo
            .begin_attempt(&BeginAttemptInput {
                workflow_id: WorkflowId(10),
                effect_id: EffectId(u64::try_from(fence_effect_id).unwrap()),
                attempt_id: phoenix_workflow::AttemptId(2),
                process_incarnation: ProcessIncarnation(1),
                now: Timestamp(6),
                lease_until: Some(phoenix_workflow::LeaseExpiry(i64::MAX as u64)),
            })
            .await
            .unwrap();
        sqlx::query(
            "UPDATE workflow_reclaimable_leases SET lease_until = 0
             WHERE workflow_id = 10 AND attempt_id = 2",
        )
        .execute(&repo.workflow_repo.pool)
        .await
        .unwrap();
        let expired_receipt = repo
            .accept_fence_receipt(&AcceptReceiptInput {
                authority: begun.authority.clone().unwrap(),
                receipt_id: ReceiptId(98),
                delivery_id: DeliveryId(98),
                attempt_id: Some(phoenix_workflow::AttemptId(2)),
                origin: ReceiptOrigin::Execution,
                receipt_codec: codec(),
                receipt_payload: vec![0],
                receipt_event_codec: codec(),
                receipt_event_payload: vec![0],
                receipt_event_requires_runtime_acceptance: false,
                request_runtime_acceptance_for_cancellation: false,
            })
            .await
            .unwrap();
        assert_eq!(expired_receipt.outcome, AuthorityOutcome::StaleAuthority);
        sqlx::query(
            "UPDATE workflow_reclaimable_leases SET lease_until = ?1
             WHERE workflow_id = 10 AND attempt_id = 2",
        )
        .bind(i64::MAX)
        .execute(&repo.workflow_repo.pool)
        .await
        .unwrap();
        let receipt = repo
            .accept_fence_receipt(&AcceptReceiptInput {
                authority: begun.authority.unwrap(),
                receipt_id: ReceiptId(99),
                delivery_id: DeliveryId(99),
                attempt_id: Some(phoenix_workflow::AttemptId(2)),
                origin: ReceiptOrigin::Execution,
                receipt_codec: codec(),
                receipt_payload: vec![0],
                receipt_event_codec: codec(),
                receipt_event_payload: vec![0],
                receipt_event_requires_runtime_acceptance: false,
                request_runtime_acceptance_for_cancellation: false,
            })
            .await
            .unwrap();
        assert_eq!(receipt.outcome, AuthorityOutcome::Authorized);
        assert!(receipt.delivery.is_none());
        assert_eq!(table_count(&repo, "deliveries", 10).await, 0);
        assert!(matches!(
            repo.finalize_receipted_proposal(WorkflowId(10), TransitionId(3))
                .await
                .unwrap(),
            CommitWakeCommandOutcome::Applied { .. }
        ));
        let cancellation_delivery: (String, Option<String>) = sqlx::query_as(
            "SELECT status, runtime_acceptance_status
             FROM workflow_deliveries WHERE workflow_id = 10",
        )
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert_eq!(cancellation_delivery, ("Accepted".into(), None));
    }

    #[tokio::test]
    async fn earlier_terminal_evidence_supersedes_cancellation_without_a_fence_receipt() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let registered = repo
            .commit_wake_command(&register(WorkflowId(11)))
            .await
            .unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = registered else {
            panic!("registration should apply")
        };
        persist_observation_before_fence(&repo, WorkflowId(11)).await;
        let WakeState::Present(contract) = &state else {
            unreachable!("registration produced a present contract")
        };
        let prior_authority = phoenix_workflow::wake_contract::WakeObservationAuthority::for_test(
            contract,
            phoenix_workflow::AttemptId(1),
        );
        let (proposal, _) = propose_cancel(&state, WorkflowId(11));
        let proposed = repo.commit_wake_command(&proposal).await.unwrap();
        let CommitWakeCommandOutcome::Applied {
            state: proposed_state,
            ..
        } = proposed
        else {
            panic!("proposal should apply")
        };

        assert!(matches!(
            repo.commit_wake_command(&observe_with_authority(
                &proposed_state,
                WorkflowId(11),
                TransitionId(3),
                Timestamp(19),
                prior_authority,
            ))
            .await
            .unwrap(),
            CommitWakeCommandOutcome::Applied {
                state: WakeState::Present(phoenix_workflow::wake_contract::WakeContract {
                    lifecycle: phoenix_workflow::wake_contract::WakeLifecycle::Closed(
                        phoenix_workflow::wake_contract::CanonicalTerminal::Fired { .. }
                    ),
                    ..
                }),
                ..
            }
        ));
        assert!(matches!(
            restarted.load_state(WorkflowId(11)).await.unwrap(),
            WakeState::Present(phoenix_workflow::wake_contract::WakeContract {
                lifecycle: phoenix_workflow::wake_contract::WakeLifecycle::Closed(
                    phoenix_workflow::wake_contract::CanonicalTerminal::Fired { .. }
                ),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn earlier_resource_loss_supersedes_cancellation_without_a_fence_receipt() {
        let (_dir, repo, restarted) = open_repo_pair().await;
        let registered = repo
            .commit_wake_command(&register(WorkflowId(12)))
            .await
            .unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = registered else {
            panic!("registration should apply")
        };
        let (proposal, _) = propose_cancel(&state, WorkflowId(12));
        let proposed = repo.commit_wake_command(&proposal).await.unwrap();
        let CommitWakeCommandOutcome::Applied {
            state: proposed_state,
            ..
        } = proposed
        else {
            panic!("proposal should apply")
        };
        let WakeState::Present(contract) = &proposed_state else {
            panic!("proposal should preserve the contract")
        };
        let unavailable = CommitWakeCommandInput {
            workflow_id: WorkflowId(12),
            command: WakeCommand {
                transition_id: TransitionId(3),
                kind: WakeCommandKind::Reconcile {
                    expected_head: contract.head(),
                    observation:
                        phoenix_workflow::wake_contract::ReconcileObservation::ResourceUnavailable {
                            authority:
                                phoenix_workflow::wake_contract::WakeRecoveryAuthority::for_test(
                                    contract,
                                ),
                            cause: ForgottenCause::CascadeDestroyedHandle,
                            occurred_at: Timestamp(19),
                        },
                },
            },
        };

        assert!(matches!(
            repo.commit_wake_command(&unavailable).await.unwrap(),
            CommitWakeCommandOutcome::Applied {
                state: WakeState::Present(phoenix_workflow::wake_contract::WakeContract {
                    lifecycle: phoenix_workflow::wake_contract::WakeLifecycle::Closed(
                        phoenix_workflow::wake_contract::CanonicalTerminal::Forgotten { .. }
                    ),
                    ..
                }),
                ..
            }
        ));
        assert!(matches!(
            restarted.load_state(WorkflowId(12)).await.unwrap(),
            WakeState::Present(phoenix_workflow::wake_contract::WakeContract {
                lifecycle: phoenix_workflow::wake_contract::WakeLifecycle::Closed(
                    phoenix_workflow::wake_contract::CanonicalTerminal::Forgotten { .. }
                ),
                ..
            })
        ));
        let forgotten_reason: String = sqlx::query_scalar(
            "SELECT forgotten_reason FROM wake_contract_identity_bindings WHERE workflow_id = 12",
        )
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert_eq!(forgotten_reason, "CascadeDestroyedHandle");
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
        start_observation_attempt(&repo, WorkflowId(5)).await;
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
        let workflow_status: String =
            sqlx::query_scalar("SELECT status FROM workflows WHERE workflow_id = 5")
                .fetch_one(&repo.workflow_repo.pool)
                .await
                .unwrap();
        assert_eq!(workflow_status, "Active");
        let acceptance_status: String = sqlx::query_scalar(
            "SELECT runtime_acceptance_status FROM workflow_deliveries WHERE workflow_id = 5",
        )
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert_eq!(acceptance_status, "Owed");
        assert!(matches!(
            restarted.load_state(WorkflowId(5)).await.unwrap(),
            WakeState::Present(phoenix_workflow::wake_contract::WakeContract {
                lifecycle: phoenix_workflow::wake_contract::WakeLifecycle::Closed(_),
                ..
            })
        ));
        let authority: (i64, String, String, String, String) = sqlx::query_as(
            "SELECT version, delivery_owner, registering_tool_use_id,
                    delivery_transferability, lifecycle_kind
             FROM wake_contract_identity_bindings WHERE workflow_id = 5",
        )
        .fetch_one(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert_eq!(
            authority,
            (
                2,
                "conversation".into(),
                "tool-use".into(),
                "WorkScope".into(),
                "Fired".into(),
            )
        );
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
        start_observation_attempt(&repo, WorkflowId(6)).await;
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
    async fn expired_observation_lease_cannot_terminalize() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(14);
        let registered = repo
            .commit_wake_command(&register(workflow_id))
            .await
            .unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = registered else {
            panic!("registration should apply")
        };
        start_observation_attempt(&repo, workflow_id).await;
        sqlx::query(
            "UPDATE workflow_reclaimable_leases SET lease_until = 0 WHERE workflow_id = ?1",
        )
        .bind(i64::try_from(workflow_id.0).unwrap())
        .execute(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert!(matches!(
            repo.commit_wake_command(&observe(&state, workflow_id))
                .await
                .unwrap(),
            CommitWakeCommandOutcome::Rejected(WakeRejection::ObservationAuthorityMismatch)
        ));
    }

    #[tokio::test]
    async fn exact_durable_observation_survives_lease_expiry_before_cleanup() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(19);
        let registered = repo
            .commit_wake_command(&register(workflow_id))
            .await
            .unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = registered else {
            panic!("registration should apply")
        };
        persist_observation_before_fence(&repo, workflow_id).await;
        sqlx::query(
            "INSERT INTO workflow_authoritative_observations
             SELECT workflow_id, 2, effect_id, attempt_id, declared_workflow_version, generation,
                    process_incarnation, observation_codec_family, observation_codec_version,
                    observation_payload, observed_at, recorded_at
             FROM workflow_authoritative_observations
             WHERE workflow_id = ?1 AND observation_id = 1",
        )
        .bind(i64::try_from(workflow_id.0).unwrap())
        .execute(&repo.workflow_repo.pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE workflow_reclaimable_leases SET lease_until = 0 WHERE workflow_id = ?1",
        )
        .bind(i64::try_from(workflow_id.0).unwrap())
        .execute(&repo.workflow_repo.pool)
        .await
        .unwrap();
        assert!(matches!(
            repo.commit_wake_command(&observe_at(
                &state,
                workflow_id,
                TransitionId(2),
                Timestamp(19),
            ))
            .await
            .unwrap(),
            CommitWakeCommandOutcome::Applied { .. }
        ));
    }

    #[tokio::test]
    async fn fenced_observation_must_exactly_match_the_durable_evidence() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(15);
        let registered = repo
            .commit_wake_command(&register(workflow_id))
            .await
            .unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = registered else {
            panic!("registration should apply")
        };
        persist_observation_before_fence(&repo, workflow_id).await;
        let (proposal, _) = propose_cancel(&state, workflow_id);
        let proposed = repo.commit_wake_command(&proposal).await.unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = proposed else {
            panic!("cancellation proposal should apply")
        };
        assert!(matches!(
            repo.commit_wake_command(&observe_at(
                &state,
                workflow_id,
                TransitionId(3),
                Timestamp(18),
            ))
            .await
            .unwrap(),
            CommitWakeCommandOutcome::Rejected(WakeRejection::ObservationAuthorityMismatch)
        ));
    }

    #[tokio::test]
    async fn persisted_protocol_failure_survives_cancellation_fencing() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(16);
        let registered = repo
            .commit_wake_command(&register(workflow_id))
            .await
            .unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = registered else {
            panic!("registration should apply")
        };
        let evidence = phoenix_workflow::wake_contract::ProtocolFailureEvidence {
            cause: ForgottenCause::CascadeDestroyedHandle,
            occurred_at: Timestamp(19),
        };
        persist_protocol_failure_before_fence(&repo, workflow_id, &evidence).await;
        let WakeState::Present(contract) = &state else {
            unreachable!("registration produced a present contract")
        };
        let prior_authority = phoenix_workflow::wake_contract::WakeObservationAuthority::for_test(
            contract,
            phoenix_workflow::AttemptId(1),
        );
        let (proposal, _) = propose_cancel(&state, workflow_id);
        let proposed = repo.commit_wake_command(&proposal).await.unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = proposed else {
            panic!("cancellation proposal should apply")
        };
        let WakeState::Present(contract) = &state else {
            unreachable!("proposal produced a present contract")
        };
        let reconcile = CommitWakeCommandInput {
            workflow_id,
            command: WakeCommand {
                transition_id: TransitionId(3),
                kind: WakeCommandKind::Reconcile {
                    expected_head: contract.head(),
                    observation:
                        phoenix_workflow::wake_contract::ReconcileObservation::ProtocolFailure {
                            authority: prior_authority,
                            cause: evidence.cause,
                            occurred_at: evidence.occurred_at,
                        },
                },
            },
        };
        assert!(matches!(
            repo.commit_wake_command(&reconcile).await.unwrap(),
            CommitWakeCommandOutcome::Applied {
                state: WakeState::Present(phoenix_workflow::wake_contract::WakeContract {
                    lifecycle: phoenix_workflow::wake_contract::WakeLifecycle::Closed(
                        phoenix_workflow::wake_contract::CanonicalTerminal::Forgotten { .. }
                    ),
                    ..
                }),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn closed_delivery_transfer_replays_against_the_original_terminal_bundle() {
        let (_dir, repo, _) = open_repo_pair().await;
        let workflow_id = WorkflowId(13);
        let registered = repo
            .commit_wake_command(&register(workflow_id))
            .await
            .unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = registered else {
            panic!("registration should apply")
        };
        start_observation_attempt(&repo, workflow_id).await;
        let terminalized = repo
            .commit_wake_command(&observe(&state, workflow_id))
            .await
            .unwrap();
        let CommitWakeCommandOutcome::Applied { state, .. } = terminalized else {
            panic!("terminal observation should apply")
        };
        let transfer = transfer(&state, workflow_id, TransitionId(3));
        assert!(matches!(
            repo.commit_wake_command(&transfer).await.unwrap(),
            CommitWakeCommandOutcome::Applied { .. }
        ));
        assert!(matches!(
            repo.commit_wake_command(&transfer).await.unwrap(),
            CommitWakeCommandOutcome::Replayed { .. }
        ));
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
