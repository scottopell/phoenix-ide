//! Phoenix IDE - LLM-powered development environment
//!
//! A Rust backend implementing a conversation state machine for
//! interacting with LLM agents.

mod analytics;
mod api;
mod chain_qa;
mod chain_runtime;
mod conversation_cwd;
mod coordinator_tools;
mod discovery;
pub mod drive_turn;
pub(crate) mod git_ops;
pub(crate) mod git_start;
mod mcp_oauth_store;
mod message_expander;
mod phx_cli;
mod project_opportunistic_build_warm;
mod resolution_root;
mod runtime;
mod suggest;
mod system_prompt;
mod task_listing;
mod title_generator;
mod tls;

// Domain-vocabulary leaves now live in the acyclic `phoenix-core` base crate.
// Re-export them at their historical crate-root paths so existing
// `crate::llm_language::…` / `crate::task_source::…` call sites resolve
// unchanged (move-down, re-export-up).
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
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
use phoenix_llm::{LlmConfig, ModelRegistry};
use std::future::IntoFuture;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
};
mod hot_restart;
mod logging;

/// Assemble the static deployment facts reported by `GET /api/deployment`.
/// Resolves every path from the same logic the rest of the process uses so the
/// page reports the locations the process actually opens (specs/deployment-info/).
fn build_deployment_config(
    bind_address: SocketAddr,
    runtime_env: &PhoenixRuntimeEnvironment,
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

    let locations = build_disk_locations(runtime_env, tls_source, loaded_tls);

    api::DeploymentConfig {
        bind_address,
        tls,
        log,
        locations,
    }
}

/// The canonical externally-reachable origin used as the OAuth redirect base
/// (REQ-MCP-020): `PHOENIX_EXTERNAL_URL` if set, else derived from the scheme
/// (TLS presence), the operator's TLS host (the reachable domain set for the
/// certificate), and the bind address. Reading the env var here keeps
/// `resolve_external_origin` pure and unit-testable.
fn canonical_external_origin(
    bind_address: SocketAddr,
    tls_loaded: bool,
    external_host: Option<String>,
) -> String {
    let explicit = std::env::var("PHOENIX_EXTERNAL_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty());
    resolve_external_origin(explicit, tls_loaded, external_host, bind_address)
}

/// Pure core of [`canonical_external_origin`]. `explicit` is an operator
/// override; `external_host` is the reachable domain from the TLS config.
fn resolve_external_origin(
    explicit: Option<String>,
    tls_loaded: bool,
    external_host: Option<String>,
    bind_address: SocketAddr,
) -> String {
    if let Some(external) = explicit {
        return external;
    }
    let scheme = if tls_loaded { "https" } else { "http" };
    // An IPv6 literal needs brackets to form a valid authority next to the
    // port, whether it comes from the TLS host config or the bind fallback.
    let host = external_host.map_or_else(
        || {
            let ip = bind_address.ip();
            if ip.is_unspecified() || ip.is_loopback() {
                "localhost".to_string()
            } else if let std::net::IpAddr::V6(v6) = ip {
                format!("[{v6}]")
            } else {
                ip.to_string()
            }
        },
        bracket_if_ipv6,
    );
    let port = bind_address.port();
    let default_port = if tls_loaded { 443 } else { 80 };
    if port == default_port {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    }
}

/// Bracket a bare IPv6 literal so it forms a valid URL authority; pass any
/// other host (a domain or IPv4 literal, or an already-bracketed address)
/// through unchanged.
fn bracket_if_ipv6(host: String) -> String {
    if !host.starts_with('[') && host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]")
    } else {
        host
    }
}

/// Whether a redirect base's host is a loopback name/address -- the signal that
/// an all-interfaces deployment has no reachable name configured.
fn redirect_is_loopback(redirect_base: &str) -> bool {
    let authority = redirect_base
        .split_once("://")
        .map_or(redirect_base, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or("");
    // Strip the port; an IPv6 literal is bracketed, so split a port only after
    // the closing bracket.
    let host = match authority.rsplit_once(']') {
        Some((bracketed, _)) => bracketed.trim_start_matches('['),
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

#[cfg(test)]
mod external_origin_tests {
    use super::{redirect_is_loopback, resolve_external_origin};
    use std::net::SocketAddr;

    fn addr(s: &str) -> SocketAddr {
        s.parse().expect("addr")
    }

    #[test]
    fn local_bind_derives_localhost() {
        // No TLS, loopback or all-interfaces bind, no domain -> localhost.
        assert_eq!(
            resolve_external_origin(None, false, None, addr("127.0.0.1:8042")),
            "http://localhost:8042"
        );
        assert_eq!(
            resolve_external_origin(None, false, None, addr("0.0.0.0:8031")),
            "http://localhost:8031"
        );
    }

    #[test]
    fn tls_domain_drives_the_origin() {
        // The reachable TLS domain is the canonical host; the default https
        // port is dropped, a non-default port kept (REQ-MCP-020).
        assert_eq!(
            resolve_external_origin(
                None,
                true,
                Some("phoenix.example.com".into()),
                addr("0.0.0.0:443")
            ),
            "https://phoenix.example.com"
        );
        assert_eq!(
            resolve_external_origin(
                None,
                true,
                Some("phoenix.example.com".into()),
                addr("0.0.0.0:8443")
            ),
            "https://phoenix.example.com:8443"
        );
    }

    #[test]
    fn explicit_override_wins_over_the_derived_host() {
        assert_eq!(
            resolve_external_origin(
                Some("https://proxy.example".into()),
                false,
                Some("ignored.example".into()),
                addr("0.0.0.0:8031")
            ),
            "https://proxy.example"
        );
    }

    #[test]
    fn non_loopback_ip_bind_uses_the_ip() {
        assert_eq!(
            resolve_external_origin(None, false, None, addr("192.168.1.5:8042")),
            "http://192.168.1.5:8042"
        );
    }

    #[test]
    fn tls_ipv6_host_is_bracketed() {
        // A bare IPv6 TLS host must be bracketed to form a valid authority.
        assert_eq!(
            resolve_external_origin(None, true, Some("2001:db8::1".into()), addr("0.0.0.0:8443")),
            "https://[2001:db8::1]:8443"
        );
        // An already-bracketed literal passes through unchanged.
        assert_eq!(
            resolve_external_origin(
                None,
                true,
                Some("[2001:db8::1]".into()),
                addr("0.0.0.0:8443")
            ),
            "https://[2001:db8::1]:8443"
        );
    }

    #[test]
    fn loopback_detection_covers_names_and_literals() {
        assert!(redirect_is_loopback("http://localhost:8042"));
        assert!(redirect_is_loopback("http://127.0.0.1:8042"));
        assert!(redirect_is_loopback("https://[::1]:8443"));
        assert!(!redirect_is_loopback("https://phoenix.example.com"));
        assert!(!redirect_is_loopback("http://192.168.1.5:8042"));
    }
}

/// Build the on-disk location rows reported by `GET /api/deployment`, each with
/// its sizing policy. Every path is normalized to absolute.
fn build_disk_locations(
    runtime_env: &PhoenixRuntimeEnvironment,
    tls_source: Option<&tls::ConfigSource>,
    loaded_tls: Option<&tls::LoadedConfig>,
) -> Vec<api::DiskLocation> {
    use api::{DiskCategory, DiskLocation, MeasureMode};

    let db_pb = api::absolutize(&runtime_env.db_path());
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
            category: DiskCategory::Database,
            label: "Database".to_string(),
            path: db_pb.clone(),
            mode: MeasureMode::File,
        },
        DiskLocation {
            category: DiskCategory::DataDirectory,
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
                category: DiskCategory::Tls,
                label: "TLS directory".to_string(),
                path: dir.clone(),
                mode: MeasureMode::RecurseSmall,
            });
        }
        (Some(tls::ConfigSource::Manual(_)), Some(loaded)) => {
            locations.push(DiskLocation {
                category: DiskCategory::Tls,
                label: "TLS certificate".to_string(),
                path: loaded.cert_path.clone(),
                mode: MeasureMode::File,
            });
            locations.push(DiskLocation {
                category: DiskCategory::Tls,
                label: "TLS key".to_string(),
                path: loaded.key_path.clone(),
                mode: MeasureMode::File,
            });
        }
        _ => {}
    }

    locations.push(DiskLocation {
        category: DiskCategory::Skills,
        label: "Built-in skills".to_string(),
        path: runtime_env.builtin_skills_dir(),
        mode: MeasureMode::RecurseSmall,
    });

    // The codex credential row is NOT built here: the active credential source
    // can change at runtime via the in-app login flow, so the handler resolves
    // and measures it per request (see `active_codex_credentials_location`).

    // Attachments are stored inline in the database. This row is the stable home
    // for the file-based attachment directory once that storage mode is active.
    locations.push(DiskLocation {
        category: DiskCategory::Attachments,
        label: "Attachments".to_string(),
        path: db_pb,
        mode: MeasureMode::InlineDb,
    });

    locations.push(DiskLocation {
        category: DiskCategory::BrowserCache,
        label: "Browser binary cache".to_string(),
        path: runtime_env.chromium_cache_dir(),
        mode: MeasureMode::NoMeasure,
    });

    // Per-scope Chrome profiles created on demand while browser sessions are
    // active. A glob, not a single dir, and potentially large — reported as an
    // unsized pattern row.
    locations.push(DiskLocation {
        category: DiskCategory::BrowserProfiles,
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

/// Resolve the suggest capability token, persisting it across restarts.
///
/// The token is stored in `app_settings` alongside the password fingerprint it
/// was minted under (mirroring [`api::auth`]'s session binding). A persisted
/// token is reused only when that fingerprint still matches the current
/// password, so a normal restart keeps the same token — existing terminals
/// stay authorized — while rotating `PHOENIX_PASSWORD` re-mints it. Persistence
/// failures degrade to a fresh per-process token (logged), never fatal.
async fn resolve_suggest_token(db: &crate::db::Database, password: Option<&str>) -> String {
    const TOKEN_KEY: &str = "suggest_token";
    const FINGERPRINT_KEY: &str = "suggest_token_password_fingerprint";

    let current_fp = password
        .map(api::auth::password_fingerprint)
        .unwrap_or_default();

    if let (Ok(Some(token)), Ok(Some(fp))) = (
        db.get_app_setting(TOKEN_KEY).await,
        db.get_app_setting(FINGERPRINT_KEY).await,
    ) {
        if !token.is_empty() && fp == current_fp {
            return token;
        }
    }

    let token = mint_suggest_token();
    if let Err(e) = db.set_app_setting(TOKEN_KEY, &token).await {
        tracing::warn!(error = %e, "failed to persist suggest token; phx will need a fresh token after restart");
    } else if let Err(e) = db.set_app_setting(FINGERPRINT_KEY, &current_fp).await {
        tracing::warn!(error = %e, "failed to persist suggest-token fingerprint");
    }
    token
}

/// Mint a fresh 256-bit suggest capability token (URL-safe base64).
fn mint_suggest_token() -> String {
    use base64::Engine as _;
    use rand::Rng as _;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Install the in-terminal `phx` companion and the PTY env injection it relies
/// on.
///
/// Materializes `<data_dir>/bin/phx` as a symlink to this binary (refreshed
/// each start so it tracks upgrades) and registers a [`PtyEnvInjection`] that
/// prepends that bin dir to every terminal's `PATH` and injects
/// `PHOENIX_API_URL` + `PHOENIX_SUGGEST_TOKEN`. Symlink failure is non-fatal
/// (logged); `phx` simply won't be on PATH.
fn setup_phx_companion(
    runtime_env: &PhoenixRuntimeEnvironment,
    bind_address: std::net::SocketAddr,
    tls_loaded: bool,
    token: &str,
) {
    let scheme = if tls_loaded { "https" } else { "http" };
    // Derive the loopback host phx should call from the actual bind address: a
    // wildcard bind is reachable via the matching-family loopback, a specific
    // address is used verbatim (IPv6 bracketed). Hardcoding 127.0.0.1 would
    // break a `::1`/IPv6 or specific-interface bind.
    let host = match bind_address.ip() {
        std::net::IpAddr::V4(v4) if v4.is_unspecified() => "127.0.0.1".to_string(),
        std::net::IpAddr::V6(v6) if v6.is_unspecified() => "[::1]".to_string(),
        std::net::IpAddr::V4(v4) => v4.to_string(),
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
    };
    let api_url = format!("{scheme}://{host}:{}", bind_address.port());

    let mut injection = phoenix_terminal::spawn::PtyEnvInjection {
        path_prefix: None,
        extra: vec![
            ("PHOENIX_API_URL".to_string(), api_url),
            ("PHOENIX_SUGGEST_TOKEN".to_string(), token.to_string()),
        ],
    };

    match install_phx_symlink(runtime_env) {
        Ok(bin_dir) => injection.path_prefix = Some(bin_dir),
        Err(e) => {
            tracing::warn!(error = %e, "could not install `phx` shim; in-terminal phx unavailable");
        }
    }

    phoenix_terminal::spawn::set_pty_env_injection(injection);
}

/// Create `<data_dir>/bin/phx` as a symlink to the running binary, returning the
/// bin directory. Replaces any existing link so the shim tracks binary upgrades.
fn install_phx_symlink(
    runtime_env: &PhoenixRuntimeEnvironment,
) -> std::io::Result<std::path::PathBuf> {
    let bin_dir = runtime_env.data_dir().join("bin");
    std::fs::create_dir_all(&bin_dir)?;
    let link = bin_dir.join("phx");
    let target = std::env::current_exe()?;
    match std::fs::symlink_metadata(&link) {
        Ok(_) => std::fs::remove_file(&link)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::os::unix::fs::symlink(&target, &link)?;
    Ok(bin_dir)
}

fn sandbox_exec_arg() -> Option<String> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--sandbox-exec") {
        return None;
    }
    if args.next().as_deref() != Some("--") {
        eprintln!("usage: phoenix_ide --sandbox-exec -- <cmd>");
        std::process::exit(2);
    }
    match args.next() {
        Some(cmd) if args.next().is_none() => Some(cmd),
        _ => {
            eprintln!("usage: phoenix_ide --sandbox-exec -- <cmd>");
            std::process::exit(2);
        }
    }
}

fn build_identity_requested() -> bool {
    let mut args = std::env::args().skip(1);
    matches!(args.next().as_deref(), Some("--build-identity")) && args.next().is_none()
}

/// Start the Phoenix HTTP server with the production composition root.
///
/// # Errors
///
/// Returns an error when production bootstrap, binding, or server execution fails.
#[allow(clippy::too_many_lines)] // Startup sequence is inherently sequential; splitting would obscure the flow.
pub async fn run_server() -> Result<(), Box<dyn std::error::Error>> {
    // `phx` companion: when this binary is invoked through the PATH-injected
    // `phx` symlink (or `phoenix_ide suggest …`), run the thin suggestion
    // client and exit instead of starting the server. Branch before logging
    // setup so the client's stdout stays clean (OSC 8 links only).
    if build_identity_requested() {
        println!(
            "{{\"version\":\"{}\",\"git_sha\":\"{}\"}}",
            env!("CARGO_PKG_VERSION"),
            env!("PHOENIX_GIT_SHA")
        );
        return Ok(());
    }

    if phx_cli::is_cli_invocation() {
        std::process::exit(phx_cli::run().await);
    }

    if let Some(cmd) = sandbox_exec_arg() {
        crate::tools::bash::apply_explore_read_only_from_env(&cmd);
    }

    // Initialize logging from the configured sinks (PHOENIX_LOG_STDOUT /
    // PHOENIX_LOG_FILE) and the Datadog tracer provider (DD_* env vars). The
    // handles must outlive the program so the file appender worker and the
    // tracer provider flush on shutdown; held until `main` returns.
    let log_config = logging::LogConfig::from_env();
    let tracing_handles = logging::init(&log_config)?;

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

    // Resolve every filesystem-environment path ($HOME / $CODEX_HOME / temp_dir
    // and the Phoenix layout under them) once, behind a typed surface. Every
    // subsystem that needs an on-disk location reads it from here.
    let runtime_env = Arc::new(PhoenixRuntimeEnvironment::detect());

    // REQ-BASH-007: install the child subreaper so descendants whose
    // parent dies (double-forks, setsid daemons) reparent to Phoenix
    // rather than init. Must run before any tool spawns a child.
    crate::tools::bash::install_reaper();

    // Log startup context: binary path, version, and whether this looks like a deploy
    let exe_path =
        std::env::current_exe().map_or_else(|_| "unknown".to_string(), |p| p.display().to_string());
    let is_prod = runtime_env.is_production();
    tracing::info!(
        exe = %exe_path,
        pid = std::process::id(),
        mode = if is_prod { "production" } else { "development" },
        git_sha = env!("PHOENIX_GIT_SHA"),
        version = env!("CARGO_PKG_VERSION"),
        "Phoenix IDE starting"
    );

    // Configuration. db_path materializes the env's resolved PathBuf as a
    // string for the `&str`-taking APIs below (TLS config, Database::open).
    let db_path = runtime_env.db_path().to_string_lossy().into_owned();

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
    let builtin_skills_dir = runtime_env.builtin_skills_dir();
    match skills::builtin::extract_to(&builtin_skills_dir) {
        Ok(()) => tracing::info!(path = %builtin_skills_dir.display(), "extracted built-in skills"),
        Err(e) => tracing::warn!(
            path = %builtin_skills_dir.display(),
            error = %e,
            "failed to extract built-in skills",
        ),
    }

    // Initialize database
    tracing::info!(path = %db_path, "Opening database");
    let db = Database::open(&db_path).await?;

    // Run pending data migrations before anything reads conversation data
    db::run_pending_migrations(db.pool()).await?;
    // The numbered migrations above may have created the `-wal`/`-shm` sidecars
    // after `open`'s chmod ran. Re-tighten so the sidecars (which hold the same
    // conversation data) are owner-only on a multi-user host. Best-effort.
    db.restrict_file_permissions();

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
    let llm_config = LlmConfig::from_env(runtime_env.clone());
    let credential_helper = llm_config.credential_helper.clone();
    let llm_registry = Arc::new(ModelRegistry::new_with_discovery(&llm_config).await);

    if llm_registry.has_models() {
        tracing::info!(
            models = %llm_registry.available_models().join(", "),
            default = %llm_registry.default_model_id(),
            "LLM registry initialized"
        );
    } else {
        tracing::warn!("No LLM API keys configured. Set ANTHROPIC_API_KEY, OPENAI_API_KEY, or LLM_API_KEY_HELPER.");
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

    // Create the MCP manager. Discovery starts after the listener is bound,
    // below: the OAuth flow needs the server's own address for its callback
    // redirect before any server can 401 (REQ-MCP-011).
    let mcp_manager = Arc::new(crate::tools::mcp::McpClientManager::new());
    mcp_manager.set_oauth_store(Arc::new(mcp_oauth_store::DbOAuthStore::new(db.clone())));

    // Load persisted disabled-server set before discovery starts.
    let disabled = db.get_disabled_mcp_servers().await.unwrap_or_default();
    if !disabled.is_empty() {
        tracing::info!(count = disabled.len(), servers = ?disabled, "Loaded disabled MCP servers from DB");
    }
    mcp_manager.set_disabled_servers(disabled).await;

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
    // address comes from systemd, not the fallback default.
    //
    // The fallback bind IP defaults to 0.0.0.0 (all interfaces) so a
    // non-socket-activated prod launcher (daemon/launchd) reaches the network.
    // PHOENIX_BIND_ADDR overrides it with a specific IP; dev sets 127.0.0.1 so
    // the dev server stays loopback-only and passes the fail-closed guard below
    // without a password. Under socket activation the fallback isn't used.
    let default_ip = IpAddr::from([0, 0, 0, 0]);
    let fallback_ip: IpAddr = std::env::var("PHOENIX_BIND_ADDR")
        .ok()
        .and_then(|raw| {
            if let Ok(ip) = raw.parse::<IpAddr>() {
                Some(ip)
            } else {
                tracing::warn!(
                    value = %raw,
                    "PHOENIX_BIND_ADDR is not a valid IP address; falling back to 0.0.0.0"
                );
                None
            }
        })
        .unwrap_or(default_ip);
    let fallback_addr = SocketAddr::from((fallback_ip, port));
    tracing::info!(%fallback_addr, "Resolved fallback bind address");
    let listener = hot_restart::get_listener(fallback_addr).await?;
    let socket_activated = hot_restart::is_socket_activated();
    let bind_address = listener.local_addr().unwrap_or(fallback_addr);

    // Fail closed: a Phoenix reachable from outside this host MUST require auth.
    // The prod systemd socket binds all interfaces, so without a password anyone
    // on the network can drive an agent that runs arbitrary commands as this
    // user. Refuse any non-loopback bind unless a password is set. The escape
    // hatch is for operators who front Phoenix with their own authenticating
    // proxy and have deliberately accepted that responsibility.
    if password.is_none()
        && !bind_address.ip().is_loopback()
        && std::env::var("PHOENIX_ALLOW_INSECURE_BIND").ok().as_deref() != Some("1")
    {
        return Err(format!(
            "Refusing to start: bound to {bind_address} (non-loopback) with no PHOENIX_PASSWORD. \
             An unauthenticated, network-reachable Phoenix lets anyone run arbitrary commands as this user. \
             Set PHOENIX_PASSWORD, bind to 127.0.0.1, or set PHOENIX_ALLOW_INSECURE_BIND=1 if Phoenix sits behind your own auth proxy. \
             For a prod deploy, set PHOENIX_PASSWORD in .phoenix-ide.env (the systemd unit reads its environment from there, not your shell)."
        )
        .into());
    }

    // The MCP OAuth callback redirect must point at an address the
    // *operator's browser* can reach. It is the canonical external origin,
    // derived from the TLS host config (the reachable domain the operator
    // already sets for the certificate) so a remote deployment needs no
    // separate redirect knob (REQ-MCP-020); PHOENIX_EXTERNAL_URL overrides it
    // for proxy-terminated TLS / manual certs. With the redirect base known,
    // background MCP discovery (which may immediately hit a 401 and start an
    // OAuth flow) can start.
    {
        let redirect_base = canonical_external_origin(
            bind_address,
            loaded_tls.is_some(),
            tls_source
                .as_ref()
                .and_then(tls::ConfigSource::external_host),
        );
        // An all-interfaces bind that still resolves to loopback has no
        // reachable name configured: the callback would point a remote browser
        // at localhost and fail. Surface the fix at startup and on every
        // `unauthorized` status entry, rather than failing silently.
        if bind_address.ip().is_unspecified() && redirect_is_loopback(&redirect_base) {
            let warning = format!(
                "OAuth callback redirects to {redirect_base}, which is unreachable from another \
                 machine. Set PHOENIX_TLS_HOSTS to your reachable domain (or PHOENIX_EXTERNAL_URL) \
                 so a remote browser can complete authorization."
            );
            tracing::warn!(redirect_base = %redirect_base, "{warning}");
            mcp_manager.set_oauth_redirect_warning(Some(warning));
        }
        mcp_manager.set_oauth_redirect_base(redirect_base);
    }
    std::mem::drop(mcp_manager.start_background_discovery());

    // Static deployment facts served read-only by GET /api/deployment
    // (specs/deployment-info/).
    let deployment = Arc::new(build_deployment_config(
        bind_address,
        &runtime_env,
        tls_source.as_ref(),
        loaded_tls.as_ref(),
        log_config.to_log_info(),
    ));

    let auth_enabled = password.is_some();

    // Install the `phx` terminal companion: a symlink to this binary on the
    // PTY PATH plus the env (API URL + capability token) it needs to call back.
    // The token persists across restarts (bound to the password fingerprint) so
    // terminals opened before a restart stay authorized.
    let suggest_token = resolve_suggest_token(&db, password.as_deref()).await;
    setup_phx_companion(
        &runtime_env,
        bind_address,
        loaded_tls.is_some(),
        &suggest_token,
    );

    // Create application state
    let state = AppState::new(
        db,
        llm_registry,
        platform,
        mcp_manager,
        credential_helper,
        password,
        deployment,
        runtime_env,
        suggest_token,
    )
    .await;

    // Create router
    //
    // CORS posture is tied to the auth posture. A password-protected deployment
    // is potentially network-reachable, so it must not advertise a wildcard
    // origin: the UI is served same-origin from the embedded bundle and needs no
    // cross-origin grant, and a wildcard would let any website drive the API
    // (CSRF / drive-by RCE) for any user whose browser can reach the server.
    // Without a password the server is loopback-only (enforced at bind, above),
    // where permissive CORS lets the Vite dev server on a separate port reach the
    // API during development.
    let cors = if auth_enabled {
        CorsLayer::new()
    } else {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    };

    let compression = CompressionLayer::new().gzip(true).br(true);

    // Clear-to-ready sweep: once a usage-limit window elapses, return the
    // errored conversation to Idle so it is usable again without a manual
    // dismiss. Detached for the process lifetime; the first tick fires
    // immediately, clearing any windows already elapsed at startup.
    tokio::spawn(crate::runtime::usage_limit_sweep::run(
        state.runtime.clone(),
    ));

    // Hold an Arc to the bash handle registry so the shutdown kill-tree
    // pass (REQ-BASH-007) can reach it after `state` moves into the router.
    let bash_handles_for_shutdown = state.runtime.bash_handles().clone();

    let app = create_router(state).layer(cors).layer(compression);

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
            // `into_make_service_with_connect_info` injects the real peer
            // `SocketAddr` into each request's extensions so handlers like
            // `auth_login` can key the login throttle on the unspoofable peer
            // IP (the TLS path injects the same extension in `serve_https`).
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
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

    // Flush in-flight Datadog spans to the agent before we kill child
    // processes and exit. Bounded by a 1s timeout inside shutdown_tracer.
    tracing_handles.shutdown_tracer();

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
                if let Err(e) = phoenix_core::git::command()
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

    reclaim_unowned_worktrees(db).await;
}

fn submodules_have_ignored_evidence(worktree_path: &std::path::Path) -> Result<bool, String> {
    let submodules = phoenix_core::git::command()
        .args(["submodule", "status", "--recursive"])
        .current_dir(worktree_path)
        .output()
        .map_err(|error| format!("submodule inventory failed to start: {error}"))?;
    if !submodules.status.success() {
        return Err(format!(
            "submodule inventory failed: {}",
            String::from_utf8_lossy(&submodules.stderr).trim()
        ));
    }
    let submodules = String::from_utf8(submodules.stdout)
        .map_err(|error| format!("submodule inventory is not UTF-8: {error}"))?;
    for line in submodules.lines() {
        let Some(relative) = line.split_ascii_whitespace().nth(1) else {
            continue;
        };
        let ignored = phoenix_core::git::command()
            .args([
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "-z",
            ])
            .current_dir(worktree_path.join(relative))
            .output()
            .map_err(|error| format!("submodule ignored-file inventory failed: {error}"))?;
        if !ignored.status.success() {
            return Err(format!(
                "submodule ignored-file inventory failed for {relative}: {}",
                String::from_utf8_lossy(&ignored.stderr).trim()
            ));
        }
        if ignored.stdout.iter().any(|byte| *byte != 0) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Reclaim a clean Phoenix worktree whose persisted scope has no owner.
/// Dirty or unreadable worktrees are retained so startup recovery cannot erase
/// uncommitted evidence.
fn reclaim_unowned_worktree(
    worktree_path: &std::path::Path,
    branch_to_delete: Option<&str>,
) -> Result<bool, String> {
    let Some(repo_root) = crate::git_ops::repo_root_from_phoenix_worktree(worktree_path) else {
        return Err("path is not a canonical Phoenix worktree".to_string());
    };

    let status = phoenix_core::git::command()
        .args([
            "status",
            "--porcelain",
            "--untracked-files=normal",
            "--ignore-submodules=none",
        ])
        .current_dir(worktree_path)
        .output()
        .map_err(|error| format!("git status failed to start: {error}"))?;
    if !status.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        ));
    }
    if !status.stdout.is_empty() {
        return Ok(false);
    }

    let ignored = phoenix_core::git::command()
        .args([
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(worktree_path)
        .output()
        .map_err(|error| format!("ignored-file inventory failed to start: {error}"))?;
    if !ignored.status.success() {
        return Err(format!(
            "ignored-file inventory failed: {}",
            String::from_utf8_lossy(&ignored.stderr).trim()
        ));
    }

    if submodules_have_ignored_evidence(worktree_path)? {
        return Ok(false);
    }
    let ignored_paths = ignored
        .stdout
        .split(|byte| *byte == 0)
        .filter(|p| !p.is_empty());
    if ignored_paths
        .clone()
        .any(|path| !path.starts_with(b"target/"))
    {
        return Ok(false);
    }

    // The repository root's target/ is the sole disposable ignored subtree.
    // Every other ignored path is treated as user evidence and retained.
    if ignored_paths.count() > 0 {
        crate::git_ops::run_git(worktree_path, &["clean", "-fdX", "--", "target"])
            .map_err(|error| format!("ignored build-output cleanup failed: {error}"))?;
    }
    let path = worktree_path.to_string_lossy().into_owned();
    // Git requires --force to remove an otherwise-clean worktree containing an
    // initialized submodule. The status and ignored-file gates above establish
    // that neither the root checkout nor any submodule contains user evidence.
    crate::git_ops::run_git(&repo_root, &["worktree", "remove", &path, "--force"])
        .map_err(|error| format!("git worktree remove refused: {error}"))?;

    if let Some(branch) = branch_to_delete {
        if crate::git_ops::find_branch_in_worktree_list(&repo_root, branch).is_none() {
            if let Err(error) = crate::git_ops::run_git(&repo_root, &["branch", "-D", branch]) {
                tracing::debug!(%error, branch, "startup worktree reconciliation: branch delete failed");
            }
        }
    }
    Ok(true)
}

fn add_physical_project_worktrees(
    projects: &[db::Project],
    by_path: &mut std::collections::BTreeMap<String, Vec<db::Conversation>>,
) {
    for project in projects {
        let worktrees_dir = std::path::Path::new(&project.canonical_path)
            .join(".phoenix")
            .join("worktrees");
        let entries = match std::fs::read_dir(&worktrees_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                tracing::warn!(path = %worktrees_dir.display(), %error, "failed to inventory Phoenix worktree directory");
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                by_path
                    .entry(path.to_string_lossy().into_owned())
                    .or_default();
            }
        }
    }
}

async fn reclaim_unowned_worktrees(db: &Database) {
    let conversations = match db.managed_worktree_conversations().await {
        Ok(conversations) => conversations,
        Err(error) => {
            tracing::warn!(%error, "failed to inventory managed worktrees for startup reclamation");
            return;
        }
    };

    let mut by_path: std::collections::BTreeMap<String, Vec<db::Conversation>> =
        std::collections::BTreeMap::new();
    for conv in conversations {
        if let Some(path) = conv.conv_mode.worktree_path() {
            by_path.entry(path.to_string()).or_default().push(conv);
        }
    }

    let projects = match db.list_projects().await {
        Ok(projects) => projects,
        Err(error) => {
            tracing::warn!(%error, "failed to inventory project roots for startup reclamation");
            return;
        }
    };
    add_physical_project_worktrees(&projects, &mut by_path);

    let mut reclaimed = 0usize;
    let mut quarantined = 0usize;
    for (path, owners) in by_path {
        if owners
            .iter()
            .any(crate::runtime::conversation_owns_work_scope)
        {
            continue;
        }
        let worktree = std::path::PathBuf::from(&path);
        if !worktree.exists() {
            continue;
        }
        let branch = if owners.is_empty() {
            crate::runtime::deterministic_explore_branch_for_worktree(&worktree)
        } else {
            crate::runtime::cleanup_branch_for_unowned_work_scope(&worktree, &owners)
        };
        let worktree_for_cleanup = worktree.clone();
        match tokio::task::spawn_blocking(move || {
            reclaim_unowned_worktree(&worktree_for_cleanup, branch.as_deref())
        })
        .await
        {
            Ok(Ok(true)) => {
                reclaimed += 1;
                tracing::info!(worktree = %worktree.display(), "reclaimed unowned Phoenix worktree");
            }
            Ok(Ok(false)) => {
                quarantined += 1;
                tracing::warn!(
                    worktree = %worktree.display(),
                    "retaining dirty unowned Phoenix worktree for manual recovery"
                );
            }
            Ok(Err(error)) => {
                quarantined += 1;
                tracing::warn!(
                    worktree = %worktree.display(),
                    %error,
                    "retaining unowned Phoenix worktree because safe reclamation failed"
                );
            }
            Err(error) => {
                quarantined += 1;
                tracing::warn!(
                    worktree = %worktree.display(),
                    %error,
                    "retaining unowned Phoenix worktree because reclamation task failed"
                );
            }
        }
    }

    if reclaimed > 0 || quarantined > 0 {
        tracing::info!(
            reclaimed,
            quarantined,
            "startup worktree reclamation complete"
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
/// - a remote default that differs (`git::resolve_remote_default_branch`, the
///   cached `refs/remotes/origin/HEAD`) is the authoritative fork base — backfill;
/// - else, with no local `main` branch, the literal is broken and would fail fork
///   approval, so repair to the current checked-out branch
///   (`git::resolve_default_branch`);
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
    phoenix_core::git::command()
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

pub(crate) async fn reconcile_project_main_refs(db: &Database) {
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
            phoenix_core::git::resolve_remote_default_branch(repo_path)
        {
            remote_default
        } else {
            if local_branch_exists(repo_path, "main") {
                continue;
            }
            let Some(current) = phoenix_core::git::resolve_default_branch(repo_path) else {
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
mod suggest_token_tests {
    use super::*;

    /// The suggest token must survive a "restart" (a fresh resolve against the
    /// same persisted DB and password) and must rotate when the password
    /// changes — the two properties the persistence fix guarantees.
    #[tokio::test]
    async fn token_persists_across_restarts_and_rotates_on_password_change() {
        let db = crate::db::Database::open_in_memory().await.unwrap();

        // First resolve mints + persists.
        let first = resolve_suggest_token(&db, None).await;
        assert!(!first.is_empty());

        // A later resolve with the same (no-)password reuses it: a restart
        // keeps existing terminals authorized.
        let after_restart = resolve_suggest_token(&db, None).await;
        assert_eq!(first, after_restart, "token must persist across restarts");

        // Setting a password rotates the token (fingerprint changed).
        let after_pw = resolve_suggest_token(&db, Some("hunter2")).await;
        assert_ne!(
            after_pw, first,
            "token must rotate when the password changes"
        );

        // The rotated token is itself stable across a subsequent restart.
        let after_pw_again = resolve_suggest_token(&db, Some("hunter2")).await;
        assert_eq!(after_pw, after_pw_again);
    }
}

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
            let status = phoenix_core::git::command()
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

    fn add_worktree(repo_root: &std::path::Path, path: &str, branch: &str) {
        let status = phoenix_core::git::command()
            .args(["worktree", "add", "-q", "-b", branch, path, "main"])
            .current_dir(repo_root)
            .status()
            .unwrap();
        assert!(status.success(), "git worktree add failed");
    }

    async fn set_state(db: &db::Database, id: &str, state: &ConvState) {
        db.update_conversation_state(id, state).await.unwrap();
    }

    async fn wire_continuation(db: &db::Database, from: &str, to: &str) {
        sqlx::query("UPDATE conversations SET continued_in_conv_id = ?1 WHERE id = ?2")
            .bind(to)
            .bind(from)
            .execute(db.pool())
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

    #[tokio::test]
    async fn reclaims_clean_scope_after_handoff_chain_terminates() {
        let (_git_tmp, repo_root) = init_repo();
        let db = fresh_db().await;
        let project = db
            .find_or_create_project(repo_root.to_str().unwrap())
            .await
            .unwrap();
        let (wt_path, mode) = work_mode_at(&repo_root, "owner", "task-clean-reclaim");
        add_worktree(&repo_root, &wt_path, "task-clean-reclaim");

        seed_work_conv(&db, "owner", "owner", &wt_path, &mode, &project.id).await;
        seed_work_conv(&db, "leaf", "leaf", &wt_path, &mode, &project.id).await;
        set_state(
            &db,
            "owner",
            &ConvState::HandedOff {
                successor_conv_id: "leaf".to_string(),
            },
        )
        .await;
        set_state(&db, "leaf", &ConvState::Terminal).await;
        wire_continuation(&db, "owner", "leaf").await;

        reconcile_worktrees(&db).await;

        assert!(!std::path::Path::new(&wt_path).exists());
        assert!(
            crate::git_ops::find_branch_in_worktree_list(&repo_root, "task-clean-reclaim")
                .is_none()
        );

        reconcile_worktrees(&db).await;
        assert!(!std::path::Path::new(&wt_path).exists());
    }

    #[tokio::test]
    async fn explore_continuation_reclaims_original_owner_temp_branch() {
        let (_git_tmp, repo_root) = init_repo();
        let db = fresh_db().await;
        let project = db
            .find_or_create_project(repo_root.to_str().unwrap())
            .await
            .unwrap();
        let owner_id = "12345678-owner";
        let leaf_id = "87654321-leaf";
        let wt_path = repo_root
            .join(".phoenix/worktrees")
            .join(owner_id)
            .to_string_lossy()
            .into_owned();
        let branch = "task-pending-12345678";
        add_worktree(&repo_root, &wt_path, branch);
        let mode = ConvMode::Explore {
            worktree_path: Some(NonEmptyString::new(&wt_path).unwrap()),
            next_taskmd_id_hint: None,
        };
        seed_work_conv(&db, owner_id, "owner", &wt_path, &mode, &project.id).await;
        seed_work_conv(&db, leaf_id, "leaf", &wt_path, &mode, &project.id).await;
        set_state(
            &db,
            owner_id,
            &ConvState::HandedOff {
                successor_conv_id: leaf_id.to_string(),
            },
        )
        .await;
        set_state(&db, leaf_id, &ConvState::Terminal).await;
        wire_continuation(&db, owner_id, leaf_id).await;

        reconcile_worktrees(&db).await;

        assert!(!std::path::Path::new(&wt_path).exists());
        let branch_exists = phoenix_core::git::command()
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .current_dir(&repo_root)
            .status()
            .unwrap()
            .success();
        assert!(
            !branch_exists,
            "the original owner's temp branch must be deleted"
        );
    }

    #[tokio::test]
    async fn reclaims_rowless_physical_worktree() {
        let (_git_tmp, repo_root) = init_repo();
        let db = fresh_db().await;
        db.find_or_create_project(repo_root.to_str().unwrap())
            .await
            .unwrap();
        let wt_path = repo_root
            .join(".phoenix/worktrees/rowless")
            .to_string_lossy()
            .into_owned();
        let branch = "task-pending-rowless";
        add_worktree(&repo_root, &wt_path, branch);

        reconcile_worktrees(&db).await;

        assert!(!std::path::Path::new(&wt_path).exists());
        let branch_exists = phoenix_core::git::command()
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .current_dir(&repo_root)
            .status()
            .unwrap()
            .success();
        assert!(!branch_exists);
    }

    #[tokio::test]
    async fn reclaims_unowned_scope_with_ignored_build_artifacts() {
        let (_git_tmp, repo_root) = init_repo();
        let db = fresh_db().await;
        let project = db
            .find_or_create_project(repo_root.to_str().unwrap())
            .await
            .unwrap();
        std::fs::write(repo_root.join(".gitignore"), "target/\n").unwrap();
        let status = phoenix_core::git::command()
            .args(["add", ".gitignore"])
            .current_dir(&repo_root)
            .status()
            .unwrap();
        assert!(status.success());
        let status = phoenix_core::git::command()
            .args([
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "ignore build output",
            ])
            .current_dir(&repo_root)
            .status()
            .unwrap();
        assert!(status.success());

        let (wt_path, mode) = work_mode_at(&repo_root, "ignored", "task-ignored-reclaim");
        add_worktree(&repo_root, &wt_path, "task-ignored-reclaim");
        std::fs::create_dir(std::path::Path::new(&wt_path).join("target")).unwrap();
        std::fs::write(
            std::path::Path::new(&wt_path).join("target/artifact"),
            "disposable",
        )
        .unwrap();
        seed_work_conv(&db, "ignored", "ignored", &wt_path, &mode, &project.id).await;
        set_state(&db, "ignored", &ConvState::Terminal).await;

        reconcile_worktrees(&db).await;

        assert!(!std::path::Path::new(&wt_path).exists());
    }

    #[tokio::test]
    async fn retains_unowned_scope_with_ignored_user_evidence() {
        let (_git_tmp, repo_root) = init_repo();
        let db = fresh_db().await;
        let project = db
            .find_or_create_project(repo_root.to_str().unwrap())
            .await
            .unwrap();
        std::fs::write(repo_root.join(".gitignore"), ".env\n").unwrap();
        let status = phoenix_core::git::command()
            .args(["add", ".gitignore"])
            .current_dir(&repo_root)
            .status()
            .unwrap();
        assert!(status.success());
        let status = phoenix_core::git::command()
            .args([
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "ignore local env",
            ])
            .current_dir(&repo_root)
            .status()
            .unwrap();
        assert!(status.success());

        let (wt_path, mode) = work_mode_at(&repo_root, "ignored-user", "task-ignore-user");
        add_worktree(&repo_root, &wt_path, "task-ignore-user");
        std::fs::write(std::path::Path::new(&wt_path).join(".env"), "SECRET=keep").unwrap();
        seed_work_conv(
            &db,
            "ignored-user",
            "ignored-user",
            &wt_path,
            &mode,
            &project.id,
        )
        .await;
        set_state(&db, "ignored-user", &ConvState::Terminal).await;

        reconcile_worktrees(&db).await;

        assert!(std::path::Path::new(&wt_path).exists());
        assert_eq!(
            std::fs::read_to_string(std::path::Path::new(&wt_path).join(".env")).unwrap(),
            "SECRET=keep"
        );
    }

    #[tokio::test]
    async fn reclaims_unowned_scope_with_clean_initialized_submodule() {
        let (_git_tmp, repo_root) = init_repo();
        let submodule_tmp = tempfile::tempdir().unwrap();
        let submodule_root = submodule_tmp.path();
        let run = |cwd: &std::path::Path, args: &[&str]| {
            let status = phoenix_core::git::command()
                .args(args)
                .current_dir(cwd)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run(submodule_root, &["init", "-q", "-b", "main"]);
        std::fs::write(submodule_root.join("tracked.txt"), "base").unwrap();
        run(submodule_root, &["add", "tracked.txt"]);
        run(
            submodule_root,
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "init submodule",
            ],
        );
        run(
            &repo_root,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                submodule_root.to_str().unwrap(),
                "vendor/sub",
            ],
        );
        run(&repo_root, &["add", ".gitmodules", "vendor/sub"]);
        run(
            &repo_root,
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "add submodule",
            ],
        );

        let db = fresh_db().await;
        let project = db
            .find_or_create_project(repo_root.to_str().unwrap())
            .await
            .unwrap();
        let (wt_path, mode) = work_mode_at(&repo_root, "clean-sub", "task-clean-sub");
        add_worktree(&repo_root, &wt_path, "task-clean-sub");
        run(
            std::path::Path::new(&wt_path),
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
                "-q",
            ],
        );
        seed_work_conv(&db, "clean-sub", "clean-sub", &wt_path, &mode, &project.id).await;
        set_state(&db, "clean-sub", &ConvState::Terminal).await;

        reconcile_worktrees(&db).await;

        assert!(!std::path::Path::new(&wt_path).exists());
    }

    #[tokio::test]
    async fn retains_unowned_scope_with_ignored_submodule_evidence() {
        let (_git_tmp, repo_root) = init_repo();
        let submodule_tmp = tempfile::tempdir().unwrap();
        let submodule_root = submodule_tmp.path();
        let run = |cwd: &std::path::Path, args: &[&str]| {
            let status = phoenix_core::git::command()
                .args(args)
                .current_dir(cwd)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run(submodule_root, &["init", "-q", "-b", "main"]);
        std::fs::write(submodule_root.join("tracked.txt"), "base").unwrap();
        std::fs::write(submodule_root.join(".gitignore"), ".env\n").unwrap();
        run(submodule_root, &["add", "tracked.txt", ".gitignore"]);
        run(
            submodule_root,
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "init submodule",
            ],
        );
        run(
            &repo_root,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                submodule_root.to_str().unwrap(),
                "vendor/sub",
            ],
        );
        run(&repo_root, &["add", ".gitmodules", "vendor/sub"]);
        run(
            &repo_root,
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "add submodule",
            ],
        );

        let db = fresh_db().await;
        let project = db
            .find_or_create_project(repo_root.to_str().unwrap())
            .await
            .unwrap();
        let (wt_path, mode) = work_mode_at(&repo_root, "dirty-sub", "task-dirty-sub");
        add_worktree(&repo_root, &wt_path, "task-dirty-sub");
        run(
            std::path::Path::new(&wt_path),
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
                "-q",
            ],
        );
        std::fs::write(
            std::path::Path::new(&wt_path).join("vendor/sub/.env"),
            "SECRET=keep",
        )
        .unwrap();
        seed_work_conv(&db, "dirty-sub", "dirty-sub", &wt_path, &mode, &project.id).await;
        set_state(&db, "dirty-sub", &ConvState::Terminal).await;

        reconcile_worktrees(&db).await;

        assert!(std::path::Path::new(&wt_path).exists());
        assert_eq!(
            std::fs::read_to_string(std::path::Path::new(&wt_path).join("vendor/sub/.env"))
                .unwrap(),
            "SECRET=keep"
        );
    }

    #[tokio::test]
    async fn retains_dirty_unowned_scope_for_manual_recovery() {
        let (_git_tmp, repo_root) = init_repo();
        let db = fresh_db().await;
        let project = db
            .find_or_create_project(repo_root.to_str().unwrap())
            .await
            .unwrap();
        let (wt_path, mode) = work_mode_at(&repo_root, "dirty", "task-dirty-retain");
        add_worktree(&repo_root, &wt_path, "task-dirty-retain");
        std::fs::write(
            std::path::Path::new(&wt_path).join("evidence.txt"),
            "keep me",
        )
        .unwrap();

        seed_work_conv(&db, "dirty", "dirty", &wt_path, &mode, &project.id).await;
        set_state(&db, "dirty", &ConvState::Terminal).await;

        reconcile_worktrees(&db).await;

        assert!(std::path::Path::new(&wt_path).exists());
        assert!(std::path::Path::new(&wt_path).join("evidence.txt").exists());
    }

    #[tokio::test]
    async fn leaves_live_worktree_scope_untouched() {
        let (_git_tmp, repo_root) = init_repo();
        let db = fresh_db().await;
        let project = db
            .find_or_create_project(repo_root.to_str().unwrap())
            .await
            .unwrap();
        let (wt_path, mode) = work_mode_at(&repo_root, "live", "task-live-keep");
        add_worktree(&repo_root, &wt_path, "task-live-keep");
        seed_work_conv(&db, "live", "live", &wt_path, &mode, &project.id).await;

        reconcile_worktrees(&db).await;

        assert!(std::path::Path::new(&wt_path).exists());
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
            let status = phoenix_core::git::command()
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
        let status = phoenix_core::git::command()
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
        let status = phoenix_core::git::command()
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
