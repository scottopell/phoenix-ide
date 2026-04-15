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
mod proptests;
#[cfg(test)]
mod project_proptests;

pub use effect::Effect;
pub use event::Event;
pub use outcome::AbortReason;
pub use state::{ConvContext, ConvState, StepResult};
pub use transition::{handle_outcome, transition};

// Re-exports for atomic persistence types (used by runtime/executor)
#[allow(unused_imports)]
pub use effect::{CheckpointData, PersistError};
#[allow(unused_imports)]
pub use state::AssistantMessage;
