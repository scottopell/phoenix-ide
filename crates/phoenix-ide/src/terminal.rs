//! PTY-backed terminal sessions — REQ-TERM-001 through REQ-TERM-014,
//! REQ-TERM-WS-001.
//!
//! Sessions are keyed by `WorkScope`, so at most one terminal is active per
//! scope (REQ-TERM-003 generalised). `WorkScope::Global` provides the
//! singleton terminal surfaced on `/new`. Sessions spawn on WebSocket
//! upgrade and tear down per the scope's lifecycle (REQ-TERM-012).
//!
//! See `specs/terminal/` for the full behavioural specification.

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
pub use ws::{terminal_ws_global_handler, terminal_ws_handler};
