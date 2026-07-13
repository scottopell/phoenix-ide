//! Production executor for durable bash/tmux wake observation effects.
//!
//! This worker only reconciles external handle state into durable workflow
//! receipts. It deliberately does not broadcast SSE or accept the resulting
//! runtime obligation; those are separate reducer/runtime concerns.

use std::{sync::Arc, time::Duration as StdDuration};

use chrono::{DateTime, Duration, Utc};
use phoenix_core::work_scope::WorkScope;
use phoenix_db::workflow::{
    wake::{
        ClaimedWakeEffect, WakeBinding, WakeObservationRequest, WakeTerminalReceiptRequest,
        WakeWorkflowAdapter,
    },
    AcceptReceiptResult, DueEffect, DurableReceiptOrigin, ReconcileEffectResult,
    WorkflowRepository,
};
use phoenix_tools::{BashTerminalInspection, TmuxTerminalInspection, TmuxWindowIdentity};
use phoenix_workflow::{
    wake_profile::{
        BashTerminalEvidence, BashTerminalStatus, TmuxTerminalEvidence, TmuxTerminalStatus,
        WakeForgottenReason, WakeResourceIdentity, WakeTerminalEvidence, WakeTerminalPayload,
        WorkScopeIdentity, WorkScopeKind,
    },
    Timestamp,
};
use tokio::sync::watch;

use super::RuntimeManager;

const CLAIM_LEASE: Duration = Duration::seconds(30);
const HANDLE_POLL: Duration = Duration::seconds(1);
const MAX_IDLE_POLL: StdDuration = StdDuration::from_secs(1);
const ERROR_BACKOFF_MIN: StdDuration = StdDuration::from_millis(250);
const ERROR_BACKOFF_MAX: StdDuration = StdDuration::from_secs(5);

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
    manager: &RuntimeManager,
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
        if matches!(item, DueEffect::RetryWait { .. }) {
            adapter
                .promote_exact_deadline(&item, item_now)
                .await
                .map_err(|error| error.to_string())?;
            continue;
        }
        let Some(claim) = adapter
            .claim(
                &item,
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
                        final_tail: tails,
                    });
                    fired(binding, evidence, now)
                }
                BashTerminalInspection::Unknown if deadline_reached => forgotten(binding, now),
                BashTerminalInspection::Live | BashTerminalInspection::Terminal { .. }
                    if deadline_reached =>
                {
                    expired(binding, now)
                }
                BashTerminalInspection::Unknown
                | BashTerminalInspection::Live
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
            duration_ms: Some(duration_ms),
            final_tail,
        } if occurred_at <= binding.expires_at => {
            let evidence = WakeTerminalEvidence::TmuxWindow(TmuxTerminalEvidence {
                identity: identity.clone(),
                status: TmuxTerminalStatus::ExitMarkerObserved,
                occurred_at: timestamp(occurred_at)?,
                exit_code: Some(exit_code),
                duration_ms: Some(duration_ms),
                final_tail,
            });
            fired(binding, evidence, now)
        }
        TmuxTerminalInspection::Missing if now >= binding.expires_at => forgotten(binding, now),
        TmuxTerminalInspection::Live
        | TmuxTerminalInspection::WindowKilled { .. }
        | TmuxTerminalInspection::Terminal { .. }
            if now >= binding.expires_at =>
        {
            expired(binding, now)
        }
        // A terminal marker without its durable occurrence timestamp is not
        // evidence that may outrank the exact deadline.
        TmuxTerminalInspection::Missing
        | TmuxTerminalInspection::Live
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
    async fn drain_processes_one_snapshot_and_refreshes_time_per_item() {
        let manager = manager_with_due_wakes(2).await;
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

    #[test]
    fn missing_is_forgotten_only_by_the_typed_deadline_decision() {
        let binding = binding(WakeResourceIdentity::TmuxWindow(TmuxResourceIdentity {
            work_scope: scope(),
            server_generation: "generation".to_owned(),
            window_id: "@1".to_owned(),
        }));
        assert!(matches!(
            retry(&binding, at(99)).unwrap(),
            InspectionDecision::RetryAt(value) if value == at(100)
        ));
        assert!(matches!(
            forgotten(&binding, at(100)).unwrap(),
            InspectionDecision::DeadlineTerminal(WakeTerminalPayload::Forgotten {
                reason: WakeForgottenReason::HandleMissing,
                ..
            })
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
