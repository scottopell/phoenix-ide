//! HTTP API for Phoenix IDE
//!
//! REQ-API-001 through REQ-API-010

mod assets;
pub mod auth;
mod chains;
mod git_handlers;
mod handlers;
mod lifecycle_handlers;
mod sse;
mod types;
mod wire;

pub use handlers::create_router;
#[allow(unused_imports)] // Public API re-exports
pub use types::*;

use crate::chain_qa::ChainQa;
use crate::db::Database;
use crate::llm::ModelRegistry;
use crate::platform::PlatformCapability;
use crate::runtime::RuntimeManager;
use crate::terminal::ActiveTerminals;
use crate::tools::mcp::McpClientManager;
use std::sync::Arc;

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<RuntimeManager>,
    pub llm_registry: Arc<ModelRegistry>,
    pub db: Database,
    #[allow(dead_code)] // Exposed for future API handlers (e.g., /status endpoint)
    pub platform: PlatformCapability,
    pub mcp_manager: Arc<McpClientManager>,
    pub credential_helper: Option<Arc<crate::llm::CredentialHelper>>,
    /// When set, all non-exempt API endpoints require this password (REQ-AUTH-001).
    pub password: Option<String>,
    /// Active PTY terminal sessions keyed by conversation ID (REQ-TERM-003).
    pub terminals: ActiveTerminals,
    /// Chain Q&A backend (REQ-CHN-001/004/005). Owns the
    /// [`crate::chain_runtime::ChainRuntimeRegistry`] that the chains API
    /// handlers subscribe to and publish onto.
    pub chain_qa: ChainQa,
}

impl AppState {
    /// Create new application state and start the sub-agent handler
    pub async fn new(
        db: Database,
        llm_registry: Arc<ModelRegistry>,
        platform: PlatformCapability,
        mcp_manager: Arc<McpClientManager>,
        credential_helper: Option<Arc<crate::llm::CredentialHelper>>,
        password: Option<String>,
    ) -> Self {
        let runtime = Arc::new(RuntimeManager::new(
            db.clone(),
            llm_registry.clone(),
            platform,
            mcp_manager.clone(),
            credential_helper.clone(),
        ));
        runtime.start_sub_agent_handler().await;
        let terminals = runtime.terminals.clone();
        // Chain Q&A is constructed last so it can share the same `Database`
        // and `ModelRegistry` handles. Its internal `ChainRuntimeRegistry`
        // is owned by this `ChainQa` value — chain SSE handlers reach into
        // it via `state.chain_qa.runtime_registry()` so subscribers and
        // publishers go through one registry.
        let chain_qa = ChainQa::new(db.clone(), llm_registry.clone());
        Self {
            runtime,
            llm_registry,
            db,
            platform,
            mcp_manager,
            credential_helper,
            password,
            terminals,
            chain_qa,
        }
    }
}
