//! Headless-browser engine for Phoenix IDE.
//!
//! This crate owns the Chrome `DevTools` Protocol driver — the chromiumoxide
//! browser/page lifecycle, per-`WorkScope` session management, console and
//! profiling capture, and the live-view screencast broker. It is Tool-free:
//! the `Tool`-trait glue (the `browser_*` tool implementations) lives in
//! phoenix-tools, which depends on this crate. See `specs/browser-tool/` for
//! the behavioural contract.

pub mod react;
pub mod screencast;
pub mod session;

pub use screencast::{ScreencastBroker, ScreencastEvent};
pub use session::{
    cascade_browser_on_delete, fetcher_cache_dir, truncate_unicode_safe, user_data_dir_glob,
    BrowserError, BrowserSession, BrowserSessionGuard, BrowserSessionLifecycleEvent,
    BrowserSessionLifecycleSink, BrowserSessionManager, ConsoleEntry, ConsoleLevel, ProfilingState,
    ScopeLivenessHook,
};
