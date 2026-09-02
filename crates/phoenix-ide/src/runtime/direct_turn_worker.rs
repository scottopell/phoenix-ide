use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use phoenix_core::domain::sm_event::{DirectTurnAttemptAuthority, PreparedDirectTurnPayload};
use phoenix_db::workflow::{
    ClaimAuthoritativeTurnEstablishment, ClaimAuthoritativeTurnInput,
    DirectTurnMaterializationEligibility, DiscoverableAcceptedTurn,
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
        manager.db().workflow_repository(),
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

    async fn list_accepted(
        &self,
        cursor: Option<phoenix_db::workflow::DirectTurnDiscoveryCursor>,
        limit: usize,
    ) -> Result<phoenix_db::workflow::DiscoverableAcceptedTurnPage, String>;
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

    async fn list_accepted(
        &self,
        cursor: Option<phoenix_db::workflow::DirectTurnDiscoveryCursor>,
        limit: usize,
    ) -> Result<phoenix_db::workflow::DiscoverableAcceptedTurnPage, String> {
        self.list_discoverable_accepted_runtime_direct_turns(cursor, limit)
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
        loop {
            match self.run_startup_recovery_once().await {
                Ok(()) => break,
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
        }
        let _ = ready_tx.send(());

        let mut wait = match self.dispatch_accepted_turns().await {
            Ok(wait) => wait,
            Err(error) => match StartupReconciliationError::from(error) {
                StartupReconciliationError::Retryable(error) => {
                    tracing::warn!(%error, "direct-turn worker pass remains owed; retrying");
                    ERROR_RETRY_INTERVAL
                }
                fatal @ StartupReconciliationError::Unclassifiable(_) => return Err(fatal),
            },
        };
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

    async fn run_startup_recovery_once(
        &self,
    ) -> Result<(), crate::runtime::DatabaseTerminalRecoveryError> {
        self.settle_terminal_obligations().await?;
        self.dispatcher.reconcile_startup_parents().await
    }

    async fn settle_terminal_obligations(
        &self,
    ) -> Result<(), crate::runtime::DatabaseTerminalRecoveryError> {
        let mut settled_obligations = std::collections::HashSet::new();
        loop {
            let obligations = self
                .terminal_discovery
                .list(DISCOVERY_BATCH_LIMIT)
                .await
                .map_err(crate::runtime::DatabaseTerminalRecoveryError::Retryable)?;
            if obligations.is_empty() {
                break;
            }
            let mut made_progress = false;
            for obligation in obligations {
                if settled_obligations.insert(obligation.turn_id) {
                    let Ok(_owner) = self.dispatcher.acquire_local_authority() else {
                        return Ok(());
                    };
                    made_progress = true;
                    self.dispatcher
                        .settle_terminal_obligation(&obligation.conversation.0)
                        .await?;
                }
            }
            if !made_progress {
                break;
            }
        }
        Ok(())
    }

    pub(crate) async fn run_once(
        &self,
    ) -> Result<Duration, crate::runtime::DatabaseTerminalRecoveryError> {
        self.settle_terminal_obligations().await?;
        self.dispatch_accepted_turns().await
    }

    async fn dispatch_accepted_turns(
        &self,
    ) -> Result<Duration, crate::runtime::DatabaseTerminalRecoveryError> {
        let mut cursor = None;
        loop {
            let Ok(discovery_owner) = self.dispatcher.acquire_local_authority() else {
                return Ok(EMPTY_RESCAN_INTERVAL);
            };
            let page = self
                .terminal_discovery
                .list_accepted(cursor, DISCOVERY_BATCH_LIMIT)
                .await
                .map_err(|error| {
                    crate::runtime::DatabaseTerminalRecoveryError::Retryable(error.clone())
                })?;
            drop(discovery_owner);
            let exhausted = page.next_cursor.is_none() || page.next_cursor == cursor;
            cursor = page.next_cursor;
            for candidate in page.candidates {
                let Ok(_owner) = self.dispatcher.acquire_local_authority() else {
                    return Ok(EMPTY_RESCAN_INTERVAL);
                };
                self.dispatch_candidate(candidate, self.clock.now()).await?;
            }
            if exhausted {
                break;
            }
        }
        Ok(EMPTY_RESCAN_INTERVAL)
    }

    #[allow(clippy::too_many_lines)]
    async fn dispatch_candidate(
        &self,
        candidate: DiscoverableAcceptedTurn,
        now: Timestamp,
    ) -> Result<(), crate::runtime::DatabaseTerminalRecoveryError> {
        let lease_until = LeaseExpiry(now.0.saturating_add(LEASE_DURATION.as_secs()));
        let claim_input = ClaimAuthoritativeTurnInput {
            turn_id: candidate.turn_id,
            workflow_id: candidate.workflow_id,
            process_incarnation: self.process_incarnation,
            now,
            lease_until,
        };
        let claim = match self
            .repo
            .establish_authoritative_turn_claim(&claim_input)
            .await
        {
            ClaimAuthoritativeTurnEstablishment::Established(claim) => *claim,
            ClaimAuthoritativeTurnEstablishment::KnownNotCommitted(error) => {
                return Err(crate::runtime::DatabaseTerminalRecoveryError::Retryable(
                    error,
                ));
            }
            ClaimAuthoritativeTurnEstablishment::Unclassifiable(error) => {
                return Err(crate::runtime::DatabaseTerminalRecoveryError::Unclassifiable(error));
            }
        };
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
                    let turn = self
                        .repo
                        .load_authoritative_turn(candidate.turn_id)
                        .await
                        .map_err(|probe_error| {
                            crate::runtime::DatabaseTerminalRecoveryError::Unclassifiable(format!(
                                "corrupt payload quarantine failed ({terminal_error}); exact probe failed ({probe_error})"
                            ))
                        })?;
                    match turn.map(|turn| (turn.generation, turn.lifecycle)) {
                        Some((
                            generation,
                            phoenix_workflow::TurnLifecycle::Terminal {
                                terminal: phoenix_workflow::TurnTerminal::Failed { ref reason },
                                ..
                            },
                        )) if generation == authority.generation.0.saturating_add(1)
                            && reason == &format!("prepared payload decode failed: {error}") => {}
                        Some((
                            generation,
                            phoenix_workflow::TurnLifecycle::Accepted {
                                disposition: phoenix_workflow::AcceptedDisposition::Runtime,
                            },
                        )) if generation == authority.generation.0 => {
                            return Err(crate::runtime::DatabaseTerminalRecoveryError::Retryable(
                                terminal_error.to_string(),
                            ));
                        }
                        _ => {
                            return Err(
                                crate::runtime::DatabaseTerminalRecoveryError::Unclassifiable(
                                    format!(
                                        "corrupt payload quarantine failed ({terminal_error}); exact terminal state is unclassifiable"
                                    ),
                                ),
                            );
                        }
                    }
                }
                tracing::warn!(turn_id = candidate.turn_id.0, error = %error, "direct-turn payload decode failed; terminally quarantined turn");
                return Ok(());
            }
        };
        #[cfg(test)]
        if let Some(hook) = &self.pre_dispatch_hook {
            hook().await;
        }
        let eligibility = self
            .preflight_candidate(&candidate, &authority, &prepared, now)
            .await?;
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
            self.release(authority, now)
                .await
                .map_err(|release_error| {
                    crate::runtime::DatabaseTerminalRecoveryError::Unclassifiable(format!(
                    "direct-turn dispatch failed: {error}; claim release failed: {release_error}"
                ))
                })?;
            tracing::warn!(conversation_id = %candidate.conversation.0, turn_id = candidate.turn_id.0, error = %error, "direct-turn dispatch failed; released claim");
        }
        Ok(())
    }

    async fn preflight_candidate(
        &self,
        candidate: &DiscoverableAcceptedTurn,
        authority: &LocalAttemptAuthority,
        prepared: &PreparedDirectTurnPayload,
        now: Timestamp,
    ) -> Result<DirectTurnMaterializationEligibility, crate::runtime::DatabaseTerminalRecoveryError>
    {
        let result = self
            .repo
            .preflight_direct_turn_materialization(&PreflightDirectTurnMaterializationInput {
                turn_id: candidate.turn_id,
                authority: authority.clone(),
                prepared: prepared.clone(),
                now,
            })
            .await;
        match result {
            Ok(eligibility) => Ok(eligibility),
            Err(error) => {
                if let Err(release_error) = self.release(authority.clone(), now).await {
                    return Err(
                        crate::runtime::DatabaseTerminalRecoveryError::Unclassifiable(format!(
                            "direct-turn preflight failed: {error}; claim release failed: {release_error}"
                        )),
                    );
                }
                Err(crate::runtime::DatabaseTerminalRecoveryError::Retryable(
                    error.to_string(),
                ))
            }
        }
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
    fn acquire_local_authority(&self) -> Result<Box<dyn Send>, ()> {
        Ok(Box::new(()))
    }

    async fn reconcile_startup_parents(
        &self,
    ) -> Result<(), crate::runtime::DatabaseTerminalRecoveryError> {
        Ok(())
    }
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
    fn acquire_local_authority(&self) -> Result<Box<dyn Send>, ()> {
        self.manager
            .acquire_local_authority_pass()
            .map(|owner| Box::new(owner) as Box<dyn Send>)
    }

    fn signal_fatal_local_authority(&self) {
        self.manager
            .signal_fatal_local_authority("direct_turn_terminal_recovery");
    }

    async fn reconcile_startup_parents(
        &self,
    ) -> Result<(), crate::runtime::DatabaseTerminalRecoveryError> {
        self.manager.reconcile_startup_obligated_parents().await
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
        if let Err(error) = self.manager.resume_pending_close_settlements().await {
            tracing::error!(%error, %conversation_id, "failed to re-evaluate Close settlement after direct-turn terminal recovery");
        }
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
        accepted_failures_remaining: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl TerminalObligationDiscovery for FailingTerminalDiscovery {
        async fn list_accepted(
            &self,
            cursor: Option<phoenix_db::workflow::DirectTurnDiscoveryCursor>,
            limit: usize,
        ) -> Result<phoenix_db::workflow::DiscoverableAcceptedTurnPage, String> {
            if self
                .accepted_failures_remaining
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                return Err("injected accepted-turn discovery read failure".to_string());
            }
            self.repo
                .list_discoverable_accepted_runtime_direct_turns(cursor, limit)
                .await
                .map_err(|error| error.to_string())
        }

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

    struct FailAfterFirstAcceptedPage {
        repo: WorkflowRepository,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl TerminalObligationDiscovery for FailAfterFirstAcceptedPage {
        async fn list_accepted(
            &self,
            cursor: Option<phoenix_db::workflow::DirectTurnDiscoveryCursor>,
            _limit: usize,
        ) -> Result<phoenix_db::workflow::DiscoverableAcceptedTurnPage, String> {
            match self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0 => self
                    .repo
                    .list_discoverable_accepted_runtime_direct_turns(cursor, 1)
                    .await
                    .map_err(|error| error.to_string()),
                1 => Err("injected later candidate discovery failure".to_string()),
                _ => self
                    .repo
                    .list_discoverable_accepted_runtime_direct_turns(cursor, 64)
                    .await
                    .map_err(|error| error.to_string()),
            }
        }

        async fn list(
            &self,
            limit: usize,
        ) -> Result<Vec<phoenix_db::workflow::DiscoverableTerminalObligation>, String> {
            self.repo
                .list_discoverable_terminal_obligations(limit)
                .await
                .map_err(|error| error.to_string())
        }
    }

    struct RealStartupOrderingDispatcher {
        manager: Arc<RuntimeManager>,
        fail_settlement_once: std::sync::atomic::AtomicBool,
        settlement_failed: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        settlement_release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        reconciliation_calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl DirectTurnDispatcher for RealStartupOrderingDispatcher {
        async fn dispatch(&self, _conversation_id: &str, _event: Event) -> Result<(), String> {
            Err("ordinary dispatch is outside the startup-ordering fixture".to_string())
        }
    }

    #[async_trait]
    impl TerminalObligationDispatcher for RealStartupOrderingDispatcher {
        fn acquire_local_authority(&self) -> Result<Box<dyn Send>, ()> {
            self.manager
                .acquire_local_authority_pass()
                .map(|owner| Box::new(owner) as Box<dyn Send>)
        }

        async fn reconcile_startup_parents(
            &self,
        ) -> Result<(), crate::runtime::DatabaseTerminalRecoveryError> {
            self.reconciliation_calls
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            self.manager.reconcile_startup_obligated_parents().await
        }

        async fn settle_terminal_obligation(
            &self,
            conversation_id: &str,
        ) -> Result<TerminalObligationSettlement, crate::runtime::DatabaseTerminalRecoveryError>
        {
            if self
                .fail_settlement_once
                .swap(false, std::sync::atomic::Ordering::AcqRel)
            {
                if let Some(failed) = self.settlement_failed.lock().unwrap().take() {
                    let _ = failed.send(());
                }
                let release = self.settlement_release.lock().unwrap().take();
                if let Some(release) = release {
                    let _ = release.await;
                }
                return Err(crate::runtime::DatabaseTerminalRecoveryError::StillOwed(
                    "controlled settlement failure".to_string(),
                ));
            }
            let recovery = self
                .manager
                .settle_database_terminal_obligation(conversation_id)
                .await?;
            Ok(if recovery.committed() {
                TerminalObligationSettlement::Committed
            } else {
                TerminalObligationSettlement::NoObligation
            })
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
        event_dispatched: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        admission_open: std::sync::atomic::AtomicBool,
        admission_attempts: std::sync::atomic::AtomicUsize,
        close_after_terminal_settlement: std::sync::atomic::AtomicBool,
        close_after_dispatch: std::sync::atomic::AtomicBool,
        startup_reconciliations: std::sync::atomic::AtomicUsize,
        startup_reconciliations_at_dispatch: Mutex<Vec<usize>>,
    }

    impl Default for RecordingDispatcher {
        fn default() -> Self {
            Self {
                result: Mutex::new(Ok(())),
                terminal_results: Mutex::new(vec![Ok(TerminalObligationSettlement::Committed)]),
                events: Mutex::new(Vec::new()),
                terminal_attempts: Mutex::new(Vec::new()),
                terminal_settled: Mutex::new(None),
                event_dispatched: Mutex::new(None),
                admission_open: std::sync::atomic::AtomicBool::new(true),
                admission_attempts: std::sync::atomic::AtomicUsize::new(0),
                close_after_terminal_settlement: std::sync::atomic::AtomicBool::new(false),
                close_after_dispatch: std::sync::atomic::AtomicBool::new(false),
                startup_reconciliations: std::sync::atomic::AtomicUsize::new(0),
                startup_reconciliations_at_dispatch: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl TerminalObligationDispatcher for RecordingDispatcher {
        fn acquire_local_authority(&self) -> Result<Box<dyn Send>, ()> {
            self.admission_attempts
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            self.admission_open
                .load(std::sync::atomic::Ordering::Acquire)
                .then(|| Box::new(()) as Box<dyn Send>)
                .ok_or(())
        }

        async fn reconcile_startup_parents(
            &self,
        ) -> Result<(), crate::runtime::DatabaseTerminalRecoveryError> {
            self.startup_reconciliations
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            Ok(())
        }

        async fn settle_terminal_obligation(
            &self,
            conversation_id: &str,
        ) -> Result<TerminalObligationSettlement, crate::runtime::DatabaseTerminalRecoveryError>
        {
            self.terminal_attempts
                .lock()
                .unwrap()
                .push(conversation_id.to_string());
            if self
                .close_after_terminal_settlement
                .load(std::sync::atomic::Ordering::Acquire)
            {
                self.admission_open
                    .store(false, std::sync::atomic::Ordering::Release);
            }
            let mut results = self.terminal_results.lock().unwrap();
            let result = if results.len() == 1 {
                results[0].clone()
            } else {
                results.remove(0)
            };
            if result.is_ok() {
                if let Some(settled) = self.terminal_settled.lock().unwrap().take() {
                    let _ = settled.send(());
                }
            }
            result
        }
    }

    #[async_trait]
    impl DirectTurnDispatcher for RecordingDispatcher {
        async fn dispatch(&self, conversation_id: &str, event: Event) -> Result<(), String> {
            self.startup_reconciliations_at_dispatch
                .lock()
                .unwrap()
                .push(
                    self.startup_reconciliations
                        .load(std::sync::atomic::Ordering::Acquire),
                );
            self.events
                .lock()
                .unwrap()
                .push((conversation_id.to_string(), event));
            if let Some(dispatched) = self.event_dispatched.lock().unwrap().take() {
                let _ = dispatched.send(());
            }
            if self
                .close_after_dispatch
                .load(std::sync::atomic::Ordering::Acquire)
            {
                self.admission_open
                    .store(false, std::sync::atomic::Ordering::Release);
            }
            self.result.lock().unwrap().clone()
        }
    }

    async fn fixture() -> (WorkflowRepository, Arc<RecordingDispatcher>) {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("conv-a", "A", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation("conv-b", "B", "/tmp", true, None, None)
            .await
            .unwrap();
        let repo = db.workflow_repository();
        (repo, Arc::new(RecordingDispatcher::default()))
    }

    #[tokio::test]
    async fn startup_recovery_dispatches_accepted_subordinate_direct_turn() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("recovery-parent", "Parent", "/tmp", true, None, None)
            .await
            .unwrap();
        let parent = db.get_conversation("recovery-parent").await.unwrap();
        db.create_subagent_conversation(
            "recovery-subordinate",
            "Recovery subordinate",
            "/tmp",
            "recovery-parent",
            "test-model",
            &phoenix_db::ConvMode::Direct,
            phoenix_core::llm_language::LlmLanguage::default(),
            parent.attached_work_scope_id.as_ref(),
        )
        .await
        .unwrap();
        let repo = db.workflow_repository();
        let turn_id =
            accept_for_conversation(&repo, "accepted-before-restart", "recovery-subordinate").await;
        let dispatcher = Arc::new(RecordingDispatcher::default());

        worker(repo.clone(), dispatcher.clone(), 10, 2)
            .run_once()
            .await
            .unwrap();

        assert_eq!(
            dispatcher
                .events
                .lock()
                .unwrap()
                .iter()
                .map(|(conversation_id, _)| conversation_id.as_str())
                .collect::<Vec<_>>(),
            vec!["recovery-subordinate"],
        );
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let attempts = repo.list_attempts(workflow_id, EffectId(1)).await.unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].status, phoenix_workflow::AttemptStatus::Begun);
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

    fn prepared_turn_for(conversation_id: &str, message_id: &str) -> PreparedTurn {
        PreparedTurn::from_exact_payload(
            &ConversationAuthority(conversation_id.to_string()),
            prepared_payload(message_id).to_exact_bytes().unwrap(),
        )
    }

    fn prepared_turn(message_id: &str) -> PreparedTurn {
        prepared_turn_for("conv-a", message_id)
    }

    async fn accept_for_conversation(
        repo: &WorkflowRepository,
        key: &str,
        conversation_id: &str,
    ) -> phoenix_workflow::TurnAuthorityId {
        let step = repo
            .accept_authoritative_turn(&phoenix_db::workflow::AcceptAuthoritativeTurn {
                client_key: ClientTurnKey::new(key).unwrap(),
                prepared: prepared_turn_for(conversation_id, &format!("message-{key}")),
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

    async fn accept(repo: &WorkflowRepository, key: &str) -> phoenix_workflow::TurnAuthorityId {
        accept_for_conversation(repo, key, "conv-a").await
    }

    async fn seed_terminal_obligation_for(
        repo: &WorkflowRepository,
        key: &str,
        conversation_id: &str,
    ) {
        let turn_id = accept_for_conversation(repo, key, conversation_id).await;
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
        .established()
        .expect("classified direct-turn materialization");
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

    async fn seed_terminal_obligation(repo: &WorkflowRepository, key: &str) {
        seed_terminal_obligation_for(repo, key, "conv-a").await;
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
    async fn live_parent_reconciliation_preserves_in_flight_tool_authority() {
        use phoenix_core::domain::db_schema::{MessageContent, ToolContent};
        use phoenix_core::domain::llm_types::ContentBlock;
        use phoenix_core::domain::sm_state::{
            AssistantMessage, ConvState, ThinkInput, ToolCall, ToolInput,
        };

        let db = Database::open_in_memory().await.unwrap();
        let parent_id = "live-worker-parent";
        db.create_conversation(parent_id, parent_id, "/tmp", true, None, None)
            .await
            .unwrap();
        let in_flight = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                "live-tool",
                ToolInput::Think(ThinkInput {
                    thoughts: "authoritative side effect".to_string(),
                }),
            ),
            remaining_tools: Vec::new(),
            completed_results: Vec::new(),
            pending_sub_agents: Vec::new(),
            assistant_message: AssistantMessage::new(
                "live-assistant".to_string(),
                vec![ContentBlock::tool_use(
                    "live-tool",
                    "think",
                    serde_json::json!({"thoughts": "authoritative side effect"}),
                )],
                None,
                None,
            ),
        };
        db.update_conversation_state(parent_id, &in_flight)
            .await
            .unwrap();
        db.establish_parent_reconcile_action(parent_id)
            .await
            .unwrap();
        let repo = db.workflow_repository();
        let dispatcher = Arc::new(RecordingDispatcher::default());
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let real_result = {
            let db = db.clone();
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                let result = MessageContent::Tool(ToolContent::new(
                    "live-tool",
                    "real side effect completed",
                    false,
                ));
                db.add_message("live-real-result", parent_id, &result, None, None)
                    .await
                    .unwrap();
                db.update_conversation_state(parent_id, &ConvState::LlmRequesting { attempt: 1 })
                    .await
                    .unwrap();
            })
        };
        let actions_before = db.list_startup_parent_actions().await.unwrap();

        worker(repo, Arc::clone(&dispatcher), 10, 1)
            .run_once()
            .await
            .unwrap();

        assert_eq!(
            dispatcher
                .startup_reconciliations
                .load(std::sync::atomic::Ordering::Acquire),
            0,
            "a normal worker pass cannot enter startup reconciliation"
        );
        assert_eq!(
            db.get_conversation(parent_id).await.unwrap().state,
            in_flight
        );
        assert!(db.get_messages(parent_id).await.unwrap().is_empty());
        assert_eq!(
            db.list_startup_parent_actions().await.unwrap(),
            actions_before,
            "live pass cannot consume recovery actions or auto-continue"
        );

        barrier.wait().await;
        real_result.await.unwrap();

        let messages = db.get_messages(parent_id).await.unwrap();
        assert_eq!(messages.len(), 1, "real result commits exactly once");
        let MessageContent::Tool(result) = &messages[0].content else {
            panic!("expected real tool result")
        };
        assert_eq!(result.content, "real side effect completed");
        assert!(!result.is_error);
        assert_eq!(
            db.get_conversation(parent_id).await.unwrap().state,
            ConvState::LlmRequesting { attempt: 1 }
        );
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
    async fn closure_between_terminal_units_prevents_next_settlement_and_claim() {
        let (repo, dispatcher) = fixture().await;
        seed_terminal_obligation_for(&repo, "fatal-first", "conv-a").await;
        seed_terminal_obligation_for(&repo, "fatal-second", "conv-b").await;
        dispatcher
            .close_after_terminal_settlement
            .store(true, std::sync::atomic::Ordering::Release);

        worker(repo, dispatcher.clone(), 10, 1)
            .run_once()
            .await
            .unwrap();

        assert_eq!(dispatcher.terminal_attempts.lock().unwrap().len(), 1);
        assert_eq!(
            dispatcher
                .admission_attempts
                .load(std::sync::atomic::Ordering::Acquire),
            3
        );
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
        .established()
        .expect("classified direct-turn materialization");
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
    async fn closure_between_candidates_prevents_next_claim() {
        let (repo, dispatcher) = fixture().await;
        let _first_turn = accept_for_conversation(&repo, "fatal-candidate-first", "conv-a").await;
        let _second_turn = accept_for_conversation(&repo, "fatal-candidate-second", "conv-b").await;
        dispatcher
            .close_after_dispatch
            .store(true, std::sync::atomic::Ordering::Release);

        worker(repo.clone(), dispatcher.clone(), 10, 1)
            .run_once()
            .await
            .unwrap();

        assert_eq!(dispatcher.events.lock().unwrap().len(), 1);
        let attempts = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workflow_attempts")
            .fetch_one(repo.pool())
            .await
            .unwrap();
        assert_eq!(attempts, 1);
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
                    .established()
                    .expect("classified direct-turn materialization");
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
        dispatcher
            .admission_open
            .store(false, std::sync::atomic::Ordering::Release);

        worker.run_once().await.unwrap();

        assert!(dispatcher.events.lock().unwrap().is_empty());
        assert!(repo
            .list_attempts(workflow_id, phoenix_workflow::EffectId(1))
            .await
            .unwrap()
            .is_empty());
        dispatcher
            .admission_open
            .store(true, std::sync::atomic::Ordering::Release);
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
    async fn prewrite_claim_read_failure_retries_without_dispatch_or_duplicate_attempt() {
        let (repo, dispatcher) = fixture().await;
        let turn_id = accept(&repo, "claim-read-retry").await;
        let page = repo
            .list_discoverable_accepted_runtime_direct_turns(None, 10)
            .await
            .unwrap();
        let candidate = page
            .candidates
            .into_iter()
            .find(|candidate| candidate.turn_id == turn_id)
            .unwrap();
        let mut invalid = candidate.clone();
        invalid.workflow_id = phoenix_workflow::WorkflowId(u64::MAX);
        let worker = DirectTurnWorker::new(
            repo.clone(),
            dispatcher.clone(),
            Arc::new(TestClock {
                now: Timestamp(1),
                sleeps: None,
            }),
            ProcessIncarnation(1),
        );

        assert!(matches!(
            worker.dispatch_candidate(invalid, Timestamp(1)).await,
            Err(crate::runtime::DatabaseTerminalRecoveryError::Retryable(_))
        ));
        assert!(dispatcher.events.lock().unwrap().is_empty());
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        assert!(repo
            .list_attempts(workflow_id, EffectId(1))
            .await
            .unwrap()
            .is_empty());

        worker
            .dispatch_candidate(candidate, Timestamp(1))
            .await
            .unwrap();
        assert_eq!(dispatcher.events.lock().unwrap().len(), 1);
        assert_eq!(
            repo.list_attempts(workflow_id, EffectId(1))
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn startup_released_preflight_failure_backs_off_then_dispatches() {
        let (repo, dispatcher) = fixture().await;
        let turn_id = accept(&repo, "startup-preflight-read").await;
        let hook_repo = repo.clone();
        let first_preflight = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let hook_first_preflight = first_preflight.clone();
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let (retry_started_tx, retry_started_rx) = tokio::sync::oneshot::channel();
        let (retry_release_tx, retry_release_rx) = tokio::sync::oneshot::channel();
        let clock = Arc::new(GatedRetryClock {
            sleeps: sleeps.clone(),
            retry_started: Mutex::new(Some(retry_started_tx)),
            retry_release: Mutex::new(Some(retry_release_rx)),
        });
        let worker = DirectTurnWorker::new(
            repo.clone(),
            dispatcher.clone(),
            clock,
            ProcessIncarnation(1),
        )
        .with_pre_dispatch_hook(Arc::new(move || {
            let repo = hook_repo.clone();
            let first_preflight = hook_first_preflight.clone();
            Box::pin(async move {
                if first_preflight.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    let replacement = prepared_payload("message-startup-preflight-retry");
                    sqlx::query(
                        "UPDATE durable_turns
                             SET prepared_payload = ?1, prepared_fingerprint = ?2
                             WHERE turn_id = ?3",
                    )
                    .bind(replacement.to_exact_bytes().unwrap())
                    .bind(
                        PreparedTurn::from_exact_payload(
                            &ConversationAuthority("conv-a".to_string()),
                            replacement.to_exact_bytes().unwrap(),
                        )
                        .fingerprint(),
                    )
                    .bind(i64::try_from(turn_id.0).unwrap())
                    .execute(repo.pool())
                    .await
                    .unwrap();
                }
            })
        }));
        let (kick_tx, kick_rx) = watch::channel(0);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (dispatched_tx, dispatched_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .event_dispatched
            .lock()
            .unwrap()
            .replace(dispatched_tx);
        let handle = tokio::spawn(async move { worker.run_loop(kick_rx, ready_tx).await });

        ready_rx.await.unwrap();
        retry_started_rx.await.unwrap();
        assert!(dispatcher.events.lock().unwrap().is_empty());
        let workflow_id = repo.workflow_id_for_turn(turn_id).await.unwrap().unwrap();
        let attempts = repo.list_attempts(workflow_id, EffectId(1)).await.unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(
            attempts[0].status,
            phoenix_workflow::AttemptStatus::AuthorityLost
        );

        retry_release_tx.send(()).unwrap();
        dispatched_rx.await.unwrap();
        assert_eq!(dispatcher.events.lock().unwrap().len(), 1);
        assert_eq!(sleeps.lock().unwrap().first(), Some(&ERROR_RETRY_INTERVAL));
        drop(kick_tx);
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn steady_retry_after_first_dispatch_cannot_reenter_startup_reconciliation() {
        let (repo, dispatcher) = fixture().await;
        accept(&repo, "first-dispatch-before-retry").await;
        accept_for_conversation(&repo, "later-candidate", "conv-b").await;
        let discovery = Arc::new(FailAfterFirstAcceptedPage {
            repo: repo.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let (retry_started_tx, retry_started_rx) = tokio::sync::oneshot::channel();
        let (retry_release_tx, retry_release_rx) = tokio::sync::oneshot::channel();
        let clock = Arc::new(GatedRetryClock {
            sleeps: Arc::clone(&sleeps),
            retry_started: Mutex::new(Some(retry_started_tx)),
            retry_release: Mutex::new(Some(retry_release_rx)),
        });
        let (kick_tx, kick_rx) = watch::channel(0);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (dispatched_tx, dispatched_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .event_dispatched
            .lock()
            .unwrap()
            .replace(dispatched_tx);
        let worker =
            DirectTurnWorker::new(repo, Arc::clone(&dispatcher), clock, ProcessIncarnation(1))
                .with_terminal_discovery(discovery);
        let handle = tokio::spawn(async move { worker.run_loop(kick_rx, ready_tx).await });

        ready_rx.await.unwrap();
        dispatched_rx.await.unwrap();
        retry_started_rx.await.unwrap();
        assert_eq!(
            dispatcher
                .startup_reconciliations
                .load(std::sync::atomic::Ordering::Acquire),
            1,
            "startup phase is sealed before ordinary dispatch begins"
        );
        assert_eq!(dispatcher.events.lock().unwrap().len(), 1);

        assert_eq!(
            dispatcher
                .startup_reconciliations_at_dispatch
                .lock()
                .unwrap()
                .as_slice(),
            &[1],
            "ordinary dispatch observes the completed startup phase"
        );

        retry_release_tx.send(()).unwrap();
        drop(kick_tx);
        handle.await.unwrap().unwrap();
        assert_eq!(
            dispatcher
                .startup_reconciliations
                .load(std::sync::atomic::Ordering::Acquire),
            1,
            "ordinary retry cannot regain startup authority"
        );
        let first_message_count = dispatcher
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, event)| {
                matches!(
                    event,
                    Event::AuthoritativeUserMessage { payload, .. }
                        if payload.message_id() == "message-first-dispatch-before-retry"
                )
            })
            .count();
        assert_eq!(
            first_message_count, 1,
            "the first authoritative dispatch remains exactly once across retry"
        );
    }

    #[tokio::test]
    async fn startup_accepted_discovery_read_failure_backs_off_before_ready_then_dispatches() {
        let (repo, dispatcher) = fixture().await;
        accept(&repo, "startup-accepted-read").await;
        let discovery = Arc::new(FailingTerminalDiscovery {
            repo: repo.clone(),
            failures_remaining: std::sync::atomic::AtomicUsize::new(0),
            attempts: std::sync::atomic::AtomicUsize::new(0),
            initial_empty: false,
            accepted_failures_remaining: std::sync::atomic::AtomicUsize::new(1),
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
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (dispatched_tx, dispatched_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .event_dispatched
            .lock()
            .unwrap()
            .replace(dispatched_tx);
        let worker = DirectTurnWorker::new(repo, dispatcher.clone(), clock, ProcessIncarnation(1))
            .with_terminal_discovery(discovery);
        let handle = tokio::spawn(async move { worker.run_loop(kick_rx, ready_tx).await });

        ready_rx.await.unwrap();
        retry_started_rx.await.unwrap();
        assert!(dispatcher.events.lock().unwrap().is_empty());
        retry_release_tx.send(()).unwrap();
        dispatched_rx.await.unwrap();
        assert_eq!(dispatcher.events.lock().unwrap().len(), 1);
        assert_eq!(sleeps.lock().unwrap().first(), Some(&ERROR_RETRY_INTERVAL));
        drop(kick_tx);
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn steady_accepted_discovery_read_failure_backs_off_then_dispatches() {
        let (repo, dispatcher) = fixture().await;
        let discovery = Arc::new(FailingTerminalDiscovery {
            repo: repo.clone(),
            failures_remaining: std::sync::atomic::AtomicUsize::new(0),
            attempts: std::sync::atomic::AtomicUsize::new(0),
            initial_empty: false,
            accepted_failures_remaining: std::sync::atomic::AtomicUsize::new(0),
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
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let worker = DirectTurnWorker::new(
            repo.clone(),
            dispatcher.clone(),
            clock,
            ProcessIncarnation(1),
        )
        .with_terminal_discovery(discovery.clone());
        let handle = tokio::spawn(async move { worker.run_loop(kick_rx, ready_tx).await });

        ready_rx.await.unwrap();
        accept(&repo, "steady-accepted-read").await;
        discovery
            .accepted_failures_remaining
            .store(1, std::sync::atomic::Ordering::SeqCst);
        kick_tx.send_replace(1);
        retry_started_rx.await.unwrap();
        assert!(dispatcher.events.lock().unwrap().is_empty());
        let (dispatched_tx, dispatched_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .event_dispatched
            .lock()
            .unwrap()
            .replace(dispatched_tx);
        retry_release_tx.send(()).unwrap();
        dispatched_rx.await.unwrap();
        drop(kick_tx);
        handle.await.unwrap().unwrap();
        assert_eq!(dispatcher.events.lock().unwrap().len(), 1);
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
    async fn startup_discovery_read_failure_backs_off_before_ready_then_settles() {
        let (repo, dispatcher) = fixture().await;
        seed_terminal_obligation(&repo, "startup-discovery-read").await;
        let discovery = Arc::new(FailingTerminalDiscovery {
            repo: repo.clone(),
            failures_remaining: std::sync::atomic::AtomicUsize::new(1),
            attempts: std::sync::atomic::AtomicUsize::new(0),
            initial_empty: false,
            accepted_failures_remaining: std::sync::atomic::AtomicUsize::new(0),
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
        assert_eq!(sleeps.lock().unwrap().first(), Some(&ERROR_RETRY_INTERVAL));
        assert_eq!(
            discovery.attempts.load(std::sync::atomic::Ordering::SeqCst),
            3
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
            accepted_failures_remaining: std::sync::atomic::AtomicUsize::new(0),
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
            4
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
    async fn startup_exact_obligation_read_failure_retries_before_ready_then_settles() {
        let (repo, dispatcher) = fixture().await;
        seed_terminal_obligation(&repo, "startup-exact-read").await;
        *dispatcher.terminal_results.lock().unwrap() = vec![
            Err(crate::runtime::DatabaseTerminalRecoveryError::Retryable(
                "injected exact obligation read failure".to_string(),
            )),
            Ok(TerminalObligationSettlement::Committed),
        ];
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
        let worker = DirectTurnWorker::new(repo, dispatcher.clone(), clock, ProcessIncarnation(1));
        let handle = tokio::spawn(async move { worker.run_loop(kick_rx, ready_tx).await });

        retry_started_rx.await.unwrap();
        assert!(ready_rx.try_recv().is_err());
        assert!(dispatcher.events.lock().unwrap().is_empty());
        retry_release_tx.send(()).unwrap();
        settled_rx.await.unwrap();
        ready_rx.await.unwrap();
        assert_eq!(
            dispatcher.terminal_attempts.lock().unwrap().as_slice(),
            &["conv-a".to_string(), "conv-a".to_string()]
        );
        assert_eq!(sleeps.lock().unwrap().first(), Some(&ERROR_RETRY_INTERVAL));
        drop(kick_tx);
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn steady_state_exact_obligation_read_failure_retries_then_settles_once() {
        let (repo, dispatcher) = fixture().await;
        seed_terminal_obligation(&repo, "steady-exact-read").await;
        *dispatcher.terminal_results.lock().unwrap() = vec![
            Ok(TerminalObligationSettlement::NoObligation),
            Err(crate::runtime::DatabaseTerminalRecoveryError::Retryable(
                "injected exact obligation read failure".to_string(),
            )),
            Ok(TerminalObligationSettlement::Committed),
        ];
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let (retry_started_tx, retry_started_rx) = tokio::sync::oneshot::channel();
        let (retry_release_tx, retry_release_rx) = tokio::sync::oneshot::channel();
        let clock = Arc::new(GatedRetryClock {
            sleeps: sleeps.clone(),
            retry_started: Mutex::new(Some(retry_started_tx)),
            retry_release: Mutex::new(Some(retry_release_rx)),
        });
        let (kick_tx, kick_rx) = watch::channel(0);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let worker = DirectTurnWorker::new(repo, dispatcher.clone(), clock, ProcessIncarnation(1));
        let handle = tokio::spawn(async move { worker.run_loop(kick_rx, ready_tx).await });

        ready_rx.await.unwrap();
        kick_tx.send_replace(1);
        retry_started_rx.await.unwrap();
        assert_eq!(dispatcher.terminal_attempts.lock().unwrap().len(), 2);
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
            &[
                "conv-a".to_string(),
                "conv-a".to_string(),
                "conv-a".to_string(),
            ]
        );
        assert!(dispatcher.events.lock().unwrap().is_empty());
        assert_eq!(
            sleeps.lock().unwrap().as_slice(),
            &[
                EMPTY_RESCAN_INTERVAL,
                ERROR_RETRY_INTERVAL,
                EMPTY_RESCAN_INTERVAL,
            ]
        );
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn terminal_settlement_completes_before_startup_resume_can_create_runtime() {
        use crate::platform::PlatformCapability;
        use crate::tools::mcp::McpClientManager;
        use phoenix_core::domain::sm_state::ConvState;
        use phoenix_llm::ModelRegistry;

        let db = Database::open_in_memory().await.unwrap();
        let parent_id = "settle-before-resume-parent";
        db.create_conversation(parent_id, "parent", "/tmp", true, None, None)
            .await
            .unwrap();
        db.create_conversation("conv-a", "obligation", "/tmp", true, None, None)
            .await
            .unwrap();
        db.update_conversation_state(parent_id, &ConvState::LlmRequesting { attempt: 1 })
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO startup_parent_actions
                 (conversation_id, action, transcript_generation, created_at)
             SELECT ?1, 'Resume', transcript_generation, ?2
             FROM conversations WHERE id = ?1",
        )
        .bind(parent_id)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();

        let repo = db.workflow_repository();
        let turn_id = accept(&repo, "settle-before-resume").await;
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
                prepared: prepared_payload("message-settle-before-resume"),
                sequence_id: 110,
                created_at: Timestamp(110),
                accepted_state: ConvState::LlmRequesting { attempt: 1 },
                state_updated_at: chrono::DateTime::from_timestamp(110, 0).unwrap(),
                now: Timestamp(10),
            },
        )
        .await
        .established()
        .unwrap();
        repo.persist_terminal_obligation(
            &phoenix_db::workflow::DirectTurnTerminalObligationInput {
                turn_id,
                expected_generation: 0,
                terminal: phoenix_workflow::TurnTerminal::Completed,
                projection: phoenix_db::workflow::PersistedConversationProjection {
                    state: ConvState::Idle,
                    state_updated_at: chrono::Utc::now(),
                },
                response_message_id: None,
            },
        )
        .await
        .unwrap();

        let manager = Arc::new(RuntimeManager::new(
            db.clone(),
            Arc::new(ModelRegistry::new_empty()),
            PlatformCapability::None {
                details: "test".to_string(),
            },
            Arc::new(McpClientManager::new()),
            None,
        ));
        let (settlement_failed_tx, settlement_failed_rx) = tokio::sync::oneshot::channel();
        let (settlement_release_tx, settlement_release_rx) = tokio::sync::oneshot::channel();
        let dispatcher = Arc::new(RealStartupOrderingDispatcher {
            manager: Arc::clone(&manager),
            fail_settlement_once: std::sync::atomic::AtomicBool::new(true),
            settlement_failed: Mutex::new(Some(settlement_failed_tx)),
            settlement_release: Mutex::new(Some(settlement_release_rx)),
            reconciliation_calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let (kick_tx, kick_rx) = watch::channel(0);
        let (ready_tx, mut ready_rx) = tokio::sync::oneshot::channel();
        let worker = DirectTurnWorker::new(
            repo.clone(),
            Arc::clone(&dispatcher),
            Arc::new(TestClock {
                now: Timestamp(10),
                sleeps: Some(Arc::new(Mutex::new(Vec::new()))),
            }),
            ProcessIncarnation(1),
        );
        let worker_handle = tokio::spawn(async move { worker.run_loop(kick_rx, ready_tx).await });

        settlement_failed_rx.await.unwrap();
        assert!(ready_rx.try_recv().is_err());
        assert_eq!(
            dispatcher
                .reconciliation_calls
                .load(std::sync::atomic::Ordering::Acquire),
            0
        );
        assert!(manager.try_get_handle(parent_id).await.is_none());
        let failed_actions = db.list_startup_parent_actions().await.unwrap();
        assert_eq!(failed_actions.len(), 1);
        assert_eq!(
            failed_actions[0].action,
            phoenix_db::StartupParentAction::Resume
        );
        assert!(repo
            .load_active_terminal_obligation(&ConversationAuthority("conv-a".to_string()))
            .await
            .unwrap()
            .is_some());
        assert!(
            !failed_actions
                .iter()
                .any(|action| action.action == phoenix_db::StartupParentAction::Reconcile),
            "failed settlement cannot create a new Reconcile action"
        );

        settlement_release_tx.send(()).unwrap();
        ready_rx.await.unwrap();
        assert_eq!(
            dispatcher
                .reconciliation_calls
                .load(std::sync::atomic::Ordering::Acquire),
            1
        );
        assert!(manager.try_get_handle(parent_id).await.is_some());
        assert!(db.list_startup_parent_actions().await.unwrap().is_empty());
        assert!(repo
            .load_active_terminal_obligation(&ConversationAuthority("conv-a".to_string()))
            .await
            .unwrap()
            .is_none());

        drop(kick_tx);
        worker_handle.await.unwrap().unwrap();
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
        .established()
        .expect("classified direct-turn materialization");
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
        let (retry_started_tx, retry_started_rx) = tokio::sync::oneshot::channel();
        let (retry_release_tx, retry_release_rx) = tokio::sync::oneshot::channel();
        let clock = Arc::new(GatedRetryClock {
            sleeps,
            retry_started: Mutex::new(Some(retry_started_tx)),
            retry_release: Mutex::new(Some(retry_release_rx)),
        });
        let (kick_tx, kick_rx) = watch::channel(0);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let worker =
            DirectTurnWorker::new(repo, Arc::clone(&dispatcher), clock, ProcessIncarnation(1));
        let handle = tokio::spawn(async move { worker.run_loop(kick_rx, ready_tx).await });

        retry_started_rx.await.unwrap();
        retry_release_tx.send(()).unwrap();
        ready_rx.await.unwrap();
        assert_eq!(
            dispatcher.terminal_attempts.lock().unwrap().as_slice(),
            &["conv-a".to_string(), "conv-a".to_string()]
        );
        drop(kick_tx);
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn initial_unclassifiable_reconciliation_returns_fatal_local_authority_exit() {
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
        .established()
        .expect("classified direct-turn materialization");
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
    async fn startup_phase_completes_before_ready_and_ordinary_dispatch() {
        let (repo, dispatcher) = fixture().await;
        accept(&repo, "startup").await;
        let (kick_tx, kick_rx) = watch::channel(0);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (dispatched_tx, dispatched_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .event_dispatched
            .lock()
            .unwrap()
            .replace(dispatched_tx);
        let worker = worker(repo, dispatcher.clone(), 10, 1);
        let handle = tokio::spawn(async move { worker.run_loop(kick_rx, ready_tx).await });
        ready_rx.await.unwrap();
        assert_eq!(
            dispatcher
                .startup_reconciliations
                .load(std::sync::atomic::Ordering::Acquire),
            1
        );
        dispatched_rx.await.unwrap();
        assert_eq!(dispatcher.events.lock().unwrap().len(), 1);
        assert_eq!(
            dispatcher
                .startup_reconciliations_at_dispatch
                .lock()
                .unwrap()
                .as_slice(),
            &[1],
            "ordinary dispatch observes completed startup reconciliation"
        );
        drop(kick_tx);
        handle.await.unwrap().unwrap();
    }
}
