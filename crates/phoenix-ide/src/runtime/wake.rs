use async_trait::async_trait;
use chrono::{Duration, Utc};
use phoenix_core::domain::wake_contracts::{
    WakeContract, WakeContractHandle, WakeContractStatus, WakeForgottenReason, WakeTail,
    WakeTerminalOutcome, WakeTerminalPayload,
};
use phoenix_db::{
    Database, WakeRegistrationSuppressionOutcome, WakeResolutionInput, WakeResolutionOutcome,
};
use phoenix_tools::{
    WaitUntilTarget, WakeRegistrar, WakeRegistrarError, WakeRegistration, WakeRegistrationReceipt,
    WakeRegistrationTarget,
};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Default, PartialEq, Eq)]
struct WakeCauseCounts {
    fired: usize,
    cancelled: usize,
    expired: usize,
    forgotten: usize,
}

impl WakeCauseCounts {
    fn from_items(items: &[phoenix_core::domain::wake_contracts::WakeInboxItem]) -> Self {
        let mut counts = Self::default();
        for item in items {
            match item.cause.terminal_cause() {
                phoenix_core::domain::wake_contracts::WakeTerminalCause::Fired => counts.fired += 1,
                phoenix_core::domain::wake_contracts::WakeTerminalCause::Cancelled => {
                    counts.cancelled += 1;
                }
                phoenix_core::domain::wake_contracts::WakeTerminalCause::Expired => {
                    counts.expired += 1;
                }
                phoenix_core::domain::wake_contracts::WakeTerminalCause::Forgotten => {
                    counts.forgotten += 1;
                }
            }
        }
        counts
    }
}

fn resolution_latency_ms(contract: &WakeContract, outcome: &WakeTerminalOutcome) -> i64 {
    (outcome.resolved_at() - contract.registered_at)
        .num_milliseconds()
        .max(0)
}

fn log_resolution(contract: &WakeContract, outcome: &WakeTerminalOutcome, startup: bool) {
    let cause = outcome.terminal_cause().as_str();
    let latency_ms = resolution_latency_ms(contract, outcome);
    if let Some(reason) = outcome.forgotten_reason() {
        tracing::info!(
            contract_id = %contract.id,
            delivery_conversation_id = %contract.current_conversation_id,
            handle_kind = contract.handle.kind().as_str(),
            handle_id = %contract.handle.handle_id(),
            cause,
            forgotten_reason = reason.as_str(),
            latency_ms,
            startup,
            "wake contract resolved"
        );
    } else {
        tracing::info!(
            contract_id = %contract.id,
            delivery_conversation_id = %contract.current_conversation_id,
            handle_kind = contract.handle.kind().as_str(),
            handle_id = %contract.handle.handle_id(),
            cause,
            latency_ms,
            startup,
            "wake contract resolved"
        );
    }
}

/// Runtime-owned durable implementation of the tool crate's narrow wake capability.
pub(crate) struct DbWakeRegistrar {
    db: Database,
    broadcaster: Option<crate::runtime::SseBroadcaster>,
    successful_tool_uses: Mutex<HashSet<String>>,
}

impl DbWakeRegistrar {
    #[cfg(test)]
    pub(crate) fn new(db: Database) -> Self {
        Self {
            db,
            broadcaster: None,
            successful_tool_uses: Mutex::new(HashSet::new()),
        }
    }

    pub(crate) fn with_broadcaster(
        db: Database,
        broadcaster: crate::runtime::SseBroadcaster,
    ) -> Self {
        Self {
            db,
            broadcaster: Some(broadcaster),
            successful_tool_uses: Mutex::new(HashSet::new()),
        }
    }

    /// Check, without consuming, whether a completed serial tool round contains
    /// a registration that succeeded in this runtime.
    pub(crate) async fn has_round(&self, tool_use_ids: impl Iterator<Item = String>) -> bool {
        let successful = self.successful_tool_uses.lock().await;
        tool_use_ids.into_iter().any(|id| successful.contains(&id))
    }

    /// Check whether a round still owns at least one durable pending contract.
    /// Terminal history never authorizes parking.
    pub(crate) async fn has_pending_round(
        &self,
        conversation_id: &str,
        tool_use_ids: impl Iterator<Item = String>,
    ) -> Result<bool, String> {
        let ids = tool_use_ids.collect::<Vec<_>>();
        self.db
            .has_pending_wake_registration(conversation_id, &ids)
            .await
            .map_err(|error| error.to_string())
    }

    /// Consume the successful registrations represented by a completed serial tool round.
    pub(crate) async fn consume_round(&self, tool_use_ids: impl Iterator<Item = String>) -> bool {
        let mut successful = self.successful_tool_uses.lock().await;
        let mut found = false;
        for id in tool_use_ids {
            found |= successful.remove(&id);
        }
        found
    }
}

#[allow(clippy::too_many_lines)]
#[async_trait]
impl WakeRegistrar for DbWakeRegistrar {
    async fn register(
        &self,
        registration: WakeRegistration,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<WakeRegistrationReceipt, WakeRegistrarError> {
        let seconds = u16::try_from(registration.max_wait_seconds)
            .ok()
            .and_then(|value| {
                phoenix_core::domain::wake_contracts::WakeDeadlineSeconds::new(value).ok()
            })
            .ok_or_else(|| WakeRegistrarError("max_wait_seconds must be in 1..=1800".into()))?;
        let registered_at = Utc::now();
        let expires_at = registered_at + Duration::seconds(i64::from(seconds.get()));
        let contract_id = uuid::Uuid::new_v4().to_string();

        let (handle, initial) = match registration.target {
            WakeRegistrationTarget::Bash {
                handle_id,
                initial_terminal_evidence,
            } => (
                WakeContractHandle::Bash {
                    handle_id: handle_id.clone(),
                },
                initial_terminal_evidence.map(|evidence| WakeTerminalOutcome::Fired {
                    terminal_payload: WakeTerminalPayload::Bash {
                        bash: evidence.payload,
                    },
                    tails: wake_tails(evidence.tails),
                    resolved_at: evidence.observed_at,
                }),
            ),
            WakeRegistrationTarget::TmuxWindow {
                handle_id,
                initial_terminal_evidence,
            } => (
                WakeContractHandle::TmuxWindow {
                    handle_id: handle_id.clone(),
                },
                initial_terminal_evidence.map(|evidence| WakeTerminalOutcome::Fired {
                    terminal_payload: WakeTerminalPayload::TmuxWindow {
                        tmux_window: evidence.payload,
                    },
                    tails: wake_tails(evidence.tails),
                    resolved_at: evidence.observed_at,
                }),
            ),
        };
        let contract = WakeContract {
            id: contract_id.clone(),
            current_conversation_id: registration.conversation_id,
            registration_work_scope: registration.work_scope,
            handle: handle.clone(),
            registering_tool_use_id: Some(registration.tool_use_id.clone()),
            registered_at,
            expires_at,
            status: WakeContractStatus::Pending,
            terminal_cause: None,
            forgotten_reason: None,
            terminal_payload: None,
            resolved_at: None,
        };
        if cancellation.is_cancelled() {
            return Err(WakeRegistrarError("wake registration cancelled".into()));
        }
        self.db
            .register_wake_contract(&contract, initial.as_ref())
            .await
            .map_err(|error| WakeRegistrarError(error.to_string()))?;
        // Cancellation may race the commit. Compensate before exposing a receipt
        // or recording the live-executor success marker, so ToolAborted cannot
        // coexist with a pending auto-resume contract.
        if cancellation.is_cancelled() {
            let outcome = self
                .db
                .suppress_registration_after_tool_cancel(
                    &contract.id,
                    &contract.current_conversation_id,
                )
                .await
                .map_err(|error| WakeRegistrarError(error.to_string()))?;
            if outcome == WakeRegistrationSuppressionOutcome::CancellationWon {
                return Err(WakeRegistrarError("wake registration cancelled".into()));
            }
        }
        tracing::info!(
            contract_id = %contract.id,
            conversation_id = %contract.current_conversation_id,
            handle_kind = contract.handle.kind().as_str(),
            handle_id = %contract.handle.handle_id(),
            max_wait_seconds = seconds.get(),
            expires_at = %contract.expires_at,
            initial_terminal = initial.is_some(),
            "wake contract registered"
        );
        if let Some(outcome) = initial.as_ref() {
            log_resolution(&contract, outcome, false);
        }
        self.successful_tool_uses
            .lock()
            .await
            .insert(registration.tool_use_id.clone());

        let target = match handle.clone() {
            WakeContractHandle::Bash { handle_id } => WaitUntilTarget::Bash { handle_id },
            WakeContractHandle::TmuxWindow { handle_id } => {
                WaitUntilTarget::TmuxWindow { handle_id }
            }
        };
        let receipt = WakeRegistrationReceipt {
            contract_id: contract_id.clone(),
            target,
            expires_at,
            registering_tool_use_id: registration.tool_use_id.clone(),
        };
        if initial.is_none() {
            if let Some(broadcaster) = &self.broadcaster {
                let registration = phoenix_core::domain::wake_contracts::WakeContractRegistered {
                    conversation_id: contract.current_conversation_id,
                    contract_id,
                    handle: match handle {
                        WakeContractHandle::Bash { handle_id } => {
                            phoenix_core::domain::wake_contracts::WakeRegisteredHandle::Bash {
                                id: handle_id,
                            }
                        }
                        WakeContractHandle::TmuxWindow { handle_id } => {
                            phoenix_core::domain::wake_contracts::WakeRegisteredHandle::TmuxWindow {
                                id: handle_id,
                            }
                        }
                    },
                    expires_at: receipt.expires_at,
                    registering_tool_use_id: Some(registration.tool_use_id),
                };
                let _ = broadcaster.send_seq(|sequence_id| {
                    crate::runtime::SseEvent::WakeContractRegistered {
                        sequence_id,
                        registration,
                    }
                });
            }
        }
        Ok(receipt)
    }
}

fn wake_tails(lines: Vec<String>) -> Vec<WakeTail> {
    lines
        .into_iter()
        .enumerate()
        .map(|(ordinal, line)| WakeTail {
            ordinal: i64::try_from(ordinal).unwrap_or(i64::MAX),
            line,
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn observe_contract(
    db: &Database,
    bash_handles: &phoenix_tools::BashHandleRegistry,
    tmux_registry: &phoenix_tools::TmuxRegistry,
    contract: &WakeContract,
    startup: bool,
) -> Result<(), String> {
    let scope = &contract.registration_work_scope;
    let terminal = match &contract.handle {
        WakeContractHandle::Bash { handle_id } => {
            let inspection = bash_handles.inspect(scope, handle_id).await;
            let now = Utc::now();
            match inspection {
                phoenix_tools::BashHandleInspection::Terminal {
                    observed_at,
                    payload,
                    tails,
                } if observed_at <= contract.expires_at => Some(WakeTerminalOutcome::Fired {
                    terminal_payload: WakeTerminalPayload::Bash { bash: payload },
                    tails: wake_tails(tails),
                    resolved_at: observed_at,
                }),
                phoenix_tools::BashHandleInspection::Terminal { .. } => {
                    Some(WakeTerminalOutcome::Expired { resolved_at: now })
                }
                phoenix_tools::BashHandleInspection::Unknown
                | phoenix_tools::BashHandleInspection::Live
                    if now >= contract.expires_at =>
                {
                    Some(WakeTerminalOutcome::Expired { resolved_at: now })
                }
                phoenix_tools::BashHandleInspection::Unknown => {
                    Some(WakeTerminalOutcome::Forgotten {
                        forgotten_reason: if startup {
                            WakeForgottenReason::RuntimeUnrecoverableAfterRestart
                        } else {
                            WakeForgottenReason::HandleMissing
                        },
                        resolved_at: now,
                    })
                }
                phoenix_tools::BashHandleInspection::Live => None,
            }
        }
        WakeContractHandle::TmuxWindow { handle_id } => {
            let inspection = tmux_registry.inspect_window(scope, handle_id).await;
            let now = Utc::now();
            match inspection {
                Ok(phoenix_tools::TmuxWindowInspection::Terminal(evidence))
                    if chrono::DateTime::<Utc>::from(evidence.observed_at)
                        <= contract.expires_at =>
                {
                    let status = match evidence.status {
                        phoenix_tools::TmuxTerminalStatus::Exited => phoenix_core::domain::wake_contracts::WakeTmuxObservedStatus::ExitMarkerObserved,
                        phoenix_tools::TmuxTerminalStatus::Killed => phoenix_core::domain::wake_contracts::WakeTmuxObservedStatus::WindowKilled,
                    };
                    Some(WakeTerminalOutcome::Fired {
                        terminal_payload: WakeTerminalPayload::TmuxWindow {
                            tmux_window:
                                phoenix_core::domain::wake_contracts::WakeTmuxFiredPayload {
                                    status,
                                    exit_code: evidence.exit_code.map(i64::from),
                                    duration_ms: Some(
                                        i64::try_from(evidence.duration_ms).unwrap_or(i64::MAX),
                                    ),
                                },
                        },
                        tails: wake_tails(evidence.tail.lines().map(str::to_owned).collect()),
                        resolved_at: chrono::DateTime::<Utc>::from(evidence.observed_at),
                    })
                }
                Ok(phoenix_tools::TmuxWindowInspection::Terminal(_)) => {
                    Some(WakeTerminalOutcome::Expired { resolved_at: now })
                }
                Ok(
                    phoenix_tools::TmuxWindowInspection::Missing
                    | phoenix_tools::TmuxWindowInspection::Live,
                ) if now >= contract.expires_at => {
                    Some(WakeTerminalOutcome::Expired { resolved_at: now })
                }
                Ok(phoenix_tools::TmuxWindowInspection::Missing) => {
                    Some(WakeTerminalOutcome::Forgotten {
                        forgotten_reason: WakeForgottenReason::HandleMissing,
                        resolved_at: now,
                    })
                }
                Ok(phoenix_tools::TmuxWindowInspection::Live) => None,
                Err(error) if now >= contract.expires_at => {
                    tracing::warn!(%error, %handle_id, "tmux wake inspection failed after deadline; expiring contract");
                    Some(WakeTerminalOutcome::Expired { resolved_at: now })
                }
                Err(error) => {
                    tracing::warn!(%error, %handle_id, "tmux wake inspection failed; retaining pending contract");
                    None
                }
            }
        }
    };
    if let Some(outcome) = terminal {
        let resolution = db
            .resolve_terminal_wake_contract(&WakeResolutionInput {
                contract_id: contract.id.clone(),
                outcome: outcome.clone(),
            })
            .await
            .map_err(|error| error.to_string())?;
        match resolution {
            WakeResolutionOutcome::Resolved(_) => log_resolution(contract, &outcome, startup),
            WakeResolutionOutcome::AlreadyTerminal(existing_status) => {
                let attempted_cause = outcome.terminal_cause().as_str();
                let existing_cause = existing_status.as_str();
                tracing::debug!(
                    contract_id = %contract.id,
                    conversation_id = %contract.current_conversation_id,
                    handle_kind = contract.handle.kind().as_str(),
                    handle_id = %contract.handle.handle_id(),
                    existing_cause,
                    attempted_cause,
                    idempotency_conflict = existing_cause != attempted_cause,
                    startup,
                    "wake resolution found terminal contract"
                );
            }
        }
    }
    Ok(())
}

pub(crate) async fn reconcile_pending(manager: &Arc<super::RuntimeManager>, startup: bool) {
    let contracts = match manager.db().list_pending_wake_contracts().await {
        Ok(contracts) => contracts,
        Err(error) => {
            tracing::warn!(%error, "failed to list pending wake contracts");
            return;
        }
    };
    tracing::debug!(
        batch_count = contracts.len(),
        startup,
        "wake reconciliation batch loaded"
    );
    if startup {
        tracing::info!(
            pending_count = contracts.len(),
            "wake recovery loaded pending contracts"
        );
    }
    for contract in contracts {
        if startup {
            if let Some(tool_use_id) = contract.registering_tool_use_id.as_deref() {
                let receipt_message_id = format!("{tool_use_id}-result");
                match manager.db().message_exists(&receipt_message_id).await {
                    Ok(false) => {
                        match manager
                            .db()
                            .suppress_registration_after_tool_cancel(
                                &contract.id,
                                &contract.current_conversation_id,
                            )
                            .await
                        {
                            Ok(WakeRegistrationSuppressionOutcome::CancellationWon) => {
                                tracing::info!(contract_id = %contract.id, %tool_use_id, "suppressed wake registration whose tool receipt was lost before restart");
                                continue;
                            }
                            Ok(WakeRegistrationSuppressionOutcome::RegistrationWon(_)) => {}
                            Err(error) => {
                                tracing::warn!(contract_id = %contract.id, %error, "failed to suppress wake registration with no durable tool receipt");
                                continue;
                            }
                        }
                    }
                    Ok(true) => {}
                    Err(error) => {
                        tracing::warn!(contract_id = %contract.id, %error, "failed to verify durable wake registration receipt");
                        continue;
                    }
                }
            }
        }
        if let Err(error) = observe_contract(
            manager.db(),
            manager.bash_handles(),
            manager.tmux_registry(),
            &contract,
            startup,
        )
        .await
        {
            tracing::warn!(contract_id = %contract.id, %error, "wake observation failed");
        }
    }
}

/// Materialize deterministic inbox snapshots, then dispatch from the durable outbox.
#[allow(clippy::too_many_lines)]
pub(crate) async fn dispatch_pending(manager: &Arc<super::RuntimeManager>, startup: bool) {
    let conversations = match manager
        .db()
        .list_pending_wake_auto_resume_conversations()
        .await
    {
        Ok(conversations) => conversations,
        Err(error) => {
            tracing::warn!(%error, "failed to query wake dispatcher admission");
            return;
        }
    };
    for conversation_id in conversations {
        let Some(_claim) = manager.try_claim_wake_dispatch(&conversation_id) else {
            continue;
        };
        let conversation = match manager.db().get_conversation(&conversation_id).await {
            Ok(conversation) if !conversation.archived => conversation,
            Ok(_) => continue,
            Err(error) => {
                tracing::warn!(%conversation_id, %error, "wake dispatcher could not read conversation");
                continue;
            }
        };
        let state = manager
            .effective_conversation_state(&conversation_id)
            .await
            .unwrap_or(conversation.state);
        if !matches!(state, crate::state_machine::ConvState::Idle) {
            tracing::debug!(%conversation_id, "wake inbox remains pending; conversation not idle");
            continue;
        }
        let snapshot = match manager
            .db()
            .snapshot_pending_wake_inbox(&conversation_id)
            .await
        {
            Ok(snapshot) if snapshot.items.iter().any(|item| item.cause.auto_resume()) => snapshot,
            Ok(_) => continue,
            Err(error) => {
                tracing::warn!(%conversation_id, %error, "wake dispatcher could not snapshot inbox");
                continue;
            }
        };
        let cause_counts = WakeCauseCounts::from_items(&snapshot.items);
        tracing::debug!(
            conversation_id = %conversation_id,
            item_count = snapshot.items.len(),
            max_inbox_id = snapshot.max_inbox_id,
            fired_count = cause_counts.fired,
            cancelled_count = cause_counts.cancelled,
            expired_count = cause_counts.expired,
            forgotten_count = cause_counts.forgotten,
            "wake inbox snapshot materialized"
        );
        let text = match serde_json::to_string_pretty(&snapshot.items) {
            Ok(payload) => format!(
                "[Phoenix wake observations]\nThe following durable terminal observations arrived in inbox order:\n{payload}"
            ),
            Err(error) => {
                tracing::warn!(%conversation_id, %error, "wake dispatcher could not encode inbox");
                continue;
            }
        };
        let message_id = format!("wake-inbox-{}-{}", conversation_id, snapshot.max_inbox_id);
        let content = phoenix_db::MessageContent::User(phoenix_db::UserContent::meta(&text));
        match manager.db().message_exists(&message_id).await {
            Ok(true) => {
                tracing::debug!(%conversation_id, %message_id, "wake resume message already exists");
                continue;
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%conversation_id, %error, "wake dispatcher could not check deterministic resume message");
                continue;
            }
        }
        let conversation = match manager.db().get_conversation(&conversation_id).await {
            Ok(conversation) if !conversation.archived => conversation,
            Ok(_) => continue,
            Err(error) => {
                tracing::warn!(%conversation_id, %error, "wake dispatcher could not recheck conversation");
                continue;
            }
        };
        let state = manager
            .effective_conversation_state(&conversation_id)
            .await
            .unwrap_or(conversation.state);
        if !matches!(state, crate::state_machine::ConvState::Idle) {
            continue;
        }
        // A wake message participates in the same total SSE order as every runtime
        // event. Reserve only after every no-broadcast gate has passed.
        let handle = match manager.get_or_create(&conversation_id).await {
            Ok(handle) => handle,
            Err(error) => {
                tracing::warn!(%conversation_id, %error, "wake dispatcher could not create runtime broadcaster");
                continue;
            }
        };
        let (reserved_range, reserved_seqs) =
            handle.broadcast_tx.reserve_next_persisted_message_range(1);
        let reserved_seq = reserved_seqs[0];
        let outcome = manager
            .db()
            .persist_wake_inbox_snapshot_message(
                &conversation_id,
                snapshot.max_inbox_id,
                &message_id,
                reserved_seq,
                content,
            )
            .await;
        match outcome {
            Ok(outcome) if outcome.message_inserted => {
                tracing::debug!(
                    conversation_id = %conversation_id,
                    item_count = outcome.items.len(),
                    max_inbox_id = snapshot.max_inbox_id,
                    sequence_id = outcome.message.sequence_id,
                    "wake resume queued"
                );
                let _ = handle.broadcast_tx.send_message(outcome.message);
            }
            Ok(outcome) => {
                tracing::debug!(
                    conversation_id = %conversation_id,
                    max_inbox_id = snapshot.max_inbox_id,
                    existing_sequence_id = outcome.message.sequence_id,
                    reserved_seq,
                    "wake resume already queued"
                );
            }
            Err(error) => {
                tracing::warn!(%conversation_id, %error, "wake dispatcher could not atomically create resume outbox");
            }
        }
        drop(reserved_range);
    }

    let resumes = match manager.db().list_pending_wake_resumes().await {
        Ok(resumes) => resumes,
        Err(error) => {
            tracing::warn!(%error, "failed to list wake resume outbox");
            return;
        }
    };
    tracing::debug!(pending_count = resumes.len(), "wake resume outbox loaded");
    if startup {
        tracing::info!(
            pending_count = resumes.len(),
            "wake recovery loaded pending resumes"
        );
    }
    for resume in resumes {
        let conversation_id = &resume.conversation_id;
        let Some(_claim) = manager.try_claim_wake_dispatch(conversation_id) else {
            tracing::debug!(
                conversation_id = %conversation_id,
                max_inbox_id = resume.snapshot_max_inbox_id,
                "wake resume remains pending; dispatch already claimed"
            );
            continue;
        };
        let conversation = match manager.db().get_conversation(conversation_id).await {
            Ok(conversation) if !conversation.archived => conversation,
            Ok(_) => continue,
            Err(error) => {
                tracing::warn!(%conversation_id, %error, "wake dispatcher could not read conversation");
                continue;
            }
        };
        let state = match manager.effective_conversation_state(conversation_id).await {
            Some(state) => state,
            None => conversation.state,
        };
        if !matches!(state, crate::state_machine::ConvState::Idle) {
            tracing::debug!(
                conversation_id = %conversation_id,
                max_inbox_id = resume.snapshot_max_inbox_id,
                "wake resume remains pending; conversation not idle"
            );
            continue;
        }
        tracing::debug!(
            conversation_id = %conversation_id,
            max_inbox_id = resume.snapshot_max_inbox_id,
            "wake resume dispatch attempted"
        );
        if let Err(error) = manager
            .send_event(
                conversation_id,
                crate::state_machine::Event::WakeResume {
                    message_id: resume.message_id,
                    text: resume.text,
                },
            )
            .await
        {
            tracing::warn!(%conversation_id, %error, "wake dispatcher could not schedule durable resume");
        } else {
            tracing::debug!(
                conversation_id = %conversation_id,
                max_inbox_id = resume.snapshot_max_inbox_id,
                "wake resume dispatch event accepted"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::SseEvent;
    use async_trait::async_trait;
    use phoenix_core::domain::wake_contracts::{
        WakeBashFiredPayload, WakeBashObservedStatus, WakeInboxCause,
    };
    use phoenix_core::work_scope::WorkScope;
    use phoenix_llm::{LlmError, LlmRequest, LlmResponse, LlmService, ModelRegistry, TokenChunk};
    use phoenix_tools::{BashTool, Tool, ToolContext, WakeBashInitialTerminalEvidence};
    use serde_json::json;
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;

    struct BlockingLlm;

    #[async_trait]
    impl LlmService for BlockingLlm {
        async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
            std::future::pending().await
        }

        async fn complete_streaming(
            &self,
            _request: &LlmRequest,
            _tx: &broadcast::Sender<TokenChunk>,
        ) -> Result<LlmResponse, LlmError> {
            std::future::pending().await
        }

        #[allow(clippy::unnecessary_literal_bound)]
        fn model_id(&self) -> &str {
            "claude-sonnet-5"
        }
    }

    async fn test_manager() -> Arc<super::super::RuntimeManager> {
        let db = Database::open_in_memory().await.expect("database");
        Arc::new(super::super::RuntimeManager::new(
            db,
            Arc::new(ModelRegistry::for_test_with_sonnet(Arc::new(BlockingLlm))),
            crate::platform::PlatformCapability::None {
                details: "test".to_string(),
            },
            Arc::new(crate::tools::mcp::McpClientManager::new()),
            None,
        ))
    }

    async fn create_conversation(db: &Database, id: &str) {
        db.create_conversation(id, id, "/tmp", true, None, None)
            .await
            .expect("conversation");
    }

    fn pending_contract(id: &str, conversation_id: &str, handle_id: &str) -> WakeContract {
        let registered_at = Utc::now();
        WakeContract {
            id: id.to_string(),
            current_conversation_id: conversation_id.to_string(),
            registration_work_scope: WorkScope::Conversation(conversation_id.to_string()),
            handle: WakeContractHandle::Bash {
                handle_id: handle_id.to_string(),
            },
            registering_tool_use_id: Some(format!("tool-{id}")),
            registered_at,
            expires_at: registered_at + Duration::seconds(60),
            status: WakeContractStatus::Pending,
            terminal_cause: None,
            forgotten_reason: None,
            terminal_payload: None,
            resolved_at: None,
        }
    }

    fn fired_outcome(line: &str) -> WakeTerminalOutcome {
        WakeTerminalOutcome::Fired {
            terminal_payload: WakeTerminalPayload::Bash {
                bash: WakeBashFiredPayload {
                    status: WakeBashObservedStatus::Exited,
                    exit_code: Some(0),
                    duration_ms: Some(1),
                    signal_number: None,
                    kill_signal_sent: None,
                },
            },
            tails: wake_tails(vec![line.to_string()]),
            resolved_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn registrar_emits_committed_receipt_payload_once() {
        let db = Database::open_in_memory().await.expect("database");
        create_conversation(&db, "wake-edge").await;
        let broadcaster = crate::runtime::SseBroadcaster::new(8, 40);
        let mut events = broadcaster.subscribe();
        let registrar = DbWakeRegistrar::with_broadcaster(db.clone(), broadcaster);
        let receipt = registrar
            .register(
                WakeRegistration {
                    conversation_id: "wake-edge".to_string(),
                    tool_use_id: "tool-edge".to_string(),
                    work_scope: WorkScope::Conversation("wake-edge".to_string()),
                    target: WakeRegistrationTarget::Bash {
                        handle_id: "bash-edge".to_string(),
                        initial_terminal_evidence: None,
                    },
                    max_wait_seconds: 60,
                },
                CancellationToken::new(),
            )
            .await
            .expect("register");
        let persisted = db
            .get_wake_contract(&receipt.contract_id)
            .await
            .expect("read")
            .expect("persisted before event");
        let event = events.recv().await.expect("registration edge");
        let crate::runtime::SseEvent::WakeContractRegistered {
            sequence_id,
            registration,
        } = event
        else {
            panic!("unexpected event");
        };
        assert_eq!(sequence_id, 41);
        assert_eq!(
            registration.conversation_id,
            persisted.current_conversation_id
        );
        assert_eq!(registration.contract_id, receipt.contract_id);
        assert_eq!(
            registration.handle,
            phoenix_core::domain::wake_contracts::WakeRegisteredHandle::Bash {
                id: persisted.handle.handle_id().to_string()
            }
        );
        assert_eq!(registration.expires_at, persisted.expires_at);
        assert_eq!(
            registration.registering_tool_use_id,
            persisted.registering_tool_use_id
        );
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn registrar_persists_immediate_terminal_evidence_atomically() {
        let db = Database::open_in_memory().await.expect("database");
        db.create_conversation("wake-reg", "wake-reg", "/tmp", true, None, None)
            .await
            .expect("conversation");
        let registrar = DbWakeRegistrar::new(db.clone());
        let observed_at = Utc::now();
        let receipt = registrar
            .register(
                WakeRegistration {
                    conversation_id: "wake-reg".to_string(),
                    tool_use_id: "tool-wait-1".to_string(),
                    work_scope: WorkScope::Conversation("wake-reg".to_string()),
                    target: WakeRegistrationTarget::Bash {
                        handle_id: "bash-1".to_string(),
                        initial_terminal_evidence: Some(WakeBashInitialTerminalEvidence {
                            observed_at,
                            payload: WakeBashFiredPayload {
                                status: WakeBashObservedStatus::Exited,
                                exit_code: Some(0),
                                duration_ms: Some(10),
                                signal_number: None,
                                kill_signal_sent: None,
                            },
                            tails: vec!["done".to_string()],
                        }),
                    },
                    max_wait_seconds: 60,
                },
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .expect("register");

        let contract = db
            .get_wake_contract(&receipt.contract_id)
            .await
            .expect("read contract")
            .expect("contract");
        assert_eq!(contract.status, WakeContractStatus::Fired);
        let inbox = db
            .list_wake_inbox_items_for_conversation("wake-reg")
            .await
            .expect("inbox");
        assert_eq!(inbox.len(), 1);
        assert!(matches!(inbox[0].cause, WakeInboxCause::Fired { .. }));
        assert!(
            registrar
                .consume_round(std::iter::once("tool-wait-1".to_string()))
                .await,
            "immediate-terminal registration must park so its durable observation is consumed before another LLM request"
        );
    }

    #[tokio::test]
    async fn consume_round_removes_all_matching_ids_and_preserves_unrelated_ids() {
        let db = Database::open_in_memory().await.expect("database");
        create_conversation(&db, "round").await;
        let registrar = DbWakeRegistrar::new(db);
        for id in ["wait-a", "wait-b", "wait-c"] {
            registrar
                .register(
                    WakeRegistration {
                        conversation_id: "round".to_string(),
                        tool_use_id: id.to_string(),
                        work_scope: WorkScope::Conversation("round".to_string()),
                        target: WakeRegistrationTarget::Bash {
                            handle_id: format!("bash-{id}"),
                            initial_terminal_evidence: None,
                        },
                        max_wait_seconds: 60,
                    },
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
                .expect("register");
        }

        assert!(
            registrar
                .consume_round(
                    ["unrelated", "wait-b", "wait-a", "also-unrelated"]
                        .into_iter()
                        .map(str::to_string),
                )
                .await
        );
        assert!(
            !registrar
                .consume_round(["wait-a", "wait-b"].into_iter().map(str::to_string))
                .await
        );
        assert!(
            registrar
                .consume_round(["wait-c"].into_iter().map(str::to_string))
                .await
        );
    }

    #[tokio::test]
    async fn observe_live_bash_resolves_fired_with_real_registry_evidence() {
        let manager = test_manager().await;
        create_conversation(manager.db(), "observe-live").await;
        let context = ToolContext::new(
            CancellationToken::new(),
            "observe-live".to_string(),
            std::path::PathBuf::from("/tmp"),
            manager.browser_sessions().clone(),
            manager.bash_handles().clone(),
            Arc::new(ModelRegistry::new_empty()),
            manager.terminals.clone(),
            manager.tmux_registry().clone(),
            None,
        );
        let spawned = BashTool
            .run(
                json!({"op":"run","cmd":"printf wake-done","wait_seconds":0}),
                context,
            )
            .await;
        let payload = spawned.display_data().expect("bash response");
        let handle_id = payload["handle"].as_str().expect("handle").to_string();
        let contract = pending_contract("wake-live", "observe-live", &handle_id);
        manager
            .db()
            .insert_wake_contract(&contract)
            .await
            .expect("contract");

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if matches!(
                    manager
                        .bash_handles()
                        .inspect(
                            &WorkScope::Conversation("observe-live".to_string()),
                            &handle_id
                        )
                        .await,
                    phoenix_tools::BashHandleInspection::Terminal { .. }
                ) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("bash terminal evidence");
        observe_contract(
            manager.db(),
            manager.bash_handles(),
            manager.tmux_registry(),
            &contract,
            false,
        )
        .await
        .expect("observe");

        let resolved = manager
            .db()
            .get_wake_contract("wake-live")
            .await
            .expect("read")
            .expect("contract");
        assert_eq!(resolved.status, WakeContractStatus::Fired);
        assert!(matches!(
            resolved.terminal_payload,
            Some(WakeTerminalPayload::Bash { .. })
        ));
    }

    #[tokio::test]
    async fn dispatch_claims_serialize_and_reclaim_entries() {
        let manager = test_manager().await;
        for index in 0..256 {
            let key = format!("claim-{index}");
            let first = manager.try_claim_wake_dispatch(&key).expect("first claim");
            assert!(manager.try_claim_wake_dispatch(&key).is_none());
            drop(first);
            assert!(manager
                .wake_dispatch_claims
                .lock()
                .expect("claims")
                .get(&key)
                .is_none());
            drop(manager.try_claim_wake_dispatch(&key).expect("replacement"));
        }
        assert!(manager
            .wake_dispatch_claims
            .lock()
            .expect("claims")
            .is_empty());
    }

    #[tokio::test]
    async fn direct_continuation_observes_handle_in_original_registration_scope() {
        let manager = test_manager().await;
        for id in ["direct-parent", "direct-successor"] {
            create_conversation(manager.db(), id).await;
        }
        let context = ToolContext::new(
            CancellationToken::new(),
            "direct-parent".to_string(),
            std::path::PathBuf::from("/tmp"),
            manager.browser_sessions().clone(),
            manager.bash_handles().clone(),
            Arc::new(ModelRegistry::new_empty()),
            manager.terminals.clone(),
            manager.tmux_registry().clone(),
            None,
        );
        let spawned = BashTool
            .run(
                json!({"op":"run","cmd":"printf direct-wake","wait_seconds":0}),
                context,
            )
            .await;
        let handle_id = spawned.display_data().unwrap()["handle"]
            .as_str()
            .unwrap()
            .to_string();
        let contract = pending_contract("wake-direct-transfer", "direct-parent", &handle_id);
        manager.db().insert_wake_contract(&contract).await.unwrap();
        manager
            .db()
            .transfer_wake_contracts("direct-parent", "direct-successor")
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if matches!(
                    manager
                        .bash_handles()
                        .inspect(&contract.registration_work_scope, &handle_id)
                        .await,
                    phoenix_tools::BashHandleInspection::Terminal { .. }
                ) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let transferred = manager
            .db()
            .get_wake_contract("wake-direct-transfer")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(transferred.current_conversation_id, "direct-successor");
        assert_eq!(
            transferred.registration_work_scope,
            WorkScope::Conversation("direct-parent".to_string())
        );
        observe_contract(
            manager.db(),
            manager.bash_handles(),
            manager.tmux_registry(),
            &transferred,
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            manager
                .db()
                .get_wake_contract("wake-direct-transfer")
                .await
                .unwrap()
                .unwrap()
                .status,
            WakeContractStatus::Fired
        );
        assert_eq!(
            manager
                .db()
                .list_wake_inbox_items_for_conversation("direct-successor")
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn evidence_before_deadline_fires_even_when_observed_after_deadline() {
        let manager = test_manager().await;
        create_conversation(manager.db(), "evidence-precedes-deadline").await;
        let context = ToolContext::new(
            CancellationToken::new(),
            "evidence-precedes-deadline".to_string(),
            std::path::PathBuf::from("/tmp"),
            manager.browser_sessions().clone(),
            manager.bash_handles().clone(),
            Arc::new(ModelRegistry::new_empty()),
            manager.terminals.clone(),
            manager.tmux_registry().clone(),
            None,
        );
        let spawned = BashTool
            .run(json!({"op":"run","cmd":"true","wait_seconds":0}), context)
            .await;
        let handle_id = spawned.display_data().unwrap()["handle"]
            .as_str()
            .unwrap()
            .to_string();
        let scope = WorkScope::Conversation("evidence-precedes-deadline".to_string());
        let observed_at = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let phoenix_tools::BashHandleInspection::Terminal { observed_at, .. } =
                    manager.bash_handles().inspect(&scope, &handle_id).await
                {
                    break observed_at;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut contract = pending_contract(
            "wake-evidence-precedes-deadline",
            "evidence-precedes-deadline",
            &handle_id,
        );
        contract.expires_at = observed_at + Duration::milliseconds(1);
        while Utc::now() < contract.expires_at {
            tokio::task::yield_now().await;
        }
        manager.db().insert_wake_contract(&contract).await.unwrap();
        observe_contract(
            manager.db(),
            manager.bash_handles(),
            manager.tmux_registry(),
            &contract,
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            manager
                .db()
                .get_wake_contract(&contract.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            WakeContractStatus::Fired
        );
    }

    #[tokio::test]
    async fn observe_unknown_bash_at_startup_records_unrecoverable_forgotten_reason() {
        let manager = test_manager().await;
        create_conversation(manager.db(), "observe-startup").await;
        let contract = pending_contract("wake-missing", "observe-startup", "b-missing");
        manager
            .db()
            .insert_wake_contract(&contract)
            .await
            .expect("contract");

        observe_contract(
            manager.db(),
            manager.bash_handles(),
            manager.tmux_registry(),
            &contract,
            true,
        )
        .await
        .expect("observe");

        let resolved = manager
            .db()
            .get_wake_contract("wake-missing")
            .await
            .expect("read")
            .expect("contract");
        assert_eq!(resolved.status, WakeContractStatus::Forgotten);
        assert_eq!(
            resolved.forgotten_reason,
            Some(WakeForgottenReason::RuntimeUnrecoverableAfterRestart)
        );
    }

    #[tokio::test]
    async fn elapsed_deadline_beats_missing_handle_for_startup_and_live_observation() {
        for startup in [false, true] {
            let manager = test_manager().await;
            let conversation_id = if startup {
                "expired-startup"
            } else {
                "expired-live"
            };
            create_conversation(manager.db(), conversation_id).await;
            let mut contract = pending_contract(
                &format!("wake-{conversation_id}"),
                conversation_id,
                "missing",
            );
            contract.expires_at = Utc::now() - Duration::seconds(1);
            manager.db().insert_wake_contract(&contract).await.unwrap();
            observe_contract(
                manager.db(),
                manager.bash_handles(),
                manager.tmux_registry(),
                &contract,
                startup,
            )
            .await
            .unwrap();
            assert_eq!(
                manager
                    .db()
                    .get_wake_contract(&contract.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                WakeContractStatus::Expired
            );
        }
    }

    #[tokio::test]
    async fn dispatcher_keeps_cancellation_only_out_of_outbox_and_busy_resume_pending() {
        let manager = test_manager().await;
        for id in ["cancel-only", "busy-fired"] {
            create_conversation(manager.db(), id).await;
        }
        let cancelled = pending_contract("wake-cancel", "cancel-only", "b-cancel");
        manager
            .db()
            .insert_wake_contract(&cancelled)
            .await
            .expect("cancel contract");
        manager
            .db()
            .cancel_wake_contract("wake-cancel")
            .await
            .expect("cancel");
        let fired = pending_contract("wake-busy", "busy-fired", "b-busy");
        manager
            .db()
            .register_wake_contract(&fired, Some(&fired_outcome("busy")))
            .await
            .expect("fired contract");
        manager
            .inject_handle_for_test(
                "busy-fired",
                crate::state_machine::ConvState::LlmRequesting { attempt: 1 },
            )
            .await;

        dispatch_pending(&manager, false).await;

        let cancelled_inbox = manager
            .db()
            .list_wake_inbox_items_for_conversation("cancel-only")
            .await
            .expect("cancel inbox");
        assert_eq!(cancelled_inbox.len(), 1);
        assert!(cancelled_inbox[0].consumed_at.is_none());
        let busy_inbox = manager
            .db()
            .list_wake_inbox_items_for_conversation("busy-fired")
            .await
            .expect("busy inbox");
        assert_eq!(busy_inbox.len(), 1);
        assert!(busy_inbox[0].consumed_at.is_none());
        assert!(manager
            .db()
            .get_messages("busy-fired")
            .await
            .unwrap()
            .is_empty());
        let pending = manager.db().list_pending_wake_resumes().await.unwrap();
        assert!(pending.is_empty());
        assert!(matches!(
            manager.effective_conversation_state("busy-fired").await,
            Some(crate::state_machine::ConvState::LlmRequesting { attempt: 1 })
        ));
        assert!(manager.try_get_handle("cancel-only").await.is_none());
    }

    #[tokio::test]
    async fn dispatcher_send_failure_leaves_resume_pending_for_startup_retry() {
        let manager = test_manager().await;
        create_conversation(manager.db(), "retry-resume").await;
        let fired = pending_contract("wake-retry", "retry-resume", "b-retry");
        manager
            .db()
            .register_wake_contract(&fired, Some(&fired_outcome("retry")))
            .await
            .unwrap();
        manager
            .inject_closed_handle_for_test("retry-resume", crate::state_machine::ConvState::Idle)
            .await;

        dispatch_pending(&manager, false).await;

        let pending = manager.db().list_pending_wake_resumes().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].conversation_id, "retry-resume");
        assert!(matches!(
            manager
                .db()
                .get_conversation("retry-resume")
                .await
                .unwrap()
                .state,
            crate::state_machine::ConvState::Idle
        ));

        manager.runtimes.write().await.remove("retry-resume");
        dispatch_pending(&manager, false).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if manager
                    .db()
                    .list_pending_wake_resumes()
                    .await
                    .unwrap()
                    .is_empty()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("startup-style retry accepts pending resume");
        assert!(matches!(
            manager
                .db()
                .get_conversation("retry-resume")
                .await
                .unwrap()
                .state,
            crate::state_machine::ConvState::LlmRequesting { attempt: 1 }
        ));
    }

    #[tokio::test]
    async fn dispatcher_coalesces_cancel_and_fire_preserves_meta_and_is_idempotent() {
        let manager = test_manager().await;
        create_conversation(manager.db(), "dispatch-idle").await;
        let cancelled = pending_contract("wake-first", "dispatch-idle", "b-first");
        manager
            .db()
            .insert_wake_contract(&cancelled)
            .await
            .expect("cancel contract");
        manager
            .db()
            .cancel_wake_contract("wake-first")
            .await
            .expect("cancel");
        let fired = pending_contract("wake-second", "dispatch-idle", "b-second");
        manager
            .db()
            .register_wake_contract(&fired, Some(&fired_outcome("done")))
            .await
            .expect("fired contract");

        dispatch_pending(&manager, false).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if matches!(
                    manager.effective_conversation_state("dispatch-idle").await,
                    Some(crate::state_machine::ConvState::LlmRequesting { .. })
                ) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("normal LLM transition");

        let inbox = manager
            .db()
            .list_wake_inbox_items_for_conversation("dispatch-idle")
            .await
            .expect("inbox");
        assert_eq!(inbox.len(), 2);
        assert!(inbox.iter().all(|item| item.consumed_at.is_some()));
        let messages = manager
            .db()
            .get_messages("dispatch-idle")
            .await
            .expect("messages");
        assert_eq!(messages.len(), 1);
        let phoenix_db::MessageContent::User(user) = &messages[0].content else {
            panic!("wake message must be user content");
        };
        assert!(user.is_meta);
        assert!(user.text.find("wake-first").unwrap() < user.text.find("wake-second").unwrap());

        dispatch_pending(&manager, false).await;
        assert_eq!(
            manager
                .db()
                .get_messages("dispatch-idle")
                .await
                .expect("messages")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn wake_message_sequence_exceeds_post_checkpoint_ephemeral_event() {
        let manager = test_manager().await;
        create_conversation(manager.db(), "wake-sequence").await;
        let handle = manager
            .get_or_create("wake-sequence")
            .await
            .expect("runtime");
        let mut events = handle.broadcast_tx.subscribe();
        let ephemeral_seq = handle.broadcast_tx.current_seq() + 1;
        let _ = handle
            .broadcast_tx
            .send_seq(|sequence_id| SseEvent::StateChange {
                state: crate::state_machine::ConvState::Idle,
                presentation_mode: "idle".to_string(),
                state_updated_at: Utc::now(),
                sequence_id,
            });
        let emitted = events.recv().await.expect("ephemeral event");
        assert!(
            matches!(emitted, SseEvent::StateChange { sequence_id, .. } if sequence_id == ephemeral_seq)
        );

        let fired = pending_contract("wake-sequence-contract", "wake-sequence", "b-sequence");
        manager
            .db()
            .register_wake_contract(&fired, Some(&fired_outcome("done")))
            .await
            .expect("fired contract");
        dispatch_pending(&manager, false).await;

        let wake_message = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let SseEvent::Message { message } = events.recv().await.expect("event") {
                    if message.message_id.starts_with("wake-inbox-wake-sequence-") {
                        break message;
                    }
                }
            }
        })
        .await
        .expect("wake message broadcast");
        assert!(wake_message.sequence_id > ephemeral_seq);
        assert_eq!(
            manager.db().get_messages("wake-sequence").await.unwrap()[0].sequence_id,
            wake_message.sequence_id
        );
    }

    #[test]
    fn observability_helpers_derive_stable_causes_counts_and_latency() {
        let registered_at = Utc::now();
        let mut contract = pending_contract("wake-observability", "observability", "bash-1");
        contract.registered_at = registered_at;
        let forgotten = WakeTerminalOutcome::Forgotten {
            forgotten_reason: WakeForgottenReason::RuntimeUnrecoverableAfterRestart,
            resolved_at: registered_at + Duration::milliseconds(1250),
        };

        assert_eq!(contract.handle.kind().as_str(), "bash");
        assert_eq!(contract.handle.handle_id(), "bash-1");
        assert_eq!(forgotten.terminal_cause().as_str(), "forgotten");
        assert_eq!(
            forgotten
                .forgotten_reason()
                .map(WakeForgottenReason::as_str),
            Some("runtime_unrecoverable_after_restart")
        );
        assert_eq!(resolution_latency_ms(&contract, &forgotten), 1250);

        let receipt = phoenix_core::domain::wake_contracts::WakeRegistrationReceipt {
            contract_id: contract.id.clone(),
            handle: contract.handle.clone(),
            expires_at: contract.expires_at,
            registering_tool_use_id: contract.registering_tool_use_id.clone(),
        };
        let make_item = |inbox_id, cause| phoenix_core::domain::wake_contracts::WakeInboxItem {
            inbox_id,
            contract_id: format!("contract-{inbox_id}"),
            conversation_id: "observability".to_string(),
            receipt: receipt.clone(),
            cause,
            delivered_at: None,
            consumed_at: None,
        };
        let items = vec![
            make_item(
                1,
                WakeInboxCause::Fired {
                    terminal_payload: WakeTerminalPayload::Bash {
                        bash: WakeBashFiredPayload {
                            status: WakeBashObservedStatus::Exited,
                            exit_code: Some(0),
                            duration_ms: Some(1),
                            signal_number: None,
                            kill_signal_sent: None,
                        },
                    },
                    tails: vec![],
                    auto_resume: true,
                },
            ),
            make_item(2, WakeInboxCause::Cancelled { auto_resume: false }),
            make_item(3, WakeInboxCause::Expired { auto_resume: true }),
            make_item(
                4,
                WakeInboxCause::Forgotten {
                    forgotten_reason: WakeForgottenReason::HandleMissing,
                    auto_resume: true,
                },
            ),
        ];
        assert_eq!(
            WakeCauseCounts::from_items(&items),
            WakeCauseCounts {
                fired: 1,
                cancelled: 1,
                expired: 1,
                forgotten: 1,
            }
        );
    }

    #[test]
    fn late_terminal_evidence_routes_to_expired() {
        let now = Utc::now();
        let contract = WakeContract {
            id: "late".to_string(),
            current_conversation_id: "conv".to_string(),
            registration_work_scope: WorkScope::Conversation("conv".to_string()),
            handle: WakeContractHandle::Bash {
                handle_id: "bash".to_string(),
            },
            registering_tool_use_id: None,
            registered_at: now - Duration::seconds(2),
            expires_at: now - Duration::seconds(1),
            status: WakeContractStatus::Pending,
            terminal_cause: None,
            forgotten_reason: None,
            terminal_payload: None,
            resolved_at: None,
        };
        let observed_at = now;
        let outcome = if observed_at <= contract.expires_at {
            WakeTerminalOutcome::Fired {
                terminal_payload: WakeTerminalPayload::Bash {
                    bash: WakeBashFiredPayload {
                        status: WakeBashObservedStatus::Exited,
                        exit_code: Some(0),
                        duration_ms: None,
                        signal_number: None,
                        kill_signal_sent: None,
                    },
                },
                tails: vec![],
                resolved_at: observed_at,
            }
        } else {
            WakeTerminalOutcome::Expired { resolved_at: now }
        };
        assert_eq!(outcome.status(), WakeContractStatus::Expired);
    }
}
