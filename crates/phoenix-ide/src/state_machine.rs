//! Core conversation state machine
//!
//! Implements the Elm Architecture pattern with pure state transitions.
//! Two pure entry points: `transition()` for user events, `handle_outcome()`
//! for executor-produced outcomes.

pub(crate) mod effect;
pub mod event;
pub mod outcome;
pub mod state;
pub(crate) mod transition;

#[cfg(test)]
mod project_proptests;
#[cfg(test)]
mod proptests;

pub use effect::Effect;
pub use event::Event;
pub use outcome::AbortReason;
pub use state::{ConvContext, ConvState, StepResult};
// Re-exports for split state types (used by future callers that adopt the split API)
#[allow(unused_imports)]
pub use state::{CoreState, ParentState, SubAgentState};
pub use transition::{check_user_message_acceptable, handle_outcome, transition, TransitionError};

// Re-exports for atomic persistence types (used by runtime/executor)
pub use effect::tool_result_message_id;
#[allow(unused_imports)]
pub use effect::{CheckpointData, PersistError};
#[allow(unused_imports)]
pub use state::AssistantMessage;
