//! One-shot production conversation runtime driver.

use crate::runtime::RuntimeManager;
use crate::state_machine::{ConvState, Event};
use phoenix_core::domain::db_schema::{ConvMode, Message, MessageType};
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use phoenix_db::Database;
use phoenix_llm::{LlmConfig, ModelRegistry};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

/// `SQLite` storage for one driven turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseMode {
    /// A one-connection database that disappears when the process exits.
    Memory,
    /// A unique file in the operating system temp directory, retained for inspection.
    TemporaryFile,
    /// A caller-owned database file retained after the process exits.
    File(PathBuf),
}

/// Input for one production conversation turn.
#[derive(Debug, Clone)]
pub struct DriveTurnRequest {
    pub cwd: PathBuf,
    pub model: String,
    pub prompt: String,
    pub database: DatabaseMode,
    pub timeout: Duration,
}

/// Stable boundary reached after the driven user message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StableOutcome {
    Idle,
    Error {
        message: String,
        error_kind: phoenix_core::domain::db_schema::ErrorKind,
    },
    AwaitingTaskApproval,
    AwaitingUserResponse,
    AwaitingCommissionReviewApproval,
    ContextExhausted,
    HandedOff,
    Terminal,
}

/// Raw evidence produced by one driven turn.
#[derive(Debug, Serialize)]
pub struct DriveTurnResult {
    pub conversation_id: String,
    pub git_sha: String,
    pub model: String,
    pub database: DatabaseResult,
    pub outcome: StableOutcome,
    pub elapsed_ms: u128,
    pub messages: Vec<Message>,
}

/// Database lifetime recorded in the output without inventing a path for memory mode.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DatabaseResult {
    Memory,
    File { path: PathBuf },
}

#[derive(Debug, Error)]
pub enum DriveTurnError {
    #[error("working directory is invalid: {0}")]
    InvalidWorkingDirectory(String),
    #[error("model '{requested}' is unavailable; available models: {available}")]
    UnknownModel {
        requested: String,
        available: String,
    },
    #[error("database initialization failed: {0}")]
    Database(String),
    #[error("runtime initialization failed: {0}")]
    Runtime(String),
    #[error("turn timed out after {0:?}")]
    Timeout(Duration),
}

impl DriveTurnRequest {
    /// Validate path, prompt, model, and timeout before any external request runs.
    fn validate(&self) -> Result<PathBuf, DriveTurnError> {
        if self.prompt.trim().is_empty() {
            return Err(DriveTurnError::Runtime("prompt must not be empty".into()));
        }
        if self.timeout.is_zero() {
            return Err(DriveTurnError::Runtime(
                "timeout must be greater than zero".into(),
            ));
        }
        crate::conversation_cwd::validate_conversation_cwd(
            self.cwd
                .to_str()
                .ok_or_else(|| DriveTurnError::InvalidWorkingDirectory("not UTF-8".into()))?,
        )
        .map(crate::conversation_cwd::ValidConversationCwd::into_raw)
        .map(PathBuf::from)
        .map_err(|error| DriveTurnError::InvalidWorkingDirectory(error.to_string()))
    }
}

fn extract_builtin_skills(runtime_env: &PhoenixRuntimeEnvironment) {
    let skills_dir = runtime_env.builtin_skills_dir();
    if let Err(error) = crate::skills::builtin::extract_to(&skills_dir) {
        tracing::warn!(error = %error, "Failed to extract built-in skills");
    }
}

async fn project_id_for_cwd(
    db: &Database,
    cwd: &std::path::Path,
) -> Result<Option<String>, DriveTurnError> {
    let Some(repo_root) = phoenix_core::git::detect_git_repo_root(cwd) else {
        return Ok(None);
    };
    db.find_or_create_project(&repo_root)
        .await
        .map(|project| Some(project.id))
        .map_err(|error| DriveTurnError::Database(error.to_string()))
}

async fn configure_mcp_manager(db: &Database) -> Arc<crate::tools::mcp::McpClientManager> {
    let manager = Arc::new(crate::tools::mcp::McpClientManager::new());
    manager.set_oauth_store(Arc::new(crate::mcp_oauth_store::DbOAuthStore::new(
        db.clone(),
    )));
    match db.get_disabled_mcp_servers().await {
        Ok(disabled) => manager.set_disabled_servers(disabled).await,
        Err(error) => tracing::warn!(error = %error, "Failed to load disabled MCP servers"),
    }
    manager.start_background_discovery();
    manager
}

/// Drive one user turn through the same runtime, provider adapters, and tool
/// registry used by the Phoenix server.
///
/// # Errors
///
/// Returns an error if validation/bootstrap fails or the runtime does not reach
/// a stable state within the requested timeout.
pub async fn run(request: DriveTurnRequest) -> Result<DriveTurnResult, DriveTurnError> {
    let cwd = request.validate()?;
    install_crypto_provider();
    crate::tools::bash::install_reaper();

    let started = std::time::Instant::now();
    let runtime_env = Arc::new(PhoenixRuntimeEnvironment::detect());
    extract_builtin_skills(&runtime_env);
    let (db, database_result) = open_database(&request.database, &runtime_env).await?;
    crate::reconcile_project_main_refs(&db).await;

    let llm_config = LlmConfig::from_env(runtime_env);
    let credential_helper = llm_config.credential_helper.clone();
    let llm_registry = Arc::new(ModelRegistry::new_with_discovery(&llm_config).await);
    if llm_registry.get(&request.model).is_none() {
        return Err(DriveTurnError::UnknownModel {
            requested: request.model,
            available: llm_registry.available_models().join(", "),
        });
    }

    let project_id = project_id_for_cwd(&db, &cwd).await?;
    let default_language = db.get_default_llm_language().await.unwrap_or_else(|error| {
        tracing::warn!(
            error = %error,
            "Failed to read default LLM language; falling back to default"
        );
        phoenix_core::llm_language::LlmLanguage::default()
    });
    let conversation_id = uuid::Uuid::new_v4().to_string();
    db.create_conversation_with_project(
        &conversation_id,
        "drive-turn",
        cwd.to_string_lossy().as_ref(),
        true,
        None,
        Some(&request.model),
        project_id.as_deref(),
        &ConvMode::Direct,
        None,
        None,
        None,
        default_language,
    )
    .await
    .map_err(|error| DriveTurnError::Database(error.to_string()))?;

    let mcp_manager = configure_mcp_manager(&db).await;
    let manager = Arc::new(RuntimeManager::new(
        db.clone(),
        llm_registry,
        crate::platform::PlatformCapability::detect(),
        mcp_manager,
        credential_helper,
    ));
    manager.start_sub_agent_handler().await;

    let result = drive_conversation(
        &request,
        &db,
        &database_result,
        &manager,
        &conversation_id,
        started,
    )
    .await;
    manager.browser_sessions().shutdown_all().await;
    crate::tools::bash::shutdown_kill_tree(manager.bash_handles()).await;
    result
}

async fn drive_conversation(
    request: &DriveTurnRequest,
    db: &Database,
    database_result: &DatabaseResult,
    manager: &Arc<RuntimeManager>,
    conversation_id: &str,
    started: std::time::Instant,
) -> Result<DriveTurnResult, DriveTurnError> {
    let mut state_rx = manager
        .subscribe_state(conversation_id)
        .await
        .map_err(DriveTurnError::Runtime)?;
    let message_id = uuid::Uuid::new_v4().to_string();
    manager
        .send_event(
            conversation_id,
            Event::UserMessage {
                text: request.prompt.clone(),
                llm_text: None,
                images: Vec::new(),
                files: Vec::new(),
                message_id,
                user_agent: Some("drive-turn".into()),
                skill_invocation: None,
            },
        )
        .await
        .map_err(DriveTurnError::Runtime)?;

    let outcome = if let Ok(result) = tokio::time::timeout(
        request.timeout,
        wait_for_stable_turn(db, conversation_id, &mut state_rx),
    )
    .await
    {
        result?
    } else {
        cancel_timed_out_turn(manager, conversation_id, &mut state_rx).await?;
        return Err(DriveTurnError::Timeout(request.timeout));
    };
    let messages = db
        .get_messages(conversation_id)
        .await
        .map_err(|error| DriveTurnError::Database(error.to_string()))?;

    Ok(DriveTurnResult {
        conversation_id: conversation_id.to_string(),
        git_sha: option_env!("PHOENIX_GIT_SHA")
            .unwrap_or("unknown")
            .to_string(),
        model: request.model.clone(),
        database: database_result.clone(),
        outcome,
        elapsed_ms: started.elapsed().as_millis(),
        messages,
    })
}

async fn cancel_timed_out_turn(
    manager: &Arc<RuntimeManager>,
    conversation_id: &str,
    state_rx: &mut tokio::sync::watch::Receiver<ConvState>,
) -> Result<(), DriveTurnError> {
    state_rx.borrow_and_update();
    manager
        .send_event(
            conversation_id,
            Event::UserCancel {
                reason: Some("drive-turn timeout".into()),
                cause: phoenix_core::domain::sm_event::CancelCause::Timeout,
            },
        )
        .await
        .map_err(|error| {
            DriveTurnError::Runtime(format!("timeout cancellation failed: {error}"))
        })?;

    tokio::time::timeout(
        Duration::from_secs(10),
        wait_for_post_cancel_stable_state(state_rx),
    )
    .await
    .map_err(|_| {
        DriveTurnError::Runtime(
            "timed-out turn did not reach a stable state after cancellation".into(),
        )
    })??;
    Ok(())
}

async fn open_database(
    mode: &DatabaseMode,
    runtime_env: &PhoenixRuntimeEnvironment,
) -> Result<(Database, DatabaseResult), DriveTurnError> {
    match mode {
        DatabaseMode::Memory => Database::open_in_memory()
            .await
            .map(|db| (db, DatabaseResult::Memory))
            .map_err(|error| DriveTurnError::Database(error.to_string())),
        DatabaseMode::TemporaryFile => {
            let directory = runtime_env
                .tmp_subdir("drive-turn")
                .map_err(|error| DriveTurnError::Database(error.to_string()))?;
            let path = directory.join(format!("{}.db", uuid::Uuid::new_v4()));
            open_file_database(path).await
        }
        DatabaseMode::File(path) => open_file_database(path.clone()).await,
    }
}

fn parent_directory(path: &std::path::Path) -> Option<&std::path::Path> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
}

async fn open_file_database(path: PathBuf) -> Result<(Database, DatabaseResult), DriveTurnError> {
    if let Some(parent) = parent_directory(&path) {
        std::fs::create_dir_all(parent)
            .map_err(|error| DriveTurnError::Database(error.to_string()))?;
    }
    let db = Database::open(path.to_string_lossy().as_ref())
        .await
        .map_err(|error| DriveTurnError::Database(error.to_string()))?;
    phoenix_db::run_pending_migrations(db.pool())
        .await
        .map_err(|error| DriveTurnError::Database(error.to_string()))?;
    db.restrict_file_permissions();
    Ok((db, DatabaseResult::File { path }))
}

async fn wait_for_stable_turn(
    db: &Database,
    conversation_id: &str,
    state_rx: &mut tokio::sync::watch::Receiver<ConvState>,
) -> Result<StableOutcome, DriveTurnError> {
    loop {
        let state = state_rx.borrow().clone();
        if let Some(outcome) = stable_outcome(&state) {
            let messages = db
                .get_messages(conversation_id)
                .await
                .map_err(|error| DriveTurnError::Database(error.to_string()))?;
            let has_agent_output = messages
                .iter()
                .any(|message| message.message_type == MessageType::Agent);
            if has_agent_output || !matches!(outcome, StableOutcome::Idle) {
                return Ok(outcome);
            }
        }
        state_rx
            .changed()
            .await
            .map_err(|_| DriveTurnError::Runtime("runtime state channel closed".into()))?;
    }
}

async fn wait_for_post_cancel_stable_state(
    state_rx: &mut tokio::sync::watch::Receiver<ConvState>,
) -> Result<StableOutcome, DriveTurnError> {
    state_rx
        .changed()
        .await
        .map_err(|_| DriveTurnError::Runtime("runtime state channel closed".into()))?;
    loop {
        if let Some(outcome) = stable_outcome(&state_rx.borrow()) {
            return Ok(outcome);
        }
        state_rx
            .changed()
            .await
            .map_err(|_| DriveTurnError::Runtime("runtime state channel closed".into()))?;
    }
}

fn stable_outcome(state: &ConvState) -> Option<StableOutcome> {
    match state {
        ConvState::Idle => Some(StableOutcome::Idle),
        ConvState::Error {
            message,
            error_kind,
            ..
        } => Some(StableOutcome::Error {
            message: message.clone(),
            error_kind: error_kind.clone(),
        }),
        ConvState::AwaitingTaskApproval { .. } => Some(StableOutcome::AwaitingTaskApproval),
        ConvState::AwaitingUserResponse { .. } => Some(StableOutcome::AwaitingUserResponse),
        ConvState::AwaitingCommissionReviewApproval { .. } => {
            Some(StableOutcome::AwaitingCommissionReviewApproval)
        }
        ConvState::ContextExhausted { .. } => Some(StableOutcome::ContextExhausted),
        ConvState::HandedOff { .. } => Some(StableOutcome::HandedOff),
        ConvState::Terminal => Some(StableOutcome::Terminal),
        ConvState::LlmRequesting { .. }
        | ConvState::SeededLlmRequesting { .. }
        | ConvState::ToolExecuting { .. }
        | ConvState::CancellingTool { .. }
        | ConvState::AwaitingSubAgents { .. }
        | ConvState::CancellingSubAgents { .. }
        | ConvState::Completed { .. }
        | ConvState::Failed { .. }
        | ConvState::AwaitingRecovery { .. }
        | ConvState::AwaitingContinuation { .. } => None,
    }
}

fn install_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_database_filename_has_no_directory_to_create() {
        assert_eq!(parent_directory(std::path::Path::new("results.db")), None);
        assert_eq!(
            parent_directory(std::path::Path::new("results/run.db")),
            Some(std::path::Path::new("results"))
        );
    }

    #[test]
    fn transient_states_are_not_stable() {
        assert_eq!(
            stable_outcome(&ConvState::LlmRequesting { attempt: 1 }),
            None
        );
    }

    #[tokio::test]
    async fn post_cancel_waiter_requires_a_new_state_observation() {
        let (tx, mut rx) = tokio::sync::watch::channel(ConvState::Idle);
        rx.borrow_and_update();
        let waiter = tokio::spawn(async move { wait_for_post_cancel_stable_state(&mut rx).await });

        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        tx.send(ConvState::Terminal).unwrap();

        assert_eq!(waiter.await.unwrap().unwrap(), StableOutcome::Terminal);
    }

    #[test]
    fn error_state_preserves_typed_failure() {
        let state = ConvState::Error {
            message: "failed".into(),
            error_kind: phoenix_core::domain::db_schema::ErrorKind::InvalidRequest,
            resets_at: None,
        };
        assert!(matches!(
            stable_outcome(&state),
            Some(StableOutcome::Error { message, .. }) if message == "failed"
        ));
    }
}
