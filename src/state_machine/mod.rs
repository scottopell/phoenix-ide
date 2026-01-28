//! Core conversation state machine
//!
//! Implements the Elm Architecture pattern with pure state transitions.

mod effect;
pub mod event;
mod state;
mod transition;

pub use effect::Effect;
pub use event::Event;
pub use state::{ConvContext, ConvState};
pub use transition::transition;
