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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalObligationSettlement {
    NoObligation,
    AlreadyCommitted,
    Committed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StartupReconciliationError {
    Retryable(String),
    Unclassifiable(String),
}

impl From<crate::runtime::DatabaseTerminalRecoveryError> for StartupReconciliationError {
    fn from(error: crate::runtime::DatabaseTerminalRecoveryError) -> Self {
        match error {
            crate::runtime::DatabaseTerminalRecoveryError::StillOwed(error)
            | crate::runtime::DatabaseTerminalRecoveryError::Retryable(error) => {
                Self::Retryable(error)
            }
            crate::runtime::DatabaseTerminalRecoveryError::Unclassifiable(error) => {
                Self::Unclassifiable(error)
            }
        }
    }
}

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
        match error {
            StartupReconciliationError::Retryable(error) => {
                tracing::warn!(%error, "direct-turn worker stopped with settlement still owed");
            }
            StartupReconciliationError::Unclassifiable(error) => {
                tracing::error!(%error, "fatal local SQLite authority loss in direct-turn worker");
                worker.dispatcher.signal_fatal_local_authority();
            }
        }
    }
}

#[async_trait]
trait TerminalObligationDiscovery: Send + Sync + 'static {
    async fn list(
        &self,
        limit: usize,
    ) -> Result<Vec<phoenix_db::workflow::DiscoverableTerminalObligation>, String>;
}

#[async_trait]
impl TerminalObligationDiscovery for WorkflowRepository {
    async fn list(
        &self,
        limit: usize,
    ) -> Result<Vec<phoenix_db::workflow::DiscoverableTerminalObligation>, String> {
        self.list_discoverable_terminal_obligations(limit)
            .await
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
pub(crate) struct DirectTurnWorker<D: DirectTurnDispatcher, C: DirectTurnClock> {
    repo: WorkflowRepository,
    terminal_discovery: Arc<dyn TerminalObligationDiscovery>,
    dispatcher: Arc<D>,
    clock: Arc<C>,
    process_incarnation: ProcessIncarnation,
    #[cfg(test)]
    pre_dispatch_hook: Option<PreDispatchHook>,
}

#[cfg(test)]
type PreDispatchHook =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static>;

impl<D: DirectTurnDispatcher + TerminalObligationDispatcher, C: DirectTurnClock>
    DirectTurnWorker<D, C>
{
    pub(crate) fn new(
        repo: WorkflowRepository,
        dispatcher: Arc<D>,
        clock: Arc<C>,
        process_incarnation: ProcessIncarnation,
    ) -> Self {
        Self {
            terminal_discovery: Arc::new(repo.clone()),
            repo,
            dispatcher,
            clock,
            process_incarnation,
            #[cfg(test)]
            pre_dispatch_hook: None,
        }
    }

    #[cfg(test)]
    fn with_terminal_discovery(
        mut self,
        terminal_discovery: Arc<dyn TerminalObligationDiscovery>,
    ) -> Self {
        self.terminal_discovery = terminal_discovery;
        self
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
    ) -> Result<(), StartupReconciliationError> {
        let mut wait = loop {
            match self.run_once().await {
                Ok(wait) => break wait,
                Err(error) => match StartupReconciliationError::from(error) {
                    StartupReconciliationError::Retryable(error) => {
                        tracing::warn!(
                            %error,
                            "direct-turn startup reconciliation remains owed; retrying"
                        );
                        self.clock.sleep(ERROR_RETRY_INTERVAL).await;
                    }
                    fatal @ StartupReconciliationError::Unclassifiable(_) => return Err(fatal),
                },
            }
        };
        let _ = ready_tx.send(());
        loop {
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
            wait = match self.run_once().await {
                Ok(wait) => wait,
                Err(error) => match StartupReconciliationError::from(error) {
                    StartupReconciliationError::Retryable(error) => {
                        tracing::warn!(%error, "direct-turn worker pass remains owed; retrying");
                        ERROR_RETRY_INTERVAL
                    }
                    fatal @ StartupReconciliationError::Unclassifiable(_) => return Err(fatal),
                },
            };
        }
    }

    pub(crate) async fn run_once(
        &self,
    ) -> Result<Duration, crate::runtime::DatabaseTerminalRecoveryError> {
        for obligation in self
            .terminal_discovery
            .list(DISCOVERY_BATCH_LIMIT)
            .await
            .map_err(crate::runtime::DatabaseTerminalRecoveryError::Retryable)?
        {
            self.dispatcher
                .settle_terminal_obligation(&obligation.conversation.0)
                .await?;
        }

        let mut cursor = None;
        loop {
            let page = self
                .repo
                .list_discoverable_accepted_runtime_direct_turns(cursor, DISCOVERY_BATCH_LIMIT)
                .await
                .map_err(|error| {
                    crate::runtime::DatabaseTerminalRecoveryError::Unclassifiable(error.to_string())
                })?;
            let exhausted = page.next_cursor.is_none() || page.next_cursor == cursor;
            cursor = page.next_cursor;
            for candidate in page.candidates {
                self.dispatch_candidate(candidate, self.clock.now())
                    .await
                    .map_err(crate::runtime::DatabaseTerminalRecoveryError::Unclassifiable)?;
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

#[async_trait]
pub(crate) trait TerminalObligationDispatcher: Send + Sync + 'static {
    async fn settle_terminal_obligation(
        &self,
        conversation_id: &str,
    ) -> Result<TerminalObligationSettlement, crate::runtime::DatabaseTerminalRecoveryError>;

    fn signal_fatal_local_authority(&self) {}
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

#[async_trait]
impl TerminalObligationDispatcher for ProductionDirectTurnDispatcher {
    fn signal_fatal_local_authority(&self) {
        self.manager
            .signal_fatal_local_authority("direct_turn_terminal_recovery");
    }

    async fn settle_terminal_obligation(
        &self,
        conversation_id: &str,
    ) -> Result<TerminalObligationSettlement, crate::runtime::DatabaseTerminalRecoveryError> {
        let recovery = self
            .manager
            .settle_database_terminal_obligation(conversation_id)
            .await?;
        let outcome = match &recovery {
            super::DatabaseTerminalRecovery::NoObligation => {
                TerminalObligationSettlement::NoObligation
            }
            super::DatabaseTerminalRecovery::AlreadyCommitted => {
                TerminalObligationSettlement::AlreadyCommitted
            }
            super::DatabaseTerminalRecovery::Committed { .. } => {
                TerminalObligationSettlement::Committed
            }
        };
        self.manager
            .complete_database_terminal_recovery(conversation_id, recovery)
            .await;
        Ok(outcome)
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
        sleeps: Option<Arc<Mutex<Vec<Duration>>>>,
    }

    impl DirectTurnClock for TestClock {
        fn now(&self) -> Timestamp {
            self.now
        }

        fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            if let Some(sleeps) = &self.sleeps {
                sleeps.lock().unwrap().push(duration);
                if duration == ERROR_RETRY_INTERVAL {
                    Box::pin(std::future::ready(()))
                } else {
                    Box::pin(std::future::pending())
                }
            } else {
                Box::pin(std::future::pending())
            }
        }
    }

    struct GatedRetryClock {
        sleeps: Arc<Mutex<Vec<Duration>>>,
        retry_started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        retry_release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    impl DirectTurnClock for GatedRetryClock {
        fn now(&self) -> Timestamp {
            Timestamp(10)
        }

        fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            self.sleeps.lock().unwrap().push(duration);
            if duration == ERROR_RETRY_INTERVAL {
                if let Some(started) = self.retry_started.lock().unwrap().take() {
                    let _ = started.send(());
                }
                let release = self.retry_release.lock().unwrap().take();
                Box::pin(async move {
                    if let Some(release) = release {
                        let _ = release.await;
                    }
                })
            } else {
                Box::pin(std::future::pending())
            }
        }
    }

    struct FailingTerminalDiscovery {
        repo: WorkflowRepository,
        failures_remaining: std::sync::atomic::AtomicUsize,
        attempts: std::sync::atomic::AtomicUsize,
        initial_empty: bool,
    }

    #[async_trait]
    impl TerminalObligationDiscovery for FailingTerminalDiscovery {
        async fn list(
            &self,
            limit: usize,
        ) -> Result<Vec<phoenix_db::workflow::DiscoverableTerminalObligation>, String> {
            let attempt = self
                .attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.initial_empty && attempt == 0 {
                return Ok(Vec::new());
            }
            if self
                .failures_remaining
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                return Err("injected discovery read failure".to_string());
            }
            self.repo
                .list_discoverable_terminal_obligations(limit)
                .await
                .map_err(|error| error.to_string())
        }
    }

    struct RecordingDispatcher {
        result: Mutex<Result<(), String>>,
        terminal_results: Mutex<
            Vec<
                Result<TerminalObligationSettlement, crate::runtime::DatabaseTerminalRecoveryError>,
            >,
        >,
        events: Mutex<Vec<(String, Event)>>,
        terminal_attempts: Mutex<Vec<String>>,
        terminal_settled: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    }

    impl Default for RecordingDispatcher {
        fn default() -> Self {
            Self {
                result: Mutex::new(Ok(())),
                terminal_results: Mutex::new(vec![Ok(TerminalObligationSettlement::Committed)]),
                events: Mutex::new(Vec::new()),
                terminal_attempts: Mutex::new(Vec::new()),
                terminal_settled: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl TerminalObligationDispatcher for RecordingDispatcher {
        async fn settle_terminal_obligation(
            &self,
            conversation_id: &str,
        ) -> Result<TerminalObligationSettlement, crate::runtime::DatabaseTerminalRecoveryError>
        {
            self.terminal_attempts
                .lock()
                .unwrap()
                .push(conversation_id.to_string());
            if let Some(settled) = self.terminal_settled.lock().unwrap().take() {
                let _ = settled.send(());
            }
            let mut results = self.terminal_results.lock().unwrap();
            if results.len() == 1 {
                results[0].clone()
            } else {
                results.remove(0)
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

    async fn seed_terminal_obligation(repo: &WorkflowRepository, key: &str) {
        let turn_id = accept(repo, key).await;
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let claim = repo
            .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
                turn_id,
                workflow_id,
                process_incarnation: ProcessIncarnation(1),
                now: Timestamp(10),
                lease_until: LeaseExpiry(40),
            })
            .await
            .unwrap();
        let authority = claim.authority.unwrap();
        repo.materialize_authoritative_turn(
            &phoenix_db::workflow::MaterializeAuthoritativeTurnInput {
                turn_id,
                authority,
                prepared: prepared_payload(&format!("message-{key}")),
                sequence_id: 199,
                created_at: Timestamp(199),
                accepted_state: phoenix_core::domain::db_schema::ConvState::LlmRequesting {
                    attempt: 1,
                },
                state_updated_at: chrono::DateTime::from_timestamp(199, 0).unwrap(),
                now: Timestamp(10),
            },
        )
        .await
        .unwrap();
        repo.persist_terminal_obligation(
            &phoenix_db::workflow::DirectTurnTerminalObligationInput {
                turn_id,
                expected_generation: 0,
                terminal: phoenix_workflow::TurnTerminal::Completed,
                projection: phoenix_db::workflow::PersistedConversationProjection {
                    state: phoenix_core::domain::sm_state::ConvState::Idle,
                    state_updated_at: chrono::Utc::now(),
                },
                response_message_id: None,
            },
        )
        .await
        .unwrap();
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
                sleeps: None,
            }),
            ProcessIncarnation(process_incarnation),
        )
    }

    fn worker_with_recorded_sleeps(
        repo: WorkflowRepository,
        dispatcher: Arc<RecordingDispatcher>,
        sleeps: Arc<Mutex<Vec<Duration>>>,
    ) -> DirectTurnWorker<RecordingDispatcher, TestClock> {
        DirectTurnWorker::new(
            repo,
            dispatcher,
            Arc::new(TestClock {
                now: Timestamp(10),
                sleeps: Some(sleeps),
            }),
            ProcessIncarnation(1),
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
    async fn durable_terminal_obligation_is_discovered_and_retried_by_owned_worker() {
        let (repo, dispatcher) = fixture().await;
        let turn_id = accept(&repo, "terminal-obligation").await;
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let claim = repo
            .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
                turn_id,
                workflow_id,
                process_incarnation: ProcessIncarnation(1),
                now: Timestamp(10),
                lease_until: LeaseExpiry(40),
            })
            .await
            .unwrap();
        let authority = claim.authority.unwrap();
        let generation = authority.generation.0;
        repo.materialize_authoritative_turn(
            &phoenix_db::workflow::MaterializeAuthoritativeTurnInput {
                turn_id,
                authority,
                prepared: prepared_payload("message-terminal-obligation"),
                sequence_id: 99,
                created_at: Timestamp(99),
                accepted_state: phoenix_core::domain::db_schema::ConvState::LlmRequesting {
                    attempt: 1,
                },
                state_updated_at: chrono::DateTime::from_timestamp(99, 0).unwrap(),
                now: Timestamp(10),
            },
        )
        .await
        .unwrap();
        repo.persist_terminal_obligation(
            &phoenix_db::workflow::DirectTurnTerminalObligationInput {
                turn_id,
                expected_generation: generation,
                terminal: phoenix_workflow::TurnTerminal::Completed,
                projection: phoenix_db::workflow::PersistedConversationProjection {
                    state: phoenix_core::domain::sm_state::ConvState::Idle,
                    state_updated_at: chrono::Utc::now(),
                },
                response_message_id: None,
            },
        )
        .await
        .unwrap();
        *dispatcher.terminal_results.lock().unwrap() = vec![
            Err(crate::runtime::DatabaseTerminalRecoveryError::StillOwed(
                "transient".to_string(),
            )),
            Ok(TerminalObligationSettlement::Committed),
        ];

        assert!(worker(repo.clone(), dispatcher.clone(), 10, 1)
            .run_once()
            .await
            .is_err());
        worker(repo, dispatcher.clone(), 11, 1)
            .run_once()
            .await
            .unwrap();

        assert_eq!(
            dispatcher.terminal_attempts.lock().unwrap().as_slice(),
            &["conv-a".to_string(), "conv-a".to_string()]
        );
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
    async fn startup_discovery_read_failure_backs_off_before_ready_then_settles() {
        let (repo, dispatcher) = fixture().await;
        seed_terminal_obligation(&repo, "startup-discovery-read").await;
        let discovery = Arc::new(FailingTerminalDiscovery {
            repo: repo.clone(),
            failures_remaining: std::sync::atomic::AtomicUsize::new(1),
            attempts: std::sync::atomic::AtomicUsize::new(0),
            initial_empty: false,
        });
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let (retry_started_tx, retry_started_rx) = tokio::sync::oneshot::channel();
        let (retry_release_tx, retry_release_rx) = tokio::sync::oneshot::channel();
        let clock = Arc::new(GatedRetryClock {
            sleeps: sleeps.clone(),
            retry_started: Mutex::new(Some(retry_started_tx)),
            retry_release: Mutex::new(Some(retry_release_rx)),
        });
        let (kick_tx, kick_rx) = watch::channel(0);
        let (ready_tx, mut ready_rx) = tokio::sync::oneshot::channel();
        let (settled_tx, settled_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .terminal_settled
            .lock()
            .unwrap()
            .replace(settled_tx);
        let worker = DirectTurnWorker::new(repo, dispatcher.clone(), clock, ProcessIncarnation(1))
            .with_terminal_discovery(discovery.clone());
        let handle = tokio::spawn(async move { worker.run_loop(kick_rx, ready_tx).await });

        retry_started_rx.await.unwrap();
        assert!(ready_rx.try_recv().is_err());
        retry_release_tx.send(()).unwrap();
        settled_rx.await.unwrap();
        ready_rx.await.unwrap();
        assert_eq!(
            dispatcher.terminal_attempts.lock().unwrap().as_slice(),
            &["conv-a".to_string()]
        );
        assert!(dispatcher.events.lock().unwrap().is_empty());
        assert_eq!(
            sleeps.lock().unwrap().as_slice(),
            &[ERROR_RETRY_INTERVAL, EMPTY_RESCAN_INTERVAL]
        );
        assert_eq!(
            discovery.attempts.load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        drop(kick_tx);
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn steady_state_discovery_read_failure_backs_off_without_fatal_then_settles() {
        let (repo, dispatcher) = fixture().await;
        seed_terminal_obligation(&repo, "steady-discovery-read").await;
        let discovery = Arc::new(FailingTerminalDiscovery {
            repo: repo.clone(),
            failures_remaining: std::sync::atomic::AtomicUsize::new(1),
            attempts: std::sync::atomic::AtomicUsize::new(0),
            initial_empty: true,
        });
        let (retry_started_tx, retry_started_rx) = tokio::sync::oneshot::channel();
        let (retry_release_tx, retry_release_rx) = tokio::sync::oneshot::channel();
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let clock = Arc::new(GatedRetryClock {
            sleeps: sleeps.clone(),
            retry_started: Mutex::new(Some(retry_started_tx)),
            retry_release: Mutex::new(Some(retry_release_rx)),
        });
        let (kick_tx, kick_rx) = watch::channel(0);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let worker = DirectTurnWorker::new(repo, dispatcher.clone(), clock, ProcessIncarnation(1))
            .with_terminal_discovery(discovery.clone());
        let handle = tokio::spawn(async move { worker.run_loop(kick_rx, ready_tx).await });

        ready_rx.await.unwrap();
        kick_tx.send_replace(1);
        retry_started_rx.await.unwrap();
        assert!(dispatcher.terminal_attempts.lock().unwrap().is_empty());
        assert!(dispatcher.events.lock().unwrap().is_empty());
        let (settled_tx, settled_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .terminal_settled
            .lock()
            .unwrap()
            .replace(settled_tx);
        retry_release_tx.send(()).unwrap();
        settled_rx.await.unwrap();
        drop(kick_tx);
        handle.await.unwrap().unwrap();

        assert_eq!(
            dispatcher.terminal_attempts.lock().unwrap().as_slice(),
            &["conv-a".to_string()]
        );
        assert!(dispatcher.events.lock().unwrap().is_empty());
        assert_eq!(
            discovery.attempts.load(std::sync::atomic::Ordering::SeqCst),
            3
        );
        assert_eq!(
            sleeps.lock().unwrap().as_slice(),
            &[
                EMPTY_RESCAN_INTERVAL,
                ERROR_RETRY_INTERVAL,
                EMPTY_RESCAN_INTERVAL,
            ]
        );
    }

    #[tokio::test]
    async fn startup_still_owed_retries_before_signalling_ready() {
        let (repo, dispatcher) = fixture().await;
        let turn_id = accept(&repo, "startup-terminal-retry").await;
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let claim = repo
            .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
                turn_id,
                workflow_id,
                process_incarnation: ProcessIncarnation(1),
                now: Timestamp(10),
                lease_until: LeaseExpiry(40),
            })
            .await
            .unwrap();
        repo.materialize_authoritative_turn(
            &phoenix_db::workflow::MaterializeAuthoritativeTurnInput {
                turn_id,
                authority: claim.authority.unwrap(),
                prepared: prepared_payload("message-startup-terminal-retry"),
                sequence_id: 100,
                created_at: Timestamp(100),
                accepted_state: phoenix_core::domain::sm_state::ConvState::LlmRequesting {
                    attempt: 1,
                },
                state_updated_at: chrono::DateTime::from_timestamp(100, 0).unwrap(),
                now: Timestamp(10),
            },
        )
        .await
        .unwrap();
        repo.persist_terminal_obligation(
            &phoenix_db::workflow::DirectTurnTerminalObligationInput {
                turn_id,
                expected_generation: 0,
                terminal: phoenix_workflow::TurnTerminal::Completed,
                projection: phoenix_db::workflow::PersistedConversationProjection {
                    state: phoenix_core::domain::sm_state::ConvState::Idle,
                    state_updated_at: chrono::Utc::now(),
                },
                response_message_id: None,
            },
        )
        .await
        .unwrap();
        *dispatcher.terminal_results.lock().unwrap() = vec![
            Err(crate::runtime::DatabaseTerminalRecoveryError::StillOwed(
                "transient".to_string(),
            )),
            Ok(TerminalObligationSettlement::Committed),
        ];
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let (kick_tx, kick_rx) = watch::channel(0);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let worker = worker_with_recorded_sleeps(repo, dispatcher.clone(), sleeps.clone());
        let handle = tokio::spawn(async move { worker.run_loop(kick_rx, ready_tx).await });

        ready_rx.await.unwrap();
        assert_eq!(
            dispatcher.terminal_attempts.lock().unwrap().as_slice(),
            &["conv-a".to_string(), "conv-a".to_string()]
        );
        assert_eq!(
            sleeps.lock().unwrap().as_slice(),
            &[ERROR_RETRY_INTERVAL, EMPTY_RESCAN_INTERVAL]
        );
        drop(kick_tx);
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn startup_unclassifiable_authority_does_not_retry_or_signal_ready() {
        let (repo, dispatcher) = fixture().await;
        let turn_id = accept(&repo, "startup-unclassifiable").await;
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let claim = repo
            .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
                turn_id,
                workflow_id,
                process_incarnation: ProcessIncarnation(1),
                now: Timestamp(10),
                lease_until: LeaseExpiry(40),
            })
            .await
            .unwrap();
        repo.materialize_authoritative_turn(
            &phoenix_db::workflow::MaterializeAuthoritativeTurnInput {
                turn_id,
                authority: claim.authority.unwrap(),
                prepared: prepared_payload("message-startup-unclassifiable"),
                sequence_id: 101,
                created_at: Timestamp(101),
                accepted_state: phoenix_core::domain::sm_state::ConvState::LlmRequesting {
                    attempt: 1,
                },
                state_updated_at: chrono::DateTime::from_timestamp(101, 0).unwrap(),
                now: Timestamp(10),
            },
        )
        .await
        .unwrap();
        repo.persist_terminal_obligation(
            &phoenix_db::workflow::DirectTurnTerminalObligationInput {
                turn_id,
                expected_generation: 0,
                terminal: phoenix_workflow::TurnTerminal::Completed,
                projection: phoenix_db::workflow::PersistedConversationProjection {
                    state: phoenix_core::domain::sm_state::ConvState::Idle,
                    state_updated_at: chrono::Utc::now(),
                },
                response_message_id: None,
            },
        )
        .await
        .unwrap();
        *dispatcher.terminal_results.lock().unwrap() = vec![Err(
            crate::runtime::DatabaseTerminalRecoveryError::Unclassifiable(
                "probe failed".to_string(),
            ),
        )];
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let (_kick_tx, kick_rx) = watch::channel(0);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let error = worker_with_recorded_sleeps(repo, dispatcher.clone(), sleeps.clone())
            .run_loop(kick_rx, ready_tx)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            StartupReconciliationError::Unclassifiable(_)
        ));
        assert!(ready_rx.await.is_err());
        assert_eq!(dispatcher.terminal_attempts.lock().unwrap().len(), 1);
        assert!(sleeps.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn closed_database_discovery_failure_is_retryable() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        pool.close().await;
        let repo = WorkflowRepository::new(pool);
        let dispatcher = Arc::new(RecordingDispatcher::default());

        assert!(matches!(
            worker(repo, dispatcher, 10, 1).run_once().await,
            Err(crate::runtime::DatabaseTerminalRecoveryError::Retryable(_))
        ));
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
