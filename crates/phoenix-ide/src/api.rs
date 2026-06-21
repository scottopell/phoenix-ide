//! HTTP API for Phoenix IDE
//!
//! REQ-API-001 through REQ-API-010

#[cfg(test)]
mod alloc_guard;
mod assets;
pub mod auth;
mod browser_view;
mod chains;
pub mod codex_login;
mod deployment;
mod git_handlers;
pub(crate) mod handlers;
mod lifecycle_handlers;
mod local_reveal;
mod pr_monitoring;
mod process_sample;
mod spa_routes;
mod sse;
mod terminal_ws;
mod types;
mod usage;
pub(crate) mod wire;

pub use deployment::{absolutize, DeploymentConfig, DiskLocation, LogInfo, MeasureMode, TlsInfo};
pub use handlers::create_router;
#[allow(unused_imports)] // Public API re-exports
pub use types::*;

use crate::chain_qa::ChainQa;
use crate::db::{Database, Fts5Retriever, MessageRetriever};
use crate::platform::PlatformCapability;
use crate::runtime::RuntimeManager;
use crate::terminal::ActiveTerminals;
use crate::tools::mcp::McpClientManager;
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use phoenix_llm::ModelRegistry;
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
    pub credential_helper: Option<Arc<phoenix_llm::CredentialHelper>>,
    /// When set, all non-exempt API endpoints require this password (REQ-AUTH-001).
    pub password: Option<String>,
    /// Server-side store of valid browser session tokens. Login mints a random
    /// token into this set; the password itself never travels in a cookie.
    pub sessions: auth::SessionStore,
    /// Per-client login attempt throttle (brute-force lockout) for
    /// `POST /api/auth/login`.
    pub login_throttle: auth::LoginThrottle,
    /// Active PTY terminal sessions keyed by conversation ID (REQ-TERM-003).
    pub terminals: ActiveTerminals,
    /// Chain Q&A backend (REQ-CHN-001/004/005). Owns the
    /// [`crate::chain_runtime::ChainRuntimeRegistry`] that the chains API
    /// handlers subscribe to and publish onto.
    pub chain_qa: ChainQa,
    /// Conversation-retrieval backend (`specs/conversation-retrieval/`): the
    /// scope-filtered message-search seam (REQ-RET-005) the chain Q&A agent
    /// and a future application-wide Q&A drive. Reconciled against `messages`
    /// once at startup.
    #[allow(dead_code)] // Consumed by the chain Q&A agent loop (REQ-CHN-009, Phase 2)
    pub message_retriever: Arc<dyn MessageRetriever>,
    /// In-flight Codex/ChatGPT login flows. See [`codex_login`].
    pub codex_login: Arc<codex_login::CodexLoginManager>,
    /// Static deployment facts (binding, TLS, on-disk layout) resolved once at
    /// startup. Served read-only by `GET /api/deployment`. See [`deployment`].
    pub deployment: Arc<DeploymentConfig>,
    /// Filesystem-environment paths (`$HOME` / `$CODEX_HOME` / `temp_dir` and
    /// the Phoenix layout under them) resolved once at startup. The single
    /// authority handlers read on-disk locations from. See
    /// [`PhoenixRuntimeEnvironment`].
    pub runtime_env: Arc<PhoenixRuntimeEnvironment>,
    /// Scoped capability token authorizing `POST /api/suggest`. Minted at
    /// startup and injected into every PTY's env as `PHOENIX_SUGGEST_TOKEN` so
    /// the in-terminal `phx` can call the endpoint without the master password.
    /// Not the password: possessing it grants only command suggestions.
    pub suggest_token: String,
}

impl AppState {
    /// Create new application state and start the sub-agent handler
    // Each argument is a distinct startup-resolved dependency (db, registry,
    // platform, mcp, credentials, password, deployment facts, runtime env);
    // bundling them into a struct would only move the same fields behind one
    // more name.
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        db: Database,
        llm_registry: Arc<ModelRegistry>,
        platform: PlatformCapability,
        mcp_manager: Arc<McpClientManager>,
        credential_helper: Option<Arc<phoenix_llm::CredentialHelper>>,
        password: Option<String>,
        deployment: Arc<DeploymentConfig>,
        runtime_env: Arc<PhoenixRuntimeEnvironment>,
        suggest_token: String,
    ) -> Self {
        let runtime = Arc::new(RuntimeManager::new(
            db.clone(),
            llm_registry.clone(),
            platform,
            mcp_manager.clone(),
            credential_helper.clone(),
        ));
        runtime.start_sub_agent_handler().await;
        runtime.start_browser_lifecycle_bridge().await;
        runtime.start_work_scope_bridge().await;
        handlers::start_attachment_cleanup_task(db.clone());
        let terminals = runtime.terminals.clone();
        // Conversation-retrieval index: bring it in line with `messages` once
        // at startup (REQ-RET-003) off the request path — retrieval works on
        // whatever is already indexed while the sweep runs, and reports
        // `index_reconciled()` when complete.
        let retriever = Arc::new(Fts5Retriever::new(db.pool().clone()));
        {
            let retriever = retriever.clone();
            tokio::spawn(async move {
                match retriever.reconcile().await {
                    Ok(stats) => tracing::info!(
                        indexed = stats.indexed,
                        reindexed = stats.reindexed,
                        pruned = stats.pruned,
                        "conversation retrieval index reconciled"
                    ),
                    Err(e) => tracing::warn!(
                        error = %e,
                        "conversation retrieval index reconcile failed; recall may be incomplete until next startup"
                    ),
                }
            });
        }
        let message_retriever: Arc<dyn MessageRetriever> = retriever;
        // Chain Q&A shares the same `Database`, `ModelRegistry`, and retrieval
        // seam. Its internal `ChainRuntimeRegistry` is owned by this `ChainQa`
        // value — chain SSE handlers reach into it via
        // `state.chain_qa.runtime_registry()` so subscribers and publishers go
        // through one registry.
        let chain_qa = ChainQa::new(db.clone(), llm_registry.clone(), message_retriever.clone());
        let codex_login = codex_login::CodexLoginManager::new();
        // Bind sessions to the configured password so rotating it invalidates
        // them. Empty when auth is disabled (password None) — never consulted,
        // since the auth middleware short-circuits in that mode.
        let session_password_fingerprint = password
            .as_deref()
            .map(auth::password_fingerprint)
            .unwrap_or_default();
        let sessions = auth::SessionStore::new(db.clone(), session_password_fingerprint);
        Self {
            runtime,
            llm_registry,
            db,
            platform,
            mcp_manager,
            credential_helper,
            password,
            sessions,
            login_throttle: auth::LoginThrottle::new(),
            terminals,
            chain_qa,
            message_retriever,
            codex_login,
            deployment,
            runtime_env,
            suggest_token,
        }
    }
}
