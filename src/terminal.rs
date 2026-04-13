//! PTY-backed terminal sessions — REQ-TERM-001 through REQ-TERM-014
//!
//! Each conversation may have at most one active terminal session (REQ-TERM-003).
//! Sessions are spawned on WebSocket upgrade and torn down on close or conversation
//! lifecycle end.
//!
//! See `specs/terminal/` for the full behavioral specification.

#[cfg(test)]
mod alacritty_parser;
#[cfg(test)]
mod proptests;
mod relay;
mod session;
mod spawn;
#[cfg(test)]
mod wezterm_parser;
mod ws;

pub use session::ActiveTerminals;
#[allow(unused_imports)]
pub use session::TerminalHandle; // Used by Task 5 teardown and read_terminal tool
pub use ws::terminal_ws_handler;
