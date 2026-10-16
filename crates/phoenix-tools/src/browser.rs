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
mod snapshot;
mod tools;

#[cfg(test)]
mod tests;

// Re-export the engine modules so callers reaching for `browser::session::X`,
// `browser::screencast::X`, or `browser::react::X` resolve unchanged.
pub use phoenix_browser::{react, screencast, session};

// Flat re-exports of the engine surface used through `browser::*`.
pub use phoenix_browser::{BrowserError, BrowserSession, BrowserSessionManager};

pub use profile::BrowserProfileTool;
pub use snapshot::BrowserSnapshotTool;
pub use tools::{
    parse_duration, BrowserClearConsoleLogsTool, BrowserClickTool, BrowserEvalTool,
    BrowserEvaluation, BrowserKeyPressTool, BrowserNavigateTool, BrowserRecentConsoleLogsTool,
    BrowserResizeTool, BrowserTakeScreenshotTool, BrowserTypeTool, BrowserWaitForSelectorTool,
    SavedEvaluationArtifact,
};

use crate::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Supplies a browser session independently of the calling harness.
#[async_trait]
pub trait BrowserAccess: Send + Sync {
    /// Resolve the browser session authorized by the caller's lifecycle owner.
    ///
    /// # Errors
    /// Returns an error when the session is unavailable or cannot be started.
    async fn browser(&self) -> Result<Arc<RwLock<BrowserSession>>, String>;
}

struct ValidationBoundary(AtomicBool);

#[async_trait]
impl BrowserAccess for ValidationBoundary {
    async fn browser(&self) -> Result<Arc<RwLock<BrowserSession>>, String> {
        self.0.store(true, Ordering::Relaxed);
        Err("input validation reached the browser boundary".into())
    }
}

#[async_trait]
impl BrowserAccess for ToolContext {
    async fn browser(&self) -> Result<Arc<RwLock<BrowserSession>>, String> {
        ToolContext::browser(self)
            .await
            .map_err(|error| error.to_string())
    }
}

macro_rules! browser_operations {
    ($($variant:ident => $tool:ident),+ $(,)?) => {
        /// Browser operations shared by Phoenix tools and standalone callers.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum BrowserOperation {
            $($variant),+
        }

        impl BrowserOperation {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            #[must_use]
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $tool.name()),+
                }
            }

            #[must_use]
            pub fn from_name(name: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|operation| operation.name() == name)
            }

            #[must_use]
            pub fn description(self) -> String {
                match self {
                    $(Self::$variant => $tool.description()),+
                }
            }

            #[must_use]
            pub fn input_schema(self) -> Value {
                match self {
                    $(Self::$variant => $tool.input_schema()),+
                }
            }

            pub async fn execute(
                self,
                input: Value,
                access: &(impl BrowserAccess + ?Sized),
            ) -> ToolOutput {
                match self {
                    $(Self::$variant => $tool::run_browser(input, access).await),+
                }
            }

            /// Validate input without launching a browser or performing an operation.
            ///
            /// # Errors
            /// Returns the operation's structured input rejection.
            pub async fn validate_input(self, input: Value) -> Result<(), ToolOutput> {
                if self == Self::Profile {
                    return BrowserProfileTool::validate_input(input);
                }
                let boundary = ValidationBoundary(AtomicBool::new(false));
                let output = self.execute(input, &boundary).await;
                if boundary.0.load(Ordering::Relaxed) || output.is_success() {
                    Ok(())
                } else {
                    Err(output)
                }
            }
        }
    };
}

browser_operations! {
    Navigate => BrowserNavigateTool,
    Snapshot => BrowserSnapshotTool,
    Eval => BrowserEvalTool,
    TakeScreenshot => BrowserTakeScreenshotTool,
    RecentConsoleLogs => BrowserRecentConsoleLogsTool,
    ClearConsoleLogs => BrowserClearConsoleLogsTool,
    Resize => BrowserResizeTool,
    WaitForSelector => BrowserWaitForSelectorTool,
    Click => BrowserClickTool,
    Type => BrowserTypeTool,
    KeyPress => BrowserKeyPressTool,
    Profile => BrowserProfileTool,
}

#[cfg(test)]
mod access_tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct UnavailableBrowser(AtomicUsize);

    #[async_trait]
    impl BrowserAccess for UnavailableBrowser {
        async fn browser(&self) -> Result<Arc<RwLock<BrowserSession>>, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err("session expired; navigate to create a new browser".to_string())
        }
    }

    #[tokio::test]
    async fn standalone_dispatch_preserves_browser_free_validation() {
        let access = UnavailableBrowser::default();
        let help = BrowserOperation::Profile
            .execute(json!({"action": "help"}), &access)
            .await;
        assert!(help.is_success());
        let invalid_target = BrowserOperation::Click.execute(json!({}), &access).await;
        assert!(!invalid_target.is_success());
        assert!(invalid_target.output().contains("Invalid input"));
        let invalid_scenario = BrowserOperation::Profile
            .execute(json!({"action": "run_scenario", "steps": []}), &access)
            .await;
        assert!(!invalid_scenario.is_success());
        assert_eq!(access.0.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn standalone_dispatch_reports_lifecycle_owner_error() {
        let access = UnavailableBrowser::default();
        let result = BrowserOperation::Snapshot.execute(json!({}), &access).await;
        assert!(!result.is_success());
        assert_eq!(
            result.output(),
            "Failed to get browser: session expired; navigate to create a new browser"
        );
        assert_eq!(access.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn semantic_preflight_rejects_input_without_browser_or_file_access() {
        for (operation, input) in [
            (BrowserOperation::Click, json!({})),
            (BrowserOperation::Click, json!({"selector":"#x","ref":"@x"})),
            (
                BrowserOperation::Profile,
                json!({"action":"run_scenario","steps":[]}),
            ),
            (BrowserOperation::Profile, json!({"action":"unknown"})),
            (BrowserOperation::Profile, json!({"action":"throttle"})),
            (
                BrowserOperation::Profile,
                json!({"action":"throttle","rate":0}),
            ),
            (
                BrowserOperation::Profile,
                json!({"action":"run_scenario","runs":u32::MAX,"warmup":1,
                    "steps":[{"kind":"eval","expression":"true"}]}),
            ),
        ] {
            assert!(operation.validate_input(input).await.is_err());
        }
        assert!(BrowserOperation::Snapshot
            .validate_input(json!({}))
            .await
            .is_ok());
        assert!(BrowserOperation::Profile
            .validate_input(json!({
                "action":"cpu_summary", "path":"/nonexistent/preflight-must-not-read.cpuprofile"
            }))
            .await
            .is_ok());
    }

    #[test]
    fn operation_catalog_uses_the_registered_tool_contracts() {
        let registered = crate::browser_tools();
        assert_eq!(registered.len(), BrowserOperation::ALL.len());
        for tool in registered {
            let operation = BrowserOperation::from_name(tool.name()).expect("registered operation");
            assert_eq!(operation.name(), tool.name());
            assert_eq!(operation.description(), tool.description());
            assert_eq!(operation.input_schema(), tool.input_schema());
        }
        assert_eq!(BrowserOperation::from_name("unknown"), None);
    }
}
