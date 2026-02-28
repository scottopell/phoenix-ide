//! Core conversation state machine
//!
//! Implements the Elm Architecture pattern with pure state transitions.

pub(crate) mod effect;
pub mod event;
pub mod state;
pub(crate) mod transition;

#[cfg(test)]
mod proptests;

pub use effect::Effect;
pub use event::Event;
pub use state::{ConvContext, ConvState};
pub use transition::transition;

// Re-exports for atomic persistence types (used by runtime/executor)
#[allow(unused_imports)]
pub use effect::{CheckpointData, PersistError};
#[allow(unused_imports)]
pub use state::AssistantMessage;
