//! Production executor for durable bash/tmux wake observation effects.
//!
//! This worker only reconciles external handle state into durable workflow
//! receipts. It deliberately does not broadcast SSE or accept the resulting
//! runtime obligation; those are separate reducer/runtime concerns.

use std::{sync::Arc, time::Duration as StdDuration};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use phoenix_core::{
    domain::sm_event::{Event, WakeObservationResult},
    work_scope::WorkScope,
};
use phoenix_db::workflow::{
    wake::{
        ClaimedWakeEffect, WakeBinding, WakeObservationRequest, WakeTerminalReceiptRequest,
        WakeWorkflowAdapter,
    },
    AcceptReceiptResult, DueEffect, DurableReceiptOrigin, ReconcileEffectResult,
    WorkflowRepository,
};
use phoenix_tools::{BashTerminalInspection, TmuxTerminalInspection, TmuxWindowIdentity};
use phoenix_tools::{
    WaitUntilTarget, WakeRegistrar, WakeRegistrarError, WakeRegistration, WakeRegistrationReceipt,
    WakeRegistrationTarget,
};
use phoenix_workflow::{
    wake_profile::{
        BashTerminalEvidence, BashTerminalStatus, TmuxTerminalEvidence, TmuxTerminalStatus,
        WakeForgottenReason, WakeResourceIdentity, WakeTerminalEvidence, WakeTerminalPayload,
        WorkScopeIdentity, WorkScopeKind,
    },
    Timestamp,
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::sync::watch;

use super::RuntimeManager;

const CLAIM_LEASE: Duration = Duration::seconds(30);
const HANDLE_POLL: Duration = Duration::seconds(1);
const MAX_IDLE_POLL: StdDuration = StdDuration::from_secs(1);
const ERROR_BACKOFF_MIN: StdDuration = StdDuration::from_millis(250);
const ERROR_BACKOFF_MAX: StdDuration = StdDuration::from_secs(5);

#[derive(Clone)]
pub(crate) struct ProductionWakeRegistrar {
    manager: Arc<RuntimeManager>,
}

impl ProductionWakeRegistrar {
    pub(crate) fn new(manager: Arc<RuntimeManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl WakeRegistrar for ProductionWakeRegistrar {
    async fn register(
        &self,
        registration: WakeRegistration,
    ) -> Result<WakeRegistrationReceipt, WakeRegistrarError> {
        let stable =
            stable_registration_key(&registration.conversation_id, &registration.tool_use_id);
        let contract_id = format!("wake-contract-{stable}");
        if let Some(receipt) = existing_receipt(&self.manager, &contract_id, &registration).await? {
            self.manager.kick_wake_worker();
            return Ok(receipt);
        }
        let (accepted_at, registered_at) = normalized_registration_time()?;
        let expires_at = accepted_at
            + Duration::seconds(
                i64::try_from(registration.max_wait_seconds)
                    .map_err(|error| WakeRegistrarError::Persistence(error.to_string()))?,
            );
        let registration_scope = scope_identity(&registration.work_scope)?;
        let (resource, target) = match registration.target {
            WakeRegistrationTarget::Bash { handle_id } => (
                WakeResourceIdentity::Bash(phoenix_workflow::wake_profile::BashResourceIdentity {
                    work_scope: registration_scope.clone(),
                    handle_id: handle_id.clone(),
                }),
                WaitUntilTarget::Bash { handle_id },
            ),
            WakeRegistrationTarget::TmuxWindow {
                server_generation,
                window_id,
            } => (
                WakeResourceIdentity::TmuxWindow(
                    phoenix_workflow::wake_profile::TmuxResourceIdentity {
                        work_scope: registration_scope.clone(),
                        server_generation,
                        window_id: window_id.clone(),
                    },
                ),
                WaitUntilTarget::TmuxWindow { window_id },
            ),
        };
        let workflow_id = format!("wake-workflow-{stable}");
        let expires_timestamp = timestamp(expires_at).map_err(WakeRegistrarError::Persistence)?;
        let expires_at = DateTime::<Utc>::from_timestamp(
            i64::try_from(expires_timestamp.0)
                .map_err(|error| WakeRegistrarError::Persistence(error.to_string()))?,
            0,
        )
        .ok_or_else(|| WakeRegistrarError::Persistence("wake expiry is out of range".to_owned()))?;
        let intent = phoenix_workflow::wake_profile::WakeRegistrationIntent {
            contract_id: contract_id.clone(),
            conversation_id: registration.conversation_id.clone(),
            registration_scope,
            resource,
            registering_tool_use_id: registration.tool_use_id.clone(),
            registered_at,
            expires_at: expires_timestamp,
        };
        let repository = WorkflowRepository::new(self.manager.db().pool().clone());
        let adapter = WakeWorkflowAdapter::new(&repository);
        let fence_version = adapter
            .registration_fence_version(&registration.conversation_id)
            .await
            .map_err(|error| WakeRegistrarError::Persistence(error.to_string()))?;
        let request = phoenix_db::workflow::wake::WakeRegistrationRequest {
            idempotency_key: format!("wake-register-{stable}"),
            intent_fingerprint: intent_fingerprint(&intent),
            workflow_id,
            transition_id: format!("wake-transition-{stable}"),
            binding_id: format!("wake-acceptance-{stable}"),
            authority_scope: registration.conversation_id.clone(),
            intent,
            fence_version,
            accepted_at,
        };
        let result = adapter
            .register(&request)
            .await
            .map_err(|error| WakeRegistrarError::Persistence(error.to_string()))?;
        match result {
            phoenix_db::workflow::wake::WakeRegistrationResult::New { .. }
            | phoenix_db::workflow::wake::WakeRegistrationResult::Replay { .. } => {
                // register() commits the complete graph before returning.
                self.manager.kick_wake_worker();
                Ok(WakeRegistrationReceipt {
                    contract_id,
                    target,
                    expires_at,
                    registering_tool_use_id: registration.tool_use_id,
                })
            }
            phoenix_db::workflow::wake::WakeRegistrationResult::Conflict => {
                Err(WakeRegistrarError::Conflict)
            }
            phoenix_db::workflow::wake::WakeRegistrationResult::Retryable => {
                Err(WakeRegistrarError::Retryable)
            }
            phoenix_db::workflow::wake::WakeRegistrationResult::NotAccepting => {
                Err(WakeRegistrarError::NotAccepting)
            }
        }
    }

    async fn cancel_registration(
        &self,
        conversation_id: &str,
        contract_id: &str,
    ) -> Result<(), WakeRegistrarError> {
        let repository = WorkflowRepository::new(self.manager.db().pool().clone());
        WakeWorkflowAdapter::new(&repository)
            .cancel_pending_contract(conversation_id, contract_id, Utc::now())
            .await
            .map(|_| ())
            .map_err(|error| WakeRegistrarError::Persistence(error.to_string()))
    }
}

async fn existing_receipt(
    manager: &RuntimeManager,
    contract_id: &str,
    registration: &WakeRegistration,
) -> Result<Option<WakeRegistrationReceipt>, WakeRegistrarError> {
    let row = sqlx::query(
        "SELECT conversation_id, resource_kind, bash_work_scope_kind, \
         bash_work_scope_stable_key, bash_handle_id, tmux_work_scope_kind, \
         tmux_work_scope_stable_key, tmux_server_generation, tmux_window_id, \
         registering_tool_use_id, registered_at, expires_at FROM wake_workflow_bindings \
         WHERE contract_id = ?1",
    )
    .bind(contract_id)
    .fetch_optional(manager.db().pool())
    .await
    .map_err(|error| WakeRegistrarError::Persistence(error.to_string()))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let matches_owner = row.get::<String, _>("conversation_id") == registration.conversation_id
        && row.get::<String, _>("registering_tool_use_id") == registration.tool_use_id;
    let target = match &registration.target {
        WakeRegistrationTarget::Bash { handle_id }
            if row.get::<String, _>("resource_kind") == "bash"
                && stored_scope_matches(&row, "bash", &registration.work_scope)
                && row.get::<Option<String>, _>("bash_handle_id").as_deref()
                    == Some(handle_id.as_str()) =>
        {
            Some(WaitUntilTarget::Bash {
                handle_id: handle_id.clone(),
            })
        }
        WakeRegistrationTarget::TmuxWindow {
            server_generation,
            window_id,
        } if row.get::<String, _>("resource_kind") == "tmux_window"
            && stored_scope_matches(&row, "tmux", &registration.work_scope)
            && row
                .get::<Option<String>, _>("tmux_server_generation")
                .as_deref()
                == Some(server_generation.as_str())
            && row.get::<Option<String>, _>("tmux_window_id").as_deref()
                == Some(window_id.as_str()) =>
        {
            Some(WaitUntilTarget::TmuxWindow {
                window_id: window_id.clone(),
            })
        }
        _ => None,
    };
    if !matches_owner || target.is_none() {
        return Err(WakeRegistrarError::Conflict);
    }
    let registered_at = DateTime::parse_from_rfc3339(&row.get::<String, _>("registered_at"))
        .map_err(|error| WakeRegistrarError::Persistence(error.to_string()))?
        .with_timezone(&Utc);
    let expires_at = DateTime::parse_from_rfc3339(&row.get::<String, _>("expires_at"))
        .map_err(|error| WakeRegistrarError::Persistence(error.to_string()))?
        .with_timezone(&Utc);
    let requested_wait = i64::try_from(registration.max_wait_seconds)
        .map_err(|error| WakeRegistrarError::Persistence(error.to_string()))?;
    if (expires_at - registered_at).num_seconds() != requested_wait {
        return Err(WakeRegistrarError::Conflict);
    }
    Ok(Some(WakeRegistrationReceipt {
        contract_id: contract_id.to_owned(),
        target: target.expect("matched target is present"),
        expires_at,
        registering_tool_use_id: registration.tool_use_id.clone(),
    }))
}

fn stored_scope_matches(row: &sqlx::sqlite::SqliteRow, prefix: &str, scope: &WorkScope) -> bool {
    let kind = row.get::<Option<String>, _>(format!("{prefix}_work_scope_kind").as_str());
    let key = row.get::<Option<String>, _>(format!("{prefix}_work_scope_stable_key").as_str());
    match scope {
        WorkScope::Conversation(id) => {
            kind.as_deref() == Some("conversation") && key.as_deref() == Some(id)
        }
        WorkScope::Worktree(path) => {
            kind.as_deref() == Some("worktree") && key.as_deref() == Some(path)
        }
        WorkScope::Global => false,
    }
}

fn stable_registration_key(conversation_id: &str, tool_use_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"phoenix.wake.registration.v1\0");
    digest.update(conversation_id.as_bytes());
    digest.update([0]);
    digest.update(tool_use_id.as_bytes());
    hex_digest(digest.finalize())
}

fn intent_fingerprint(intent: &phoenix_workflow::wake_profile::WakeRegistrationIntent) -> String {
    let mut digest = Sha256::new();
    digest.update(b"phoenix.wake.intent.v1\0");
    for field in [
        intent.contract_id.as_str(),
        intent.conversation_id.as_str(),
        intent.registration_scope.stable_key.as_str(),
        intent.registering_tool_use_id.as_str(),
    ] {
        digest.update(field.as_bytes());
        digest.update([0]);
    }
    digest.update(match intent.registration_scope.kind {
        WorkScopeKind::Conversation => b"conversation".as_slice(),
        WorkScopeKind::Worktree => b"worktree".as_slice(),
    });
    digest.update([0]);
    match &intent.resource {
        WakeResourceIdentity::Bash(identity) => {
            digest.update(b"bash\0");
            digest.update(identity.handle_id.as_bytes());
        }
        WakeResourceIdentity::TmuxWindow(identity) => {
            digest.update(b"tmux_window\0");
            digest.update(identity.server_generation.as_bytes());
            digest.update([0]);
            digest.update(identity.window_id.as_bytes());
        }
    }
    digest.update(intent.registered_at.0.to_be_bytes());
    digest.update(intent.expires_at.0.to_be_bytes());
    hex_digest(digest.finalize())
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write;
    bytes
        .as_ref()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
            out
        })
}

fn normalized_registration_time() -> Result<(DateTime<Utc>, Timestamp), WakeRegistrarError> {
    let now = Utc::now();
    let registered_at = timestamp(now).map_err(WakeRegistrarError::Persistence)?;
    let seconds = i64::try_from(registered_at.0)
        .map_err(|error| WakeRegistrarError::Persistence(error.to_string()))?;
    let accepted_at = DateTime::<Utc>::from_timestamp(seconds, 0).ok_or_else(|| {
        WakeRegistrarError::Persistence("wake registration is out of range".to_owned())
    })?;
    let accepted_at = if now.timestamp_subsec_nanos() == 0 {
        accepted_at
    } else {
        accepted_at + Duration::seconds(1)
    };
    Ok((
        accepted_at,
        timestamp(accepted_at).map_err(WakeRegistrarError::Persistence)?,
    ))
}

fn scope_identity(scope: &WorkScope) -> Result<WorkScopeIdentity, WakeRegistrarError> {
    match scope {
        WorkScope::Conversation(id) => Ok(WorkScopeIdentity {
            kind: WorkScopeKind::Conversation,
            stable_key: id.clone(),
        }),
        WorkScope::Worktree(path) => Ok(WorkScopeIdentity {
            kind: WorkScopeKind::Worktree,
            stable_key: path.clone(),
        }),
        WorkScope::Global => Err(WakeRegistrarError::Persistence(
            "global resources cannot be registered for conversation wake".to_owned(),
        )),
    }
}

pub(crate) fn durable_scope_identity(
    scope: &WorkScope,
) -> Result<WorkScopeIdentity, WakeRegistrarError> {
    scope_identity(scope)
}

pub(crate) async fn run(manager: Arc<RuntimeManager>, mut kick: watch::Receiver<u64>) {
    let worker_id = format!("wake-worker-{}", uuid::Uuid::new_v4());
    let mut error_backoff = ERROR_BACKOFF_MIN;
    loop {
        match drain_due(&manager, &worker_id, Utc::now).await {
            Ok(()) => error_backoff = ERROR_BACKOFF_MIN,
            Err(error) => {
                tracing::error!(%error, "durable wake worker drain failed");
                if wait_or_kick(&mut kick, error_backoff).await.is_err() {
                    break;
                }
                error_backoff = error_backoff.saturating_mul(2).min(ERROR_BACKOFF_MAX);
                continue;
            }
        }

        let repository = WorkflowRepository::new(manager.db().pool().clone());
        let deadline = match WakeWorkflowAdapter::new(&repository).next_deadline().await {
            Ok(deadline) => deadline,
            Err(error) => {
                tracing::error!(%error, "failed to read durable wake deadline");
                if wait_or_kick(&mut kick, error_backoff).await.is_err() {
                    break;
                }
                error_backoff = error_backoff.saturating_mul(2).min(ERROR_BACKOFF_MAX);
                continue;
            }
        };
        let delay = deadline.map_or(MAX_IDLE_POLL, |value| {
            duration_until(value, Utc::now()).min(MAX_IDLE_POLL)
        });
        if wait_or_kick(&mut kick, delay).await.is_err() {
            break;
        }
    }
    tracing::info!("Durable wake worker stopped");
}

async fn wait_or_kick(
    kick: &mut watch::Receiver<u64>,
    delay: StdDuration,
) -> Result<(), watch::error::RecvError> {
    tokio::select! {
        changed = kick.changed() => changed,
        () = tokio::time::sleep(delay) => Ok(()),
    }
}

fn duration_until(deadline: DateTime<Utc>, now: DateTime<Utc>) -> StdDuration {
    (deadline - now).to_std().unwrap_or(StdDuration::ZERO)
}

pub(crate) async fn drain_due<F>(
    manager: &Arc<RuntimeManager>,
    worker_id: &str,
    mut now: F,
) -> Result<(), String>
where
    F: FnMut() -> DateTime<Utc>,
{
    let repository = WorkflowRepository::new(manager.db().pool().clone());
    let adapter = WakeWorkflowAdapter::new(&repository);
    let due = adapter
        .due(now())
        .await
        .map_err(|error| error.to_string())?;

    // Process only the discovered snapshot. The outer worker loop is responsible
    // for discovering newly due work, avoiding a self-sustaining database loop.
    for item in due {
        let item_now = now();
        let claimable = if let DueEffect::RetryWait {
            workflow_id,
            effect_id,
            declared_workflow_version,
            generation,
            ..
        } = &item
        {
            if !adapter
                .promote_exact_deadline(&item, item_now)
                .await
                .map_err(|error| error.to_string())?
            {
                continue;
            }
            DueEffect::Eligible {
                workflow_id: workflow_id.clone(),
                effect_id: effect_id.clone(),
                declared_workflow_version: *declared_workflow_version,
                generation: *generation,
            }
        } else {
            item
        };
        let Some(claim) = adapter
            .claim(
                &claimable,
                uuid::Uuid::new_v4().to_string(),
                worker_id.to_owned(),
                item_now,
                item_now + CLAIM_LEASE,
            )
            .await
            .map_err(|error| error.to_string())?
        else {
            continue;
        };
        process_claim(manager, &adapter, claim, item_now).await?;
    }
    deliver_owed(manager, &adapter).await?;
    Ok(())
}

async fn deliver_owed(
    manager: &Arc<RuntimeManager>,
    adapter: &WakeWorkflowAdapter<'_>,
) -> Result<(), String> {
    for conversation_id in adapter
        .owed_conversations()
        .await
        .map_err(|error| error.to_string())?
    {
        let conversation = manager
            .db()
            .get_conversation(&conversation_id)
            .await
            .map_err(|error| error.to_string())?;
        if !matches!(
            conversation.state,
            phoenix_core::domain::sm_state::ConvState::Idle
        ) {
            continue;
        }
        let results: Vec<WakeObservationResult> = adapter
            .owed_tool_results(&conversation_id)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(
                |(inbox_id, registering_tool_use_id, output)| WakeObservationResult {
                    message_id: format!("wake-result-{inbox_id}"),
                    content: format!(
                    "Durable wait observation for registration {registering_tool_use_id}: {output}"
                ),
                    inbox_id,
                },
            )
            .collect();
        if results.is_empty() {
            continue;
        }
        manager
            .send_event(&conversation_id, Event::WakeObservationReady { results })
            .await
            .map_err(|error| {
                format!("failed to deliver owed wake for {conversation_id}: {error}")
            })?;
    }
    Ok(())
}

async fn process_claim(
    manager: &RuntimeManager,
    adapter: &WakeWorkflowAdapter<'_>,
    claim: ClaimedWakeEffect,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let binding = adapter
        .load_binding(&claim.authority.workflow_id)
        .await
        .map_err(|error| error.to_string())?;
    let decision = inspect_binding(manager, &binding, now).await?;
    let now = Utc::now();
    match decision {
        InspectionDecision::Terminal { evidence, terminal } => {
            let stem = format!(
                "wake:{}:{}:{}",
                claim.authority.workflow_id, claim.authority.generation, claim.attempt_id
            );
            let result = adapter
                .record_terminal_evidence(
                    &WakeObservationRequest {
                        observation_id: format!("{stem}:observation"),
                        authority: claim.authority.clone(),
                        attempt_id: claim.attempt_id.clone(),
                        evidence,
                        recorded_at: now,
                    },
                    &WakeTerminalReceiptRequest {
                        receipt_id: format!("{stem}:receipt"),
                        reducer_inbox_id: format!("{stem}:inbox"),
                        authority: claim.authority,
                        attempt_id: claim.attempt_id,
                        terminal,
                        accepted_at: now,
                        origin: DurableReceiptOrigin::Reconciliation,
                    },
                )
                .await
                .map_err(|error| error.to_string())?;
            if matches!(result, AcceptReceiptResult::Conflict) {
                return Err("wake terminal receipt conflicted with durable authority".to_owned());
            }
        }
        InspectionDecision::DeadlineTerminal(terminal) => {
            let stem = format!(
                "wake:{}:{}:{}",
                claim.authority.workflow_id, claim.authority.generation, claim.attempt_id
            );
            let result = adapter
                .accept_terminal_receipt(&WakeTerminalReceiptRequest {
                    receipt_id: format!("{stem}:receipt"),
                    reducer_inbox_id: format!("{stem}:inbox"),
                    authority: claim.authority,
                    attempt_id: claim.attempt_id,
                    terminal,
                    accepted_at: now,
                    origin: DurableReceiptOrigin::Reconciliation,
                })
                .await
                .map_err(|error| error.to_string())?;
            if matches!(result, AcceptReceiptResult::Conflict) {
                return Err("wake deadline receipt conflicted with durable authority".to_owned());
            }
        }
        InspectionDecision::RetryAt(retry_at) => {
            let result = adapter
                .schedule_retry(&claim.authority, now, retry_at)
                .await
                .map_err(|error| error.to_string())?;
            if !matches!(
                result,
                ReconcileEffectResult::ScheduledRetry | ReconcileEffectResult::StaleAuthority
            ) {
                return Err(format!(
                    "wake retry rejected by typed workflow policy: {result:?}"
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InspectionDecision {
    Terminal {
        evidence: WakeTerminalEvidence,
        terminal: WakeTerminalPayload,
    },
    DeadlineTerminal(WakeTerminalPayload),
    RetryAt(DateTime<Utc>),
}

async fn inspect_binding(
    manager: &RuntimeManager,
    binding: &WakeBinding,
    now: DateTime<Utc>,
) -> Result<InspectionDecision, String> {
    let deadline_reached = now >= binding.expires_at;
    match &binding.resource {
        WakeResourceIdentity::Bash(identity) => {
            let inspection = manager
                .bash_handles()
                .inspect_terminal(&work_scope(&identity.work_scope), &identity.handle_id)
                .await;
            match inspection {
                BashTerminalInspection::Terminal {
                    observed_at,
                    exit_code,
                    signal_number,
                    duration_ms,
                    kill_signal_sent,
                    tail_start_offset,
                    tail_end_offset,
                    tail_truncated_before,
                    tails,
                } if observed_at <= binding.expires_at => {
                    let evidence = WakeTerminalEvidence::Bash(BashTerminalEvidence {
                        identity: identity.clone(),
                        status: if signal_number.is_some() || kill_signal_sent.is_some() {
                            BashTerminalStatus::Killed
                        } else {
                            BashTerminalStatus::Exited
                        },
                        occurred_at: timestamp(observed_at)?,
                        exit_code,
                        duration_ms: Some(duration_ms),
                        signal_number,
                        kill_signal_sent: kill_signal_sent.map(|signal| signal.as_str().to_owned()),
                        tail_start_offset,
                        tail_end_offset,
                        tail_truncated_before,
                        tail_offsets: tails.iter().map(|line| line.offset).collect(),
                        final_tail: tails
                            .iter()
                            .map(|line| String::from_utf8_lossy(&line.bytes).into_owned())
                            .collect(),
                    });
                    fired(binding, evidence, now)
                }
                BashTerminalInspection::KillPendingKernel { observed_at }
                    if observed_at < binding.expires_at =>
                {
                    fired(
                        binding,
                        WakeTerminalEvidence::Bash(BashTerminalEvidence {
                            identity: identity.clone(),
                            status: BashTerminalStatus::KillPendingKernel,
                            occurred_at: timestamp(observed_at)?,
                            exit_code: None,
                            duration_ms: Some(0),
                            signal_number: None,
                            kill_signal_sent: None,
                            tail_start_offset: 0,
                            tail_end_offset: 0,
                            tail_truncated_before: false,
                            tail_offsets: Vec::new(),
                            final_tail: Vec::new(),
                        }),
                        now,
                    )
                }
                BashTerminalInspection::Unknown => forgotten(binding, now),
                BashTerminalInspection::Live
                | BashTerminalInspection::KillPendingKernel { .. }
                | BashTerminalInspection::Terminal { .. }
                    if deadline_reached =>
                {
                    expired(binding, now)
                }
                BashTerminalInspection::Live
                | BashTerminalInspection::KillPendingKernel { .. }
                | BashTerminalInspection::Terminal { .. } => retry(binding, now),
            }
        }
        WakeResourceIdentity::TmuxWindow(identity) => {
            let inspection = manager
                .tmux_registry()
                .inspect_window(&TmuxWindowIdentity {
                    work_scope: work_scope(&identity.work_scope),
                    server_generation: identity.server_generation.clone(),
                    window_id: identity.window_id.clone(),
                })
                .await;
            classify_tmux(binding, identity, inspection, now)
        }
    }
}

fn classify_tmux(
    binding: &WakeBinding,
    identity: &phoenix_workflow::wake_profile::TmuxResourceIdentity,
    inspection: TmuxTerminalInspection,
    now: DateTime<Utc>,
) -> Result<InspectionDecision, String> {
    match inspection {
        TmuxTerminalInspection::WindowKilled { occurred_at }
            if occurred_at <= binding.expires_at =>
        {
            let evidence = WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
                identity: identity.clone(),
                status: TmuxTerminalStatus::WindowKilled,
                occurred_at: timestamp(occurred_at)?,
                exit_code: None,
                duration_ms: None,
                final_tail: Vec::new(),
            });
            fired(binding, evidence, now)
        }
        TmuxTerminalInspection::Terminal {
            exit_code,
            occurred_at: Some(occurred_at),
            duration_ms,
            final_tail,
        } if occurred_at <= binding.expires_at => {
            let evidence = WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
                identity: identity.clone(),
                status: TmuxTerminalStatus::ExitMarkerObserved,
                occurred_at: timestamp(occurred_at)?,
                exit_code: Some(exit_code),
                duration_ms,
                final_tail,
            });
            fired(binding, evidence, now)
        }
        TmuxTerminalInspection::Unavailable if now >= binding.expires_at => forgotten(binding, now),
        TmuxTerminalInspection::Missing => forgotten(binding, now),
        TmuxTerminalInspection::Live
        | TmuxTerminalInspection::WindowKilled { .. }
        | TmuxTerminalInspection::Terminal { .. }
            if now >= binding.expires_at =>
        {
            expired(binding, now)
        }
        // A terminal marker without its durable occurrence timestamp is not
        // evidence that may outrank the exact deadline.
        TmuxTerminalInspection::Live
        | TmuxTerminalInspection::Unavailable
        | TmuxTerminalInspection::WindowKilled { .. }
        | TmuxTerminalInspection::Terminal { .. } => retry(binding, now),
    }
}

fn fired(
    binding: &WakeBinding,
    evidence: WakeTerminalEvidence,
    now: DateTime<Utc>,
) -> Result<InspectionDecision, String> {
    Ok(InspectionDecision::Terminal {
        evidence: evidence.clone(),
        terminal: WakeTerminalPayload::Fired {
            contract_id: binding.contract_id.clone(),
            resource: binding.resource.clone(),
            evidence,
            resolved_at: timestamp(now)?,
        },
    })
}

fn expired(binding: &WakeBinding, now: DateTime<Utc>) -> Result<InspectionDecision, String> {
    Ok(InspectionDecision::DeadlineTerminal(
        WakeTerminalPayload::Expired {
            contract_id: binding.contract_id.clone(),
            resource: binding.resource.clone(),
            resolved_at: timestamp(now)?,
        },
    ))
}

fn forgotten(binding: &WakeBinding, now: DateTime<Utc>) -> Result<InspectionDecision, String> {
    Ok(InspectionDecision::DeadlineTerminal(
        WakeTerminalPayload::Forgotten {
            contract_id: binding.contract_id.clone(),
            resource: binding.resource.clone(),
            reason: WakeForgottenReason::HandleMissing,
            resolved_at: timestamp(now)?,
        },
    ))
}

fn retry(binding: &WakeBinding, now: DateTime<Utc>) -> Result<InspectionDecision, String> {
    if now >= binding.expires_at {
        return expired(binding, now);
    }
    let retry_at = (now + HANDLE_POLL).min(binding.expires_at);
    debug_assert!(retry_at > now, "retry decisions must advance time");
    Ok(InspectionDecision::RetryAt(retry_at))
}

fn timestamp(value: DateTime<Utc>) -> Result<Timestamp, String> {
    u64::try_from(value.timestamp())
        .map(Timestamp)
        .map_err(|_| "wake timestamp precedes the Unix epoch".to_owned())
}

fn work_scope(identity: &WorkScopeIdentity) -> WorkScope {
    match identity.kind {
        WorkScopeKind::Conversation => WorkScope::Conversation(identity.stable_key.clone()),
        WorkScopeKind::Worktree => WorkScope::Worktree(identity.stable_key.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::Database, platform::PlatformCapability, tools::mcp::McpClientManager};
    use chrono::TimeZone;
    use phoenix_db::workflow::wake::{WakeRegistrationRequest, WakeRegistrationResult};
    use phoenix_llm::ModelRegistry;
    use phoenix_tools::Tool;
    use phoenix_workflow::wake_profile::{
        BashResourceIdentity, TmuxResourceIdentity, WakeRegistrationIntent,
    };

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0).single().unwrap()
    }

    fn at_nanos(seconds: i64, nanos: u32) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, nanos).single().unwrap()
    }

    fn scope() -> WorkScopeIdentity {
        WorkScopeIdentity {
            kind: WorkScopeKind::Conversation,
            stable_key: "conv".to_owned(),
        }
    }

    fn binding(resource: WakeResourceIdentity) -> WakeBinding {
        WakeBinding {
            contract_id: "contract".to_owned(),
            resource,
            expires_at: at(100),
        }
    }

    async fn manager_with_due_wakes(count: usize) -> RuntimeManager {
        let db = Database::open_in_memory().await.expect("db");
        db.create_conversation("conv", "conv", "/tmp", true, None, None)
            .await
            .expect("conversation");
        let repository = WorkflowRepository::new(db.pool().clone());
        let adapter = WakeWorkflowAdapter::new(&repository);
        for index in 0..count {
            let request = WakeRegistrationRequest {
                idempotency_key: format!("registration-{index}"),
                intent_fingerprint: format!("fingerprint-{index}"),
                workflow_id: format!("wake-{index}"),
                transition_id: format!("transition-{index}"),
                binding_id: format!("binding-{index}"),
                authority_scope: "conversation:conv".to_owned(),
                intent: WakeRegistrationIntent {
                    contract_id: format!("contract-{index}"),
                    conversation_id: "conv".to_owned(),
                    registration_scope: scope(),
                    resource: WakeResourceIdentity::Bash(BashResourceIdentity {
                        work_scope: scope(),
                        handle_id: format!("missing-{index}"),
                    }),
                    registering_tool_use_id: format!("tool-{index}"),
                    registered_at: Timestamp(1_000),
                    expires_at: Timestamp(1_100),
                },
                fence_version: u64::try_from(index + 1).expect("test fence fits u64"),
                accepted_at: at(1_000),
            };
            assert!(matches!(
                adapter.register(&request).await.unwrap(),
                WakeRegistrationResult::New { .. }
            ));
        }
        RuntimeManager::new(
            db,
            Arc::new(ModelRegistry::new_empty()),
            PlatformCapability::None {
                details: "test".to_owned(),
            },
            Arc::new(McpClientManager::new()),
            None,
        )
    }

    #[tokio::test]
    async fn production_registration_persists_graph_and_worker_processes_resource() {
        let manager = Arc::new(manager_with_due_wakes(0).await);
        let context = phoenix_tools::ToolContext::new(
            tokio_util::sync::CancellationToken::new(),
            "conv".to_owned(),
            std::path::PathBuf::from("/tmp"),
            Arc::new(phoenix_tools::BrowserSessionManager::default()),
            manager.bash_handles().clone(),
            Arc::new(ModelRegistry::new_empty()),
            phoenix_terminal::ActiveTerminals::new(),
            manager.tmux_registry().clone(),
            None,
        );
        let spawned = phoenix_tools::BashTool
            .run(
                serde_json::json!({"op":"run","cmd":"true","wait_seconds":0}),
                context.clone(),
            )
            .await;
        let handle_id = spawned.display_data().unwrap()["handle"]
            .as_str()
            .unwrap()
            .to_owned();
        let registrar = ProductionWakeRegistrar::new(manager.clone());
        let receipt = registrar
            .register(WakeRegistration {
                conversation_id: "conv".to_owned(),
                tool_use_id: "wait-tool-1".to_owned(),
                work_scope: WorkScope::Conversation("conv".to_owned()),
                target: WakeRegistrationTarget::Bash {
                    handle_id: handle_id.clone(),
                },
                max_wait_seconds: 60,
            })
            .await
            .expect("registration");

        let graph_counts: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM wake_workflow_bindings WHERE contract_id = ?1), \
             (SELECT COUNT(*) FROM workflows WHERE id LIKE 'wake-workflow-%'), \
             (SELECT COUNT(*) FROM workflow_transitions WHERE workflow_id LIKE 'wake-workflow-%'), \
             (SELECT COUNT(*) FROM workflow_effects WHERE workflow_id LIKE 'wake-workflow-%')",
        )
        .bind(&receipt.contract_id)
        .fetch_one(manager.db().pool())
        .await
        .unwrap();
        assert_eq!(graph_counts, (1, 1, 1, 1));

        tokio::time::sleep(StdDuration::from_millis(50)).await;
        drain_due(&manager, "test-worker", Utc::now).await.unwrap();
        let repository = WorkflowRepository::new(manager.db().pool().clone());
        assert!(WakeWorkflowAdapter::new(&repository)
            .due(Utc::now())
            .await
            .unwrap()
            .is_empty());
        let terminal_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workflows WHERE id LIKE 'wake-workflow-%' AND status = 'completed'",
        )
        .fetch_one(manager.db().pool())
        .await
        .unwrap();
        assert_eq!(terminal_count, 1);
    }

    #[tokio::test]
    async fn production_registration_replay_returns_original_durable_receipt() {
        let manager = Arc::new(manager_with_due_wakes(0).await);
        let registrar = ProductionWakeRegistrar::new(manager.clone());
        let registration = WakeRegistration {
            conversation_id: "conv".to_owned(),
            tool_use_id: "wait-tool-replay".to_owned(),
            work_scope: WorkScope::Conversation("conv".to_owned()),
            target: WakeRegistrationTarget::Bash {
                handle_id: "missing-but-owned-before-registration".to_owned(),
            },
            max_wait_seconds: 60,
        };
        let first = registrar.register(registration.clone()).await.unwrap();
        let second = registrar.register(registration).await.unwrap();
        assert_eq!(first, second);
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM wake_workflow_bindings WHERE contract_id = ?1",
        )
        .bind(first.contract_id)
        .fetch_one(manager.db().pool())
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn production_registration_rejects_changed_wait_for_same_tool_use() {
        let manager = Arc::new(manager_with_due_wakes(0).await);
        let registrar = ProductionWakeRegistrar::new(manager);
        let registration = WakeRegistration {
            conversation_id: "conv".to_owned(),
            tool_use_id: "wait-tool-conflict".to_owned(),
            work_scope: WorkScope::Conversation("conv".to_owned()),
            target: WakeRegistrationTarget::Bash {
                handle_id: "b-conflict".to_owned(),
            },
            max_wait_seconds: 30,
        };
        registrar.register(registration.clone()).await.unwrap();
        let mut changed = registration;
        changed.max_wait_seconds = 31;
        assert!(matches!(
            registrar.register(changed).await,
            Err(WakeRegistrarError::Conflict)
        ));
    }

    #[tokio::test]
    async fn drain_processes_one_snapshot_and_refreshes_time_per_item() {
        let manager = Arc::new(manager_with_due_wakes(2).await);
        let times = [at(1_001), at(1_002), at(1_003)];
        let mut calls = 0;
        drain_due(&manager, "worker", || {
            let value = times[calls];
            calls += 1;
            value
        })
        .await
        .unwrap();

        assert_eq!(calls, 3, "one discovery time plus one time per due item");
        let repository = WorkflowRepository::new(manager.db().pool().clone());
        assert!(WakeWorkflowAdapter::new(&repository)
            .due(at(1_002))
            .await
            .unwrap()
            .is_empty());
    }

    #[test]
    fn retry_uses_poll_bound_but_never_moves_past_exact_deadline() {
        let binding = binding(WakeResourceIdentity::Bash(BashResourceIdentity {
            work_scope: scope(),
            handle_id: "b-1".to_owned(),
        }));
        assert_eq!(
            retry(&binding, at(50)).unwrap(),
            InspectionDecision::RetryAt(at(51))
        );
        assert_eq!(
            retry(&binding, at(99)).unwrap(),
            InspectionDecision::RetryAt(at(100))
        );
        assert!(matches!(
            retry(&binding, at(100)).unwrap(),
            InspectionDecision::DeadlineTerminal(_)
        ));
    }

    #[tokio::test]
    async fn bash_missing_is_forgotten_immediately_before_deadline() {
        let manager = manager_with_due_wakes(0).await;
        let binding = binding(WakeResourceIdentity::Bash(BashResourceIdentity {
            work_scope: scope(),
            handle_id: "missing".to_owned(),
        }));

        assert!(matches!(
            inspect_binding(&manager, &binding, at(50)).await.unwrap(),
            InspectionDecision::DeadlineTerminal(WakeTerminalPayload::Forgotten {
                reason: WakeForgottenReason::HandleMissing,
                resolved_at: Timestamp(50),
                ..
            })
        ));
    }

    #[test]
    fn tmux_missing_is_forgotten_immediately_while_unavailable_retries() {
        let identity = TmuxResourceIdentity {
            work_scope: scope(),
            server_generation: "generation".to_owned(),
            window_id: "@1".to_owned(),
        };
        let binding = binding(WakeResourceIdentity::TmuxWindow(identity.clone()));

        assert!(matches!(
            classify_tmux(&binding, &identity, TmuxTerminalInspection::Missing, at(50)).unwrap(),
            InspectionDecision::DeadlineTerminal(WakeTerminalPayload::Forgotten {
                reason: WakeForgottenReason::HandleMissing,
                resolved_at: Timestamp(50),
                ..
            })
        ));
        assert_eq!(
            classify_tmux(
                &binding,
                &identity,
                TmuxTerminalInspection::Unavailable,
                at(50),
            )
            .unwrap(),
            InspectionDecision::RetryAt(at(51))
        );
    }

    #[test]
    fn tmux_exit_occurrence_beats_deadline_without_duration() {
        let identity = TmuxResourceIdentity {
            work_scope: scope(),
            server_generation: "generation".to_owned(),
            window_id: "@1".to_owned(),
        };
        let binding = binding(WakeResourceIdentity::TmuxWindow(identity.clone()));
        let decision = classify_tmux(
            &binding,
            &identity,
            TmuxTerminalInspection::Terminal {
                exit_code: 0,
                occurred_at: Some(at(99)),
                duration_ms: None,
                final_tail: vec!["done".to_owned()],
            },
            at(100),
        )
        .unwrap();
        assert!(matches!(
            decision,
            InspectionDecision::Terminal {
                evidence: WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
                    duration_ms: None,
                    ..
                }),
                ..
            }
        ));
    }

    #[test]
    fn tmux_unavailable_at_deadline_is_forgotten() {
        let identity = TmuxResourceIdentity {
            work_scope: scope(),
            server_generation: "generation".to_owned(),
            window_id: "@1".to_owned(),
        };
        let binding = binding(WakeResourceIdentity::TmuxWindow(identity.clone()));
        assert!(matches!(
            classify_tmux(
                &binding,
                &identity,
                TmuxTerminalInspection::Unavailable,
                at(100),
            )
            .unwrap(),
            InspectionDecision::DeadlineTerminal(WakeTerminalPayload::Forgotten { .. })
        ));
    }

    #[test]
    fn tmux_terminal_without_occurrence_time_cannot_beat_deadline() {
        let identity = TmuxResourceIdentity {
            work_scope: scope(),
            server_generation: "generation".to_owned(),
            window_id: "@1".to_owned(),
        };
        let binding = binding(WakeResourceIdentity::TmuxWindow(identity.clone()));
        let inspection = || TmuxTerminalInspection::Terminal {
            exit_code: 0,
            occurred_at: None,
            duration_ms: None,
            final_tail: vec!["done".to_owned()],
        };
        assert_eq!(
            classify_tmux(&binding, &identity, inspection(), at(99)).unwrap(),
            InspectionDecision::RetryAt(at(100))
        );
        assert!(matches!(
            classify_tmux(&binding, &identity, inspection(), at(100)).unwrap(),
            InspectionDecision::DeadlineTerminal(WakeTerminalPayload::Expired { .. })
        ));
    }

    #[test]
    fn tmux_exact_timestamp_beats_later_scheduler_observation() {
        let identity = TmuxResourceIdentity {
            work_scope: scope(),
            server_generation: "generation".to_owned(),
            window_id: "@1".to_owned(),
        };
        let binding = binding(WakeResourceIdentity::TmuxWindow(identity.clone()));
        assert!(matches!(
            classify_tmux(
                &binding,
                &identity,
                TmuxTerminalInspection::Terminal {
                    exit_code: 0,
                    occurred_at: Some(at(100)),
                    duration_ms: Some(12_345),
                    final_tail: vec!["done".to_owned()],
                },
                at(105),
            )
            .unwrap(),
            InspectionDecision::Terminal { .. }
        ));
    }

    #[test]
    fn registered_window_kill_becomes_typed_terminal_evidence() {
        let identity = TmuxResourceIdentity {
            work_scope: scope(),
            server_generation: "generation".to_owned(),
            window_id: "@1".to_owned(),
        };
        let binding = binding(WakeResourceIdentity::TmuxWindow(identity.clone()));
        let decision = classify_tmux(
            &binding,
            &identity,
            TmuxTerminalInspection::WindowKilled {
                occurred_at: at(99),
            },
            at(101),
        )
        .unwrap();
        let InspectionDecision::Terminal {
            evidence: WakeTerminalEvidence::TmuxWindow(evidence),
            ..
        } = decision
        else {
            panic!("expected typed tmux terminal evidence");
        };
        assert_eq!(evidence.status, TmuxTerminalStatus::WindowKilled);
        assert_eq!(evidence.identity, identity);
        assert_eq!(evidence.occurred_at, Timestamp(99));
        assert_eq!(evidence.exit_code, None);
    }

    #[test]
    fn tmux_subsecond_timestamp_obeys_exact_deadline_and_preserves_duration() {
        let identity = TmuxResourceIdentity {
            work_scope: scope(),
            server_generation: "generation".to_owned(),
            window_id: "@1".to_owned(),
        };
        let mut binding = binding(WakeResourceIdentity::TmuxWindow(identity.clone()));
        binding.expires_at = at_nanos(100, 500_000_000);

        let before = classify_tmux(
            &binding,
            &identity,
            TmuxTerminalInspection::Terminal {
                exit_code: 0,
                occurred_at: Some(at_nanos(100, 499_999_999)),
                duration_ms: Some(321),
                final_tail: vec![],
            },
            at(101),
        )
        .unwrap();
        assert!(matches!(
            before,
            InspectionDecision::Terminal {
                evidence: WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
                    duration_ms: Some(321),
                    ..
                }),
                ..
            }
        ));

        let after = classify_tmux(
            &binding,
            &identity,
            TmuxTerminalInspection::Terminal {
                exit_code: 0,
                occurred_at: Some(at_nanos(100, 500_000_001)),
                duration_ms: Some(321),
                final_tail: vec![],
            },
            at(101),
        )
        .unwrap();
        assert!(matches!(
            after,
            InspectionDecision::DeadlineTerminal(WakeTerminalPayload::Expired { .. })
        ));
    }

    #[test]
    fn exact_timestamp_is_in_deadline() {
        let binding = binding(WakeResourceIdentity::Bash(BashResourceIdentity {
            work_scope: scope(),
            handle_id: "b-1".to_owned(),
        }));
        let evidence = WakeTerminalEvidence::Bash(BashTerminalEvidence {
            identity: match binding.resource.clone() {
                WakeResourceIdentity::Bash(identity) => identity,
                WakeResourceIdentity::TmuxWindow(_) => unreachable!(),
            },
            status: BashTerminalStatus::Exited,
            occurred_at: Timestamp(100),
            exit_code: Some(0),
            duration_ms: None,
            signal_number: None,
            kill_signal_sent: None,
            tail_start_offset: 0,
            tail_end_offset: 0,
            tail_truncated_before: false,
            tail_offsets: Vec::new(),
            final_tail: Vec::new(),
        });
        assert!(matches!(
            fired(&binding, evidence, at(101)).unwrap(),
            InspectionDecision::Terminal { .. }
        ));
    }

    #[test]
    fn datetime_to_timestamp_rejects_pre_epoch_values() {
        assert_eq!(timestamp(at(0)).unwrap(), Timestamp(0));
        assert!(timestamp(at(-1)).is_err());
    }
}
