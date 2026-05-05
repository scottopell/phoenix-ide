//! Phoenix IDE - LLM-powered development environment
//!
//! A Rust backend implementing a conversation state machine for
//! interacting with LLM agents.

mod api;
mod chain_qa;
mod chain_runtime;
mod db;
pub(crate) mod git_ops;
mod llm;
mod message_expander;
mod platform;
mod runtime;
pub mod skills;
mod state_machine;
mod system_prompt;
mod terminal;
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
#[allow(clippy::too_many_lines)] // Startup sequence is inherently sequential; splitting would obscure the flow.
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

    hot_restart::record_start_time();

    // REQ-BASH-007: install the child subreaper so descendants whose
    // parent dies (double-forks, setsid daemons) reparent to Phoenix
    // rather than init. Must run before any tool spawns a child.
    crate::tools::bash::install_reaper();

    // Log startup context: binary path, version, and whether this looks like a deploy
    let exe_path =
        std::env::current_exe().map_or_else(|_| "unknown".to_string(), |p| p.display().to_string());
    let is_prod = std::env::var("PHOENIX_DB_PATH")
        .ok()
        .is_some_and(|p| p.contains("prod"));
    tracing::info!(
        exe = %exe_path,
        pid = std::process::id(),
        mode = if is_prod { "production" } else { "development" },
        "Phoenix IDE starting"
    );

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

    // Run pending data migrations before anything reads conversation data
    db::run_pending_migrations(db.pool()).await?;

    // Reset all conversations to idle on startup (REQ-BED-007)
    db.reset_all_to_idle().await?;

    // Reconcile worktrees: revert Work conversations whose worktree is missing
    reconcile_worktrees(&db).await;

    // REQ-CHN-005 startup sweep: any chain_qa row left in_flight from a
    // previous process has no live stream behind it; flip it to abandoned
    // so the UI shows a re-ask affordance instead of an indefinite spinner.
    match db.sweep_in_flight_chain_qa().await {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            count = n,
            "Swept stale in_flight chain_qa rows to abandoned"
        ),
        Err(e) => tracing::warn!(error = %e, "chain_qa startup sweep failed"),
    }

    // Initialize LLM registry with model discovery
    let llm_config = LlmConfig::from_env();
    let credential_helper = llm_config.credential_helper.clone();
    let llm_registry = Arc::new(ModelRegistry::new_with_discovery(&llm_config).await);

    if llm_registry.has_models() {
        tracing::info!(
            models = %llm_registry.available_models().join(", "),
            default = %llm_registry.default_model_id(),
            "LLM registry initialized"
        );
    } else {
        tracing::warn!("No LLM API keys configured. Set ANTHROPIC_API_KEY, LLM_GATEWAY, or LLM_API_KEY_HELPER.");
    }

    // Detect platform sandboxing capability (REQ-PROJ-013)
    let platform = crate::platform::PlatformCapability::detect();
    tracing::info!(?platform, "Platform capability detected");

    // REQ-TMUX-003 / REQ-TMUX-004: log tmux binary availability so
    // operators can correlate "in-app terminal runs $SHELL not tmux"
    // with the host PATH at startup. The registry inside RuntimeManager
    // re-runs the same probe and caches it; this is purely an
    // operational breadcrumb.
    if which::which("tmux").is_ok() {
        tracing::info!("tmux binary detected on PATH; in-app terminals will attach to per-conversation tmux sessions");
        // Best-effort version probe: warn if below 3.3 (Phoenix's
        // declared minimum). 3.3 is the floor because tmux 3.2's
        // send-keys argument parser emits chatty "no current client"
        // and "not in a mode" diagnostics for client-less servers,
        // which agents misinterpret as failures even though the keys
        // do reach the pane. tmux 3.3 reworked send-keys to not need
        // a client at all.
        if let Ok(out) = std::process::Command::new("tmux").arg("-V").output() {
            let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
            tracing::info!(version = %v, "tmux version");
            // Parse "tmux M.m" / "tmux M.ma" — minimum: 3.3.
            if let Some(rest) = v.strip_prefix("tmux ") {
                let digits: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect();
                let mut parts = digits.split('.').filter_map(|s| s.parse::<u32>().ok());
                if let (Some(major), Some(minor)) = (parts.next(), parts.next()) {
                    if (major, minor) < (3, 3) {
                        tracing::warn!(
                            version = %v,
                            "tmux version below Phoenix's declared minimum (3.3); send-keys and other client-context commands may emit benign \"no current client\" warnings that agents misread as failures. Upgrade to tmux 3.3+."
                        );
                    }
                }
            }
        }
    } else {
        tracing::info!(
            "tmux binary not found on PATH; in-app terminals will spawn $SHELL directly"
        );
    }

    // Create MCP manager and start background server discovery (non-blocking).
    // Servers connect in parallel; tools become available as each finishes.
    let mcp_manager = Arc::new(crate::tools::mcp::McpClientManager::new());

    // Load persisted disabled-server set before discovery starts.
    let disabled = db.get_disabled_mcp_servers().await.unwrap_or_default();
    if !disabled.is_empty() {
        tracing::info!(count = disabled.len(), servers = ?disabled, "Loaded disabled MCP servers from DB");
    }
    mcp_manager.set_disabled_servers(disabled).await;

    mcp_manager.start_background_discovery();

    // Read optional auth password (REQ-AUTH-001)
    let password = std::env::var("PHOENIX_PASSWORD")
        .ok()
        .filter(|p| !p.is_empty());
    if password.is_some() {
        tracing::info!("Password authentication enabled (PHOENIX_PASSWORD is set)");
    }

    // Create application state
    let state = AppState::new(
        db,
        llm_registry,
        platform,
        mcp_manager,
        credential_helper,
        password,
    )
    .await;

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

    // Hold an Arc to the bash handle registry so the shutdown kill-tree
    // pass (REQ-BASH-007) can reach it after `state` moves into the router.
    let bash_handles_for_shutdown = state.runtime.bash_handles().clone();

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

    // REQ-BASH-007: after the server stops accepting requests, walk the
    // live bash handle table and SIGKILL every process group as a final
    // cleanup pass before we relinquish control to the OS. Bounded by
    // SHUTDOWN_KILL_GRACE_SECONDS so a stuck D-state child cannot delay
    // shutdown indefinitely.
    crate::tools::bash::shutdown_kill_tree(&bash_handles_for_shutdown).await;

    // After graceful shutdown, check if we should hot restart
    // (This does not return if hot restart is performed)
    hot_restart::maybe_perform_hot_restart();

    Ok(())
}

/// Reconcile Work/Branch conversations whose worktree has been deleted or whose
/// `worktree_path` is empty (legacy rows predating M3).
///
/// For each affected conversation: revert mode (Work -> Explore, Branch -> Direct),
/// reset cwd to the project root, and run `git worktree prune` to clean stale
/// worktree bookkeeping.
///
/// REQ-BED-031 / REQ-PROJ-015 gate: skip `ContextExhausted` rows and rows whose
/// `continued_in_conv_id` is set. Their worktree is intentionally preserved
/// pending a user action (Continue / Abandon / `MarkAsMerged`) or already
/// transferred to a continuation — not a genuine orphan.
async fn reconcile_worktrees(db: &Database) {
    let work_convs = match db.get_work_conversations().await {
        Ok(convs) => convs,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to query Work/Branch conversations for reconciliation");
            return;
        }
    };

    let mut pruned_roots: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut reverted = 0usize;

    for conv in &work_convs {
        // REQ-BED-031: context-exhausted conversations own their worktree
        // until the user acts. Don't compound a missing-on-disk anomaly by
        // demoting — leave the row alone so Continue / Abandon / MarkAsMerged
        // remain structurally available.
        if matches!(conv.state, db::ConvState::ContextExhausted { .. }) {
            continue;
        }
        // REQ-BED-030: once a parent has handed ownership to a continuation,
        // its `worktree_path` is a history reference. The continuation owns
        // the on-disk directory; the parent row is reconciled via the
        // continuation's own record.
        if conv.continued_in_conv_id.is_some() {
            continue;
        }

        let wt_path = conv.conv_mode.worktree_path().unwrap_or("");
        let base_branch = conv.conv_mode.base_branch().unwrap_or("");

        let is_sentinel = |s: &str| s.is_empty() || s.starts_with("__LEGACY");

        // Legacy row (sentinel worktree_path or base_branch) or worktree directory missing on disk
        let needs_revert = is_sentinel(wt_path)
            || is_sentinel(base_branch)
            || !std::path::Path::new(wt_path).exists();

        if !needs_revert {
            continue;
        }

        let reason = if is_sentinel(wt_path) {
            "legacy row (missing worktree_path)"
        } else if is_sentinel(base_branch) {
            "legacy row (missing base_branch)"
        } else {
            "worktree directory missing"
        };

        // Branch mode reverts to Direct (no Explore phase to fall back to).
        // Work mode reverts to Explore (Managed workflow fallback).
        let is_branch = matches!(conv.conv_mode, db::ConvMode::Branch { .. });
        let revert_label = if is_branch { "Direct" } else { "Explore" };
        let revert_mode = if is_branch {
            db::ConvMode::Direct
        } else {
            db::ConvMode::Explore
        };

        tracing::warn!(
            conv_id = %conv.id,
            worktree_path = wt_path,
            reason,
            revert_to = revert_label,
            "Reverting worktree conversation"
        );

        if let Err(e) = db.update_conversation_mode(&conv.id, &revert_mode).await {
            tracing::error!(conv_id = %conv.id, error = %e, "Failed to revert conv_mode");
            continue;
        }
        reverted += 1;

        // Derive project root from worktree path: {root}/.phoenix/worktrees/{id}
        // If worktree_path is empty, try to detect from the conversation's current cwd
        let project_root = if is_sentinel(wt_path) {
            // wt_path is empty or a __LEGACY sentinel: no valid worktree path to
            // derive the repo root from, so run git rev-parse from conv.cwd instead.
            db::detect_git_repo_root(std::path::Path::new(&conv.cwd))
        } else {
            let root = crate::git_ops::repo_root_from_working_dir(std::path::Path::new(wt_path));
            Some(root.to_string_lossy().to_string())
        };

        if let Some(ref root) = project_root {
            // Allowed recovery mutation: worktree is gone, so reset cwd to
            // project root so the conversation loads into a valid directory.
            if let Err(e) = db.update_conversation_cwd(&conv.id, root).await {
                tracing::error!(conv_id = %conv.id, error = %e, "Failed to reset cwd");
            }

            // Prune stale worktrees in this project root (once per root)
            if pruned_roots.insert(root.clone()) {
                let root_path = std::path::PathBuf::from(root);
                if let Err(e) = std::process::Command::new("git")
                    .args(["worktree", "prune"])
                    .current_dir(&root_path)
                    .output()
                {
                    tracing::debug!(root = %root, error = %e, "git worktree prune failed");
                }
            }
        }
    }

    if reverted > 0 {
        tracing::info!(
            total_work = work_convs.len(),
            reverted,
            "Worktree reconciliation complete"
        );
    }
}

/// Reconcile tests — REQ-BED-031 gate behaviour (task 24696 Phase 3).
///
/// Exercises the three shapes of a Work conversation with a missing on-disk
/// worktree directory:
///   a) state = `ContextExhausted` -> skipped, mode preserved
///   b) `continued_in_conv_id` = Some -> skipped, mode preserved
///   c) neither (a) nor (b), genuine orphan -> demoted to Explore
///
/// These run against an on-disk `SQLite` DB (tempdir) so the project/
/// conversation foreign keys resolve correctly through migrations.
#[cfg(test)]
mod reconcile_worktrees_tests {
    use super::*;
    use crate::db::{ConvMode, ConvState, NonEmptyString};

    /// Initialise a git repo in a tempdir with one commit on main.
    fn init_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path().to_path_buf();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "commit",
                "--allow-empty",
                "-m",
                "init",
                "-q",
            ][..],
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        }
        (tmp, root)
    }

    /// Build a Work-mode `ConvMode` pointing at `{repo_root}/.phoenix/worktrees/{conv_id}`.
    /// The worktree directory does NOT have to exist (the caller decides whether
    /// to `git worktree add` it; for these tests we leave it missing to hit the
    /// "orphan" branch).
    fn work_mode_at(
        repo_root: &std::path::Path,
        conv_id: &str,
        branch: &str,
    ) -> (String, ConvMode) {
        let wt_path = repo_root
            .join(".phoenix")
            .join("worktrees")
            .join(conv_id)
            .to_string_lossy()
            .to_string();
        let mode = ConvMode::Work {
            branch_name: NonEmptyString::new(branch).unwrap(),
            worktree_path: NonEmptyString::new(&wt_path).unwrap(),
            base_branch: NonEmptyString::new("main").unwrap(),
            task_id: NonEmptyString::new("TK24696").unwrap(),
            task_title: NonEmptyString::new("Reconcile test").unwrap(),
        };
        (wt_path, mode)
    }

    /// Create a fresh in-memory database. `open_in_memory` runs both the
    /// baseline schema and the numbered migrations, mirroring production
    /// startup so tests can rely on columns added by later migrations
    /// (e.g. `continued_in_conv_id` added in task 24696 Phase 1).
    async fn fresh_db() -> db::Database {
        db::Database::open_in_memory().await.unwrap()
    }

    /// Helper: insert a Work conversation with the given `ConvMode`, then
    /// return its id. Caller tweaks `state` / `continued_in_conv_id` after.
    async fn seed_work_conv(
        db: &db::Database,
        id: &str,
        slug: &str,
        cwd: &str,
        mode: &ConvMode,
        project_id: &str,
    ) {
        db.create_conversation_with_project(
            id,
            slug,
            cwd,
            true,
            None,
            Some("claude-opus-test"),
            Some(project_id),
            mode,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    }

    /// Case (a): parent reached `ContextExhausted`. Worktree directory is
    /// missing on disk but the row's state is `ContextExhausted` — reconcile
    /// must SKIP it. Mode stays Work, cwd stays the worktree path.
    #[tokio::test]
    async fn skips_context_exhausted_conv_with_missing_worktree() {
        let (_git_tmp, repo_root) = init_repo();
        let db = fresh_db().await;
        let project = db
            .find_or_create_project(repo_root.to_str().unwrap())
            .await
            .unwrap();

        let conv_id = "case-a-exhausted";
        let (wt_path, mode) = work_mode_at(&repo_root, conv_id, "task-24696-a");
        seed_work_conv(&db, conv_id, conv_id, &wt_path, &mode, &project.id).await;

        // Force-set state to ContextExhausted.
        db.update_conversation_state(
            conv_id,
            &ConvState::ContextExhausted {
                summary: "exhausted".into(),
            },
        )
        .await
        .unwrap();

        // worktree dir was never created — this is the "missing on disk" signal.
        assert!(!std::path::Path::new(&wt_path).exists());

        reconcile_worktrees(&db).await;

        let after = db.get_conversation(conv_id).await.unwrap();
        assert!(
            matches!(after.conv_mode, ConvMode::Work { .. }),
            "REQ-BED-031: context-exhausted Work conv must NOT be demoted"
        );
        assert_eq!(
            after.conv_mode.worktree_path(),
            Some(wt_path.as_str()),
            "worktree_path must be preserved untouched"
        );
        assert_eq!(after.cwd, wt_path, "cwd must NOT be reset to project root");
    }

    /// Case (b): parent has already transferred ownership via
    /// `continued_in_conv_id`. Its `worktree_path` is a history reference;
    /// the continuation owns the on-disk directory. Reconcile must SKIP the
    /// parent row even when its path is missing.
    #[tokio::test]
    async fn skips_conv_with_continued_in_conv_id_and_missing_worktree() {
        let (_git_tmp, repo_root) = init_repo();
        let db = fresh_db().await;
        let project = db
            .find_or_create_project(repo_root.to_str().unwrap())
            .await
            .unwrap();

        let parent_id = "case-b-parent";
        let child_id = "case-b-child";
        let (wt_path, mode) = work_mode_at(&repo_root, parent_id, "task-24696-b");
        seed_work_conv(&db, parent_id, parent_id, &wt_path, &mode, &project.id).await;
        // Child is just a marker row — reconcile only reads the parent's
        // `continued_in_conv_id`, not the child itself.
        seed_work_conv(&db, child_id, child_id, &wt_path, &mode, &project.id).await;

        // Set parent.continued_in_conv_id = child_id via raw SQL.
        // Exposed API `continue_conversation` also updates in a transaction,
        // but we want to isolate the reconcile behaviour without running the
        // full continuation pipeline (and without needing an active runtime).
        sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
            .bind(child_id)
            .bind(parent_id)
            .execute(db.pool())
            .await
            .unwrap();

        assert!(!std::path::Path::new(&wt_path).exists());

        reconcile_worktrees(&db).await;

        let parent_after = db.get_conversation(parent_id).await.unwrap();
        assert!(
            matches!(parent_after.conv_mode, ConvMode::Work { .. }),
            "REQ-BED-030: parent with continued_in_conv_id set must NOT be demoted"
        );
        assert_eq!(
            parent_after.conv_mode.worktree_path(),
            Some(wt_path.as_str())
        );
        assert_eq!(parent_after.cwd, wt_path);
    }

    /// Case (c): genuine orphan — missing worktree, not exhausted, no
    /// continuation. The existing demotion path fires: Work -> Explore,
    /// cwd resets to project root.
    #[tokio::test]
    async fn demotes_genuine_orphan_to_explore() {
        let (_git_tmp, repo_root) = init_repo();
        let db = fresh_db().await;
        let project = db
            .find_or_create_project(repo_root.to_str().unwrap())
            .await
            .unwrap();

        let conv_id = "case-c-orphan";
        let (wt_path, mode) = work_mode_at(&repo_root, conv_id, "task-24696-c");
        seed_work_conv(&db, conv_id, conv_id, &wt_path, &mode, &project.id).await;

        // Default state after create is Idle; no continued_in_conv_id set.
        // wt_path dir missing on disk.
        assert!(!std::path::Path::new(&wt_path).exists());

        reconcile_worktrees(&db).await;

        let after = db.get_conversation(conv_id).await.unwrap();
        assert!(
            matches!(after.conv_mode, ConvMode::Explore),
            "genuine orphan (Idle, no continuation) demotes to Explore"
        );
        assert_eq!(
            after.cwd,
            repo_root.to_string_lossy(),
            "cwd resets to project root on demotion"
        );
    }
}
