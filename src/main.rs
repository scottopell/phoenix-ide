//! Phoenix IDE - LLM-powered development environment
//!
//! A Rust backend implementing a conversation state machine for
//! interacting with LLM agents.

mod api;
mod db;
mod llm;
mod message_expander;
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
    trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod hot_restart;

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
                .with_current_span(true)
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
    let db = Database::open(&db_path).await?;

    // Reset all conversations to idle on startup (REQ-BED-007)
    db.reset_all_to_idle().await?;

    // Initialize LLM registry with model discovery
    let llm_config = LlmConfig::from_env();
    let llm_registry = Arc::new(ModelRegistry::new_with_discovery(&llm_config).await);

    if llm_registry.has_models() {
        tracing::info!(
            models = %llm_registry.available_models().join(", "),
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

    // HTTP access log: one line per request with method, path, status, latency.
    // Health check endpoint (/version) is suppressed from normal INFO logging.
    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(|request: &axum::http::Request<_>| {
            // Create a span at INFO level; health checks get a separate disabled span
            // to suppress them from normal log output.
            let path = request.uri().path();
            if path == "/version" {
                tracing::debug_span!(
                    "http",
                    method = %request.method(),
                    path = %path,
                )
            } else {
                tracing::info_span!(
                    "http",
                    method = %request.method(),
                    path = %path,
                )
            }
        })
        .on_response(
            |response: &axum::http::Response<_>,
             latency: std::time::Duration,
             span: &tracing::Span| {
                tracing::info!(
                    parent: span,
                    status = response.status().as_u16(),
                    latency_ms = u64::try_from(latency.as_millis()).unwrap_or(u64::MAX),
                );
            },
        )
        .on_request(tower_http::trace::DefaultOnRequest::new().level(tracing::Level::DEBUG))
        .on_failure(tower_http::trace::DefaultOnFailure::new().level(tracing::Level::ERROR));

    let app = create_router(state)
        .layer(trace_layer)
        .layer(cors)
        .layer(compression);

    // Get listener (either from systemd socket activation or bind fresh)
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = hot_restart::get_listener(addr).await?;
    tracing::info!(
        addr = %listener.local_addr()?,
        socket_activated = hot_restart::is_socket_activated(),
        "Phoenix IDE server listening"
    );

    // Run server with graceful shutdown on signals
    let server = axum::serve(listener, app);
    server
        .with_graceful_shutdown(hot_restart::shutdown_signal())
        .await?;

    // After graceful shutdown, check if we should hot restart
    // (This does not return if hot restart is performed)
    hot_restart::maybe_perform_hot_restart();

    Ok(())
}
