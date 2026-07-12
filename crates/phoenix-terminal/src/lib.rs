//! PTY-backed terminal sessions — REQ-TERM-001 through REQ-TERM-014.
//!
//! Sessions are keyed by `WorkScope`, so at most one terminal is active per
//! scope (REQ-TERM-003 generalised). `WorkScope::Global` provides the
//! singleton terminal surfaced on `/new`. Sessions spawn on WebSocket
//! upgrade and tear down per the scope's lifecycle (REQ-TERM-012).
//!
//! This crate is terminal-core only: PTY spawn, the WS<->PTY relay, command
//! tracking, and the per-scope session registry. The axum/WebSocket glue that
//! wires these into HTTP routes lives in the binary crate's `api` layer
//! (`api::terminal_ws`), because it depends upward on `AppState` and the
//! runtime — keeping it out preserves this crate's acyclic position.
//!
//! See `specs/terminal/` for the full behavioural specification.

pub mod command_tracker;
pub mod relay;
pub mod session;
pub mod spawn;

// ANSI/OSC test-stream builders. `#[cfg(test)]` for this crate's own tests;
// also exposed to downstream crates' test builds behind `test-support` so the
// api-layer WebSocket tests can drive the CommandTracker.
#[cfg(any(test, feature = "test-support"))]
pub mod test_helpers;

#[cfg(test)]
mod proptests;

pub use session::cascade_terminal_on_delete;
pub use session::ActiveTerminals;
pub use session::ShellIntegrationStatus;
pub use session::TerminalChildKind;
