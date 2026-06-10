//! Phoenix IDE - LLM-powered development environment
//!
//! A Rust backend implementing a conversation state machine for
//! interacting with LLM agents.

mod api;
mod chain_qa;
mod chain_runtime;
pub(crate) mod git_ops;
mod llm;
mod message_expander;
mod resolution_root;
mod runtime;
mod system_prompt;
mod title_generator;
mod tls;

// Domain-vocabulary leaves now live in the acyclic `phoenix-core` base crate.
// Re-export them at their historical crate-root paths so existing
// `crate::llm_language::…` / `crate::task_source::…` call sites resolve
// unchanged (move-down, re-export-up).
use phoenix_core::{llm_language, platform, task_source, work_scope};

// Terminal-core (PTY spawn, relay, command tracking, session registry) now
// lives in the `phoenix-terminal` crate. Re-export it at its historical
// `crate::terminal::…` path so existing call sites resolve unchanged
// (move-down, re-export-up). The axum/WebSocket glue stayed behind in
// `api::terminal_ws`.
use phoenix_terminal as terminal;

// The pure conversation reducer now lives in the `phoenix-state-machine`
// crate. Re-export it at its historical `crate::state_machine::…` path so
// existing call sites resolve unchanged (move-down, re-export-up).
use phoenix_state_machine as state_machine;

// Skill discovery, metadata, and invocation now live in the `phoenix-skills`
// crate (so the `phoenix-tools` crate can call it without depending on
// phoenix-ide). Re-export at the historical `crate::skills::…` path so existing
// call sites resolve unchanged (move-down, re-export-up).
pub use phoenix_skills as skills;

// Tool implementations (bash, patch, browser, tmux, search, …) now live in the
// `phoenix-tools` crate. Re-export at the historical `crate::tools::…` path so
// existing call sites (runtime executor, api handlers, browser_view, the
// shutdown kill-tree pass) resolve unchanged (move-down, re-export-up). The
// axum/HTTP glue that drives tools stayed behind in `api`.
use phoenix_tools as tools;

// SQLite persistence (conversations, messages, steering queues, migrations)
// now lives in the `phoenix-db` crate, a leaf depending only on phoenix-core.
// Re-export at the historical `crate::db::…` path so existing call sites
// resolve unchanged (move-down, re-export-up).
use phoenix_db as db;

use api::{create_router, AppState};
use db::Database;
use llm::{LlmConfig, ModelRegistry};
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
mod hot_restart;
mod logging;

/// Assemble the static deployment facts reported by `GET /api/deployment`.
/// Resolves every path from the same logic the rest of the process uses so the
/// page reports the locations the process actually opens (specs/deployment-info/).
fn build_deployment_config(
    bind_address: SocketAddr,
    db_path: &str,
    tls_source: Option<&tls::ConfigSource>,
    loaded_tls: Option<&tls::LoadedConfig>,
    log: api::LogInfo,
) -> api::DeploymentConfig {
    use api::TlsInfo;

    let tls = match (tls_source, loaded_tls) {
        (Some(source), Some(loaded)) => {
            let hosts = match source {
                tls::ConfigSource::Auto { hosts, .. } => hosts.clone(),
                tls::ConfigSource::Manual(_) => Vec::new(),
            };
            TlsInfo {
                enabled: true,
                mode: Some(loaded.mode.to_string()),
                cert_path: Some(api::absolutize(&loaded.cert_path).display().to_string()),
                key_path: Some(api::absolutize(&loaded.key_path).display().to_string()),
                ca_cert_path: loaded
                    .ca_cert_path
                    .as_ref()
                    .map(|p| api::absolutize(p).display().to_string()),
                hosts,
            }
        }
        _ => TlsInfo::disabled(),
    };

    let locations = build_disk_locations(db_path, tls_source, loaded_tls);

    api::DeploymentConfig {
        bind_address,
        tls,
        log,
        locations,
    }
}

/// Build the on-disk location rows reported by `GET /api/deployment`, each with
/// its sizing policy. Every path is normalized to absolute.
fn build_disk_locations(
    db_path: &str,
    tls_source: Option<&tls::ConfigSource>,
    loaded_tls: Option<&tls::LoadedConfig>,
) -> Vec<api::DiskLocation> {
    use api::{DiskLocation, MeasureMode};

    let db_pb = api::absolutize(&PathBuf::from(db_path));
    let data_dir = db_pb
        .parent()
        .map_or_else(|| db_pb.clone(), std::path::Path::to_path_buf);

    // Only recurse the data directory when it is a Phoenix-owned dedicated dir
    // (`.phoenix-ide` for user installs / dev worktrees, `phoenix-ide` for the
    // native `/var/lib/phoenix-ide` production root). A custom PHOENIX_DB_PATH
    // like `/tmp/phoenix.db` or `$HOME/phoenix.db` would otherwise make every
    // request walk all of `/tmp` or the home directory — the opposite of a
    // cheap diagnostic snapshot.
    let owned_data_dir = matches!(
        data_dir.file_name().and_then(std::ffi::OsStr::to_str),
        Some(".phoenix-ide" | "phoenix-ide")
    );
    let data_dir_mode = if owned_data_dir {
        MeasureMode::RecurseSmall
    } else {
        MeasureMode::NoMeasure
    };

    let mut locations = vec![
        DiskLocation {
            label: "Database".to_string(),
            path: db_pb.clone(),
            mode: MeasureMode::File,
        },
        DiskLocation {
            label: "Data directory".to_string(),
            path: data_dir,
            mode: data_dir_mode,
        },
    ];

    // TLS inputs the process reads on disk. Auto mode owns a small managed
    // directory (cert, key, CA); manual mode points at explicit cert/key files.
    match (tls_source, loaded_tls) {
        (Some(tls::ConfigSource::Auto { dir, .. }), _) => {
            locations.push(DiskLocation {
                label: "TLS directory".to_string(),
                path: dir.clone(),
                mode: MeasureMode::RecurseSmall,
            });
        }
        (Some(tls::ConfigSource::Manual(_)), Some(loaded)) => {
            locations.push(DiskLocation {
                label: "TLS certificate".to_string(),
                path: loaded.cert_path.clone(),
                mode: MeasureMode::File,
            });
            locations.push(DiskLocation {
                label: "TLS key".to_string(),
                path: loaded.key_path.clone(),
                mode: MeasureMode::File,
            });
        }
        _ => {}
    }

    if let Some(dir) = skills::builtin::default_extract_dir() {
        locations.push(DiskLocation {
            label: "Built-in skills".to_string(),
            path: dir,
            mode: MeasureMode::RecurseSmall,
        });
    }

    // The codex credential row is NOT built here: the active credential source
    // can change at runtime via the in-app login flow, so the handler resolves
    // and measures it per request (see `active_codex_credentials_location`).

    // Attachments are stored inline in the database. This row is the stable home
    // for the file-based attachment directory once that storage mode is active.
    locations.push(DiskLocation {
        label: "Attachments".to_string(),
        path: db_pb,
        mode: MeasureMode::InlineDb,
    });

    locations.push(DiskLocation {
        label: "Browser binary cache".to_string(),
        path: tools::browser::session::fetcher_cache_dir(),
        mode: MeasureMode::NoMeasure,
    });

    // Per-scope Chrome profiles created on demand while browser sessions are
    // active. A glob, not a single dir, and potentially large — reported as an
    // unsized pattern row.
    locations.push(DiskLocation {
        label: "Browser profiles".to_string(),
        path: PathBuf::from(tools::browser::session::user_data_dir_glob()),
        mode: MeasureMode::Pattern,
    });

    // Normalize every reported path to absolute. Env-derived paths
    // (PHOENIX_DB_PATH, manual TLS cert/key, PHOENIX_TLS_DIR) may be relative;
    // the wire contract specifies absolute `path` values so operators see where
    // bytes actually live, not `phoenix.db` or `.`.
    for loc in &mut locations {
        loc.path = api::absolutize(&loc.path);
    }

    locations
}

#[tokio::main]
#[allow(clippy::too_many_lines)] // Startup sequence is inherently sequential; splitting would obscure the flow.
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging from the configured sinks (PHOENIX_LOG_STDOUT /
    // PHOENIX_LOG_FILE). The guard must outlive the program so the file
    // appender's worker flushes on shutdown; held until `main` returns.
    let log_config = logging::LogConfig::from_env();
    let _log_guard = logging::init(&log_config)?;

    // Install a rustls crypto provider explicitly. rustls 0.23 refuses
    // to auto-pick when both `ring` and `aws-lc-rs` end up in the dep
    // tree — which happens here via reqwest (aws-lc-rs) + our direct
    // rustls = { features = ["ring"] } and several transitive consumers
    // (chromiumoxide, hyper-rustls). Without this call the first
    // `ServerConfig::builder()` call panics on startup.
    //
    // Install `aws_lc_rs` when no provider is already set — matches the
    // feature flag on our direct rustls dep.
    //
    // `install_default()` returns `Err(existing)` if a provider was
    // already installed earlier in the process — typically benign
    // (an upstream library may set one as an import side effect). We
    // accept whichever provider is in place rather than crashing on
    // that race; the panic we're trying to prevent only occurs when
    // NO provider is installed at all.
    if rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .is_err()
    {
        tracing::debug!(
            "rustls default crypto provider was already installed by another component; \
             keeping the existing provider"
        );
    }

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
    let tls_source = tls::ConfigSource::from_env(&db_path)?;

    // Ensure database directory exists
    if let Some(parent) = PathBuf::from(&db_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Extract built-in skills to disk so they participate in normal skill
    // discovery (filesystem-shadows-builtin override, companion-file reads,
    // etc.). Failure is non-fatal — built-ins simply won't appear in the
    // catalog and the user can still install filesystem skills.
    if let Some(target) = skills::builtin::default_extract_dir() {
        match skills::builtin::extract_to(&target) {
            Ok(()) => tracing::info!(path = %target.display(), "extracted built-in skills"),
            Err(e) => tracing::warn!(
                path = %target.display(),
                error = %e,
                "failed to extract built-in skills",
            ),
        }
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

    // Reconcile project main_ref to the resolved default branch (REQ-PROJ-034a):
    // rows whose main_ref was defaulted to a literal `main` are corrected before
    // forks (cut from main_ref) rely on them.
    reconcile_project_main_refs(&db).await;

    // Retire fork proposals stranded `pending` against a terminal origin
    // (REQ-PROJ-035): a crash after the origin went terminal but before its
    // proposals were retired leaves a Review action that can never spawn/promote.
    // This self-heals such rows to `dismissed` on restart.
    crate::runtime::fork_resolve::reconcile_terminal_origin_fork_proposals(&db).await;

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

    // Resolve TLS once so both the deployment report and the listener use the
    // same loaded config (avoids generating the auto cert twice).
    let loaded_tls = match &tls_source {
        Some(source) => Some(tls::load_config(source)?),
        None => None,
    };

    // Bind (or adopt the systemd socket-activated) listener now, so the
    // deployment report records the address the server is actually bound to.
    // Under socket activation PHOENIX_PORT is typically unset and the real
    // address comes from systemd, not the 0.0.0.0:PORT default.
    let fallback_addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = hot_restart::get_listener(fallback_addr).await?;
    let socket_activated = hot_restart::is_socket_activated();
    let bind_address = listener.local_addr().unwrap_or(fallback_addr);

    // Static deployment facts served read-only by GET /api/deployment
    // (specs/deployment-info/).
    let deployment = Arc::new(build_deployment_config(
        bind_address,
        &db_path,
        tls_source.as_ref(),
        loaded_tls.as_ref(),
        log_config.to_log_info(),
    ));

    // Create application state
    let state = AppState::new(
        db,
        llm_registry,
        platform,
        mcp_manager,
        credential_helper,
        password,
        deployment,
    )
    .await;

    // Create router
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let compression = CompressionLayer::new().gzip(true).br(true);

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

    // The listener was bound earlier so the deployment report could record the
    // real bind address (see above).
    if let Some(loaded_tls) = loaded_tls {
        tracing::info!(
            mode = loaded_tls.mode,
            cert = %loaded_tls.cert_path.display(),
            key = %loaded_tls.key_path.display(),
            ca = loaded_tls.ca_cert_path.as_ref().map(|p| p.display().to_string()),
            "TLS enabled"
        );
        tls::serve_https(listener, app, loaded_tls.server, socket_activated).await?;
    } else {
        tracing::info!(
            addr = %listener.local_addr()?,
            socket_activated,
            "Phoenix IDE server listening"
        );

        // Run the server with graceful shutdown on signals. The graceful
        // drain is bounded by `tls::bounded_post_shutdown_drain` — same
        // deadline as the HTTPS path, single source of truth. The bound
        // must start when the shutdown signal fires, not at startup — so
        // the server runs as a task, the signal is awaited separately,
        // and `drain_tx` forks that signal into axum's own
        // graceful-shutdown hook.
        let (drain_tx, drain_rx) = tokio::sync::oneshot::channel::<()>();
        let mut server = tokio::spawn(
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = drain_rx.await;
                })
                .into_future(),
        );
        let server_abort = server.abort_handle();

        tokio::select! {
            // The server task ends on its own only via a fatal accept error.
            joined = &mut server => joined??,
            () = hot_restart::shutdown_signal() => {
                let _ = drain_tx.send(());
                match tls::bounded_post_shutdown_drain(&mut server, "HTTP").await {
                    Some(joined) => joined??,
                    None => server_abort.abort(),
                }
            }
        }
    }

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

/// Reconcile Work/Branch conversations whose worktree has been deleted.
///
/// A worktree-bound conversation whose on-disk worktree has vanished is no
/// longer useful: it cannot run tools, it cannot complete its task, and
/// silently demoting it to Explore/Direct + resetting cwd to the project
/// root would mislead the user ("why is this conversation suddenly talking
/// about main?"). Instead, mark the row as Terminal and leave `conv_mode`,
/// `worktree_path`, and cwd untouched. The user keeps the history, sees the
/// original mode/branch metadata, and can hard-delete when ready.
///
/// Also runs `git worktree prune` once per affected project root to clean
/// stale worktree bookkeeping.
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
    let mut terminated = 0usize;

    for conv in &work_convs {
        // REQ-BED-031: context-exhausted conversations own their worktree
        // until the user acts. Don't compound a missing-on-disk anomaly by
        // marking terminal — leave the row alone so Continue / Abandon /
        // MarkAsMerged remain structurally available.
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
        // Already terminal — nothing to do (and we'd just be writing the
        // same state back). Prune still happens via the per-root dedup
        // below if any sibling conv triggers it.
        if matches!(conv.state, db::ConvState::Terminal) {
            continue;
        }

        // Migration 002 guarantees Work/Branch rows have non-empty, non-sentinel
        // worktree_path and base_branch. The only remaining reason to act is a
        // worktree directory that no longer exists on disk.
        let wt_path = match conv.conv_mode.worktree_path() {
            Some(p) if !p.is_empty() => p,
            _ => continue, // shouldn't happen post-M2; skip rather than corrupt
        };

        if std::path::Path::new(wt_path).exists() {
            continue;
        }

        tracing::warn!(
            conv_id = %conv.id,
            mode = conv.conv_mode.label(),
            worktree_path = wt_path,
            reason = "worktree directory missing",
            "Marking orphaned worktree conversation as Terminal"
        );

        if let Err(e) = db
            .update_conversation_state(&conv.id, &db::ConvState::Terminal)
            .await
        {
            tracing::error!(conv_id = %conv.id, error = %e, "Failed to mark orphan as Terminal");
            continue;
        }
        terminated += 1;

        // wt_path is always {root}/.phoenix/worktrees/{id} for a real Phoenix
        // worktree. If the strict predicate fails the row is malformed; skip
        // pruning for this entry but the Terminal mark is already applied.
        let project_root =
            crate::git_ops::repo_root_from_phoenix_worktree(std::path::Path::new(wt_path))
                .map(|p| p.to_string_lossy().to_string());

        if let Some(ref root) = project_root {
            // Prune stale worktrees in this project root (once per root).
            // Hygiene only — does not depend on or affect the conv row.
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

    if terminated > 0 {
        tracing::info!(
            total_work = work_convs.len(),
            terminated,
            "Worktree reconciliation complete"
        );
    }
}

/// REQ-PROJ-034a: backfill projects whose `main_ref` is the legacy literal
/// `"main"` to the resolved default branch (the fork base). `main_ref` is the
/// immutable resolved fork base; reconciliation exists ONLY to repair rows that
/// the old hardcoded `find_or_create_project` defaulted to `"main"` on repos
/// whose real default is `master`/`develop`/etc. Forks are cut from `main_ref`,
/// so a broken literal must be corrected before forks rely on it.
///
/// ONLY a row stored as `"main"` is ever modified. Any other stored value is a
/// deliberately-resolved base and is left untouched, regardless of what
/// `origin/HEAD` resolves to now — a project created when the default was
/// `develop` keeps cutting forks from `develop` even after the remote default
/// shifts to `main`. For a row stored as `"main"`:
///
/// - a remote default that differs (`schema::resolve_remote_default_branch`, the
///   cached `refs/remotes/origin/HEAD`) is the authoritative fork base — backfill;
/// - else, with no local `main` branch, the literal is broken and would fail fork
///   approval, so repair to the current checked-out branch
///   (`schema::resolve_default_branch`);
/// - else a real `main` branch exists with no remote signal — leave it.
///
/// Best-effort and idempotent: a project whose repo is missing or has a detached
/// HEAD with no resolvable branch is skipped — never fatal to startup — and a
/// re-run over already-correct rows writes nothing. This only updates a DB
/// string; it never moves a git ref, so the "owned environments" rule is not
/// engaged.
/// Whether `refs/heads/{branch}` exists in the repo at `repo_path`. Used by
/// `reconcile_project_main_refs` to decide whether a stored `main_ref` on a
/// no-remote repo is a valid (keep) or broken (repair) value.
fn local_branch_exists(repo_path: &std::path::Path, branch: &str) -> bool {
    std::process::Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(repo_path)
        .output()
        .is_ok_and(|out| out.status.success())
}

async fn reconcile_project_main_refs(db: &Database) {
    let projects = match db.list_projects().await {
        Ok(projects) => projects,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to list projects for main_ref reconciliation");
            return;
        }
    };

    let mut updated = 0usize;
    for project in &projects {
        // `main_ref` is the immutable resolved fork base (REQ-PROJ-034a).
        // Reconciliation exists ONLY to backfill rows that were defaulted to the
        // legacy literal `"main"` by the old hardcoded `find_or_create_project`.
        // Any other stored value is a deliberately-resolved base — never touched,
        // regardless of what `origin/HEAD` resolves to now (a project created when
        // the default was `develop`, stored `develop`, must keep cutting forks
        // from `develop` even after the remote default shifts to `main`).
        if project.main_ref != "main" {
            continue;
        }

        let repo_path = std::path::Path::new(&project.canonical_path);
        // A row stored as the legacy literal `main`. Repair it:
        // - if a remote default exists and differs, it is the authoritative fork
        //   base — backfill to it (a `master`/`develop` repo must not stay `main`);
        // - else if there is no local `main` branch, the literal is broken (would
        //   later fail fork approval), so repair to the current checked-out branch;
        // - else a real `main` branch exists with no remote signal — leave it.
        let new_main_ref = if let Some(remote_default) =
            db::resolve_remote_default_branch(repo_path)
        {
            remote_default
        } else {
            if local_branch_exists(repo_path, "main") {
                continue;
            }
            let Some(current) = phoenix_core::domain::db_schema::resolve_default_branch(repo_path)
            else {
                tracing::warn!(
                    project_id = %project.id,
                    canonical_path = %project.canonical_path,
                    stored_main_ref = %project.main_ref,
                    "skipping main_ref reconciliation: legacy literal `main` has no local `main` branch and no resolvable current branch (repo missing or detached HEAD)"
                );
                continue;
            };
            current
        };

        if new_main_ref == project.main_ref {
            continue;
        }

        match db.update_project_main_ref(&project.id, &new_main_ref).await {
            Ok(()) => {
                tracing::info!(
                    project_id = %project.id,
                    old_main_ref = %project.main_ref,
                    new_main_ref = %new_main_ref,
                    "Reconciled project main_ref to resolved default branch"
                );
                updated += 1;
            }
            Err(e) => tracing::warn!(
                project_id = %project.id,
                error = %e,
                "Failed to update reconciled main_ref"
            ),
        }
    }

    if updated > 0 {
        tracing::info!(
            total_projects = projects.len(),
            updated,
            "Project main_ref reconciliation complete"
        );
    }
}

/// Reconcile tests — REQ-BED-031 gate behaviour (task 24696 Phase 3).
///
/// Exercises the three shapes of a Work conversation with a missing on-disk
/// worktree directory:
///   a) state = `ContextExhausted` -> skipped, mode preserved
///   b) `continued_in_conv_id` = Some -> skipped, mode preserved
///   c) neither (a) nor (b), genuine orphan -> marked Terminal,
///      `mode/worktree_path/cwd` preserved
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
                // Don't depend on the host's commit-signing setup in tests.
                "-c",
                "commit.gpgsign=false",
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
            llm_language::LlmLanguage::default(),
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
    /// continuation. The conversation is marked Terminal and mode/cwd/
    /// `worktree_path` are preserved untouched. The user keeps the original
    /// metadata for context and can hard-delete when ready.
    #[tokio::test]
    async fn marks_genuine_orphan_terminal() {
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
            matches!(after.state, ConvState::Terminal),
            "genuine orphan must be marked Terminal"
        );
        assert!(
            matches!(after.conv_mode, ConvMode::Work { .. }),
            "conv_mode must be preserved — user keeps original mode metadata"
        );
        assert_eq!(
            after.conv_mode.worktree_path(),
            Some(wt_path.as_str()),
            "worktree_path must be preserved untouched"
        );
        assert_eq!(after.cwd, wt_path, "cwd must NOT be reset to project root");
    }

    /// Branch-mode genuine orphan — same treatment as Work: marked Terminal,
    /// `mode/cwd/worktree_path` preserved.
    #[tokio::test]
    async fn marks_branch_orphan_terminal() {
        let (_git_tmp, repo_root) = init_repo();
        let db = fresh_db().await;
        let project = db
            .find_or_create_project(repo_root.to_str().unwrap())
            .await
            .unwrap();

        let conv_id = "branch-orphan";
        let wt_path = repo_root
            .join(".phoenix")
            .join("worktrees")
            .join(conv_id)
            .to_string_lossy()
            .to_string();
        let mode = ConvMode::Branch {
            branch_name: NonEmptyString::new("feature/x").unwrap(),
            worktree_path: NonEmptyString::new(&wt_path).unwrap(),
            base_branch: NonEmptyString::new("feature/x").unwrap(),
        };
        seed_work_conv(&db, conv_id, conv_id, &wt_path, &mode, &project.id).await;

        assert!(!std::path::Path::new(&wt_path).exists());

        reconcile_worktrees(&db).await;

        let after = db.get_conversation(conv_id).await.unwrap();
        assert!(
            matches!(after.state, ConvState::Terminal),
            "branch orphan must be marked Terminal"
        );
        assert!(
            matches!(after.conv_mode, ConvMode::Branch { .. }),
            "conv_mode must be preserved — no demotion to Direct"
        );
        assert_eq!(after.conv_mode.worktree_path(), Some(wt_path.as_str()));
        assert_eq!(after.cwd, wt_path);
    }
}

/// REQ-PROJ-034a: `reconcile_project_main_refs` backfills `main_ref` to the
/// resolved default branch.
#[cfg(test)]
mod reconcile_main_ref_tests {
    use super::*;

    /// Initialise a git repo on `initial_branch` with one commit.
    fn init_repo_on(initial_branch: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let root = tmp.path().to_path_buf();
        for args in [
            &["init", "-q", "-b", initial_branch][..],
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
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

    /// Point a repo's cached `refs/remotes/origin/HEAD` at `branch`, simulating
    /// an authoritative remote default without any network. `branch` need not be
    /// the checked-out branch — reconciliation reads only the remote signal.
    fn set_remote_default(repo: &std::path::Path, branch: &str) {
        let status = std::process::Command::new("git")
            .args([
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                &format!("refs/remotes/origin/{branch}"),
            ])
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(status.success(), "set refs/remotes/origin/HEAD failed");
    }

    /// Directly force a project row's `main_ref` to a literal, simulating a row
    /// created before resolution-at-creation existed.
    async fn force_main_ref(db: &Database, project_id: &str, value: &str) {
        db.update_project_main_ref(project_id, value).await.unwrap();
    }

    #[tokio::test]
    async fn reconciles_literal_main_to_remote_default_master() {
        let (_tmp, repo) = init_repo_on("master");
        set_remote_default(&repo, "master");
        let db = Database::open_in_memory().await.unwrap();
        let project = db
            .find_or_create_project(repo.to_str().unwrap())
            .await
            .unwrap();
        // Simulate a legacy row whose main_ref was defaulted to the literal.
        force_main_ref(&db, &project.id, "main").await;

        reconcile_project_main_refs(&db).await;

        let after = db.get_project(&project.id).await.unwrap();
        assert_eq!(
            after.main_ref, "master",
            "main_ref must be reconciled to the repo's remote default branch"
        );
    }

    /// Create a real local branch (off HEAD) without checking it out.
    fn create_branch(repo: &std::path::Path, branch: &str) {
        let status = std::process::Command::new("git")
            .args(["branch", branch])
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(status.success(), "git branch {branch} failed");
    }

    /// (d) Stored legacy literal `main`, no remote signal, a real local `main`
    /// branch exists, repo on a different (`feature`) branch: the `main` literal
    /// is a VALID base here, so it is left untouched — not rewritten to the
    /// current checkout.
    #[tokio::test]
    async fn no_origin_feature_branch_does_not_rewrite_valid_main_ref() {
        // Repo on `feature`, but a real `main` branch ALSO exists locally.
        let (_tmp, repo) = init_repo_on("feature");
        create_branch(&repo, "main");
        let db = Database::open_in_memory().await.unwrap();
        let project = db
            .find_or_create_project(repo.to_str().unwrap())
            .await
            .unwrap();
        force_main_ref(&db, &project.id, "main").await;

        reconcile_project_main_refs(&db).await;

        let after = db.get_project(&project.id).await.unwrap();
        assert_eq!(
            after.main_ref, "main",
            "a valid stored main_ref must not be clobbered by the current branch"
        );
    }

    /// (a) The key regression: a stored NON-literal `main_ref` (`develop`, which
    /// exists locally) is IMMUTABLE — reconciliation never touches it, even when
    /// `origin/HEAD` now resolves to a different branch (`main`). Only the legacy
    /// literal `"main"` is ever modified; a deliberately-resolved base survives a
    /// shifting remote default so forks keep cutting from the base they were
    /// created with.
    #[tokio::test]
    async fn stored_non_literal_is_never_overwritten_by_remote_default() {
        // Repo on `develop`; the project was created when `develop` was the
        // resolved default. The remote default ONLY moves to `main` afterwards.
        let (_tmp, repo) = init_repo_on("develop");
        let db = Database::open_in_memory().await.unwrap();
        let project = db
            .find_or_create_project(repo.to_str().unwrap())
            .await
            .unwrap();
        // The project was created resolving its real default `develop`.
        assert_eq!(project.main_ref, "develop");

        // Now the remote default shifts to `main` — reconciliation must NOT chase it.
        set_remote_default(&repo, "main");

        reconcile_project_main_refs(&db).await;

        let after = db.get_project(&project.id).await.unwrap();
        assert_eq!(
            after.main_ref, "develop",
            "a valid non-literal main_ref must never be overwritten, even when \
             origin/HEAD differs"
        );
    }

    /// F4 repair: a no-origin repo with a legacy literal `main_ref = "main"` whose
    /// `main` branch does NOT exist (real branch is `master`) gets `main_ref`
    /// repaired to the current checked-out branch — otherwise fork approval later
    /// fails "base branch 'main' not found".
    #[tokio::test]
    async fn no_origin_broken_main_ref_is_repaired_to_current_branch() {
        // Repo's only branch is `master`; there is no `main`.
        let (_tmp, repo) = init_repo_on("master");
        let db = Database::open_in_memory().await.unwrap();
        let project = db
            .find_or_create_project(repo.to_str().unwrap())
            .await
            .unwrap();
        force_main_ref(&db, &project.id, "main").await;

        reconcile_project_main_refs(&db).await;

        let after = db.get_project(&project.id).await.unwrap();
        assert_eq!(
            after.main_ref, "master",
            "a broken (non-existent) main_ref on a no-origin repo must be repaired \
             to the current branch"
        );
    }

    #[tokio::test]
    async fn reconcile_is_idempotent() {
        let (_tmp, repo) = init_repo_on("master");
        set_remote_default(&repo, "master");
        let db = Database::open_in_memory().await.unwrap();
        let project = db
            .find_or_create_project(repo.to_str().unwrap())
            .await
            .unwrap();
        force_main_ref(&db, &project.id, "main").await;

        reconcile_project_main_refs(&db).await;
        let first = db.get_project(&project.id).await.unwrap();
        // A second run over an already-correct row writes nothing and changes
        // nothing.
        reconcile_project_main_refs(&db).await;
        let second = db.get_project(&project.id).await.unwrap();

        assert_eq!(first.main_ref, "master");
        assert_eq!(second.main_ref, "master");
    }

    #[tokio::test]
    async fn missing_repo_is_skipped_without_error() {
        let db = Database::open_in_memory().await.unwrap();
        // Create a project against a real repo so creation succeeds, then point
        // its canonical_path at a now-gone directory.
        let (tmp, repo) = init_repo_on("master");
        let project = db
            .find_or_create_project(repo.to_str().unwrap())
            .await
            .unwrap();
        force_main_ref(&db, &project.id, "main").await;
        drop(tmp); // repo directory removed from disk

        // Must not panic / error; the unresolvable row is left untouched.
        reconcile_project_main_refs(&db).await;

        let after = db.get_project(&project.id).await.unwrap();
        assert_eq!(
            after.main_ref, "main",
            "an unresolvable repo is skipped, leaving the stored value as-is"
        );
    }

    #[tokio::test]
    async fn find_or_create_resolves_default_at_creation() {
        let (_tmp, repo) = init_repo_on("develop");
        let db = Database::open_in_memory().await.unwrap();
        let project = db
            .find_or_create_project(repo.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            project.main_ref, "develop",
            "a newly-created project must resolve its real default at creation"
        );
    }
}
