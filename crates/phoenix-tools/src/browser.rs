//! Browser automation tools using Chrome `DevTools` Protocol
//!
//! REQ-BT-010: Implicit Session Model
//! REQ-BT-011: State Persistence
//! REQ-BT-012: Stateless Tools with Context Injection
//! REQ-BT-017: React Component Access
//!
//! The browser *engine* (the chromiumoxide/CDP driver, session manager, and
//! screencast broker) lives in the leaf crate `phoenix-browser`. This module
//! holds only the `Tool`-trait glue that wraps it. The engine modules are
//! re-exported below so existing `browser::session::*` / `browser::screencast::*`
//! paths keep resolving.

pub mod profile;
mod tools;

#[cfg(test)]
mod tests;

// Re-export the engine modules so callers reaching for `browser::session::X`,
// `browser::screencast::X`, or `browser::react::X` resolve unchanged.
pub use phoenix_browser::{react, screencast, session};

// Flat re-exports of the engine surface used through `browser::*`.
pub use phoenix_browser::{BrowserError, BrowserSession, BrowserSessionManager};

pub use profile::BrowserProfileTool;
pub use tools::{
    BrowserClearConsoleLogsTool, BrowserClickTool, BrowserEvalTool, BrowserKeyPressTool,
    BrowserNavigateTool, BrowserRecentConsoleLogsTool, BrowserResizeTool,
    BrowserTakeScreenshotTool, BrowserTypeTool, BrowserWaitForSelectorTool,
};
