//! Phoenix IDE - LLM-powered development environment
//!
//! A Rust backend implementing a conversation state machine for
//! interacting with LLM agents.

mod api;
mod db;
mod llm;
mod runtime;
mod state_machine;
mod system_prompt;
mod title_generator;
mod tools;

use api::{create_router, AppState};
use db::Database;
use llm::{LlmConfig, ModelRegistry};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "phoenix_ide=info,tower_http=debug".into()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_current_span(false)
                .with_span_list(false),
        )
        .init();

    // Configuration
    let db_path = std::env::var("PHOENIX_DB_PATH").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{home}/.phoenix-ide/phoenix.db")
    });

    let port: u16 = std::env::var("PHOENIX_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8000);

    // Ensure database directory exists
    if let Some(parent) = PathBuf::from(&db_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Initialize database
    tracing::info!(path = %db_path, "Opening database");
    let db = Database::open(&db_path)?;

    // Reset all conversations to idle on startup (REQ-BED-007)
    db.reset_all_to_idle()?;

    // Initialize LLM registry
    let llm_config = LlmConfig::from_env();
    let llm_registry = Arc::new(ModelRegistry::new(&llm_config));

    if llm_registry.has_models() {
        tracing::info!(
            models = ?llm_registry.available_models(),
            default = %llm_registry.default_model_id(),
            "LLM registry initialized"
        );
    } else {
        tracing::warn!("No LLM API keys configured. Set ANTHROPIC_API_KEY or LLM_GATEWAY.");
    }

    // Create application state
    let state = AppState::new(db, llm_registry).await;

    // Create router
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let compression = CompressionLayer::new()
        .gzip(true)
        .br(true)
        .deflate(true)
        .zstd(true);

    let app = create_router(state).layer(cors).layer(compression);

    // Start server
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Phoenix IDE server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
