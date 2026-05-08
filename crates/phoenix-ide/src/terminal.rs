//! PTY-backed terminal sessions — REQ-TERM-001 through REQ-TERM-014
//!
//! Each conversation may have at most one active terminal session (REQ-TERM-003).
//! Sessions are spawned on WebSocket upgrade and torn down on close or conversation
//! lifecycle end.
//!
//! See `specs/terminal/` for the full behavioral specification.

pub mod command_tracker;
#[cfg(test)]
mod proptests;
mod relay;
mod session;
mod spawn;
#[cfg(test)]
pub(crate) mod test_helpers;
mod ws;

pub use session::ActiveTerminals;
pub use session::ShellIntegrationStatus;
pub use ws::terminal_ws_handler;
