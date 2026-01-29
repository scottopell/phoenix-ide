//! HTTP API for Phoenix IDE
//!
//! REQ-API-001 through REQ-API-010

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
}

impl AppState {
    pub fn new(db: Database, llm_registry: Arc<ModelRegistry>) -> Self {
        Self {
            runtime: Arc::new(RuntimeManager::new(db, llm_registry.clone())),
            llm_registry,
        }
    }
}
