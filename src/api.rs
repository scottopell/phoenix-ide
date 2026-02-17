//! HTTP API for Phoenix IDE
//!
//! REQ-API-001 through REQ-API-010

mod assets;
mod handlers;
mod sse;
mod types;

pub use handlers::create_router;
#[allow(unused_imports)] // Public API re-exports
pub use types::*;

use crate::db::Database;
use crate::llm::ModelRegistry;
use crate::runtime::RuntimeManager;
use std::sync::Arc;

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<RuntimeManager>,
    pub llm_registry: Arc<ModelRegistry>,
    pub db: Database,
}

impl AppState {
    /// Create new application state and start the sub-agent handler
    pub async fn new(db: Database, llm_registry: Arc<ModelRegistry>) -> Self {
        tracing::info!("AppState::new() - ENTERED");

        tracing::info!("AppState::new() - About to call RuntimeManager::new()");
        let runtime = Arc::new(RuntimeManager::new(db.clone(), llm_registry.clone()));
        tracing::info!("AppState::new() - RuntimeManager::new() completed");

        // Start the sub-agent spawn/cancel handler
        tracing::info!("AppState::new() - About to call start_sub_agent_handler().await");
        runtime.start_sub_agent_handler().await;
        tracing::info!("AppState::new() - start_sub_agent_handler() completed");

        tracing::info!("AppState::new() - About to return Self");
        Self {
            runtime,
            llm_registry,
            db,
        }
    }
}
