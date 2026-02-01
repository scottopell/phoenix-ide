//! Core conversation state machine
//!
//! Implements the Elm Architecture pattern with pure state transitions.

mod effect;
pub mod event;
pub mod state;
pub(crate) mod transition;

#[cfg(test)]
mod proptests;

pub use effect::Effect;
pub use event::Event;
pub use state::{ConvContext, ConvState};
pub use transition::transition;
