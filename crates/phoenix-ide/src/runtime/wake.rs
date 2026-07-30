use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::runtime::RuntimeManager;
use phoenix_core::work_scope::ResourceScopeKey;
use phoenix_db::workflow::wake::{
    MaterializePendingDeliveryMessageInput, MaterializePendingDeliveryMessageOutcome,
    WakeCancelIfUnresolvedInput, WakeCancellationOutcome, WakeForgetIfUnresolvedInput,
    WakeObservationCandidateRow, WakeObservationOutcome, WakePendingDelivery,
    WakePendingGlobalCursor, WakeRegistrationOutcome, WakeRepository, WakeTerminalEvidenceInput,
    WakeTerminalEvidenceOutcome,
};
use phoenix_db::workflow::LocalAttemptAuthority;
use phoenix_tools::bash::handle::{FinalCause, Handle, HandleState, LiveData};
use phoenix_tools::{CancelWakeInput, RegisterWakeInput, RegisteredWake, WakeRegistrar};
use phoenix_workflow::wake_profile::{
    BashResourceIdentity, BashTerminalEvidence, BashTerminalStatus, TmuxCompletionPolicy,
    TmuxTerminalEvidence, TmuxTerminalStatus, WakeForgottenReason, WakeResourceIdentity,
    WakeTerminalEvidence,
};
use phoenix_workflow::{LeaseExpiry, ProcessIncarnation, Timestamp};
use tokio::sync::watch;

const OBSERVATION_BATCH_LIMIT: usize = 64;
const CONVERSATION_DELIVERY_BATCH_LIMIT: usize = 16;
const EXPIRY_BATCH_LIMIT: usize = 64;
const LEASE_DURATION: Duration = Duration::from_secs(30);
const LIVE_HANDLE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const EMPTY_RESCAN_INTERVAL: Duration = Duration::from_secs(5);
const ERROR_RETRY_BASE_INTERVAL: Duration = Duration::from_millis(250);
const ERROR_RETRY_MAX_INTERVAL: Duration = Duration::from_secs(5);

fn fresh_process_incarnation() -> ProcessIncarnation {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&uuid::Uuid::new_v4().into_bytes()[..8]);
    bytes[7] &= 0x7f;
    ProcessIncarnation(u64::from_le_bytes(bytes))
}

#[derive(Clone)]
pub(crate) struct ProductionWakeRegistrar {
    repo: WakeRepository,
    kick_tx: watch::Sender<u64>,
    acceptance_lock: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<(String, String), crate::runtime::SteeringAcceptanceReceipt>,
        >,
    >,
    conversation_acceptance_locks: crate::runtime::ConversationAcceptanceLocks,
    pending_activation:
        Arc<std::sync::Mutex<std::collections::HashSet<phoenix_workflow::WorkflowId>>>,
}

impl ProductionWakeRegistrar {
    pub(crate) fn new(
        repo: WakeRepository,
        kick_tx: watch::Sender<u64>,
        acceptance_lock: Arc<
            tokio::sync::Mutex<
                std::collections::HashMap<
                    (String, String),
                    crate::runtime::SteeringAcceptanceReceipt,
                >,
            >,
        >,
        conversation_acceptance_locks: crate::runtime::ConversationAcceptanceLocks,
        pending_activation: Arc<
            std::sync::Mutex<std::collections::HashSet<phoenix_workflow::WorkflowId>>,
        >,
    ) -> Self {
        Self {
            repo,
            kick_tx,
            acceptance_lock,
            conversation_acceptance_locks,
            pending_activation,
        }
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
        let conversation_id = input.conversation_id.clone();
        let intent = input.into_intent(now);
        let _acceptance_guard = self.acceptance_lock.lock().await;
        let _conversation_guard = crate::runtime::acquire_conversation_acceptance_lock(
            &self.conversation_acceptance_locks,
            &conversation_id,
        )
        .await;
        let outcome = self
            .repo
            .register(&intent, &prepared_fingerprint, now)
            .await
            .map_err(|e| e.to_string())?;
        Ok(match outcome {
            WakeRegistrationOutcome::Registered {
                workflow_id,
                receipt,
            } => {
                self.pending_activation.lock().unwrap().insert(workflow_id);
                RegisteredWake::Registered {
                    workflow_id,
                    expires_at: receipt.expires_at,
                }
            }
            WakeRegistrationOutcome::Replayed {
                workflow_id,
                receipt,
            } => RegisteredWake::Replayed {
                workflow_id,
                expires_at: receipt.expires_at,
            },
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

    fn notify_activation_committed(&self, workflow_id: phoenix_workflow::WorkflowId) {
        self.pending_activation.lock().unwrap().remove(&workflow_id);
        self.kick();
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
        self.run_once_with_manager(Some(&manager)).await?;
        deliver_pending(&manager, &self.repo, self.clock.now(), None).await?;
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
            let wait = match self.run_once_with_manager(manager.as_ref()).await {
                Ok(wait) => {
                    if let Some(manager) = manager.as_ref() {
                        if let Err(error) =
                            deliver_pending(manager, &self.repo, self.clock.now(), None).await
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

    #[cfg(test)]
    async fn run_once(&self) -> Result<Duration, String> {
        self.run_once_with_manager(None).await
    }

    async fn run_once_with_manager(
        &self,
        manager: Option<&Arc<RuntimeManager>>,
    ) -> Result<Duration, String> {
        let now = self.clock.now();
        let next_wait = self.observe_candidates(now, manager).await?;
        self.expire_due(now, manager).await?;
        Ok(next_wait)
    }

    async fn expire_due(
        &self,
        now: Timestamp,
        manager: Option<&Arc<RuntimeManager>>,
    ) -> Result<(), String> {
        let expired = self
            .repo
            .list_expired_unresolved(now, EXPIRY_BATCH_LIMIT)
            .await
            .map_err(|e| e.to_string())?;
        for row in expired {
            if manager.is_some_and(|manager| {
                manager
                    .pending_wake_activation
                    .lock()
                    .unwrap()
                    .contains(&row.workflow_id)
            }) {
                continue;
            }
            let conversation_guard = if let Some(manager) = manager {
                Some(
                    manager
                        .lock_conversation_acceptance(&row.conversation_id)
                        .await,
                )
            } else {
                None
            };
            if let Err(error) = self.repo.expire_if_unresolved(row.workflow_id, now).await {
                tracing::warn!(workflow_id = row.workflow_id.0, error = %error, "wake expiry failed for one contract; continuing");
            }
            if let Some(manager) = manager {
                deliver_pending(
                    manager,
                    &self.repo,
                    self.clock.now(),
                    Some(&row.conversation_id),
                )
                .await?;
            }
            drop(conversation_guard);
        }
        Ok(())
    }

    async fn observe_candidates(
        &self,
        now: Timestamp,
        manager: Option<&Arc<RuntimeManager>>,
    ) -> Result<Duration, String> {
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
                if manager.is_some_and(|manager| {
                    manager
                        .pending_wake_activation
                        .lock()
                        .unwrap()
                        .contains(&candidate.workflow_id)
                }) {
                    continue;
                }
                saw_candidate = true;
                let claim_until = LeaseExpiry(if candidate.expires_at.0 <= now.0 {
                    now.0.saturating_add(LEASE_DURATION.as_secs())
                } else {
                    now.0
                        .saturating_add(LEASE_DURATION.as_secs())
                        .min(candidate.expires_at.0.saturating_add(1))
                });
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
                            .inspect_candidate(candidate, authority, now, claim_until, manager)
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

    #[allow(clippy::too_many_lines)]
    async fn inspect_candidate(
        &self,
        candidate: WakeObservationCandidateRow,
        authority: LocalAttemptAuthority,
        now: Timestamp,
        _lease_until: LeaseExpiry,
        manager: Option<&Arc<RuntimeManager>>,
    ) -> Result<Duration, String> {
        let Some(binding) = self
            .repo
            .reload_binding(candidate.workflow_id)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(Duration::ZERO);
        };
        let inspection = match self.inspector.inspect(&binding, &authority, now).await {
            Ok(inspection) => inspection,
            Err(error) => {
                if let Err(release_error) =
                    self.repo.release_observation_authority(&authority).await
                {
                    tracing::warn!(
                        %release_error,
                        workflow_id = candidate.workflow_id.0,
                        "failed to release wake observation authority after inspection error"
                    );
                }
                return Err(error);
            }
        };
        match inspection {
            InspectionOutcome::LiveRetry => {
                let _ = self
                    .repo
                    .release_observation_authority(&authority)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(Duration::from_secs(
                    candidate
                        .expires_at
                        .0
                        .saturating_sub(now.0)
                        .min(LIVE_HANDLE_POLL_INTERVAL.as_secs()),
                ))
            }
            InspectionOutcome::Terminal(evidence) => {
                let evidence_time = match &evidence {
                    WakeTerminalEvidence::Bash(evidence) => evidence.occurred_at,
                    WakeTerminalEvidence::TmuxWindow(evidence) => evidence.occurred_at,
                    WakeTerminalEvidence::Subagent(evidence) => evidence.occurred_at,
                };
                if evidence_time.0 > candidate.expires_at.0 {
                    let _ = self
                        .repo
                        .release_observation_authority(&authority)
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok(Duration::ZERO);
                }
                let conversation_guard = if let Some(manager) = manager {
                    Some(
                        manager
                            .lock_conversation_acceptance(&binding.conversation_id)
                            .await,
                    )
                } else {
                    None
                };
                let observation_time = self.clock.now();
                let outcome = match self
                    .repo
                    .record_terminal_allocated(&WakeTerminalEvidenceInput {
                        workflow_id: candidate.workflow_id,
                        authority: authority.clone(),
                        observation_time,
                        evidence: evidence.clone(),
                    })
                    .await
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        if let Err(release_error) =
                            self.repo.release_observation_authority(&authority).await
                        {
                            tracing::warn!(
                                %release_error,
                                workflow_id = candidate.workflow_id.0,
                                "failed to release wake observation authority after persistence error"
                            );
                        }
                        return Err(error.to_string());
                    }
                };
                if matches!(
                    outcome,
                    WakeTerminalEvidenceOutcome::Recorded { .. }
                        | WakeTerminalEvidenceOutcome::Replayed { .. }
                ) {
                    self.inspector
                        .cleanup_after_commit(&binding, &evidence)
                        .await?;
                }
                if let Some(manager) = manager {
                    deliver_pending(
                        manager,
                        &self.repo,
                        self.clock.now(),
                        Some(&binding.conversation_id),
                    )
                    .await?;
                }
                drop(conversation_guard);
                Ok(Duration::ZERO)
            }
            InspectionOutcome::Forgotten(reason) => {
                let conversation_guard = if let Some(manager) = manager {
                    Some(
                        manager
                            .lock_conversation_acceptance(&binding.conversation_id)
                            .await,
                    )
                } else {
                    None
                };
                if let Err(error) = self
                    .repo
                    .forget_if_unresolved_allocated(&WakeForgetIfUnresolvedInput {
                        workflow_id: candidate.workflow_id,
                        now,
                        reason,
                    })
                    .await
                {
                    if let Err(release_error) =
                        self.repo.release_observation_authority(&authority).await
                    {
                        tracing::warn!(
                            %release_error,
                            workflow_id = candidate.workflow_id.0,
                            "failed to release forgotten wake observation authority after persistence error"
                        );
                    }
                    return Err(error.to_string());
                }
                if let Some(manager) = manager {
                    deliver_pending(
                        manager,
                        &self.repo,
                        self.clock.now(),
                        Some(&binding.conversation_id),
                    )
                    .await?;
                }
                drop(conversation_guard);
                Ok(Duration::ZERO)
            }
        }
    }
}

async fn adopt_materialized_delivery(
    manager: &Arc<RuntimeManager>,
    handle: &crate::runtime::ConversationHandle,
    repo: &WakeRepository,
    conversation_id: &str,
    deliveries: &[(phoenix_workflow::WorkflowId, phoenix_workflow::DeliveryId)],
    now: Timestamp,
) -> Result<(), String> {
    let runtimes = manager.runtimes.read().await;
    let Some(current) = runtimes.get(conversation_id) else {
        return Ok(());
    };
    if !Arc::ptr_eq(&current.identity, &handle.identity) {
        return Ok(());
    }
    let permit = current
        .event_tx
        .reserve()
        .await
        .map_err(|error| error.to_string())?;
    let outcome = repo
        .adopt_materialized_pending_for_conversation(conversation_id, deliveries, now)
        .await
        .map_err(|error| error.to_string())?;
    if let phoenix_db::workflow::wake::WakeAdoptMaterializedPendingOutcome::Adopted(adopted) =
        outcome
    {
        if adopted.auto_resume {
            permit.send(crate::state_machine::Event::WakeBatchAdopted);
        }
    }
    Ok(())
}

fn handle_is_idle(handle: &crate::runtime::ConversationHandle) -> bool {
    matches!(
        *handle.state_rx.borrow(),
        crate::state_machine::ConvState::Idle
    )
}

fn sort_pending_for_materialization(pending: &mut [WakePendingDelivery]) {
    pending.sort_by_key(|delivery| delivery.receipt.resolution_ordinal);
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn deliver_pending(
    manager: &Arc<RuntimeManager>,
    repo: &WakeRepository,
    now: Timestamp,
    already_locked_conversation: Option<&str>,
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
        let mut processed_conversations = std::collections::HashSet::new();
        for row in pending {
            let next_cursor = WakePendingGlobalCursor {
                workflow_id: row.workflow_id,
                delivery_id: row.delivery_id,
            };
            if already_locked_conversation
                .is_some_and(|conversation_id| row.conversation_id != conversation_id)
            {
                cursor = Some(next_cursor);
                continue;
            }
            if !processed_conversations.insert(row.conversation_id.clone()) {
                cursor = Some(next_cursor);
                continue;
            }
            let current = repo
                .get_pending_exact(row.workflow_id, row.delivery_id, &row.conversation_id)
                .await
                .map_err(|error| error.to_string())?;
            let Some(current) = current else {
                cursor = Some(next_cursor);
                continue;
            };
            let _conversation_acceptance =
                if already_locked_conversation == Some(current.conversation_id.as_str()) {
                    None
                } else {
                    Some(
                        manager
                            .lock_conversation_acceptance(&current.conversation_id)
                            .await,
                    )
                };
            let active_direct_turn =
                phoenix_db::workflow::WorkflowRepository::new(manager.db().pool().clone())
                    .load_active_runtime_turn(&phoenix_workflow::ConversationAuthority(
                        current.conversation_id.clone(),
                    ))
                    .await
                    .map_err(|error| error.to_string())?;
            if active_direct_turn.is_some_and(|turn| {
                matches!(
                    turn.materialization,
                    phoenix_workflow::Materialization::Unmaterialized
                )
            }) {
                cursor = Some(next_cursor);
                continue;
            }
            let handle = match manager.try_get_handle(&current.conversation_id).await {
                Some(handle) => handle,
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
            if !handle_is_idle(&handle) {
                cursor = Some(next_cursor);
                continue;
            }
            let mut conversation_pending = repo
                .list_pending(&current.conversation_id)
                .await
                .map_err(|error| error.to_string())?;
            sort_pending_for_materialization(&mut conversation_pending);
            conversation_pending.truncate(CONVERSATION_DELIVERY_BATCH_LIMIT);
            let mut unmaterialized = Vec::new();
            for delivery in &conversation_pending {
                if repo
                    .get_delivery_message_link(
                        delivery.workflow_id,
                        delivery.canonical_delivery.delivery_id,
                    )
                    .await
                    .map_err(|error| error.to_string())?
                    .is_none()
                {
                    unmaterialized.push(delivery);
                }
            }
            let reserved = (!unmaterialized.is_empty()).then(|| {
                handle
                    .broadcast_tx
                    .reserve_next_persisted_message_range(unmaterialized.len())
            });
            let sequence_ids = reserved
                .as_ref()
                .map(|(_, sequence_ids)| sequence_ids.clone())
                .unwrap_or_default();
            for (delivery, sequence_id) in unmaterialized.into_iter().zip(sequence_ids) {
                let display_data = Some(serde_json::json!({
                    "type": "wake_result",
                    "adopted": false,
                    "terminal_kind": terminal_kind(&delivery.receipt.terminal),
                }));
                let auto_resume = !matches!(
                    delivery.receipt.terminal,
                    phoenix_workflow::wake_profile::WakeTerminalPayload::Cancelled { .. }
                );
                match repo
                    .materialize_pending_delivery_message(&MaterializePendingDeliveryMessageInput {
                        workflow_id: delivery.workflow_id,
                        delivery_id: delivery.canonical_delivery.delivery_id,
                        conversation_id: delivery.conversation_id.clone(),
                        rendered_content: render_materialized_terminal_result(delivery),
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
                        let _ = handle.broadcast_tx.send_seq(|sequence_id| {
                            crate::runtime::SseEvent::WakeContractTerminal {
                                sequence_id,
                                workflow_id: delivery.workflow_id.0,
                                contract_id: delivery.receipt.contract_id.clone(),
                                receipt_id: delivery.receipt.receipt_id.0,
                                delivery_id: delivery.canonical_delivery.delivery_id.0,
                                terminal_kind: terminal_kind(&delivery.receipt.terminal),
                            }
                        });
                    }
                    MaterializePendingDeliveryMessageOutcome::AlreadyMaterialized(_) => {}
                    MaterializePendingDeliveryMessageOutcome::WrongOwnerOrIneligible => {
                        repo.suppress_pending_for_archived_conversation(delivery, now)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
            let deliveries: Vec<_> = conversation_pending
                .iter()
                .map(|delivery| {
                    (
                        delivery.workflow_id,
                        delivery.canonical_delivery.delivery_id,
                    )
                })
                .collect();
            drop(reserved);
            adopt_materialized_delivery(
                manager,
                &handle,
                repo,
                &current.conversation_id,
                &deliveries,
                now,
            )
            .await?;
            cursor = Some(next_cursor);
        }
        if page_len < OBSERVATION_BATCH_LIMIT {
            break;
        }
    }

    Ok(())
}

#[derive(serde::Serialize)]
struct BashTombstonedWakeObservation<'a> {
    contract_id: &'a str,
    status: &'static str,
    #[serde(flatten)]
    payload: phoenix_core::domain::tool_wire::BashTombstonedPayload,
}

#[derive(serde::Serialize)]
struct BashKillPendingWakeObservation<'a> {
    contract_id: &'a str,
    status: &'static str,
    #[serde(flatten)]
    payload: phoenix_core::domain::tool_wire::BashKillPendingKernelPayload,
}

fn wake_bash_window(
    evidence: &phoenix_workflow::wake_profile::BashTerminalEvidence,
) -> phoenix_core::domain::tool_wire::BashRingWindow {
    const WAKE_TAIL_BYTES: usize = 80 * 1024;
    let mut remaining = WAKE_TAIL_BYTES;
    let mut truncated = evidence.final_tail_truncated_before;
    let mut lines: Vec<_> = evidence
        .final_tail
        .iter()
        .enumerate()
        .rev()
        .filter_map(|(ordinal, bytes)| {
            if remaining == 0 {
                truncated = true;
                return None;
            }
            let requested = bytes.len().min(remaining);
            let start = if requested == bytes.len() {
                0
            } else {
                bytes
                    .char_indices()
                    .map(|(index, _)| index)
                    .find(|index| *index >= bytes.len().saturating_sub(requested))
                    .unwrap_or(bytes.len())
            };
            let keep = bytes.len().saturating_sub(start);
            remaining -= keep;
            truncated |= start > 0;
            Some(phoenix_core::domain::tool_wire::BashRingLine {
                offset: evidence
                    .final_tail_start_offset
                    .saturating_add(u64::try_from(ordinal).unwrap_or(u64::MAX)),
                bytes: bytes.get(start..).unwrap_or_default().to_string(),
            })
        })
        .collect();
    lines.reverse();
    let start_offset = lines
        .first()
        .map_or(evidence.final_tail_end_offset, |line| line.offset);
    phoenix_core::domain::tool_wire::BashRingWindow {
        start_offset,
        end_offset: evidence.final_tail_end_offset,
        truncated_before: truncated,
        lines,
        partial: evidence.final_tail_partial.clone(),
    }
}

fn timestamp_seconds(timestamp: Timestamp) -> String {
    timestamp.0.to_string()
}

fn timestamp_rfc3339(timestamp: Timestamp) -> String {
    chrono::DateTime::from_timestamp(i64::try_from(timestamp.0).unwrap_or(i64::MAX), 0)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        .to_rfc3339()
}

#[derive(serde::Serialize)]
struct BashWakeResolutionObservation<'a, R: serde::Serialize> {
    contract_id: &'a str,
    handle: &'a str,
    status: &'static str,
    resolved_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<R>,
}

fn terminal_kind(
    terminal: &phoenix_workflow::wake_profile::WakeTerminalPayload,
) -> crate::runtime::WakeContractTerminalKind {
    match terminal {
        phoenix_workflow::wake_profile::WakeTerminalPayload::Fired { .. } => {
            crate::runtime::WakeContractTerminalKind::Fired
        }
        phoenix_workflow::wake_profile::WakeTerminalPayload::Cancelled { .. } => {
            crate::runtime::WakeContractTerminalKind::Cancelled
        }
        phoenix_workflow::wake_profile::WakeTerminalPayload::Expired { .. } => {
            crate::runtime::WakeContractTerminalKind::Expired
        }
        phoenix_workflow::wake_profile::WakeTerminalPayload::Forgotten { .. } => {
            crate::runtime::WakeContractTerminalKind::Forgotten
        }
    }
}

fn render_materialized_terminal_result(pending: &WakePendingDelivery) -> String {
    render_terminal_result(pending)
}

fn render_terminal_result(pending: &WakePendingDelivery) -> String {
    let rendered = match &pending.receipt.terminal {
        phoenix_workflow::wake_profile::WakeTerminalPayload::Fired {
            contract_id,
            evidence: WakeTerminalEvidence::Bash(evidence),
            ..
        } => match evidence.status {
            BashTerminalStatus::Exited | BashTerminalStatus::Killed => {
                serde_json::to_string(&BashTombstonedWakeObservation {
                    contract_id,
                    status: "tombstoned",
                    payload: phoenix_core::domain::tool_wire::BashTombstonedPayload {
                        handle: evidence.identity.handle_id.clone(),
                        cmd: evidence.cmd.clone(),
                        label: evidence.label.clone(),
                        final_cause: match evidence.status {
                            BashTerminalStatus::Exited => "exited".to_string(),
                            BashTerminalStatus::Killed => "killed".to_string(),
                            BashTerminalStatus::KillPendingKernel => unreachable!(),
                        },
                        exit_code: evidence.exit_code,
                        signal_number: evidence.signal_number,
                        duration_ms: evidence.duration_ms.unwrap_or_default(),
                        finished_at: timestamp_seconds(evidence.occurred_at),
                        kill_signal_sent: evidence.kill_signal_sent.clone(),
                        kill_attempted_at: evidence.kill_attempted_at.map(timestamp_seconds),
                        window: wake_bash_window(evidence),
                        display: None,
                        signal_sent: None,
                    },
                })
            }
            BashTerminalStatus::KillPendingKernel => {
                serde_json::to_string(&BashKillPendingWakeObservation {
                    contract_id,
                    status: "kill_pending_kernel",
                    payload: phoenix_core::domain::tool_wire::BashKillPendingKernelPayload {
                        handle: evidence.identity.handle_id.clone(),
                        cmd: evidence.cmd.clone(),
                        label: evidence.label.clone(),
                        window: wake_bash_window(evidence),
                        kill_signal_sent: evidence.kill_signal_sent.clone().unwrap_or_default(),
                        kill_attempted_at: timestamp_seconds(
                            evidence.kill_attempted_at.unwrap_or(evidence.occurred_at),
                        ),
                        display: None,
                        signal_sent: None,
                        waited_ms: None,
                    },
                })
            }
        },
        phoenix_workflow::wake_profile::WakeTerminalPayload::Cancelled {
            contract_id,
            resource: phoenix_workflow::wake_profile::WakeResourceIdentity::Bash(identity),
            reason,
            resolved_at,
        } => serde_json::to_string(&BashWakeResolutionObservation {
            contract_id,
            handle: &identity.handle_id,
            status: "cancelled",
            resolved_at: timestamp_rfc3339(*resolved_at),
            reason: Some(*reason),
        }),
        phoenix_workflow::wake_profile::WakeTerminalPayload::Expired {
            contract_id,
            resource: phoenix_workflow::wake_profile::WakeResourceIdentity::Bash(identity),
            resolved_at,
        } => serde_json::to_string(&BashWakeResolutionObservation {
            contract_id,
            handle: &identity.handle_id,
            status: "expired",
            resolved_at: timestamp_rfc3339(*resolved_at),
            reason: Option::<WakeForgottenReason>::None,
        }),
        phoenix_workflow::wake_profile::WakeTerminalPayload::Forgotten {
            contract_id,
            resource: phoenix_workflow::wake_profile::WakeResourceIdentity::Bash(identity),
            reason,
            resolved_at,
        } => serde_json::to_string(&BashWakeResolutionObservation {
            contract_id,
            handle: &identity.handle_id,
            status: "forgotten",
            resolved_at: timestamp_rfc3339(*resolved_at),
            reason: Some(*reason),
        }),
        terminal => serde_json::to_string(terminal),
    };
    rendered.unwrap_or_else(|_| "Wake completed; inspect display metadata for details.".to_string())
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

#[allow(clippy::large_enum_variant)]
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
    let duration = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() > 0));
    Timestamp(seconds)
}

fn inspect_live_bash(
    _handle: &Handle,
    _identity: &BashResourceIdentity,
    _live: &LiveData,
) -> InspectionOutcome {
    InspectionOutcome::LiveRetry
}

async fn inspect_bash_handle(
    handle: &Arc<phoenix_tools::bash::handle::Handle>,
    identity: &BashResourceIdentity,
) -> InspectionOutcome {
    let state = handle.state().await;
    match state.as_ref() {
        HandleState::Live(live) => {
            if matches!(
                *handle.exit_observer().borrow(),
                Some(phoenix_tools::bash::handle::ExitState::WaiterPanicked)
            ) {
                InspectionOutcome::Forgotten(WakeForgottenReason::BashWaiterPanicked)
            } else {
                inspect_live_bash(handle, identity, live)
            }
        }
        HandleState::Tombstoned(tomb) => {
            let status = match &tomb.final_cause {
                FinalCause::Exited { .. } => BashTerminalStatus::Exited,
                FinalCause::Killed { .. } => BashTerminalStatus::Killed,
            };
            let final_tail_start_offset = tomb
                .final_tail
                .first()
                .map_or(tomb.next_offset_at_exit, |line| line.offset);
            let tail = tomb
                .final_tail
                .iter()
                .map(|line| String::from_utf8_lossy(&line.bytes).into_owned())
                .collect();
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: identity.clone(),
                cmd: handle.cmd.clone(),
                label: handle.label.clone(),
                status,
                occurred_at: system_time_to_timestamp(tomb.finished_at),
                exit_code: tomb.exit_code,
                duration_ms: Some(tomb.duration_ms),
                signal_number: tomb.signal_number,
                kill_signal_sent: tomb.kill_signal_sent.map(|sig| sig.as_str().to_string()),
                kill_attempted_at: tomb.kill_attempted_at.map(system_time_to_timestamp),
                final_tail_start_offset,
                final_tail_end_offset: tomb.next_offset_at_exit,
                final_tail_truncated_before: final_tail_start_offset > 0,
                final_tail_partial: None,
                final_tail: tail,
            }))
        }
    }
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
                    Ok(inspect_bash_handle(&handle, identity).await)
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
                        .kill_exact_window(
                            &scope,
                            &evidence.identity.server_token,
                            &evidence.identity.window_id,
                        )
                        .await;
                }
            }
            Ok(())
        })
    }
}

fn work_scope_from_identity(
    scope: &phoenix_workflow::wake_profile::WorkScopeIdentity,
) -> ResourceScopeKey {
    ResourceScopeKey::Work(
        phoenix_core::work_scope::WorkScopeId::parse(scope.as_str())
            .expect("persisted wake identity has a non-empty work scope id"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_db::workflow::wake::WakeRegistrationOutcome;
    use phoenix_db::Database;
    use phoenix_workflow::wake_profile::{
        BashResourceIdentity, TmuxResourceIdentity, WakeRegistrationIntent, WorkScopeIdentity,
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

    struct BlockingInspector {
        started: tokio::sync::mpsc::UnboundedSender<()>,
        release: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<()>>,
    }

    impl TerminalInspector for BlockingInspector {
        fn inspect<'a>(
            &'a self,
            _binding: &'a phoenix_db::workflow::wake::WakeBindingRecord,
            _authority: &'a LocalAttemptAuthority,
            _observation_time: Timestamp,
        ) -> Pin<Box<dyn Future<Output = Result<InspectionOutcome, String>> + Send + 'a>> {
            Box::pin(async move {
                let _ = self.started.send(());
                tokio::time::timeout(Duration::from_secs(5), async {
                    self.release.lock().await.recv().await
                })
                .await
                .map_err(|_| "blocking inspector release timed out".to_string())?
                .ok_or_else(|| "blocking inspector release channel closed".to_string())?;
                Ok(InspectionOutcome::LiveRetry)
            })
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

    async fn open_repo() -> (Database, WakeRepository, WorkScopeIdentity) {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("conv", "conv", "/tmp", true, None, None)
            .await
            .unwrap();
        let scope = WorkScopeIdentity(
            conversation
                .work_scope_id
                .expect("created conversation has work scope")
                .as_str()
                .to_string(),
        );
        (db.clone(), WakeRepository::new(db.pool().clone()), scope)
    }

    async fn register_bash(
        repo: &WakeRepository,
        scope: &WorkScopeIdentity,
        handle: &str,
        expires_at: u64,
    ) -> u64 {
        let intent = WakeRegistrationIntent {
            contract_id: format!("contract-{handle}"),
            conversation_id: "conv".to_string(),
            root_conversation_id: "conv".to_string(),
            registration_scope: scope.clone(),
            resource: WakeResourceIdentity::Bash(BashResourceIdentity {
                work_scope: scope.clone(),
                handle_id: handle.to_string(),
            }),
            registering_tool_use_id: "tool-use".to_string(),
            registering_tool_round_id: "round-test".to_string(),
            registered_at: Timestamp(1),
            expires_at: Timestamp(expires_at),
        };
        let workflow_id = match repo.register(&intent, handle, Timestamp(1)).await.unwrap() {
            WakeRegistrationOutcome::Registered { workflow_id, .. }
            | WakeRegistrationOutcome::Replayed { workflow_id, .. } => workflow_id.0,
            WakeRegistrationOutcome::Conflict => panic!("unexpected conflict"),
        };
        repo.activate_for_test(phoenix_workflow::WorkflowId(workflow_id))
            .await
            .unwrap();
        workflow_id
    }

    async fn register_tmux(
        repo: &WakeRepository,
        scope: &WorkScopeIdentity,
        generation: &str,
        window_id: &str,
        expires_at: u64,
    ) -> u64 {
        let intent = WakeRegistrationIntent {
            contract_id: format!("contract-{window_id}"),
            conversation_id: "conv".to_string(),
            root_conversation_id: "conv".to_string(),
            registration_scope: scope.clone(),
            resource: WakeResourceIdentity::TmuxWindow(TmuxResourceIdentity {
                work_scope: scope.clone(),
                server_token: generation.to_string(),
                window_id: window_id.to_string(),
                completion_policy: TmuxCompletionPolicy::KeepOpen,
            }),
            registering_tool_use_id: "tool-use".to_string(),
            registering_tool_round_id: "round-test".to_string(),
            registered_at: Timestamp(1),
            expires_at: Timestamp(expires_at),
        };
        let workflow_id = match repo
            .register(&intent, window_id, Timestamp(1))
            .await
            .unwrap()
        {
            WakeRegistrationOutcome::Registered { workflow_id, .. }
            | WakeRegistrationOutcome::Replayed { workflow_id, .. } => workflow_id.0,
            WakeRegistrationOutcome::Conflict => panic!("unexpected conflict"),
        };
        repo.activate_for_test(phoenix_workflow::WorkflowId(workflow_id))
            .await
            .unwrap();
        workflow_id
    }

    async fn pending_count(repo: &WakeRepository) -> usize {
        repo.list_pending("conv").await.unwrap().len()
    }

    async fn accept_unmaterialized_direct_turn(db: &Database) {
        let payload = phoenix_core::domain::sm_event::PreparedDirectTurnPayload::from_parts(
            phoenix_core::domain::sm_event::SubmittedDirectTurnIdentity {
                text: "user message".to_string(),
                images: Vec::new(),
                files: Vec::new(),
                message_id: "user-1".to_string(),
                user_agent: None,
                skill_invocation: None,
                expansion_policy: phoenix_core::domain::sm_event::SubmittedDirectTurnExpansionPolicy::ExpandReferences,
            },
            phoenix_core::domain::sm_event::PreparedDirectTurnDelivery {
                text: "user message".to_string(),
                llm_text: None,
                images: Vec::new(),
                files: Vec::new(),
                user_agent: None,
                skill_invocation: None,
            },
        );
        phoenix_db::workflow::WorkflowRepository::new(db.pool().clone())
            .accept_authoritative_turn(&phoenix_db::workflow::AcceptAuthoritativeTurn {
                client_key: phoenix_workflow::ClientTurnKey::new("user-1").unwrap(),
                prepared: phoenix_workflow::PreparedTurn::from_exact_payload(
                    &phoenix_workflow::ConversationAuthority("conv".to_string()),
                    payload.to_exact_bytes().unwrap(),
                ),
                disposition: phoenix_workflow::AcceptedDisposition::Runtime,
                accepted_at: Timestamp(15),
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn materialized_wake_result_uses_shared_tool_output_cap() {
        let (_db, repo, scope) = open_repo().await;
        let workflow_id = register_bash(&repo, &scope, "b-1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: scope,
                    handle_id: "b-1".to_string(),
                },
                cmd: "test command".to_string(),
                label: None,
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(10),
                exit_code: Some(0),
                duration_ms: Some(5),
                signal_number: None,
                kill_signal_sent: None,
                kill_attempted_at: None,
                final_tail_start_offset: 0,
                final_tail_end_offset: 200,
                final_tail_truncated_before: false,
                final_tail_partial: None,
                final_tail: vec!["x".repeat(2_000); 200],
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
        let pending = repo.list_pending("conv").await.unwrap();

        let rendered = render_materialized_terminal_result(&pending[0]);

        assert!(rendered.len() <= 100 * 1024);
        let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed["truncated_before"], true);
        assert_eq!(parsed["start_offset"], 159);
        assert_eq!(parsed["end_offset"], 200);
        assert_eq!(parsed["lines"].as_array().unwrap().len(), 41);
        assert_eq!(parsed["lines"][40]["offset"], 199);
    }

    #[test]
    fn system_time_evidence_rounds_fractional_seconds_up_at_deadline() {
        assert_eq!(
            system_time_to_timestamp(std::time::UNIX_EPOCH + Duration::from_secs(10)),
            Timestamp(10)
        );
        assert_eq!(
            system_time_to_timestamp(
                std::time::UNIX_EPOCH + Duration::from_secs(10) + Duration::from_nanos(1),
            ),
            Timestamp(11)
        );
    }

    #[tokio::test]
    async fn fired_bash_wake_renders_wait_compatible_observation() {
        let (_db, repo, scope) = open_repo().await;
        let workflow_id = register_bash(&repo, &scope, "b-1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: scope.clone(),
                    handle_id: "b-1".to_string(),
                },
                cmd: "cargo test".to_string(),
                label: Some("tests".to_string()),
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(10),
                exit_code: Some(0),
                duration_ms: Some(25),
                signal_number: None,
                kill_signal_sent: None,
                kill_attempted_at: None,
                final_tail_start_offset: 40,
                final_tail_end_offset: 42,
                final_tail_truncated_before: true,
                final_tail_partial: None,
                final_tail: vec!["first".to_string(), "second".to_string()],
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

        let rendered: serde_json::Value =
            serde_json::from_str(&render_terminal_result(&pending)).unwrap();
        assert_eq!(rendered["contract_id"], "contract-b-1");
        assert_eq!(rendered["handle"], "b-1");
        assert_eq!(rendered["cmd"], "cargo test");
        assert_eq!(rendered["label"], "tests");
        assert_eq!(rendered["status"], "tombstoned");
        assert_eq!(rendered["final_cause"], "exited");
        assert_eq!(rendered["exit_code"], 0);
        assert_eq!(rendered["duration_ms"], 25);
        assert_eq!(rendered["finished_at"], "10");
        assert_eq!(rendered["start_offset"], 40);
        assert_eq!(rendered["end_offset"], 42);
        assert_eq!(rendered["truncated_before"], true);
        assert_eq!(
            rendered["lines"],
            serde_json::json!([
                {"offset": 40, "bytes": "first"},
                {"offset": 41, "bytes": "second"}
            ])
        );
        assert!(rendered.get("Fired").is_none());
        assert!(rendered.get("resource").is_none());
        assert!(rendered.get("evidence").is_none());
    }

    #[tokio::test]
    async fn due_expiry_first_projects_terminal() {
        let (_db, repo, scope) = open_repo().await;
        register_bash(&repo, &scope, "b-1", 5).await;
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
        let (_db, repo, scope) = open_repo().await;
        let workflow_id = register_bash(&repo, &scope, "b-1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: scope.clone(),
                    handle_id: "b-1".to_string(),
                },
                cmd: "test command".to_string(),
                label: None,
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(10),
                exit_code: Some(0),
                duration_ms: Some(5),
                signal_number: None,
                kill_signal_sent: None,
                kill_attempted_at: None,
                final_tail_start_offset: 0,
                final_tail_end_offset: 1,
                final_tail_truncated_before: false,
                final_tail_partial: None,
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
        let (_db, repo, scope) = open_repo().await;
        let workflow_id = register_tmux(&repo, &scope, "g1", "w1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
                identity: TmuxResourceIdentity {
                    work_scope: scope.clone(),
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
        let (_db, repo, scope) = open_repo().await;
        let workflow_id = register_tmux(&repo, &scope, "g1", "w1", 50).await;
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
        let (_db, repo, scope) = open_repo().await;
        register_bash(&repo, &scope, "b-1", 50).await;
        let worker = WakeWorker::new(
            repo.clone(),
            Arc::new(MockInspector::new()),
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(1),
        );
        let wait = worker.run_once().await.unwrap();
        assert_eq!(pending_count(&repo).await, 0);
        assert_eq!(wait, LIVE_HANDLE_POLL_INTERVAL);
    }

    #[tokio::test]
    async fn waiter_panicked_bash_resolves_as_forgotten_without_polling_to_expiry() {
        use phoenix_tools::bash::handle::{ExitState, Handle, HandleId};
        use phoenix_tools::bash::ring::RING_BUFFER_BYTES;

        let (_db, _repo, scope) = open_repo().await;
        let handle = Handle::new_live(
            work_scope_from_identity(&scope),
            HandleId::new("b-1"),
            "test command".to_string(),
            None,
            123,
            123,
            RING_BUFFER_BYTES,
        );
        handle.publish_exit(ExitState::WaiterPanicked);

        let outcome = inspect_bash_handle(
            &handle,
            &BashResourceIdentity {
                work_scope: scope,
                handle_id: "b-1".to_string(),
            },
        )
        .await;

        assert!(matches!(
            outcome,
            InspectionOutcome::Forgotten(WakeForgottenReason::BashWaiterPanicked)
        ));
    }

    #[tokio::test]
    async fn kill_pending_bash_remains_live_until_true_terminal_exit() {
        use phoenix_core::domain::kill_signal::KillSignal;
        use phoenix_tools::bash::handle::{Handle, HandleId};
        use phoenix_tools::bash::ring::RING_BUFFER_BYTES;

        let (_db, repo, scope) = open_repo().await;
        register_bash(&repo, &scope, "b-1", 50).await;
        let bash = Arc::new(phoenix_tools::BashHandleRegistry::new());
        let resource_scope = work_scope_from_identity(&scope);
        let handle = Handle::new_live(
            resource_scope.clone(),
            HandleId::new("b-1"),
            "sleep 60".to_string(),
            None,
            123,
            123,
            RING_BUFFER_BYTES,
        );
        let attempted_at = std::time::UNIX_EPOCH + Duration::from_secs(9);
        assert!(
            handle
                .mark_kill_pending_kernel(KillSignal::Term, attempted_at)
                .await
        );
        let live = handle.state().await;
        let phoenix_tools::bash::handle::HandleState::Live(live) = live.as_ref() else {
            panic!("expected live handle");
        };
        live.ring.lock().await.append(b"unterminated");
        bash.get_or_create(&resource_scope)
            .await
            .write()
            .await
            .insert(handle);
        let worker = WakeWorker::new(
            repo.clone(),
            Arc::new(RuntimeRegistryInspector::new(
                bash,
                Arc::new(phoenix_tools::TmuxRegistry::new()),
            )),
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(1),
        );

        worker.run_once().await.unwrap();

        assert!(repo.list_pending("conv").await.unwrap().is_empty());
        assert_eq!(
            repo.list_observation_candidates(Timestamp(10), None, 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn slow_terminal_inspection_does_not_hold_process_wide_acceptance_lock() {
        let (db, repo, scope) = open_repo().await;
        register_bash(&repo, &scope, "b-1", 50).await;
        let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release_rx) = tokio::sync::mpsc::unbounded_channel();
        let worker = WakeWorker::new(
            repo,
            Arc::new(BlockingInspector {
                started: started_tx,
                release: tokio::sync::Mutex::new(release_rx),
            }),
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(1),
        );
        let manager = Arc::new(crate::runtime::RuntimeManager::new(
            db,
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            phoenix_core::platform::PlatformCapability::None {
                details: "test".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        ));
        let task = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { worker.run_once_with_manager(Some(&manager)).await }
        });
        tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await
            .expect("inspector start signal timed out")
            .expect("inspector start channel closed");

        let unrelated_guard = manager.lock_steering_acceptance().await;
        drop(unrelated_guard);
        release_tx.send(()).unwrap();

        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("worker completion timed out")
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn startup_restart_discovery_marks_missing_bash_forgotten() {
        let (_db, repo, scope) = open_repo().await;
        register_bash(&repo, &scope, "missing", 50).await;
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
        let pending = repo.list_pending("conv").await.unwrap();
        assert!(matches!(
            pending[0].receipt.terminal,
            phoenix_workflow::wake_profile::WakeTerminalPayload::Forgotten {
                reason: WakeForgottenReason::PhoenixRestart,
                ..
            }
        ));
        let rendered: serde_json::Value =
            serde_json::from_str(&render_terminal_result(&pending[0])).unwrap();
        assert_eq!(rendered["status"], "forgotten");
        assert_eq!(rendered["handle"], "missing");
        assert_eq!(rendered["reason"], "phoenix_restart");
        assert!(rendered.get("Forgotten").is_none());

        worker.run_once().await.unwrap();
        assert_eq!(pending_count(&repo).await, 1);
    }

    #[tokio::test]
    async fn restart_lost_bash_delivers_one_interruption_and_unparks_conversation() {
        let (db, repo, scope) = open_repo().await;
        register_bash(&repo, &scope, "missing", 50).await;
        let worker = WakeWorker::new(
            repo.clone(),
            Arc::new(RuntimeRegistryInspector::new(
                Arc::new(phoenix_tools::BashHandleRegistry::new()),
                Arc::new(phoenix_tools::TmuxRegistry::new()),
            )),
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(99),
        );

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

        worker.run_once_with_manager(Some(&manager)).await.unwrap();
        let messages_after_first_delivery = db.get_messages("conv").await.unwrap();
        let interruption = messages_after_first_delivery
            .last()
            .expect("wake interruption");
        assert!(matches!(
            &interruption.content,
            crate::db::MessageContent::User(user)
                if user.is_meta
                    && user.text.contains("forgotten")
                    && user.text.contains("phoenix_restart")
        ));
        assert!(!handle_is_idle(&handle));

        worker.run_once().await.unwrap();
        deliver_pending(&manager, &repo, Timestamp(21), None)
            .await
            .unwrap();
        assert_eq!(
            db.get_messages("conv").await.unwrap().len(),
            messages_after_first_delivery.len()
        );
    }

    #[tokio::test]
    async fn overdue_missing_bash_is_forgotten_before_expiry() {
        let (_db, repo, scope) = open_repo().await;
        register_bash(&repo, &scope, "missing", 50).await;
        let inspector = RuntimeRegistryInspector::new(
            Arc::new(phoenix_tools::BashHandleRegistry::new()),
            Arc::new(phoenix_tools::TmuxRegistry::new()),
        );
        let worker = WakeWorker::new(
            repo.clone(),
            Arc::new(inspector),
            Arc::new(TestClock::new(100)),
            ProcessIncarnation(99),
        );

        worker.run_once().await.unwrap();

        let pending = repo.list_pending("conv").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].receipt.terminal,
            phoenix_workflow::wake_profile::WakeTerminalPayload::Forgotten {
                reason: WakeForgottenReason::PhoenixRestart,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn startup_restart_discovery_reuses_live_tmux_socket_without_registry_entry() {
        if which::which("tmux").is_err() {
            return;
        }
        let (_db, repo, scope) = open_repo().await;
        let tmux_owner = phoenix_tools::tmux::test_server::TestTmuxServerOwner::new();
        let cwd_tmp = tempfile::TempDir::new().unwrap();
        let tmux = Arc::new(tmux_owner.registry());
        let resource_scope = crate::work_scope::ResourceScopeKey::Work(
            crate::work_scope::WorkScopeId::parse(&scope.0).unwrap(),
        );
        let server = tmux
            .ensure_live(&resource_scope, cwd_tmp.path(), None, None)
            .await
            .unwrap();
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
        let fresh_registry = Arc::new(tmux_owner.registry());
        let workflow_id = register_tmux(&repo, &scope, &server_token, &window_id, 50).await;
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
        tmux_owner.shutdown();
    }

    #[tokio::test]
    async fn stale_process_fencing_prevents_duplicate_projection() {
        let (_db, repo, scope) = open_repo().await;
        let workflow_id = register_bash(&repo, &scope, "b-1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: scope.clone(),
                    handle_id: "b-1".to_string(),
                },
                cmd: "test command".to_string(),
                label: None,
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(10),
                exit_code: Some(0),
                duration_ms: Some(5),
                signal_number: None,
                kill_signal_sent: None,
                kill_attempted_at: None,
                final_tail_start_offset: 0,
                final_tail_end_offset: 0,
                final_tail_truncated_before: false,
                final_tail_partial: None,
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

    fn register_input(
        scope: &WorkScopeIdentity,
        handle: &str,
        fingerprint: &str,
        max_wait_seconds: u64,
    ) -> RegisterWakeInput {
        RegisterWakeInput {
            contract_id: format!("contract-{handle}"),
            conversation_id: "conv".to_string(),
            root_conversation_id: "root".to_string(),
            registering_tool_use_id: "tool-use".to_string(),
            registering_tool_round_id: "round-test".to_string(),
            registration_scope: scope.clone(),
            resource: WakeResourceIdentity::Bash(BashResourceIdentity {
                work_scope: scope.clone(),
                handle_id: handle.to_string(),
            }),
            max_wait_seconds,
            prepared_fingerprint: fingerprint.to_string(),
        }
    }

    #[tokio::test]
    async fn production_registration_waits_for_lifecycle_acceptance_lock() {
        let (_db, repo, scope) = open_repo().await;
        let (kick_tx, _) = watch::channel(0u64);
        let acceptance_lock = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let lifecycle_guard = acceptance_lock.lock().await;
        let registrar = ProductionWakeRegistrar::new(
            repo,
            kick_tx,
            Arc::clone(&acceptance_lock),
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        );
        let registration = tokio::spawn(async move {
            registrar
                .register(register_input(&scope, "b-1", "fp-1", 50))
                .await
        });

        tokio::task::yield_now().await;
        assert!(!registration.is_finished());
        drop(lifecycle_guard);

        assert!(matches!(
            registration.await.unwrap().unwrap(),
            RegisteredWake::Registered { .. }
        ));
    }

    #[tokio::test]
    async fn production_registrar_replays_and_conflicts_exactly() {
        let (_db, repo, scope) = open_repo().await;
        let (kick_tx, kick_rx) = watch::channel(0u64);
        let pending_activation = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        let registrar = ProductionWakeRegistrar::new(
            repo,
            kick_tx,
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            Arc::clone(&pending_activation),
        );

        let before_registration = Timestamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
        let first = registrar
            .register(register_input(&scope, "b-1", "fp-1", 50))
            .await
            .unwrap();
        let replay = registrar
            .register(register_input(&scope, "b-1", "fp-1", 50))
            .await
            .unwrap();
        let conflict = registrar
            .register(register_input(&scope, "b-1", "fp-2", 50))
            .await
            .unwrap();

        let (first_id, first_expiry) = match first {
            RegisteredWake::Registered {
                workflow_id,
                expires_at,
            } => (workflow_id, expires_at),
            other => panic!("expected registration, got {other:?}"),
        };
        assert!(first_expiry.0 >= before_registration.0.saturating_add(50));
        assert_eq!(
            replay,
            RegisteredWake::Replayed {
                workflow_id: first_id,
                expires_at: first_expiry,
            }
        );
        assert_eq!(conflict, RegisteredWake::Conflict);
        assert!(pending_activation.lock().unwrap().contains(&first_id));
        assert_eq!(*kick_rx.borrow(), 0);
        registrar.notify_activation_committed(first_id);
        assert_eq!(*kick_rx.borrow(), 1);
        assert!(!pending_activation.lock().unwrap().contains(&first_id));
    }

    #[tokio::test]
    async fn production_registrar_cancel_kicks_and_replays() {
        let (_db, repo, scope) = open_repo().await;
        let workflow_id = register_bash(&repo, &scope, "b-1", 50).await;
        let (kick_tx, kick_rx) = watch::channel(0u64);
        let registrar = ProductionWakeRegistrar::new(
            repo,
            kick_tx,
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        );

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

        let registration_replay = registrar
            .register(register_input(&scope, "b-1", "b-1", 50))
            .await
            .unwrap();
        assert!(matches!(
            registration_replay,
            RegisteredWake::Replayed {
                workflow_id: replayed_id,
                ..
            } if replayed_id == phoenix_workflow::WorkflowId(workflow_id)
        ));
    }

    #[tokio::test]
    async fn kick_preempts_deadline_wait() {
        let (_db, repo, scope) = open_repo().await;
        register_bash(&repo, &scope, "b-1", 50).await;
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
        assert_eq!(observed_sleep, LIVE_HANDLE_POLL_INTERVAL);
        tx.send(1).unwrap();
        join.abort();
    }

    #[tokio::test]
    async fn live_handle_poll_is_short_and_bounded_by_expiry() {
        let (_db, repo, scope) = open_repo().await;
        register_bash(&repo, &scope, "b-1", 12).await;
        let clock = Arc::new(TestClock::new(10));
        let worker = WakeWorker::new(
            repo,
            Arc::new(MockInspector::new()),
            clock,
            ProcessIncarnation(1),
        );

        assert_eq!(worker.run_once().await.unwrap(), LIVE_HANDLE_POLL_INTERVAL);
    }

    #[tokio::test]
    async fn post_expiry_terminal_evidence_yields_to_expiry() {
        let (_db, repo, scope) = open_repo().await;
        register_bash(&repo, &scope, "b-1", 12).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            1,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: scope.clone(),
                    handle_id: "b-1".to_string(),
                },
                cmd: "test command".to_string(),
                label: None,
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(13),
                exit_code: Some(0),
                duration_ms: Some(1),
                signal_number: None,
                kill_signal_sent: None,
                kill_attempted_at: None,
                final_tail_start_offset: 0,
                final_tail_end_offset: 0,
                final_tail_truncated_before: false,
                final_tail_partial: None,
                final_tail: vec![],
            })),
        );
        let worker = WakeWorker::new(
            repo.clone(),
            inspector,
            Arc::new(TestClock::new(14)),
            ProcessIncarnation(1),
        );

        worker.run_once().await.unwrap();

        let pending = repo.list_pending("conv").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].receipt.terminal,
            phoenix_workflow::wake_profile::WakeTerminalPayload::Expired { .. }
        ));
        let rendered: serde_json::Value =
            serde_json::from_str(&render_terminal_result(&pending[0])).unwrap();
        assert_eq!(rendered["status"], "expired");
        assert_eq!(rendered["handle"], "b-1");
        assert!(rendered.get("Expired").is_none());
    }

    #[tokio::test]
    async fn deadline_advances_to_expiry() {
        let (_db, repo, scope) = open_repo().await;
        register_bash(&repo, &scope, "b-1", 12).await;
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
        let (_db, repo, scope) = open_repo().await;
        let workflow_id = register_bash(&repo, &scope, "b-1", 12).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: scope.clone(),
                    handle_id: "b-1".to_string(),
                },
                cmd: "test command".to_string(),
                label: None,
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(11),
                exit_code: Some(0),
                duration_ms: Some(5),
                signal_number: None,
                kill_signal_sent: None,
                kill_attempted_at: None,
                final_tail_start_offset: 0,
                final_tail_end_offset: 1,
                final_tail_truncated_before: false,
                final_tail_partial: None,
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
    async fn forgotten_persistence_error_releases_authority_for_immediate_retry() {
        let (db, repo, scope) = open_repo().await;
        let workflow_id = register_bash(&repo, &scope, "b-1", 50).await;
        sqlx::query(
            "CREATE TRIGGER reject_forgotten_receipt BEFORE INSERT ON wake_terminal_receipts
             WHEN NEW.terminal_kind = 'Forgotten'
             BEGIN SELECT RAISE(ABORT, 'injected forgotten persistence failure'); END",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Forgotten(WakeForgottenReason::PhoenixRestart),
        );
        let worker = WakeWorker::new(
            repo.clone(),
            inspector,
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(1),
        );

        worker.run_once().await.unwrap();

        let candidates = repo
            .list_observation_candidates(Timestamp(10), None, 10)
            .await
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].workflow_id,
            phoenix_workflow::WorkflowId(workflow_id)
        );
    }

    #[tokio::test]
    async fn inspection_error_releases_authority_for_immediate_retry() {
        let (_db, repo, scope) = open_repo().await;
        let workflow_id = register_bash(&repo, &scope, "b-1", 50).await;
        let worker = WakeWorker::new(
            repo.clone(),
            Arc::new(FlakyInspector::new(1)),
            Arc::new(TestClock::new(10)),
            ProcessIncarnation(1),
        );

        worker.run_once().await.unwrap();
        let candidates = repo
            .list_observation_candidates(Timestamp(10), None, 10)
            .await
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].workflow_id,
            phoenix_workflow::WorkflowId(workflow_id)
        );
    }

    #[tokio::test]
    async fn worker_retries_after_transient_inspection_error() {
        let (_db, repo, scope) = open_repo().await;
        register_bash(&repo, &scope, "b-1", 50).await;
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
    async fn accepted_direct_turn_prevents_wake_materialization_and_adoption() {
        let (db, repo, scope) = open_repo().await;
        let workflow_id = register_bash(&repo, &scope, "b-1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: scope,
                    handle_id: "b-1".to_string(),
                },
                cmd: "test command".to_string(),
                label: None,
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(10),
                exit_code: Some(0),
                duration_ms: Some(5),
                signal_number: None,
                kill_signal_sent: None,
                kill_attempted_at: None,
                final_tail_start_offset: 0,
                final_tail_end_offset: 1,
                final_tail_truncated_before: false,
                final_tail_partial: None,
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
        let pending = repo.list_pending("conv").await.unwrap();
        let delivery_id = pending[0].canonical_delivery.delivery_id;

        accept_unmaterialized_direct_turn(&db).await;
        let manager = Arc::new(crate::runtime::RuntimeManager::new(
            db.clone(),
            Arc::new(phoenix_llm::ModelRegistry::new_empty()),
            phoenix_core::platform::PlatformCapability::None {
                details: "test".into(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        ));

        deliver_pending(&manager, &repo, Timestamp(20), None)
            .await
            .unwrap();

        assert!(repo
            .get_delivery_message_link(phoenix_workflow::WorkflowId(workflow_id), delivery_id,)
            .await
            .unwrap()
            .is_none());
        assert_eq!(repo.list_pending("conv").await.unwrap().len(), 1);
        assert!(db.get_messages("conv").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn recover_pending_deliveries_preallocates_broadcaster_sequence_for_materialized_message()
    {
        let (db, repo, scope) = open_repo().await;
        let workflow_id = register_bash(&repo, &scope, "b-1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: scope.clone(),
                    handle_id: "b-1".to_string(),
                },
                cmd: "test command".to_string(),
                label: None,
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(10),
                exit_code: Some(0),
                duration_ms: Some(5),
                signal_number: None,
                kill_signal_sent: None,
                kill_attempted_at: None,
                final_tail_start_offset: 0,
                final_tail_end_offset: 1,
                final_tail_truncated_before: false,
                final_tail_partial: None,
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
        let mut event_rx = handle.broadcast_tx.subscribe();
        let _ = handle.broadcast_tx.next_seq();
        let _ = handle.broadcast_tx.next_seq();

        deliver_pending(&manager, &repo, Timestamp(20), None)
            .await
            .unwrap();

        let messages = db.get_messages("conv").await.unwrap();
        let wake = messages.last().expect("wake message persisted");
        assert_eq!(wake.sequence_id, 4);
        assert!(matches!(
            &wake.content,
            crate::db::MessageContent::User(user) if user.is_meta && user.text.contains("done")
        ));
        let terminal = loop {
            match event_rx.try_recv() {
                Ok(crate::runtime::SseEvent::WakeContractTerminal {
                    workflow_id,
                    contract_id,
                    receipt_id,
                    delivery_id,
                    terminal_kind,
                    ..
                }) => {
                    break (
                        workflow_id,
                        contract_id,
                        receipt_id,
                        delivery_id,
                        terminal_kind,
                    )
                }
                Ok(_) => {}
                Err(error) => panic!("terminal event missing: {error}"),
            }
        };
        assert_eq!(terminal.0, workflow_id);
        assert_eq!(terminal.1, "contract-b-1");
        assert!(terminal.2 > 0);
        assert!(terminal.3 > 0);
        assert_eq!(terminal.4, crate::runtime::WakeContractTerminalKind::Fired);
    }

    #[tokio::test]
    async fn newly_obtained_non_idle_runtime_is_not_delivery_eligible() {
        let (db, _repo, _scope) = open_repo().await;
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
            .event_tx
            .send(crate::state_machine::Event::UserMessage {
                text: "resume".into(),
                llm_text: None,
                images: vec![],
                files: vec![],
                message_id: "user-1".into(),
                user_agent: None,
                skill_invocation: None,
            })
            .await
            .unwrap();
        let mut state_rx = handle.state_rx.clone();
        tokio::time::timeout(Duration::from_secs(1), async {
            while matches!(*state_rx.borrow(), crate::state_machine::ConvState::Idle) {
                state_rx.changed().await.unwrap();
            }
        })
        .await
        .unwrap();

        assert!(!handle_is_idle(&handle));
    }

    #[tokio::test]
    async fn stale_runtime_identity_does_not_accept_wake_event() {
        let (db, repo, _scope) = open_repo().await;
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
        let stale = crate::runtime::ConversationHandle {
            identity: Arc::new(()),
            ..handle.clone()
        };
        adopt_materialized_delivery(&manager, &stale, &repo, "conv", &[], Timestamp(1))
            .await
            .unwrap();
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn mixed_materialized_batch_uses_batch_auto_resume_decision() {
        let (db, repo, scope) = open_repo().await;
        let fired_id = register_bash(&repo, &scope, "b-fired", 50).await;
        let cancelled_id = register_bash(&repo, &scope, "b-cancelled", 50).await;
        let (kick_tx, _) = watch::channel(0u64);
        ProductionWakeRegistrar::new(
            repo.clone(),
            kick_tx,
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        )
        .cancel(CancelWakeInput {
            workflow_id: phoenix_workflow::WorkflowId(cancelled_id),
            timestamp: Timestamp(5),
            reason: phoenix_workflow::wake_profile::WakeCancellationReason::ExplicitCancel,
        })
        .await
        .unwrap();
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            fired_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: scope.clone(),
                    handle_id: "b-fired".to_string(),
                },
                cmd: "test command".to_string(),
                label: None,
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(10),
                exit_code: Some(0),
                duration_ms: Some(5),
                signal_number: None,
                kill_signal_sent: None,
                kill_attempted_at: None,
                final_tail_start_offset: 0,
                final_tail_end_offset: 1,
                final_tail_truncated_before: false,
                final_tail_partial: None,
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

        let pending = repo.list_pending("conv").await.unwrap();
        assert_eq!(pending.len(), 2);
        assert!(matches!(
            pending[0].receipt.terminal,
            phoenix_workflow::wake_profile::WakeTerminalPayload::Fired { .. }
        ));
        let mut pending = pending;
        sort_pending_for_materialization(&mut pending);
        assert!(pending[0].receipt.resolution_ordinal < pending[1].receipt.resolution_ordinal);
        for (index, delivery) in pending.iter().enumerate() {
            let auto_resume = !matches!(
                delivery.receipt.terminal,
                phoenix_workflow::wake_profile::WakeTerminalPayload::Cancelled { .. }
            );
            repo.materialize_pending_delivery_message(&MaterializePendingDeliveryMessageInput {
                workflow_id: delivery.workflow_id,
                delivery_id: delivery.canonical_delivery.delivery_id,
                conversation_id: "conv".to_string(),
                rendered_content: render_terminal_result(delivery),
                display_data: None,
                auto_resume,
                created_at: Timestamp(20),
                sequence_id: Some(i64::try_from(index + 1).unwrap()),
            })
            .await
            .unwrap();
        }
        assert!(matches!(
            pending[0].receipt.terminal,
            phoenix_workflow::wake_profile::WakeTerminalPayload::Cancelled { .. }
        ));
        let cancelled: serde_json::Value =
            serde_json::from_str(&render_terminal_result(&pending[0])).unwrap();
        assert_eq!(cancelled["status"], "cancelled");
        assert_eq!(cancelled["handle"], "b-cancelled");
        assert_eq!(cancelled["reason"], "explicit_cancel");
        assert!(cancelled.get("Cancelled").is_none());

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
        let mut state_rx = handle.state_rx.clone();
        adopt_materialized_delivery(
            &manager,
            &handle,
            &repo,
            "conv",
            &pending
                .iter()
                .map(|delivery| {
                    (
                        delivery.workflow_id,
                        delivery.canonical_delivery.delivery_id,
                    )
                })
                .collect::<Vec<_>>(),
            Timestamp(21),
        )
        .await
        .unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if matches!(
                    *state_rx.borrow(),
                    crate::state_machine::ConvState::LlmRequesting { .. }
                ) {
                    break;
                }
                state_rx.changed().await.unwrap();
            }
        })
        .await
        .expect("mixed batch should resume the live runtime");
    }

    #[tokio::test]
    async fn already_materialized_wake_does_not_reset_newer_replay_events() {
        let (db, repo, scope) = open_repo().await;
        let workflow_id = register_bash(&repo, &scope, "b-1", 50).await;
        let inspector = Arc::new(MockInspector::new());
        inspector.push(
            workflow_id,
            InspectionOutcome::Terminal(WakeTerminalEvidence::Bash(BashTerminalEvidence {
                identity: BashResourceIdentity {
                    work_scope: scope.clone(),
                    handle_id: "b-1".to_string(),
                },
                cmd: "test command".to_string(),
                label: None,
                status: BashTerminalStatus::Exited,
                occurred_at: Timestamp(10),
                exit_code: Some(0),
                duration_ms: Some(5),
                signal_number: None,
                kill_signal_sent: None,
                kill_attempted_at: None,
                final_tail_start_offset: 0,
                final_tail_end_offset: 1,
                final_tail_truncated_before: false,
                final_tail_partial: None,
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
                    "terminal_kind": terminal_kind(&pending.receipt.terminal),
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

        let sequence_before = handle.broadcast_tx.current_seq();
        deliver_pending(&manager, &repo, Timestamp(21), None)
            .await
            .unwrap();

        let after = handle.broadcast_tx.snapshot_pending();
        assert_eq!(handle.broadcast_tx.current_seq(), sequence_before);
        assert_eq!(after.0, before.0);
        assert!(after.3.iter().any(|event| matches!(
            event,
            crate::runtime::SseEvent::Token { text, .. } if text == "newer"
        )));
    }

    #[test]
    fn process_incarnation_fits_signed_sqlite_range() {
        for _ in 0..128 {
            let incarnation = fresh_process_incarnation();
            assert!(i64::try_from(incarnation.0).is_ok());
        }
    }
}
