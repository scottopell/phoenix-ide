//! Core conversation state machine
//!
//! Implements the Elm Architecture pattern with pure state transitions.

mod effect;
pub mod event;
pub mod state;
mod transition;

#[cfg(test)]
mod proptests;

pub use effect::Effect;
pub use event::Event;
#[allow(unused_imports)]
pub use state::{ConvContext, ConvState, ToolCall, ToolInput};
pub use transition::{transition, TransitionError};
