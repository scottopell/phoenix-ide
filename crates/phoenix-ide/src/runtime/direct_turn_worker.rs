use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use phoenix_core::domain::sm_event::{DirectTurnAttemptAuthority, PreparedDirectTurnPayload};
use phoenix_db::workflow::{
    ClaimAuthoritativeTurnInput, DirectTurnMaterializationEligibility, DiscoverableAcceptedTurn,
    PreflightDirectTurnMaterializationInput, ReleaseAuthoritativeTurnInput, WorkflowRepository,
};
use phoenix_db::LocalAttemptAuthority;
use phoenix_workflow::{ClaimOutcome, LeaseExpiry, ProcessIncarnation, Timestamp};
use tokio::sync::watch;

use crate::runtime::RuntimeManager;
use crate::state_machine::Event;

const DISCOVERY_BATCH_LIMIT: usize = 64;
const LEASE_DURATION: Duration = Duration::from_secs(30);
const EMPTY_RESCAN_INTERVAL: Duration = Duration::from_secs(5);
const ERROR_RETRY_INTERVAL: Duration = Duration::from_millis(250);

fn fresh_process_incarnation() -> ProcessIncarnation {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&uuid::Uuid::new_v4().into_bytes()[..8]);
    bytes[7] &= 0x7f;
    ProcessIncarnation(u64::from_le_bytes(bytes))
}

pub(crate) async fn run(
    manager: Arc<RuntimeManager>,
    kick_rx: watch::Receiver<u64>,
    ready_tx: tokio::sync::oneshot::Sender<()>,
) {
    let worker = DirectTurnWorker::new(
        WorkflowRepository::new(manager.db().pool().clone()),
        Arc::new(ProductionDirectTurnDispatcher { manager }),
        Arc::new(SystemClock),
        fresh_process_incarnation(),
    );
    if let Err(error) = worker.run_loop(kick_rx, ready_tx).await {
        tracing::warn!(error = %error, "direct-turn worker stopped");
    }
}

#[derive(Clone)]
pub(crate) struct DirectTurnWorker<D: DirectTurnDispatcher, C: DirectTurnClock> {
    repo: WorkflowRepository,
    dispatcher: Arc<D>,
    clock: Arc<C>,
    process_incarnation: ProcessIncarnation,
    #[cfg(test)]
    pre_dispatch_hook: Option<PreDispatchHook>,
}

#[cfg(test)]
type PreDispatchHook =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static>;

impl<D: DirectTurnDispatcher, C: DirectTurnClock> DirectTurnWorker<D, C> {
    pub(crate) fn new(
        repo: WorkflowRepository,
        dispatcher: Arc<D>,
        clock: Arc<C>,
        process_incarnation: ProcessIncarnation,
    ) -> Self {
        Self {
            repo,
            dispatcher,
            clock,
            process_incarnation,
            #[cfg(test)]
            pre_dispatch_hook: None,
        }
    }

    #[cfg(test)]
    fn with_pre_dispatch_hook(mut self, hook: PreDispatchHook) -> Self {
        self.pre_dispatch_hook = Some(hook);
        self
    }

    async fn run_loop(
        &self,
        mut kick_rx: watch::Receiver<u64>,
        ready_tx: tokio::sync::oneshot::Sender<()>,
    ) -> Result<(), String> {
        self.run_once().await?;
        let _ = ready_tx.send(());
        loop {
            let wait = match self.run_once().await {
                Ok(wait) => wait,
                Err(error) => {
                    tracing::warn!(error = %error, "direct-turn worker pass failed; retrying");
                    ERROR_RETRY_INTERVAL
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

    pub(crate) async fn run_once(&self) -> Result<Duration, String> {
        let mut cursor = None;
        loop {
            let page = self
                .repo
                .list_discoverable_accepted_runtime_direct_turns(cursor, DISCOVERY_BATCH_LIMIT)
                .await
                .map_err(|error| error.to_string())?;
            let exhausted = page.next_cursor.is_none() || page.next_cursor == cursor;
            cursor = page.next_cursor;
            for candidate in page.candidates {
                self.dispatch_candidate(candidate, self.clock.now()).await?;
            }
            if exhausted {
                break;
            }
        }
        Ok(EMPTY_RESCAN_INTERVAL)
    }

    async fn dispatch_candidate(
        &self,
        candidate: DiscoverableAcceptedTurn,
        now: Timestamp,
    ) -> Result<(), String> {
        let lease_until = LeaseExpiry(now.0.saturating_add(LEASE_DURATION.as_secs()));
        let claim = self
            .repo
            .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
                turn_id: candidate.turn_id,
                workflow_id: candidate.workflow_id,
                process_incarnation: self.process_incarnation,
                now,
                lease_until,
            })
            .await
            .map_err(|error| error.to_string())?;
        if claim.outcome != ClaimOutcome::Started {
            return Ok(());
        }
        let Some(authority) = claim.authority else {
            return Ok(());
        };
        let prepared = match PreparedDirectTurnPayload::from_exact_bytes(
            candidate.prepared.payload(),
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                let terminal = phoenix_workflow::TurnCommand::Fail {
                    turn_id: candidate.turn_id,
                    expected_generation: authority.generation.0,
                    reason: format!("prepared payload decode failed: {error}"),
                };
                if let Err(terminal_error) = self.repo.terminate_authoritative_turn(terminal).await
                {
                    tracing::error!(turn_id = candidate.turn_id.0, error = %terminal_error, "failed to quarantine corrupt direct-turn payload");
                }
                tracing::warn!(turn_id = candidate.turn_id.0, error = %error, "direct-turn payload decode failed; terminally quarantined turn");
                return Ok(());
            }
        };
        #[cfg(test)]
        if let Some(hook) = &self.pre_dispatch_hook {
            hook().await;
        }
        let eligibility = match self
            .repo
            .preflight_direct_turn_materialization(&PreflightDirectTurnMaterializationInput {
                turn_id: candidate.turn_id,
                authority: authority.clone(),
                prepared: prepared.clone(),
                now,
            })
            .await
        {
            Ok(eligibility) => eligibility,
            Err(error) => {
                self.release(authority, now).await?;
                return Err(error.to_string());
            }
        };
        match eligibility {
            DirectTurnMaterializationEligibility::Fresh => {}
            DirectTurnMaterializationEligibility::ExactReplay => {
                tracing::debug!(
                    turn_id = candidate.turn_id.0,
                    "direct-turn already materialized before dispatch; skipping"
                );
                return Ok(());
            }
            DirectTurnMaterializationEligibility::StaleAuthority => {
                tracing::debug!(
                    turn_id = candidate.turn_id.0,
                    "direct-turn authority stale before dispatch; skipping"
                );
                return Ok(());
            }
        }
        let event = Event::AuthoritativeUserMessage {
            payload: prepared,
            authority: authority_to_event(&authority, candidate.turn_id),
        };
        if let Err(error) = self
            .dispatcher
            .dispatch(&candidate.conversation.0, event)
            .await
        {
            self.release(authority, now).await?;
            tracing::warn!(conversation_id = %candidate.conversation.0, turn_id = candidate.turn_id.0, error = %error, "direct-turn dispatch failed; released claim");
        }
        Ok(())
    }

    async fn release(
        &self,
        authority: LocalAttemptAuthority,
        now: Timestamp,
    ) -> Result<(), String> {
        match self
            .repo
            .release_authoritative_turn_dispatch_failure(&ReleaseAuthoritativeTurnInput {
                authority,
                now,
            })
            .await
            .map_err(|error| error.to_string())?
        {
            phoenix_workflow::AuthorityOutcome::Authorized => Ok(()),
            phoenix_workflow::AuthorityOutcome::StaleAuthority => {
                tracing::debug!("direct-turn claim was already fenced before release");
                Ok(())
            }
        }
    }
}

fn authority_to_event(
    authority: &LocalAttemptAuthority,
    turn_id: phoenix_workflow::TurnAuthorityId,
) -> DirectTurnAttemptAuthority {
    DirectTurnAttemptAuthority::new(
        authority.workflow_id.0,
        turn_id.0,
        authority.effect_id.0,
        authority.attempt_id.0,
        authority.declared_workflow_version.0,
        authority.generation.0,
        authority.process_incarnation.0,
    )
}

#[async_trait]
pub(crate) trait DirectTurnDispatcher: Send + Sync + 'static {
    async fn dispatch(&self, conversation_id: &str, event: Event) -> Result<(), String>;
}

struct ProductionDirectTurnDispatcher {
    manager: Arc<RuntimeManager>,
}

#[async_trait]
impl DirectTurnDispatcher for ProductionDirectTurnDispatcher {
    async fn dispatch(&self, conversation_id: &str, event: Event) -> Result<(), String> {
        let handle = self.manager.get_or_create(conversation_id).await?;
        if !matches!(
            *handle.state_rx.borrow(),
            phoenix_core::domain::sm_state::ConvState::Idle
                | phoenix_core::domain::sm_state::ConvState::Error { .. }
        ) {
            return Err("direct-turn reducer cannot accept a new user turn".to_string());
        }
        handle
            .event_tx
            .send(event)
            .await
            .map_err(|error| format!("Failed to send direct-turn event: {error}"))
    }
}

pub(crate) trait DirectTurnClock: Send + Sync + 'static {
    fn now(&self) -> Timestamp;
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

struct SystemClock;

impl DirectTurnClock for SystemClock {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use phoenix_core::domain::sm_event::PreparedDirectTurnPayload;
    use phoenix_workflow::{
        AcceptedDisposition, CanonicalMessageId, ClientTurnKey, ConversationAuthority, EffectId,
        PreparedTurn, TurnCommand, TurnOutcome,
    };
    use std::sync::Mutex;

    #[derive(Clone)]
    struct TestClock {
        now: Timestamp,
    }

    impl DirectTurnClock for TestClock {
        fn now(&self) -> Timestamp {
            self.now
        }

        fn sleep(&self, _duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            Box::pin(std::future::pending())
        }
    }

    struct RecordingDispatcher {
        result: Mutex<Result<(), String>>,
        events: Mutex<Vec<(String, Event)>>,
    }

    impl Default for RecordingDispatcher {
        fn default() -> Self {
            Self {
                result: Mutex::new(Ok(())),
                events: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl DirectTurnDispatcher for RecordingDispatcher {
        async fn dispatch(&self, conversation_id: &str, event: Event) -> Result<(), String> {
            self.events
                .lock()
                .unwrap()
                .push((conversation_id.to_string(), event));
            self.result.lock().unwrap().clone()
        }
    }

    async fn fixture() -> (WorkflowRepository, Arc<RecordingDispatcher>) {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-a", "A", "/tmp", true, None, None)
            .await
            .unwrap();
        let repo = WorkflowRepository::new(db.pool().clone());
        (repo, Arc::new(RecordingDispatcher::default()))
    }

    fn prepared_payload(message_id: &str) -> PreparedDirectTurnPayload {
        PreparedDirectTurnPayload::from_parts(
            phoenix_core::domain::sm_event::SubmittedDirectTurnIdentity {
                text: format!("text-{message_id}"),
                images: Vec::new(),
                files: Vec::new(),
                message_id: message_id.to_string(),
                user_agent: Some("agent/test".to_string()),
                skill_invocation: None,
                expansion_policy: phoenix_core::domain::sm_event::SubmittedDirectTurnExpansionPolicy::ExpandReferences,
            },
            phoenix_core::domain::sm_event::PreparedDirectTurnDelivery {
                text: format!("text-{message_id}"),
                llm_text: None,
                images: Vec::new(),
                files: Vec::new(),
                user_agent: Some("agent/test".to_string()),
                skill_invocation: None,
            },
        )
    }

    fn prepared_turn(message_id: &str) -> PreparedTurn {
        PreparedTurn::from_exact_payload(
            &ConversationAuthority("conv-a".to_string()),
            prepared_payload(message_id).to_exact_bytes().unwrap(),
        )
    }

    async fn accept(repo: &WorkflowRepository, key: &str) -> phoenix_workflow::TurnAuthorityId {
        let step = repo
            .accept_authoritative_turn(&phoenix_db::workflow::AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new(key).unwrap(),
                prepared: prepared_turn(&format!("message-{key}")),
                disposition: AcceptedDisposition::Runtime,
                accepted_at: Timestamp(1),
            })
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = step.outcome else {
            panic!("expected created turn")
        };
        turn_id
    }

    async fn latest_authority(
        repo: &WorkflowRepository,
        workflow_id: phoenix_workflow::WorkflowId,
    ) -> LocalAttemptAuthority {
        repo.list_attempts(workflow_id, EffectId(1))
            .await
            .unwrap()
            .into_iter()
            .last()
            .unwrap()
            .authority
    }

    async fn receipt_count(
        repo: &WorkflowRepository,
        workflow_id: phoenix_workflow::WorkflowId,
    ) -> usize {
        repo.list_receipts(workflow_id).await.unwrap().len()
    }

    fn worker(
        repo: WorkflowRepository,
        dispatcher: Arc<RecordingDispatcher>,
        now: u64,
        process_incarnation: u64,
    ) -> DirectTurnWorker<RecordingDispatcher, TestClock> {
        DirectTurnWorker::new(
            repo,
            dispatcher,
            Arc::new(TestClock {
                now: Timestamp(now),
            }),
            ProcessIncarnation(process_incarnation),
        )
    }

    #[tokio::test]
    async fn dormant_empty_pass_discovers_nothing() {
        let (repo, dispatcher) = fixture().await;
        let wait = worker(repo, dispatcher.clone(), 10, 1)
            .run_once()
            .await
            .unwrap();
        assert_eq!(wait, EMPTY_RESCAN_INTERVAL);
        assert!(dispatcher.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn claim_dispatches_exact_authoritative_event() {
        let (repo, dispatcher) = fixture().await;
        let turn_id = accept(&repo, "dispatch").await;
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let wait = worker(repo, dispatcher.clone(), 10, 77)
            .run_once()
            .await
            .unwrap();
        assert_eq!(wait, EMPTY_RESCAN_INTERVAL);
        let events = dispatcher.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "conv-a");
        let Event::AuthoritativeUserMessage { payload, authority } = &events[0].1 else {
            panic!("expected authoritative message")
        };
        assert_eq!(payload, &prepared_payload("message-dispatch"));
        assert_eq!(authority.workflow_id.0, workflow_id.0);
        assert_eq!(authority.turn_id.0, turn_id.0);
        assert_eq!(authority.effect_id.0, 1);
        assert_eq!(authority.process_incarnation.0, 77);
    }

    #[tokio::test]
    async fn live_contention_is_skipped_without_dispatch() {
        let (repo, dispatcher) = fixture().await;
        let turn_id = accept(&repo, "contention").await;
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        repo.claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
            turn_id,
            workflow_id,
            process_incarnation: ProcessIncarnation(1),
            now: Timestamp(10),
            lease_until: LeaseExpiry(40),
        })
        .await
        .unwrap();
        worker(repo, dispatcher.clone(), 11, 2)
            .run_once()
            .await
            .unwrap();
        assert!(dispatcher.events.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn expired_lease_is_reclaimed_and_dispatched() {
        let (repo, dispatcher) = fixture().await;
        let turn_id = accept(&repo, "expired").await;
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        repo.claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
            turn_id,
            workflow_id,
            process_incarnation: ProcessIncarnation(1),
            now: Timestamp(10),
            lease_until: LeaseExpiry(12),
        })
        .await
        .unwrap();
        worker(repo.clone(), dispatcher.clone(), 13, 2)
            .run_once()
            .await
            .unwrap();
        assert_eq!(dispatcher.events.lock().unwrap().len(), 1);
        let attempts = repo
            .list_attempts(workflow_id, phoenix_workflow::EffectId(1))
            .await
            .unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(
            attempts[0].status,
            phoenix_workflow::AttemptStatus::AuthorityLost
        );
        assert_eq!(attempts[1].status, phoenix_workflow::AttemptStatus::Begun);
    }

    #[tokio::test]
    async fn claim_terminalized_between_claim_and_dispatch_is_not_dispatched_or_released() {
        let (repo, dispatcher) = fixture().await;
        let turn_id = accept(&repo, "terminalized").await;
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let hook_repo = repo.clone();
        let worker = worker(repo.clone(), dispatcher.clone(), 10, 1).with_pre_dispatch_hook(
            Arc::new(move || {
                let repo = hook_repo.clone();
                Box::pin(async move {
                    repo.terminate_authoritative_turn(TurnCommand::Complete {
                        turn_id,
                        expected_generation: 0,
                    })
                    .await
                    .unwrap();
                })
            }),
        );

        worker.run_once().await.unwrap();

        assert!(dispatcher.events.lock().unwrap().is_empty());
        assert_eq!(receipt_count(&repo, workflow_id).await, 0);
        let attempts = repo
            .list_attempts(workflow_id, phoenix_workflow::EffectId(1))
            .await
            .unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(
            attempts[0].status,
            phoenix_workflow::AttemptStatus::AuthorityLost
        );
    }

    #[tokio::test]
    async fn already_materialized_between_claim_and_dispatch_is_not_dispatched_or_duplicated() {
        let (repo, dispatcher) = fixture().await;
        let turn_id = accept(&repo, "materialized").await;
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let hook_repo = repo.clone();
        let worker = worker(repo.clone(), dispatcher.clone(), 10, 1).with_pre_dispatch_hook(
            Arc::new(move || {
                let repo = hook_repo.clone();
                Box::pin(async move {
                    let authority = latest_authority(&repo, workflow_id).await;
                    repo.materialize_authoritative_turn(
                        &phoenix_db::workflow::MaterializeAuthoritativeTurnInput {
                            turn_id,
                            authority,
                            prepared: prepared_payload("message-materialized"),
                            sequence_id: 99,
                            created_at: Timestamp(99),
                            accepted_state:
                                phoenix_core::domain::db_schema::ConvState::LlmRequesting {
                                    attempt: 1,
                                },
                            state_updated_at: chrono::DateTime::from_timestamp(99, 0).unwrap(),
                            now: Timestamp(10),
                        },
                    )
                    .await
                    .unwrap();
                })
            }),
        );

        worker.run_once().await.unwrap();

        assert!(dispatcher.events.lock().unwrap().is_empty());
        assert_eq!(receipt_count(&repo, workflow_id).await, 1);
        let turn = repo
            .load_authoritative_turn(turn_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(turn.generation, 0);
        assert_eq!(
            turn.materialization,
            phoenix_workflow::Materialization::Materialized {
                message_id: CanonicalMessageId("conv-a:message-materialized".to_string())
            }
        );
    }

    #[tokio::test]
    async fn unavailable_runtime_or_closed_channel_releases_claim() {
        let (repo, dispatcher) = fixture().await;
        *dispatcher.result.lock().unwrap() = Err("closed".to_string());
        let turn_id = accept(&repo, "release").await;
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        worker(repo.clone(), dispatcher.clone(), 10, 1)
            .run_once()
            .await
            .unwrap();
        assert_eq!(dispatcher.events.lock().unwrap().len(), 1);
        let attempts = repo
            .list_attempts(workflow_id, phoenix_workflow::EffectId(1))
            .await
            .unwrap();
        assert_eq!(
            attempts[0].status,
            phoenix_workflow::AttemptStatus::AuthorityLost
        );
        assert!(repo
            .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
                turn_id,
                workflow_id,
                process_incarnation: ProcessIncarnation(2),
                now: Timestamp(11),
                lease_until: LeaseExpiry(20),
            })
            .await
            .unwrap()
            .authority
            .is_some());
    }

    #[tokio::test]
    async fn malformed_payload_is_skipped_without_dispatch() {
        let (repo, dispatcher) = fixture().await;
        let step = repo
            .accept_authoritative_turn(&phoenix_db::workflow::AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new("bad").unwrap(),
                prepared: prepared_turn("bad"),
                disposition: AcceptedDisposition::Runtime,
                accepted_at: Timestamp(1),
            })
            .await
            .unwrap();
        let TurnOutcome::Created { turn_id, .. } = step.outcome else {
            panic!("expected created turn")
        };
        let malformed = b"not-json";
        let malformed_prepared = PreparedTurn::from_exact_payload(
            &ConversationAuthority("conv-a".to_string()),
            malformed.to_vec(),
        );
        sqlx::query(
            "UPDATE durable_turns
             SET prepared_payload = ?1, prepared_fingerprint = ?2
             WHERE turn_id = ?3",
        )
        .bind(malformed.as_slice())
        .bind(malformed_prepared.fingerprint())
        .bind(i64::try_from(turn_id.0).unwrap())
        .execute(repo.pool())
        .await
        .unwrap();
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let worker = worker(repo.clone(), dispatcher.clone(), 10, 1);
        worker.run_once().await.unwrap();
        worker.run_once().await.unwrap();
        assert!(dispatcher.events.lock().unwrap().is_empty());
        let attempts = repo
            .list_attempts(workflow_id, phoenix_workflow::EffectId(1))
            .await
            .unwrap();
        assert!(attempts
            .iter()
            .all(|attempt| attempt.status == phoenix_workflow::AttemptStatus::AuthorityLost));
        let terminal_kind: String =
            sqlx::query_scalar("SELECT terminal_kind FROM durable_turns WHERE turn_id = ?1")
                .bind(i64::try_from(turn_id.0).unwrap())
                .fetch_one(repo.pool())
                .await
                .unwrap();
        assert!(terminal_kind.is_empty());
    }

    #[tokio::test]
    async fn startup_failure_does_not_signal_ready() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        pool.close().await;
        let repo = WorkflowRepository::new(pool);
        let dispatcher = Arc::new(RecordingDispatcher::default());
        let (_kick_tx, kick_rx) = watch::channel(0);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let worker = worker(repo, dispatcher, 10, 1);

        assert!(worker.run_loop(kick_rx, ready_tx).await.is_err());
        assert!(ready_rx.await.is_err());
    }

    #[tokio::test]
    async fn startup_pass_runs_before_ready_signal() {
        let (repo, dispatcher) = fixture().await;
        accept(&repo, "startup").await;
        let (kick_tx, kick_rx) = watch::channel(0);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let worker = worker(repo, dispatcher.clone(), 10, 1);
        let handle = tokio::spawn(async move { worker.run_loop(kick_rx, ready_tx).await });
        ready_rx.await.unwrap();
        assert_eq!(dispatcher.events.lock().unwrap().len(), 1);
        drop(kick_tx);
        handle.await.unwrap().unwrap();
    }
}
