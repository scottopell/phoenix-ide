use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::runtime::RuntimeManager;
use phoenix_core::work_scope::WorkScope;
use phoenix_db::workflow::wake::{
    MaterializePendingDeliveryMessageInput, MaterializePendingDeliveryMessageOutcome,
    WakeAdoptMaterializedPendingOutcome, WakeCancelIfUnresolvedInput, WakeCancellationOutcome,
    WakeForgetIfUnresolvedInput, WakeObservationCandidateRow, WakeObservationOutcome,
    WakePendingDelivery, WakePendingGlobalCursor, WakeRegistrationOutcome, WakeRepository,
    WakeTerminalEvidenceInput, WakeTerminalEvidenceOutcome,
};
use phoenix_db::workflow::LocalAttemptAuthority;
use phoenix_tools::bash::handle::{FinalCause, HandleState};
use phoenix_tools::{CancelWakeInput, RegisterWakeInput, RegisteredWake, WakeRegistrar};
use phoenix_workflow::wake_profile::{
    BashTerminalEvidence, BashTerminalStatus, TmuxCompletionPolicy, TmuxTerminalEvidence,
    TmuxTerminalStatus, WakeForgottenReason, WakeResourceIdentity, WakeTerminalEvidence,
};
use phoenix_workflow::{LeaseExpiry, ProcessIncarnation, Timestamp};
use tokio::sync::watch;

const OBSERVATION_BATCH_LIMIT: usize = 64;
const EXPIRY_BATCH_LIMIT: usize = 64;
const LEASE_DURATION: Duration = Duration::from_secs(30);
const EMPTY_RESCAN_INTERVAL: Duration = Duration::from_secs(5);
const ERROR_RETRY_BASE_INTERVAL: Duration = Duration::from_millis(250);
const ERROR_RETRY_MAX_INTERVAL: Duration = Duration::from_secs(5);

fn fresh_process_incarnation() -> ProcessIncarnation {
    super::process_incarnation()
}

#[derive(Clone)]
pub(crate) struct ProductionWakeRegistrar {
    repo: WakeRepository,
    kick_tx: watch::Sender<u64>,
}

impl ProductionWakeRegistrar {
    pub(crate) fn new(repo: WakeRepository, kick_tx: watch::Sender<u64>) -> Self {
        Self { repo, kick_tx }
    }

    fn kick(&self) {
        self.kick_tx
            .send_modify(|value| *value = value.wrapping_add(1));
    }
}

#[async_trait]
impl WakeRegistrar for ProductionWakeRegistrar {
    async fn register(&self, input: RegisterWakeInput) -> Result<RegisteredWake, String> {
        let now = Timestamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );
        let prepared_fingerprint = input.prepared_fingerprint.clone();
        let intent = input.into_intent(now);
        let outcome = self
            .repo
            .register(&intent, &prepared_fingerprint, now)
            .await
            .map_err(|e| e.to_string())?;
        self.kick();
        Ok(match outcome {
            WakeRegistrationOutcome::Registered { workflow_id, .. } => {
                RegisteredWake::Registered { workflow_id }
            }
            WakeRegistrationOutcome::Replayed { workflow_id, .. } => {
                RegisteredWake::Replayed { workflow_id }
            }
            WakeRegistrationOutcome::Conflict => RegisteredWake::Conflict,
        })
    }

    async fn cancel(&self, input: CancelWakeInput) -> Result<RegisteredWake, String> {
        let outcome = self
            .repo
            .cancel_allocated(&WakeCancelIfUnresolvedInput {
                workflow_id: input.workflow_id,
                expected_conversation_id: None,
                expected_contract_id: None,
                timestamp: input.timestamp,
                reason: input.reason,
            })
            .await
            .map_err(|e| e.to_string())?;
        self.kick();
        Ok(match outcome {
            WakeCancellationOutcome::Cancelled { .. } => RegisteredWake::Cancelled,
            WakeCancellationOutcome::Replayed { .. } => RegisteredWake::CancelReplayed,
            WakeCancellationOutcome::Stale => RegisteredWake::CancelStale,
        })
    }
}

pub(crate) async fn run(
    manager: Arc<RuntimeManager>,
    kick_rx: watch::Receiver<u64>,
    ready_tx: tokio::sync::oneshot::Sender<()>,
) {
    let worker = WakeWorker::new(
        WakeRepository::new(manager.db().pool().clone()),
        Arc::new(RuntimeRegistryInspector::new(
            manager.bash_handles().clone(),
            manager.tmux_registry().clone(),
        )),
        Arc::new(SystemClock),
        fresh_process_incarnation(),
    );
    if let Err(error) = worker
        .run_loop_with_manager(kick_rx, manager, ready_tx)
        .await
    {
        tracing::warn!(error = %error, "wake worker stopped after DB/inspection error");
    }
}

#[derive(Clone)]
pub(crate) struct WakeWorker<I: TerminalInspector, C: WakeClock> {
    repo: WakeRepository,
    inspector: Arc<I>,
    clock: Arc<C>,
    process_incarnation: ProcessIncarnation,
}

impl<I: TerminalInspector, C: WakeClock> WakeWorker<I, C> {
    pub(crate) fn new(
        repo: WakeRepository,
        inspector: Arc<I>,
        clock: Arc<C>,
        process_incarnation: ProcessIncarnation,
    ) -> Self {
        Self {
            repo,
            inspector,
            clock,
            process_incarnation,
        }
    }

    async fn run_loop_with_manager(
        &self,
        mut kick_rx: watch::Receiver<u64>,
        manager: Arc<RuntimeManager>,
        ready_tx: tokio::sync::oneshot::Sender<()>,
    ) -> Result<(), String> {
        self.repo
            .reconcile_continuation_transfers(self.clock.now())
            .await
            .map_err(|error| error.to_string())?;
        self.run_once().await?;
        phoenix_db::WorkflowRepository::new(manager.db().pool().clone())
            .reclaim_workflow_delivery_claims(super::process_incarnation())
            .await
            .map_err(|error| error.to_string())?;
        phoenix_db::WorkflowRepository::new(manager.db().pool().clone())
            .interrupt_begun_top_level_llm_tools(super::process_incarnation())
            .await
            .map_err(|error| error.to_string())?;
        recover_top_level_llm_attempts(&manager).await?;
        deliver_owed_top_level_llm_receipts(&manager).await?;
        deliver_owed_top_level_llm_tools(&manager).await?;
        deliver_pending(&manager, &self.repo, self.clock.now()).await?;
        deliver_pending_direct_turns(&manager).await?;
        let _ = ready_tx.send(());
        self.run_loop_inner(&mut kick_rx, Some(manager)).await
    }

    async fn run_loop_inner(
        &self,
        kick_rx: &mut watch::Receiver<u64>,
        manager: Option<Arc<RuntimeManager>>,
    ) -> Result<(), String> {
        let mut error_backoff = ERROR_RETRY_BASE_INTERVAL;
        loop {
            let wait = match self.run_once().await {
                Ok(wait) => {
                    if let Some(manager) = manager.as_ref() {
                        if let Err(error) = recover_top_level_llm_attempts(manager).await {
                            tracing::warn!(error = %error, "top-level LLM recovery failed; retrying");
                        }
                        if let Err(error) = deliver_owed_top_level_llm_receipts(manager).await {
                            tracing::warn!(error = %error, "owed LLM receipt delivery failed; retrying");
                        }
                        if let Err(error) = deliver_owed_top_level_llm_tools(manager).await {
                            tracing::warn!(error = %error, "owed LLM tool delivery failed; retrying");
                        }
                        if let Err(error) = deliver_pending_direct_turns(manager).await {
                            tracing::warn!(error = %error, retry_in = ?error_backoff, "direct-turn recovery failed; retrying");
                        }
                        if let Err(error) =
                            deliver_pending(manager, &self.repo, self.clock.now()).await
                        {
                            tracing::warn!(error = %error, retry_in = ?error_backoff, "wake worker delivery failed; retrying");
                            let wait = error_backoff;
                            error_backoff =
                                (error_backoff.saturating_mul(2)).min(ERROR_RETRY_MAX_INTERVAL);
                            wait
                        } else {
                            error_backoff = ERROR_RETRY_BASE_INTERVAL;
                            wait
                        }
                    } else {
                        error_backoff = ERROR_RETRY_BASE_INTERVAL;
                        wait
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, retry_in = ?error_backoff, "wake worker iteration failed; retrying");
                    let wait = error_backoff;
                    error_backoff = (error_backoff.saturating_mul(2)).min(ERROR_RETRY_MAX_INTERVAL);
                    wait
                }
            };
            let sleep = self.clock.sleep(wait);
            tokio::pin!(sleep);
            tokio::select! {
                () = &mut sleep => {}
                changed = kick_rx.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }

    #[cfg(test)]
    async fn run_loop(&self, mut kick_rx: watch::Receiver<u64>) -> Result<(), String> {
        self.run_loop_inner(&mut kick_rx, None).await
    }

    async fn run_once(&self) -> Result<Duration, String> {
        let now = self.clock.now();
        let next_wait = self.observe_candidates(now).await?;
        self.expire_due(now).await?;
        Ok(next_wait)
    }

    async fn expire_due(&self, now: Timestamp) -> Result<(), String> {
        let expired = self
            .repo
            .list_expired_unresolved(now, EXPIRY_BATCH_LIMIT)
            .await
            .map_err(|e| e.to_string())?;
        for row in expired {
            if let Err(error) = self.repo.expire_if_unresolved(row.workflow_id, now).await {
                tracing::warn!(workflow_id = row.workflow_id.0, error = %error, "wake expiry failed for one contract; continuing");
            }
        }
        Ok(())
    }

    async fn observe_candidates(&self, now: Timestamp) -> Result<Duration, String> {
        let mut next_wait = EMPTY_RESCAN_INTERVAL;
        let mut saw_candidate = false;
        let mut cursor = None;
        loop {
            let candidates = self
                .repo
                .list_observation_candidates(now, cursor, OBSERVATION_BATCH_LIMIT)
                .await
                .map_err(|e| e.to_string())?;
            let page_len = candidates.len();
            for candidate in candidates {
                cursor = Some(candidate.workflow_id);
                saw_candidate = true;
                let claim_until = LeaseExpiry(
                    now.0
                        .saturating_add(LEASE_DURATION.as_secs())
                        .min(candidate.expires_at.0.saturating_add(1)),
                );
                match self
                    .repo
                    .claim_observation_if_eligible(
                        candidate.workflow_id,
                        self.process_incarnation,
                        now,
                        claim_until,
                    )
                    .await
                    .map_err(|e| e.to_string())?
                {
                    WakeObservationOutcome::Started { canonical } => {
                        let workflow_id = candidate.workflow_id.0;
                        let Some(authority) = canonical.authority else {
                            continue;
                        };
                        let Some(_attempt) = canonical.attempt else {
                            continue;
                        };
                        match self
                            .inspect_candidate(candidate, authority, now, claim_until)
                            .await
                        {
                            Ok(wait) => {
                                next_wait = next_wait.min(wait);
                            }
                            Err(error) => {
                                tracing::warn!(workflow_id, error = %error, "wake inspection failed for one contract; continuing");
                            }
                        }
                    }
                    WakeObservationOutcome::Busy { lease_until } => {
                        next_wait = next_wait.min(duration_until(now, lease_until.0));
                    }
                    WakeObservationOutcome::Ineligible => {}
                }
            }
            if page_len < OBSERVATION_BATCH_LIMIT {
                break;
            }
        }
        Ok(if saw_candidate {
            next_wait.min(LEASE_DURATION)
        } else {
            next_wait
        })
    }

    async fn inspect_candidate(
        &self,
        candidate: WakeObservationCandidateRow,
        authority: LocalAttemptAuthority,
        now: Timestamp,
        _lease_until: LeaseExpiry,
    ) -> Result<Duration, String> {
        let Some(binding) = self
            .repo
            .reload_binding(candidate.workflow_id)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(Duration::ZERO);
        };
        match self
            .inspector
            .inspect(&binding, &authority, now)
            .await
            .map_err(|error| error.clone())?
        {
            InspectionOutcome::LiveRetry => Ok(LEASE_DURATION),
            InspectionOutcome::Terminal(evidence) => {
                let observation_time = self.clock.now();
                let outcome = self
                    .repo
                    .record_terminal_allocated(&WakeTerminalEvidenceInput {
                        workflow_id: candidate.workflow_id,
                        authority,
                        observation_time,
                        evidence: evidence.clone(),
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                if matches!(
                    outcome,
                    WakeTerminalEvidenceOutcome::Recorded { .. }
                        | WakeTerminalEvidenceOutcome::Replayed { .. }
                ) {
                    self.inspector
                        .cleanup_after_commit(&binding, &evidence)
                        .await?;
                }
                Ok(Duration::ZERO)
            }
            InspectionOutcome::Forgotten(reason) => {
                let _ = self
                    .repo
                    .forget_if_unresolved_allocated(&WakeForgetIfUnresolvedInput {
                        workflow_id: candidate.workflow_id,
                        now,
                        reason,
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(Duration::ZERO)
            }
        }
    }
}

async fn deliver_owed_top_level_llm_tools(manager: &Arc<RuntimeManager>) -> Result<(), String> {
    let repo = phoenix_db::WorkflowRepository::new(manager.db().pool().clone());
    for conversation_id in repo
        .load_conversations_with_owed_top_level_llm_tools()
        .await
        .map_err(|error| error.to_string())?
    {
        let handle = match manager.get_or_create(&conversation_id).await {
            Ok(handle) => handle,
            Err(error) => {
                tracing::warn!(conversation_id, error = %error, "owed LLM tool runtime unavailable");
                continue;
            }
        };
        if !matches!(
            *handle.state_rx.borrow(),
            crate::state_machine::ConvState::ToolExecuting { .. }
        ) {
            continue;
        }
        if let Err(error) = handle
            .event_tx
            .send(phoenix_core::domain::sm_event::Event::ResumeDurableToolExecution)
            .await
        {
            tracing::warn!(conversation_id, error = %error, "owed LLM tool runtime channel closed");
        }
    }
    Ok(())
}

async fn deliver_owed_top_level_llm_receipts(manager: &Arc<RuntimeManager>) -> Result<(), String> {
    let repo = phoenix_db::WorkflowRepository::new(manager.db().pool().clone());
    for owed in repo
        .load_owed_top_level_llm_receipts()
        .await
        .map_err(|error| error.to_string())?
    {
        let incarnation = super::process_incarnation();
        if !repo
            .claim_workflow_delivery(
                owed.workflow.workflow_id,
                owed.delivery.delivery_id,
                incarnation,
                Timestamp(u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)),
            )
            .await
            .map_err(|error| error.to_string())?
        {
            continue;
        }
        let durable: phoenix_llm::DurableLlmResponse =
            serde_json::from_str(&owed.llm_receipt.response_aggregate)
                .map_err(|error| error.to_string())?;
        let handle = match manager.get_or_create(&owed.workflow.conversation_id).await {
            Ok(handle) => handle,
            Err(error) => {
                repo.release_workflow_delivery_claim(
                    owed.workflow.workflow_id,
                    owed.delivery.delivery_id,
                    incarnation,
                )
                .await
                .map_err(|release_error| release_error.to_string())?;
                tracing::warn!(%error, "owed LLM receipt runtime unavailable");
                continue;
            }
        };
        let event = phoenix_core::domain::sm_event::Event::LlmResponse {
            tool_calls: durable
                .response
                .tool_uses()
                .into_iter()
                .map(
                    |(id, name, input)| phoenix_core::domain::sm_state::ToolCall {
                        id: id.to_string(),
                        input: phoenix_core::domain::sm_state::ToolInput::from_name_and_value(
                            name,
                            input.clone(),
                        ),
                    },
                )
                .collect(),
            content: durable.response.content,
            end_turn: durable.response.end_turn,
            usage: durable.response.usage,
            request_id: owed
                .llm_receipt
                .provider_request_id
                .unwrap_or_else(|| format!("llm-receipt-{}", owed.receipt.receipt_id.0)),
        };
        if let Err(error) = handle.event_tx.send(event).await {
            repo.release_workflow_delivery_claim(
                owed.workflow.workflow_id,
                owed.delivery.delivery_id,
                incarnation,
            )
            .await
            .map_err(|release_error| release_error.to_string())?;
            tracing::warn!(error = %error, "owed LLM receipt runtime channel closed");
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn recover_top_level_llm_attempts(manager: &Arc<RuntimeManager>) -> Result<(), String> {
    use sha2::Digest as _;

    let repo = phoenix_db::WorkflowRepository::new(manager.db().pool().clone());
    for recovery in repo
        .recover_top_level_llm_attempts(super::process_incarnation())
        .await
        .map_err(|error| error.to_string())?
    {
        let durable_request: phoenix_llm::DurableLlmRequest =
            serde_json::from_str(&recovery.prepared_request.request_aggregate)
                .map_err(|error| error.to_string())?;
        let llm = manager
            .model_registry()
            .get(&recovery.prepared_request.model);
        let begun = repo
            .begin_recovered_top_level_llm_attempt(
                recovery.workflow.workflow_id,
                recovery.prepared_request.effect_id,
                super::process_incarnation(),
                Timestamp(u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)),
            )
            .await
            .map_err(|error| error.to_string())?;
        let Some(authority) = begun.authority else {
            continue;
        };
        let Some(llm) = llm else {
            let attempt = u32::try_from(authority.attempt_id.0).unwrap_or(u32::MAX);
            let message = format!(
                "Recovered LLM model '{}' is unavailable",
                recovery.prepared_request.model
            );
            repo.record_top_level_llm_failure(&phoenix_db::RecordTopLevelLlmFailureInput {
                authority,
                observed_at: Timestamp(
                    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0),
                ),
                outcome_payload: message.as_bytes().to_vec(),
            })
            .await
            .map_err(|error| error.to_string())?;
            let handle = manager
                .get_or_create(&recovery.workflow.conversation_id)
                .await?;
            if let Err(error) = handle
                .event_tx
                .send(phoenix_core::domain::sm_event::Event::LlmError {
                    message,
                    error_kind: phoenix_core::domain::db_schema::ErrorKind::InvalidRequest,
                    attempt,
                    recovery_in_progress: false,
                    resets_at: None,
                })
                .await
            {
                tracing::warn!(%error, "unavailable recovered model runtime channel closed");
            }
            continue;
        };
        let request_id = uuid::Uuid::new_v4().to_string();
        let request = durable_request.into_attempt(phoenix_llm::LlmRequestTelemetry {
            conversation_id: recovery.workflow.conversation_id.clone(),
            root_conversation_id: recovery.workflow.conversation_id.clone(),
            request_id: request_id.clone(),
            retry_attempt: u32::try_from(authority.attempt_id.0).unwrap_or(u32::MAX),
            attempt_capture: phoenix_llm::LlmAttemptCapture::default(),
        });
        match llm.complete(&request).await {
            Ok(response) => {
                let response_usage = response.usage.clone();
                let aggregate = serde_json::to_string(&phoenix_llm::DurableLlmResponse {
                    response: response.clone(),
                })
                .map_err(|error| error.to_string())?;
                let fingerprint = sha2::Sha256::digest(aggregate.as_bytes()).iter().fold(
                    String::with_capacity(64),
                    |mut output, byte| {
                        use std::fmt::Write as _;
                        write!(output, "{byte:02x}").expect("writing to String cannot fail");
                        output
                    },
                );
                let tool_intents = response
                    .tool_uses()
                    .into_iter()
                    .enumerate()
                    .map(
                        |(ordinal, (id, name, arguments))| phoenix_db::ToolIntentRecord {
                            intent_ordinal: u32::try_from(ordinal).unwrap_or(u32::MAX),
                            status: phoenix_db::ToolIntentStatus::PendingAcceptance,
                            tool_name: name.to_string(),
                            tool_kind: phoenix_db::ToolKindRecord::Function,
                            tool_use_id: id.to_string(),
                            arguments_json: arguments.to_string(),
                        },
                    )
                    .collect();
                let acceptance = phoenix_db::AcceptCompleteLlmResponseInput {
                    authority,
                    delivery_id: None,
                    receipt_id: None,
                    response: phoenix_workflow::llm_profile::CompleteLlmResponse {
                        codec_version: phoenix_llm::DURABLE_LLM_RESPONSE_CODEC_VERSION,
                        response_fingerprint: fingerprint,
                        response_aggregate: aggregate,
                    },
                    provider_request_id: Some(request_id),
                    tool_intents,
                    local_delivery_claim: None,
                };
                let persistence_outcome = loop {
                    let outcome = repo
                        .accept_complete_top_level_llm_response(&acceptance)
                        .await
                        .map_err(|error| error.to_string())?;
                    if outcome.outcome
                        != phoenix_db::CompleteLlmResponsePersistenceOutcome::RetryablePersistence
                    {
                        break outcome.outcome;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                };
                if persistence_outcome
                    == phoenix_db::CompleteLlmResponsePersistenceOutcome::Accepted
                {
                    let root_conversation_id = super::find_root_conversation_id(
                        manager.db(),
                        &recovery.workflow.conversation_id,
                    )
                    .await;
                    if let Err(error) = manager
                        .db()
                        .insert_turn_usage(
                            &recovery.workflow.conversation_id,
                            &root_conversation_id,
                            &recovery.prepared_request.model,
                            &response_usage,
                            None,
                        )
                        .await
                    {
                        tracing::warn!(%error, "failed to record recovered LLM usage");
                    }
                }
            }
            Err(error) => {
                let error_kind = super::executor::llm_error_to_db_error(error.kind);
                let recovery_in_progress = error.recovery_in_progress;
                let resets_at = error.quota.as_ref().and_then(|quota| quota.resets_at);
                let message = error.message;
                let attempt = u32::try_from(authority.attempt_id.0).unwrap_or(u32::MAX);
                repo.record_top_level_llm_failure(&phoenix_db::RecordTopLevelLlmFailureInput {
                    authority,
                    observed_at: Timestamp(
                        u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0),
                    ),
                    outcome_payload: message.as_bytes().to_vec(),
                })
                .await
                .map_err(|error| error.to_string())?;
                let handle = manager
                    .get_or_create(&recovery.workflow.conversation_id)
                    .await?;
                if let Err(send_error) = handle
                    .event_tx
                    .send(phoenix_core::domain::sm_event::Event::LlmError {
                        message,
                        error_kind,
                        attempt,
                        recovery_in_progress,
                        resets_at,
                    })
                    .await
                {
                    tracing::warn!(error = %send_error, "recovered LLM error runtime channel closed");
                }
            }
        }
    }
    Ok(())
}

async fn deliver_pending_direct_turns(manager: &Arc<RuntimeManager>) -> Result<(), String> {
    let repo = phoenix_db::WorkflowRepository::new(manager.db().pool().clone());
    for pending in repo
        .claim_recoverable_direct_turns(super::process_incarnation())
        .await
        .map_err(|error| error.to_string())?
    {
        let event = match pending.committed_outcome {
            phoenix_db::DirectTurnCommittedOutcome::PendingRuntime => {
                let prepared: phoenix_core::domain::sm_event::PreparedDirectTurn =
                    match serde_json::from_str(&pending.prepared_payload) {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            tracing::error!(
                                workflow_id = pending.workflow_id.0,
                                conversation_id = %pending.conversation_id,
                                error = %error,
                                "pending direct turn has an unreadable prepared payload"
                            );
                            continue;
                        }
                    };
                if prepared.codec_version
                    != phoenix_core::domain::sm_event::PREPARED_DIRECT_TURN_CODEC_VERSION
                {
                    tracing::error!(
                        workflow_id = pending.workflow_id.0,
                        codec_version = prepared.codec_version,
                        "pending direct turn uses an unsupported codec version"
                    );
                    continue;
                }
                prepared.into_event()
            }
            phoenix_db::DirectTurnCommittedOutcome::RuntimeAccepted => {
                phoenix_core::domain::sm_event::Event::ResumeDurableLlmRequest
            }
            phoenix_db::DirectTurnCommittedOutcome::QueuedSteering
            | phoenix_db::DirectTurnCommittedOutcome::CancelledSteering => continue,
        };
        let handle = match manager.get_or_create(&pending.conversation_id).await {
            Ok(handle) => handle,
            Err(error) => {
                tracing::error!(
                    workflow_id = pending.workflow_id.0,
                    conversation_id = %pending.conversation_id,
                    error = %error,
                    "pending direct turn runtime could not be constructed"
                );
                repo.release_direct_turn_runtime_delivery(
                    &pending.conversation_id,
                    &pending.client_message_id,
                    super::process_incarnation(),
                )
                .await
                .map_err(|release_error| release_error.to_string())?;
                continue;
            }
        };
        let state_accepts_delivery = match pending.committed_outcome {
            phoenix_db::DirectTurnCommittedOutcome::PendingRuntime => matches!(
                *handle.state_rx.borrow(),
                crate::state_machine::ConvState::Idle
                    | crate::state_machine::ConvState::Error { .. }
            ),
            phoenix_db::DirectTurnCommittedOutcome::RuntimeAccepted => matches!(
                *handle.state_rx.borrow(),
                crate::state_machine::ConvState::LlmRequesting { .. }
            ),
            phoenix_db::DirectTurnCommittedOutcome::QueuedSteering
            | phoenix_db::DirectTurnCommittedOutcome::CancelledSteering => false,
        };
        if !state_accepts_delivery {
            repo.release_direct_turn_runtime_delivery(
                &pending.conversation_id,
                &pending.client_message_id,
                super::process_incarnation(),
            )
            .await
            .map_err(|error| error.to_string())?;
            continue;
        }
        if let Err(error) = handle.event_tx.send(event).await {
            repo.release_direct_turn_runtime_delivery(
                &pending.conversation_id,
                &pending.client_message_id,
                super::process_incarnation(),
            )
            .await
            .map_err(|release_error| release_error.to_string())?;
            tracing::warn!(
                workflow_id = pending.workflow_id.0,
                conversation_id = %pending.conversation_id,
                error = %error,
                "recoverable direct turn runtime channel closed"
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn deliver_pending(
    manager: &Arc<RuntimeManager>,
    repo: &WakeRepository,
    now: Timestamp,
) -> Result<(), String> {
    let mut cursor = None;
    loop {
        let pending = repo
            .list_pending_global(cursor, OBSERVATION_BATCH_LIMIT)
            .await
            .map_err(|error| error.to_string())?;
        if pending.is_empty() {
            break;
        }
        let page_len = pending.len();
        for row in pending {
            let next_cursor = WakePendingGlobalCursor {
                workflow_id: row.workflow_id,
                delivery_id: row.delivery_id,
            };
            let current = repo
                .get_pending_exact(row.workflow_id, row.delivery_id, &row.conversation_id)
                .await
                .map_err(|error| error.to_string())?;
            let Some(current) = current else {
                cursor = Some(next_cursor);
                continue;
            };
            let _chat_acceptance = manager.lock_chat_acceptance().await;
            let rendered = render_terminal_result(&current);
            let display_data = Some(serde_json::json!({
                "type": "wake_result",
                "adopted": false,
                "terminal": &current.receipt.terminal,
            }));
            let auto_resume = !matches!(
                current.receipt.terminal,
                phoenix_workflow::wake_profile::WakeTerminalPayload::Cancelled { .. }
            );
            let handle = match manager.try_get_handle(&current.conversation_id).await {
                Some(handle) => {
                    if !matches!(
                        *handle.state_rx.borrow(),
                        crate::state_machine::ConvState::Idle
                    ) {
                        cursor = Some(next_cursor);
                        continue;
                    }
                    handle
                }
                None => match manager.get_or_create(&current.conversation_id).await {
                    Ok(handle) => handle,
                    Err(error) => {
                        tracing::warn!(
                            workflow_id = current.workflow_id.0,
                            conversation_id = %current.conversation_id,
                            %error,
                            "skipping wake delivery whose conversation runtime could not start"
                        );
                        cursor = Some(next_cursor);
                        continue;
                    }
                },
            };
            let (sequence_guard, sequence_ids) =
                handle.broadcast_tx.reserve_next_persisted_message_range(1);
            let sequence_id = sequence_ids[0];
            match repo
                .materialize_pending_delivery_message(&MaterializePendingDeliveryMessageInput {
                    workflow_id: current.workflow_id,
                    delivery_id: current.canonical_delivery.delivery_id,
                    conversation_id: current.conversation_id.clone(),
                    rendered_content: rendered,
                    display_data,
                    auto_resume,
                    created_at: now,
                    sequence_id: Some(sequence_id),
                })
                .await
                .map_err(|error| error.to_string())?
            {
                MaterializePendingDeliveryMessageOutcome::Materialized(link) => {
                    let _ = handle
                        .broadcast_tx
                        .send_message(link.linked_message.message.clone());
                    let conversation_id = current.conversation_id;
                    match repo
                        .adopt_materialized_pending_for_conversation(&conversation_id, now)
                        .await
                        .map_err(|error| error.to_string())?
                    {
                        WakeAdoptMaterializedPendingOutcome::Adopted(adopted) => {
                            if adopted.auto_resume {
                                handle
                                    .event_tx
                                    .send(crate::state_machine::Event::WakeBatchAdopted)
                                    .await
                                    .map_err(|error| error.to_string())?;
                            }
                        }
                        WakeAdoptMaterializedPendingOutcome::Busy(_)
                        | WakeAdoptMaterializedPendingOutcome::NothingPending
                        | WakeAdoptMaterializedPendingOutcome::NotFullyMaterialized { .. } => {}
                    }
                }
                MaterializePendingDeliveryMessageOutcome::AlreadyMaterialized(_) => {
                    let conversation_id = current.conversation_id;
                    match repo
                        .adopt_materialized_pending_for_conversation(&conversation_id, now)
                        .await
                        .map_err(|error| error.to_string())?
                    {
                        WakeAdoptMaterializedPendingOutcome::Adopted(adopted) => {
                            if adopted.auto_resume {
                                handle
                                    .event_tx
                                    .send(crate::state_machine::Event::WakeBatchAdopted)
                                    .await
                                    .map_err(|error| error.to_string())?;
                            }
                        }
                        WakeAdoptMaterializedPendingOutcome::Busy(_)
                        | WakeAdoptMaterializedPendingOutcome::NothingPending
                        | WakeAdoptMaterializedPendingOutcome::NotFullyMaterialized { .. } => {}
                    }
                }
                MaterializePendingDeliveryMessageOutcome::WrongOwnerOrIneligible => {
                    repo.suppress_pending_for_archived_conversation(&current, now)
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }
            drop(sequence_guard);
            cursor = Some(next_cursor);
        }
        if page_len < OBSERVATION_BATCH_LIMIT {
            break;
        }
    }

    Ok(())
}

fn render_terminal_result(pending: &WakePendingDelivery) -> String {
    serde_json::to_string(&pending.receipt.terminal)
        .unwrap_or_else(|_| "Wake completed; inspect display metadata for details.".to_string())
}

fn duration_until(now: Timestamp, then: u64) -> Duration {
    Duration::from_secs(then.saturating_sub(now.0))
}

pub(crate) trait WakeClock: Send + Sync + 'static {
    fn now(&self) -> Timestamp;
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

struct SystemClock;

impl WakeClock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        )
    }

    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(tokio::time::sleep(duration))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InspectionOutcome {
    LiveRetry,
    Terminal(WakeTerminalEvidence),
    Forgotten(WakeForgottenReason),
}

pub(crate) trait TerminalInspector: Send + Sync + 'static {
    fn inspect<'a>(
        &'a self,
        binding: &'a phoenix_db::workflow::wake::WakeBindingRecord,
        authority: &'a LocalAttemptAuthority,
        observation_time: Timestamp,
    ) -> Pin<Box<dyn Future<Output = Result<InspectionOutcome, String>> + Send + 'a>>;

    fn cleanup_after_commit<'a>(
        &'a self,
        _binding: &'a phoenix_db::workflow::wake::WakeBindingRecord,
        _evidence: &'a WakeTerminalEvidence,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

struct RuntimeRegistryInspector {
    bash: Arc<phoenix_tools::BashHandleRegistry>,
    tmux: Arc<phoenix_tools::TmuxRegistry>,
}

impl RuntimeRegistryInspector {
    fn new(
        bash: Arc<phoenix_tools::BashHandleRegistry>,
        tmux: Arc<phoenix_tools::TmuxRegistry>,
    ) -> Self {
        Self { bash, tmux }
    }
}

fn system_time_to_timestamp(time: std::time::SystemTime) -> Timestamp {
    let seconds = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Timestamp(seconds)
}

impl TerminalInspector for RuntimeRegistryInspector {
    fn inspect<'a>(
        &'a self,
        binding: &'a phoenix_db::workflow::wake::WakeBindingRecord,
        _authority: &'a LocalAttemptAuthority,
        observation_time: Timestamp,
    ) -> Pin<Box<dyn Future<Output = Result<InspectionOutcome, String>> + Send + 'a>> {
        Box::pin(async move {
            match &binding.resource {
                WakeResourceIdentity::Bash(identity) => {
                    let scope = work_scope_from_identity(&identity.work_scope);
                    let Some(scope_handles) = self.bash.get_existing(&scope).await else {
                        return Ok(InspectionOutcome::Forgotten(
                            WakeForgottenReason::PhoenixRestart,
                        ));
                    };
                    let scope_handles = scope_handles.read().await;
                    let handle_id =
                        phoenix_tools::bash::handle::HandleId::new(identity.handle_id.clone());
                    let Some(handle) = scope_handles.get(&handle_id) else {
                        return Ok(InspectionOutcome::Forgotten(
                            WakeForgottenReason::PhoenixRestart,
                        ));
                    };
                    let state = handle.state().await;
                    match state.as_ref() {
                        HandleState::Live(_) => Ok(InspectionOutcome::LiveRetry),
                        HandleState::Tombstoned(tomb) => {
                            let status = match &tomb.final_cause {
                                FinalCause::Exited { .. } => BashTerminalStatus::Exited,
                                FinalCause::Killed { .. } => BashTerminalStatus::Killed,
                            };
                            let tail = tomb
                                .final_tail
                                .iter()
                                .map(|line| String::from_utf8_lossy(&line.bytes).into_owned())
                                .collect();
                            Ok(InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(
                                BashTerminalEvidence {
                                    identity: identity.clone(),
                                    status,
                                    occurred_at: system_time_to_timestamp(tomb.finished_at),
                                    exit_code: tomb.exit_code,
                                    duration_ms: Some(tomb.duration_ms),
                                    signal_number: tomb.signal_number,
                                    kill_signal_sent: tomb
                                        .kill_signal_sent
                                        .map(|sig| format!("{sig:?}")),
                                    final_tail: tail,
                                },
                            )))
                        }
                    }
                }
                WakeResourceIdentity::TmuxWindow(identity) => {
                    let scope = work_scope_from_identity(&identity.work_scope);
                    match self
                        .tmux
                        .inspect_existing_window(
                            &scope,
                            &identity.server_token,
                            &identity.window_id,
                        )
                        .await
                        .map_err(|error| error.to_string())?
                    {
                        Some(window) => Ok(match window.exit_code {
                            Some(exit_code) => InspectionOutcome::Terminal(
                                WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
                                    identity: identity.clone(),
                                    status: TmuxTerminalStatus::ExitMarkerObserved,
                                    occurred_at: window
                                        .occurred_at
                                        .map_or(observation_time, system_time_to_timestamp),
                                    exit_code: Some(exit_code),
                                    duration_ms: None,
                                    final_tail: window.final_tail,
                                }),
                            ),
                            None => InspectionOutcome::LiveRetry,
                        }),
                        None => Ok(InspectionOutcome::Forgotten(
                            WakeForgottenReason::TmuxHandleMissing,
                        )),
                    }
                }
                WakeResourceIdentity::Subagent(_) => {
                    Err("subagent wake bindings not implemented".to_string())
                }
            }
        })
    }

    fn cleanup_after_commit<'a>(
        &'a self,
        _binding: &'a phoenix_db::workflow::wake::WakeBindingRecord,
        evidence: &'a WakeTerminalEvidence,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            if let WakeTerminalEvidence::TmuxWindow(evidence) = evidence {
                if evidence.identity.completion_policy == TmuxCompletionPolicy::CloseAfterCompletion
                {
                    let scope = work_scope_from_identity(&evidence.identity.work_scope);
                    let _ = self
                        .tmux
                        .kill_exact_window(&scope, &evidence.identity.window_id)
                        .await;
                }
            }
            Ok(())
        })
    }
}

fn work_scope_from_identity(
    scope: &phoenix_workflow::wake_profile::WorkScopeIdentity,
) -> WorkScope {
    match scope.kind {
        phoenix_workflow::wake_profile::WorkScopeKind::Conversation => {
            WorkScope::Conversation(scope.stable_key.clone())
        }
        phoenix_workflow::wake_profile::WorkScopeKind::Worktree => {
            WorkScope::Worktree(scope.stable_key.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_db::workflow::wake::WakeRegistrationOutcome;
    use phoenix_db::Database;
    use phoenix_workflow::wake_profile::{
        BashResourceIdentity, TmuxResourceIdentity, WakeRegistrationIntent, WorkScopeIdentity,
        WorkScopeKind,
    };
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tokio::sync::oneshot;

    #[derive(Clone)]
    struct TestClock {
        now: Arc<Mutex<Timestamp>>,
        sleep_tx: Arc<Mutex<Option<oneshot::Sender<Duration>>>>,
    }

    impl TestClock {
        fn new(now: u64) -> Self {
            Self {
                now: Arc::new(Mutex::new(Timestamp(now))),
                sleep_tx: Arc::new(Mutex::new(None)),
            }
        }

        fn set(&self, now: u64) {
            *self.now.lock().unwrap() = Timestamp(now);
        }

        fn expect_sleep(&self) -> oneshot::Receiver<Duration> {
            let (tx, rx) = oneshot::channel();
            *self.sleep_tx.lock().unwrap() = Some(tx);
            rx
        }
    }

    impl WakeClock for TestClock {
        fn now(&self) -> Timestamp {
            *self.now.lock().unwrap()
        }

        fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            if let Some(tx) = self.sleep_tx.lock().unwrap().take() {
                let _ = tx.send(duration);
            }
            Box::pin(std::future::pending())
        }
    }

    struct MockInspector {
        outcomes: Mutex<HashMap<u64, VecDeque<InspectionOutcome>>>,
        cleanup_calls: AtomicUsize,
    }

    struct FlakyInspector {
        remaining_failures: AtomicUsize,
    }

    impl MockInspector {
        fn new() -> Self {
            Self {
                outcomes: Mutex::new(HashMap::new()),
                cleanup_calls: AtomicUsize::new(0),
            }
        }

        fn push(&self, workflow_id: u64, outcome: InspectionOutcome) {
            self.outcomes
                .lock()
                .unwrap()
                .entry(workflow_id)
                .or_default()
                .push_back(outcome);
        }

        fn cleanup_calls(&self) -> usize {
            self.cleanup_calls.load(Ordering::SeqCst)
        }
    }

    impl FlakyInspector {
        fn new(failures: usize) -> Self {
            Self {
                remaining_failures: AtomicUsize::new(failures),
            }
        }
    }

    impl TerminalInspector for MockInspector {
        fn inspect<'a>(
            &'a self,
            binding: &'a phoenix_db::workflow::wake::WakeBindingRecord,
            _authority: &'a LocalAttemptAuthority,
            _observation_time: Timestamp,
        ) -> Pin<Box<dyn Future<Output = Result<InspectionOutcome, String>> + Send + 'a>> {
            Box::pin(async move {
                Ok(self
                    .outcomes
                    .lock()
                    .unwrap()
                    .get_mut(&binding.workflow_id.0)
                    .and_then(VecDeque::pop_front)
                    .unwrap_or(InspectionOutcome::LiveRetry))
            })
        }

        fn cleanup_after_commit<'a>(
            &'a self,
            _binding: &'a phoenix_db::workflow::wake::WakeBindingRecord,
            _evidence: &'a WakeTerminalEvidence,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
            self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    impl TerminalInspector for FlakyInspector {
        fn inspect<'a>(
            &'a self,
            _binding: &'a phoenix_db::workflow::wake::WakeBindingRecord,
            _authority: &'a LocalAttemptAuthority,
            _observation_time: Timestamp,
        ) -> Pin<Box<dyn Future<Output = Result<InspectionOutcome, String>> + Send + 'a>> {
            Box::pin(async move {
                let remaining = self.remaining_failures.load(Ordering::SeqCst);
                if remaining > 0 {
                    self.remaining_failures.fetch_sub(1, Ordering::SeqCst);
                    Err("transient inspection failure".to_string())
                } else {
                    Ok(InspectionOutcome::LiveRetry)
                }
            })
        }
    }

    async fn open_repo() -> (Database, WakeRepository) {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv", "conv", "/tmp", true, None, None)
            .await
            .unwrap();
        (db.clone(), WakeRepository::new(db.pool().clone()))
    }

    fn conv_scope() -> WorkScopeIdentity {
        WorkScopeIdentity {
            kind: WorkScopeKind::Conversation,
            stable_key: "conv".to_string(),
        }
    }

    async fn register_bash(repo: &WakeRepository, handle: &str, expires_at: u64) -> u64 {
        let intent = WakeRegistrationIntent {
            contract_id: format!("contract-{handle}"),
            conversation_id: "conv".to_string(),
            registration_scope: conv_scope(),
            resource: WakeResourceIdentity::Bash(BashResourceIdentity {
                work_scope: conv_scope(),
                handle_id: handle.to_string(),
            }),
            registering_tool_use_id: "tool-use".to_string(),
            registered_at: Timestamp(1),
            expires_at: Timestamp(expires_at),
        };
        match repo.register(&intent, handle, Timestamp(1)).await.unwrap() {
            WakeRegistrationOutcome::Registered { workflow_id, .. }
            | WakeRegistrationOutcome::Replayed { workflow_id, .. } => workflow_id.0,
            WakeRegistrationOutcome::Conflict => panic!("unexpected conflict"),
        }
    }

    async fn register_tmux(
        repo: &WakeRepository,
        generation: &str,
        window_id: &str,
        expires_at: u64,
    ) -> u64 {
        let intent = WakeRegistrationIntent {
            contract_id: format!("contract-{window_id}"),
            conversation_id: "conv".to_string(),
            registration_scope: conv_scope(),
            resource: WakeResourceIdentity::TmuxWindow(TmuxResourceIdentity {
                work_scope: conv_scope(),
                server_token: generation.to_string(),
                window_id: window_id.to_string(),
                completion_policy: TmuxCompletionPolicy::KeepOpen,
            }),
            registering_tool_use_id: "tool-use".to_string(),
            registered_at: Timestamp(1),
            expires_at: Timestamp(expires_at),
        };
        match repo
            .register(&intent, window_id, Timestamp(1))
            .await
            .unwrap()
        {
            WakeRegistrationOutcome::Registered { workflow_id, .. }
            | WakeRegistrationOutcome::Replayed { workflow_id, .. } => workflow_id.0,
            WakeRegistrationOutcome::Conflict => panic!("unexpected conflict"),
        }
    }

    async fn pending_count(repo: &WakeRepository) -> usize {
        repo.list_pending("conv").await.unwrap().len()
    }

    #[tokio::test]
    async fn due_expiry_first_projects_terminal() {
        let (_db, repo) = open_repo().await;
        register_bash(&repo, "b-1", 5).await;
        let worker = WakeWorker::new(
            repo.clone(),
            Arc::new(MockInspector::new()),
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(1),
        );
        worker.run_once().await.unwrap();
        assert_eq!(pending_count(&repo).await, 1);
    }

    #[tokio::test]
    async fn fired_bash_via_mock_inspector_records_terminal() {
        let (_db, repo) = open_repo().await;
        let workflow_id = register_bash(&repo, "b-1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: conv_scope(),
                    handle_id: "b-1".to_string(),
                },
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(10),
                exit_code: Some(0),
                duration_ms: Some(5),
                signal_number: None,
                kill_signal_sent: None,
                final_tail: vec!["done".to_string()],
            })),
        );
        let worker = WakeWorker::new(
            repo.clone(),
            inspector,
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(1),
        );
        worker.run_once().await.unwrap();
        assert_eq!(pending_count(&repo).await, 1);
    }

    #[tokio::test]
    async fn fired_tmux_via_mock_inspector_records_terminal() {
        let (_db, repo) = open_repo().await;
        let workflow_id = register_tmux(&repo, "g1", "w1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
                identity: TmuxResourceIdentity {
                    work_scope: conv_scope(),
                    server_token: "g1".to_string(),
                    window_id: "w1".to_string(),
                    completion_policy: TmuxCompletionPolicy::KeepOpen,
                },
                status: TmuxTerminalStatus::ExitMarkerObserved,
                occurred_at: Timestamp(10),
                exit_code: Some(0),
                duration_ms: None,
                final_tail: vec!["done".to_string()],
            })),
        );
        let worker = WakeWorker::new(
            repo.clone(),
            inspector.clone(),
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(1),
        );
        worker.run_once().await.unwrap();
        assert_eq!(pending_count(&repo).await, 1);
        assert_eq!(inspector.cleanup_calls(), 1);
    }

    #[tokio::test]
    async fn forgotten_records_forgotten_terminal() {
        let (_db, repo) = open_repo().await;
        let workflow_id = register_tmux(&repo, "g1", "w1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Forgotten(WakeForgottenReason::TmuxHandleMissing),
        );
        let worker = WakeWorker::new(
            repo.clone(),
            inspector,
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(1),
        );
        worker.run_once().await.unwrap();
        assert_eq!(pending_count(&repo).await, 1);
    }

    #[tokio::test]
    async fn live_retry_leaves_unresolved_and_waits_for_lease() {
        let (_db, repo) = open_repo().await;
        register_bash(&repo, "b-1", 50).await;
        let worker = WakeWorker::new(
            repo.clone(),
            Arc::new(MockInspector::new()),
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(1),
        );
        let wait = worker.run_once().await.unwrap();
        assert_eq!(pending_count(&repo).await, 0);
        assert_eq!(wait, EMPTY_RESCAN_INTERVAL);
    }

    #[tokio::test]
    async fn startup_restart_discovery_marks_missing_bash_forgotten() {
        let (_db, repo) = open_repo().await;
        register_bash(&repo, "missing", 50).await;
        let inspector = RuntimeRegistryInspector::new(
            Arc::new(phoenix_tools::BashHandleRegistry::new()),
            Arc::new(phoenix_tools::TmuxRegistry::new()),
        );
        let worker = WakeWorker::new(
            repo.clone(),
            Arc::new(inspector),
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(99),
        );
        worker.run_once().await.unwrap();
        assert_eq!(pending_count(&repo).await, 1);
    }

    #[tokio::test]
    async fn startup_restart_discovery_reuses_live_tmux_socket_without_registry_entry() {
        if which::which("tmux").is_err() {
            return;
        }
        let (_db, repo) = open_repo().await;
        let socket_tmp = tempfile::TempDir::new().unwrap();
        let cwd_tmp = tempfile::TempDir::new().unwrap();
        let tmux = Arc::new(phoenix_tools::TmuxRegistry::with_socket_dir(
            socket_tmp.path().to_path_buf(),
        ));
        let scope = WorkScope::Conversation("conv".to_string());
        let server = tmux.ensure_live(&scope, cwd_tmp.path()).await.unwrap();
        let socket_path = server.read().await.socket_path.clone();
        let server_token = server.read().await.server_token.clone();
        let output = tokio::process::Command::new("tmux")
            .args([
                "-S",
                &socket_path.to_string_lossy(),
                "new-window",
                "-d",
                "-P",
                "-F",
                "#{window_id}",
                "bash",
                "-lc",
                "printf '__PHOENIX_EXIT__ exit_code=0 occurred_at_ms=1700000000000\\n'; exec ${SHELL:-/bin/bash} -i",
            ])
            .output()
            .await
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let window_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        drop(server);
        let fresh_registry = Arc::new(phoenix_tools::TmuxRegistry::with_socket_dir(
            socket_tmp.path().to_path_buf(),
        ));
        let workflow_id = register_tmux(&repo, &server_token, &window_id, 50).await;
        let inspector = Arc::new(RuntimeRegistryInspector::new(
            Arc::new(phoenix_tools::BashHandleRegistry::new()),
            fresh_registry,
        ));
        let worker = WakeWorker::new(
            repo.clone(),
            inspector,
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(99),
        );
        worker.run_once().await.unwrap();
        let binding = repo
            .fetch_binding(phoenix_workflow::WorkflowId(workflow_id))
            .await
            .unwrap();
        assert!(binding.is_some());
        tokio::process::Command::new("tmux")
            .args(["-S", &socket_path.to_string_lossy(), "kill-server"])
            .output()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn stale_process_fencing_prevents_duplicate_projection() {
        let (_db, repo) = open_repo().await;
        let workflow_id = register_bash(&repo, "b-1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: conv_scope(),
                    handle_id: "b-1".to_string(),
                },
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(10),
                exit_code: Some(0),
                duration_ms: Some(5),
                signal_number: None,
                kill_signal_sent: None,
                final_tail: vec![],
            })),
        );
        let stale_worker = WakeWorker::new(
            repo.clone(),
            inspector,
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(1),
        );
        let fresh_worker = WakeWorker::new(
            repo.clone(),
            Arc::new(MockInspector::new()),
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(2),
        );
        stale_worker.run_once().await.unwrap();
        fresh_worker.run_once().await.unwrap();
        assert_eq!(pending_count(&repo).await, 1);
    }

    fn register_input(handle: &str, fingerprint: &str, expires_at: u64) -> RegisterWakeInput {
        RegisterWakeInput {
            contract_id: format!("contract-{handle}"),
            conversation_id: "conv".to_string(),
            root_conversation_id: "root".to_string(),
            registering_tool_use_id: "tool-use".to_string(),
            registration_scope: conv_scope(),
            resource: WakeResourceIdentity::Bash(BashResourceIdentity {
                work_scope: conv_scope(),
                handle_id: handle.to_string(),
            }),
            expires_at: Timestamp(expires_at),
            prepared_fingerprint: fingerprint.to_string(),
        }
    }

    #[tokio::test]
    async fn production_registrar_replays_and_conflicts_exactly() {
        let (_db, repo) = open_repo().await;
        let (kick_tx, kick_rx) = watch::channel(0u64);
        let registrar = ProductionWakeRegistrar::new(repo, kick_tx);

        let first = registrar
            .register(register_input("b-1", "fp-1", 50))
            .await
            .unwrap();
        let replay = registrar
            .register(register_input("b-1", "fp-1", 50))
            .await
            .unwrap();
        let conflict = registrar
            .register(register_input("b-1", "fp-2", 50))
            .await
            .unwrap();

        let first_id = first
            .workflow_id()
            .expect("first registration should allocate workflow");
        assert_eq!(
            replay,
            RegisteredWake::Replayed {
                workflow_id: first_id
            }
        );
        assert_eq!(conflict, RegisteredWake::Conflict);
        assert_eq!(*kick_rx.borrow(), 3);
    }

    #[tokio::test]
    async fn production_registrar_cancel_kicks_and_replays() {
        let (_db, repo) = open_repo().await;
        let workflow_id = register_bash(&repo, "b-1", 50).await;
        let (kick_tx, kick_rx) = watch::channel(0u64);
        let registrar = ProductionWakeRegistrar::new(repo, kick_tx);

        let cancelled = registrar
            .cancel(CancelWakeInput {
                workflow_id: phoenix_workflow::WorkflowId(workflow_id),
                timestamp: Timestamp(5),
                reason: phoenix_workflow::wake_profile::WakeCancellationReason::ExplicitCancel,
            })
            .await
            .unwrap();
        let replay = registrar
            .cancel(CancelWakeInput {
                workflow_id: phoenix_workflow::WorkflowId(workflow_id),
                timestamp: Timestamp(5),
                reason: phoenix_workflow::wake_profile::WakeCancellationReason::ExplicitCancel,
            })
            .await
            .unwrap();

        assert_eq!(cancelled, RegisteredWake::Cancelled);
        assert_eq!(replay, RegisteredWake::CancelReplayed);
        assert_eq!(*kick_rx.borrow(), 2);
    }

    #[tokio::test]
    async fn kick_preempts_deadline_wait() {
        let (_db, repo) = open_repo().await;
        register_bash(&repo, "b-1", 50).await;
        let clock = Arc::new(TestClock::new(10));
        let sleep_rx = clock.expect_sleep();
        let worker = WakeWorker::new(
            repo,
            Arc::new(MockInspector::new()),
            clock,
            ProcessIncarnation(1),
        );
        let (tx, rx) = watch::channel(0u64);
        let join = tokio::spawn(async move { worker.run_loop(rx).await });
        let observed_sleep = sleep_rx.await.unwrap();
        assert_eq!(observed_sleep, EMPTY_RESCAN_INTERVAL);
        tx.send(1).unwrap();
        join.abort();
    }

    #[tokio::test]
    async fn deadline_advances_to_expiry() {
        let (_db, repo) = open_repo().await;
        register_bash(&repo, "b-1", 12).await;
        let clock = Arc::new(TestClock::new(10));
        let worker = WakeWorker::new(
            repo.clone(),
            Arc::new(MockInspector::new()),
            clock.clone(),
            ProcessIncarnation(1),
        );
        worker.run_once().await.unwrap();
        clock.set(13);
        worker.run_once().await.unwrap();
        assert_eq!(pending_count(&repo).await, 1);
    }

    #[tokio::test]
    async fn terminal_inspection_wins_over_same_tick_expiry() {
        let (_db, repo) = open_repo().await;
        let workflow_id = register_bash(&repo, "b-1", 12).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: conv_scope(),
                    handle_id: "b-1".to_string(),
                },
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(11),
                exit_code: Some(0),
                duration_ms: Some(5),
                signal_number: None,
                kill_signal_sent: None,
                final_tail: vec!["done".to_string()],
            })),
        );
        let worker = WakeWorker::new(
            repo.clone(),
            inspector,
            Arc::new(TestClock::new(12)),
            ProcessIncarnation(1),
        );

        worker.run_once().await.unwrap();

        let pending = repo.list_pending("conv").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].receipt.terminal,
            phoenix_workflow::wake_profile::WakeTerminalPayload::Fired { .. }
        ));
    }
    #[tokio::test]
    async fn worker_retries_after_transient_inspection_error() {
        let (_db, repo) = open_repo().await;
        register_bash(&repo, "b-1", 50).await;
        let clock = Arc::new(TestClock::new(10));
        let sleep_rx = clock.expect_sleep();
        let worker = WakeWorker::new(
            repo,
            Arc::new(FlakyInspector::new(1)),
            clock,
            ProcessIncarnation(1),
        );
        let (_tx, rx) = watch::channel(0u64);
        let join = tokio::spawn(async move { worker.run_loop(rx).await });

        let observed_sleep = sleep_rx.await.unwrap();
        assert_eq!(observed_sleep, ERROR_RETRY_MAX_INTERVAL);
        join.abort();
    }

    #[tokio::test]
    async fn restart_redelivers_pending_direct_turn_from_prepared_payload() {
        let (db, _repo) = open_repo().await;
        let prepared = phoenix_core::domain::sm_event::PreparedDirectTurn {
            codec_version: phoenix_core::domain::sm_event::PREPARED_DIRECT_TURN_CODEC_VERSION,
            expand_references: false,
            text: "recovered".to_string(),
            llm_text: None,
            images: vec![],
            files: vec![],
            message_id: "message-recovered".to_string(),
            user_agent: None,
            skill_invocation: None,
        };
        let prepared_payload = serde_json::to_string(&prepared).unwrap();
        phoenix_db::WorkflowRepository::new(db.pool().clone())
            .accept_direct_turn(&phoenix_db::DirectTurnAcceptanceInput {
                initial_outcome: phoenix_db::DirectTurnInitialOutcome::PendingRuntime,
                conversation_id: "conv".to_string(),
                client_message_id: "message-recovered".to_string(),
                prepared_fingerprint: "fingerprint".to_string(),
                prepared_payload,
                accepted_at: Timestamp(1),
                snapshot: phoenix_workflow::llm_profile::TopLevelLlmSnapshot {
                    turn_ref: phoenix_workflow::llm_profile::TopLevelTurnRef {
                        conversation_id: "conv".to_string(),
                        accepted_turn_id: "message-recovered".to_string(),
                        generation: 0,
                    },
                    accepted_assistant_message_id: None,
                    stopped_at: None,
                },
            })
            .await
            .unwrap();
        let manager = Arc::new(crate::runtime::RuntimeManager::new(
            db.clone(),
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            phoenix_core::platform::PlatformCapability::None {
                details: "test".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        ));

        deliver_pending_direct_turns(&manager).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !db.message_exists("message-recovered").await.unwrap() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let pending_dispatch = phoenix_db::WorkflowRepository::new(db.pool().clone())
            .load_pending_direct_turns()
            .await
            .unwrap();
        assert!(pending_dispatch.is_empty());
        assert!(matches!(
            db.get_conversation("conv").await.unwrap().state,
            phoenix_core::domain::sm_state::ConvState::LlmRequesting { .. }
        ));
    }

    #[tokio::test]
    async fn recover_pending_deliveries_preallocates_broadcaster_sequence_for_materialized_message()
    {
        let (db, repo) = open_repo().await;
        let workflow_id = register_bash(&repo, "b-1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: conv_scope(),
                    handle_id: "b-1".to_string(),
                },
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(10),
                exit_code: Some(0),
                duration_ms: Some(5),
                signal_number: None,
                kill_signal_sent: None,
                final_tail: vec!["done".to_string()],
            })),
        );
        let worker = WakeWorker::new(
            repo.clone(),
            inspector,
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(1),
        );
        worker.run_once().await.unwrap();

        let manager = Arc::new(crate::runtime::RuntimeManager::new(
            db.clone(),
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            phoenix_core::platform::PlatformCapability::None {
                details: "test".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        ));
        let handle = manager.get_or_create("conv").await.unwrap();
        let _ = handle.broadcast_tx.next_seq();
        let _ = handle.broadcast_tx.next_seq();
        let _ = handle.broadcast_tx.next_seq();

        deliver_pending(&manager, &repo, Timestamp(20))
            .await
            .unwrap();

        let messages = db.get_messages("conv").await.unwrap();
        let wake = messages.last().expect("wake message persisted");
        assert_eq!(wake.sequence_id, 4);
        assert!(matches!(
            &wake.content,
            crate::db::MessageContent::User(user) if user.is_meta && user.text.contains("done")
        ));
    }

    #[tokio::test]
    async fn already_materialized_wake_does_not_reset_newer_replay_events() {
        let (db, repo) = open_repo().await;
        let workflow_id = register_bash(&repo, "b-1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: conv_scope(),
                    handle_id: "b-1".to_string(),
                },
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(10),
                exit_code: Some(0),
                duration_ms: Some(5),
                signal_number: None,
                kill_signal_sent: None,
                final_tail: vec!["done".to_string()],
            })),
        );
        WakeWorker::new(
            repo.clone(),
            inspector,
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(1),
        )
        .run_once()
        .await
        .unwrap();
        let pending = repo.list_pending("conv").await.unwrap().pop().unwrap();
        let materialized = repo
            .materialize_pending_delivery_message(&MaterializePendingDeliveryMessageInput {
                workflow_id: phoenix_workflow::WorkflowId(workflow_id),
                delivery_id: pending.canonical_delivery.delivery_id,
                conversation_id: "conv".to_string(),
                rendered_content: render_terminal_result(&pending),
                display_data: Some(serde_json::json!({
                    "type": "wake_result",
                    "adopted": false,
                    "terminal": &pending.receipt.terminal,
                })),
                auto_resume: true,
                created_at: Timestamp(20),
                sequence_id: Some(1),
            })
            .await
            .unwrap();
        assert!(matches!(
            materialized,
            MaterializePendingDeliveryMessageOutcome::Materialized(_)
        ));

        let manager = Arc::new(crate::runtime::RuntimeManager::new(
            db,
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            phoenix_core::platform::PlatformCapability::None {
                details: "test".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        ));
        let handle = manager.get_or_create("conv").await.unwrap();
        handle
            .broadcast_tx
            .send_seq(|sequence_id| crate::runtime::SseEvent::Token {
                sequence_id,
                text: "newer".to_string(),
                request_id: "request".to_string(),
            })
            .ok();
        let before = handle.broadcast_tx.snapshot_pending();
        assert_eq!(before.3.len(), 1);

        deliver_pending(&manager, &repo, Timestamp(21))
            .await
            .unwrap();

        let after = handle.broadcast_tx.snapshot_pending();
        assert_eq!(after.0, before.0);
        assert_eq!(after.3.len(), 1);
        assert!(matches!(
            &after.3[0],
            crate::runtime::SseEvent::Token { text, .. } if text == "newer"
        ));
    }

    #[test]
    fn process_incarnation_fits_signed_sqlite_range() {
        for _ in 0..128 {
            let incarnation = fresh_process_incarnation();
            assert!(i64::try_from(incarnation.0).is_ok());
        }
    }
}
