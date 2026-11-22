//! Actor-owned eligibility for the original native Close source's first live effect.
//!
//! This primitive deliberately does not perform effects or persist state. The actor
//! validates the borrowed live state; workers receive only a cloneable leaf handle.

use crate::runtime::native_close_application::initial_command_from_executing_source;
use phoenix_core::domain::sm_state::ConvState;
use phoenix_db::native_close::{NativeCloseCommand, NativeCloseIntent};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Eligibility {
    Unavailable,
    Eligible,
    Revoked,
}

struct Shared {
    eligibility: Mutex<Eligibility>,
    revoked: std::sync::atomic::AtomicBool,
    original: NativeCloseCommand,
    deadline: Instant,
}

/// The non-cloneable authority retained by the conversation actor.
///
/// Only methods on this value accept the actor's borrowed live state. Cloning the
/// worker handle cannot establish or restore eligibility.
pub(crate) struct SourceEntryController {
    shared: Arc<Shared>,
}

/// Worker capability scoped to one exact original command and deadline.
#[derive(Clone)]
pub(crate) struct SourceEntryWorker {
    shared: Arc<Shared>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActorEntryResult {
    Updated,
    Busy,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerEntryResult<T> {
    Ran(T),
    Busy,
    Refused,
}

impl SourceEntryController {
    /// Creates a closed cell bound to the original command and its original deadline.
    pub(crate) fn new(original: NativeCloseCommand, deadline: Instant) -> Self {
        Self {
            shared: Arc::new(Shared {
                eligibility: Mutex::new(Eligibility::Unavailable),
                revoked: std::sync::atomic::AtomicBool::new(false),
                original,
                deadline,
            }),
        }
    }

    /// Read-only projection used before the actor installs a proposed source.
    /// It never opens eligibility or treats the proposal as committed state.
    pub(crate) fn matches_state(&self, state: &ConvState) -> bool {
        source_matches(state, &self.shared.original)
    }

    pub(crate) fn worker(&self) -> SourceEntryWorker {
        SourceEntryWorker {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Revalidates eligibility using the actor's actual borrowed state and exact
    /// original command. An expired deadline or any source mismatch refuses entry.
    pub(crate) fn revalidate(
        &mut self,
        live_state: &ConvState,
        exact_original: &NativeCloseCommand,
    ) -> ActorEntryResult {
        if self
            .shared
            .revoked
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return ActorEntryResult::Refused;
        }
        let mut eligibility = match try_lock(&self.shared) {
            LockAttempt::Acquired(guard) => guard,
            LockAttempt::Busy => return ActorEntryResult::Busy,
            LockAttempt::Poisoned => return ActorEntryResult::Refused,
        };
        if *eligibility == Eligibility::Revoked {
            return ActorEntryResult::Refused;
        }
        if Instant::now() >= self.shared.deadline
            || exact_original != &self.shared.original
            || !source_matches(live_state, &self.shared.original)
        {
            self.shared
                .revoked
                .store(true, std::sync::atomic::Ordering::Release);
            *eligibility = Eligibility::Revoked;
            return ActorEntryResult::Refused;
        }
        *eligibility = Eligibility::Eligible;
        ActorEntryResult::Updated
    }

    /// Closes admission immediately, then performs the actor's state transition
    /// without waiting for a worker's synchronous first-effect callback. On Busy,
    /// the caller must retain and defer the source event, then retry this method
    /// before applying it; the atomic flag already refuses later worker entrants.
    pub(crate) fn revoke(&mut self) -> ActorEntryResult {
        self.shared
            .revoked
            .store(true, std::sync::atomic::Ordering::Release);
        match try_lock(&self.shared) {
            LockAttempt::Acquired(mut eligibility) => {
                *eligibility = Eligibility::Revoked;
                ActorEntryResult::Updated
            }
            LockAttempt::Busy => ActorEntryResult::Busy,
            LockAttempt::Poisoned => ActorEntryResult::Refused,
        }
    }
}

impl Drop for SourceEntryController {
    fn drop(&mut self) {
        self.shared
            .revoked
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

impl SourceEntryWorker {
    #[cfg(test)]
    pub(crate) fn is_revoked_for_test(&self) -> bool {
        self.shared
            .revoked
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Read-only witness for the terminal-entry test hook. Do not use `try_lock`:
    /// that helper turns mutex poisoning into revocation, which would perturb the
    /// behavior this observation is meant to witness.
    #[cfg(test)]
    pub(crate) fn is_eligible_unrevoked_before_deadline_for_test(&self) -> bool {
        if self
            .shared
            .revoked
            .load(std::sync::atomic::Ordering::Acquire)
            || Instant::now() >= self.shared.deadline
        {
            return false;
        }
        match self.shared.eligibility.try_lock() {
            Ok(eligibility) => {
                *eligibility == Eligibility::Eligible
                    && !self
                        .shared
                        .revoked
                        .load(std::sync::atomic::Ordering::Acquire)
                    && Instant::now() < self.shared.deadline
            }
            Err(_) => false,
        }
    }

    /// Read-only deadline witness sampled from the original source deadline.
    #[cfg(test)]
    pub(crate) fn original_deadline_unexpired_for_test(&self) -> bool {
        Instant::now() < self.shared.deadline
    }

    /// Runs a synchronous first-effect step only while the actor-established
    /// original-source eligibility is held. The mutex guard spans the callback,
    /// so an actor revocation racing an admitted callback returns `Busy` and must
    /// defer its event until the callback completes. Multiple distinct leaves of
    /// the same command may use this; per-leaf affine ownership prevents duplicates.
    pub(crate) fn try_first_effect<T>(
        &self,
        exact_original: &NativeCloseCommand,
        effect: impl FnOnce() -> T,
    ) -> WorkerEntryResult<T> {
        let guard = match try_lock(&self.shared) {
            LockAttempt::Acquired(guard) => guard,
            LockAttempt::Busy => return WorkerEntryResult::Busy,
            LockAttempt::Poisoned => return WorkerEntryResult::Refused,
        };
        if self
            .shared
            .revoked
            .load(std::sync::atomic::Ordering::Acquire)
            || *guard != Eligibility::Eligible
        {
            return WorkerEntryResult::Refused;
        }
        if Instant::now() >= self.shared.deadline || exact_original != &self.shared.original {
            self.shared
                .revoked
                .store(true, std::sync::atomic::Ordering::Release);
            return WorkerEntryResult::Refused;
        }
        WorkerEntryResult::Ran(effect())
    }
}

enum LockAttempt<'a> {
    Acquired(MutexGuard<'a, Eligibility>),
    Busy,
    Poisoned,
}

fn try_lock(shared: &Shared) -> LockAttempt<'_> {
    match shared.eligibility.try_lock() {
        Ok(guard) => LockAttempt::Acquired(guard),
        Err(TryLockError::WouldBlock) => LockAttempt::Busy,
        Err(TryLockError::Poisoned(guard)) => {
            shared
                .revoked
                .store(true, std::sync::atomic::Ordering::Release);
            drop(guard);
            LockAttempt::Poisoned
        }
    }
}

fn source_matches(live_state: &ConvState, command: &NativeCloseCommand) -> bool {
    let NativeCloseIntent::Initial { target } = command.intent() else {
        return false;
    };
    initial_command_from_executing_source(
        live_state,
        command.source_transcript_id(),
        command.source(),
        target.clone(),
    )
    .is_ok_and(|matched| matched == *command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::domain::{
        db_schema::SourceToolCall,
        llm_types::ContentBlock,
        product_conversation::ProductConversationId,
        sm_state::{AssistantMessage, ToolCall, ToolInput},
    };

    fn fixture() -> (ConvState, NativeCloseCommand, NativeCloseCommand) {
        let source = SourceToolCall {
            message_id: "source-message".into(),
            tool_use_id: "source-call".into(),
        };
        let target = ProductConversationId::parse("target").unwrap();
        let command = NativeCloseCommand::new(
            "transcript".into(),
            source.clone(),
            NativeCloseIntent::Initial {
                target: target.clone(),
            },
        )
        .unwrap();
        let input = command.intent().source_input();
        let state = ConvState::ToolExecuting {
            current_tool: ToolCall::new(
                &source.tool_use_id,
                ToolInput::Unknown {
                    name: "close_conversation".into(),
                    input: input.clone(),
                },
            ),
            remaining_tools: Vec::new(),
            completed_results: Vec::new(),
            pending_sub_agents: Vec::new(),
            assistant_message: AssistantMessage::new(
                source.message_id,
                vec![ContentBlock::ToolUse {
                    id: source.tool_use_id,
                    name: "close_conversation".into(),
                    input,
                }],
                None,
                None,
            ),
        };
        let mismatched = NativeCloseCommand::new(
            "other-transcript".into(),
            command.source().clone(),
            command.intent().clone(),
        )
        .unwrap();
        (state, command, mismatched)
    }

    #[test]
    fn source_mismatch_and_unavailable_or_revoked_cells_refuse_worker_entry() {
        let (state, command, mismatched) = fixture();
        let mut controller = SourceEntryController::new(
            command.clone(),
            Instant::now() + std::time::Duration::from_secs(5),
        );
        let worker = controller.worker();
        let mut called = false;
        assert_eq!(
            worker.try_first_effect(&command, || called = true),
            WorkerEntryResult::Refused
        );
        assert!(!called);
        assert_eq!(
            controller.revalidate(&state, &mismatched),
            ActorEntryResult::Refused
        );
        assert_eq!(
            controller.revalidate(&state, &command),
            ActorEntryResult::Refused
        );
        assert_eq!(
            worker.try_first_effect(&command, || called = true),
            WorkerEntryResult::Refused
        );
        assert!(!called);
    }

    #[test]
    fn eligible_then_mismatch_closes_admission() {
        let (state, command, mismatched) = fixture();
        let mut controller = SourceEntryController::new(
            command.clone(),
            Instant::now() + std::time::Duration::from_secs(5),
        );
        assert_eq!(
            controller.revalidate(&state, &command),
            ActorEntryResult::Updated
        );
        assert_eq!(
            controller.revalidate(&state, &mismatched),
            ActorEntryResult::Refused
        );
        let worker = controller.worker();
        let mut called = false;
        assert_eq!(
            worker.try_first_effect(&command, || called = true),
            WorkerEntryResult::Refused
        );
        assert!(!called);
        assert_eq!(
            controller.revalidate(&state, &command),
            ActorEntryResult::Refused
        );
    }

    #[test]
    fn dropped_controller_revokes_worker_admission() {
        let (state, command, _) = fixture();
        let mut controller = SourceEntryController::new(
            command.clone(),
            Instant::now() + std::time::Duration::from_secs(5),
        );
        assert_eq!(
            controller.revalidate(&state, &command),
            ActorEntryResult::Updated
        );
        let worker = controller.worker();
        drop(controller);
        let mut called = false;
        assert_eq!(
            worker.try_first_effect(&command, || called = true),
            WorkerEntryResult::Refused
        );
        assert!(!called);
    }

    #[test]
    fn expired_deadline_refuses_revalidation_and_worker_entry() {
        let (state, command, _) = fixture();
        let mut controller = SourceEntryController::new(command.clone(), Instant::now());
        assert_eq!(
            controller.revalidate(&state, &command),
            ActorEntryResult::Refused
        );
        let worker = controller.worker();
        let mut called = false;
        assert_eq!(
            worker.try_first_effect(&command, || called = true),
            WorkerEntryResult::Refused
        );
        assert!(!called);
    }

    #[test]
    fn held_leaf_makes_actor_revoke_nonblocking_busy() {
        let (state, command, _) = fixture();
        let mut controller = SourceEntryController::new(
            command.clone(),
            Instant::now() + std::time::Duration::from_secs(5),
        );
        assert_eq!(
            controller.revalidate(&state, &command),
            ActorEntryResult::Updated
        );
        let worker = controller.worker();
        let result = worker.try_first_effect(&command, || {
            assert_eq!(controller.revoke(), ActorEntryResult::Busy);
            42
        });
        assert_eq!(result, WorkerEntryResult::Ran(42));
        assert_eq!(
            worker.try_first_effect(&command, || ()),
            WorkerEntryResult::Refused
        );
    }

    #[test]
    fn poisoned_eligibility_permanently_refuses_actor_and_worker_entry() {
        let (state, command, _) = fixture();
        let mut controller = SourceEntryController::new(
            command.clone(),
            Instant::now() + std::time::Duration::from_secs(5),
        );
        assert_eq!(
            controller.revalidate(&state, &command),
            ActorEntryResult::Updated
        );
        let worker = controller.worker();

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            worker.try_first_effect(&command, || panic!("poison eligibility mutex"))
        }));
        assert!(panic.is_err());

        assert_eq!(
            controller.revalidate(&state, &command),
            ActorEntryResult::Refused
        );
        assert_eq!(controller.revoke(), ActorEntryResult::Refused);
        let mut called = false;
        assert_eq!(
            worker.try_first_effect(&command, || called = true),
            WorkerEntryResult::Refused
        );
        assert!(!called);
        assert_eq!(
            controller.revalidate(&state, &command),
            ActorEntryResult::Refused
        );
        assert_eq!(controller.revoke(), ActorEntryResult::Refused);
    }

    #[test]
    fn invalidation_before_leaf_prevents_callback() {
        let (state, command, _) = fixture();
        let mut controller = SourceEntryController::new(
            command.clone(),
            Instant::now() + std::time::Duration::from_secs(5),
        );
        assert_eq!(
            controller.revalidate(&state, &command),
            ActorEntryResult::Updated
        );
        assert_eq!(controller.revoke(), ActorEntryResult::Updated);
        let worker = controller.worker();
        let mut called = false;
        assert_eq!(
            worker.try_first_effect(&command, || called = true),
            WorkerEntryResult::Refused
        );
        assert!(!called);
    }
}
