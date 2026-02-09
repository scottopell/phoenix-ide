//! Browser automation tools using Chrome `DevTools` Protocol
//!
//! REQ-BT-010: Implicit Session Model
//! REQ-BT-011: State Persistence
//! REQ-BT-012: Stateless Tools with Context Injection

pub mod session;
mod tools;

#[cfg(test)]
mod tests;

pub use session::{BrowserError, BrowserSessionManager};
pub use tools::{
    BrowserClearConsoleLogsTool, BrowserEvalTool, BrowserNavigateTool,
    BrowserRecentConsoleLogsTool, BrowserResizeTool, BrowserTakeScreenshotTool,
};
