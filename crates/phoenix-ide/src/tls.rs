use axum::{extract::ConnectInfo, Router};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::graceful::GracefulShutdown,
    service::TowerToHyperService,
};
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use rustls::ServerConfig;
use rustls_pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use std::{
    env,
    error::Error,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::pin,
    sync::Arc,
    time::Duration,
};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// Upper bound on how long either server path (HTTP in `main`, HTTPS here)
/// waits for in-flight connections to drain on shutdown. An SSE stream with
/// keepalive pings never completes on its own, so without this bound a
/// single such client pins the process alive past a deploy indefinitely.
pub(crate) const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

/// Bound the post-shutdown drain by [`SHUTDOWN_GRACE`]. Both server paths
/// (HTTP `axum::serve` task + HTTPS `hyper_util` graceful shutdown) route
/// through here so the deadline lives in exactly one place — re-introducing
/// the shutdown-clock divergence from task 02708 + PR #117 now requires
/// editing this function (or its only constant) rather than two unrelated
/// `tokio::time::timeout` / `tokio::select! { … sleep(…) }` sites.
///
/// Returns `Some(drain_result)` if the drain completed within the bound;
/// `None` if the deadline elapsed first. Callers that need to *force-close*
/// remaining work on timeout (e.g. aborting an `axum::serve` `JoinHandle`) do
/// that themselves on `None`; this helper does not own task ownership.
pub(crate) async fn bounded_post_shutdown_drain_until<F>(
    deadline: tokio::time::Instant,
    drain: F,
    label: &'static str,
) -> Option<F::Output>
where
    F: std::future::Future,
{
    match tokio::time::timeout_at(deadline, drain).await {
        Ok(v) => Some(v),
        Err(_elapsed) => {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            tracing::warn!(
                timeout_seconds = SHUTDOWN_GRACE.as_secs(),
                remaining_ms = remaining.as_millis(),
                "Timed out waiting for {label} connections to drain"
            );
            None
        }
    }
}

pub(crate) async fn bounded_post_shutdown_drain<F>(
    drain: F,
    label: &'static str,
) -> Option<F::Output>
where
    F: std::future::Future,
{
    bounded_post_shutdown_drain_until(tokio::time::Instant::now() + SHUTDOWN_GRACE, drain, label)
        .await
}

#[derive(Debug, Clone)]
pub(crate) enum ConfigSource {
    Manual(Paths),
    Auto { dir: PathBuf, hosts: Vec<String> },
}

#[derive(Debug, Clone)]
pub(crate) struct Paths {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

pub(crate) struct LoadedConfig {
    pub server: ServerConfig,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub ca_cert_path: Option<PathBuf>,
    pub mode: &'static str,
}

/// Whether a configured TLS host is a loopback name/address rather than an
/// externally reachable domain.
fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

impl ConfigSource {
    /// The operator's chosen externally reachable host (the first non-loopback
    /// `PHOENIX_TLS_HOSTS` entry), if any. This is the single domain that drives
    /// both the TLS certificate and the OAuth redirect origin, so a remote
    /// deployment needs no separate redirect knob (REQ-MCP-020). `None` for
    /// manual TLS (the domain lives in the externally provided cert) -- those
    /// rely on `PHOENIX_EXTERNAL_URL`.
    pub(crate) fn external_host(&self) -> Option<String> {
        match self {
            Self::Auto { hosts, .. } => hosts.iter().find(|h| !is_loopback_host(h)).cloned(),
            Self::Manual(_) => None,
        }
    }

    pub(crate) fn from_env(db_path: &str) -> Result<Option<Self>, Box<dyn Error>> {
        let cert_path = env::var_os("PHOENIX_TLS_CERT_PATH");
        let key_path = env::var_os("PHOENIX_TLS_KEY_PATH");
        let mode = env::var("PHOENIX_TLS").unwrap_or_default();

        match (cert_path, key_path) {
            (Some(cert_path), Some(key_path)) => Ok(Some(Self::Manual(Paths {
                cert_path: PathBuf::from(cert_path),
                key_path: PathBuf::from(key_path),
            }))),
            (Some(_), None) => {
                Err("PHOENIX_TLS_CERT_PATH is set but PHOENIX_TLS_KEY_PATH is missing".into())
            }
            (None, Some(_)) => {
                Err("PHOENIX_TLS_KEY_PATH is set but PHOENIX_TLS_CERT_PATH is missing".into())
            }
            (None, None) => match mode.trim().to_ascii_lowercase().as_str() {
                "" | "0" | "false" | "off" | "none" => Ok(None),
                "1" | "true" | "on" | "auto" => Ok(Some(Self::Auto {
                    dir: tls_dir_from_env(db_path),
                    hosts: hosts_from_env(),
                })),
                "manual" => Err(
                    "PHOENIX_TLS=manual requires PHOENIX_TLS_CERT_PATH and PHOENIX_TLS_KEY_PATH"
                        .into(),
                ),
                other => Err(format!(
                    "unsupported PHOENIX_TLS value {other:?}; use off, auto, or manual"
                )
                .into()),
            },
        }
    }
}

pub(crate) fn load_config(source: &ConfigSource) -> Result<LoadedConfig, Box<dyn Error>> {
    let (paths, ca_cert_path, mode) = match source {
        ConfigSource::Manual(paths) => (paths.clone(), None, "manual"),
        ConfigSource::Auto { dir, hosts } => {
            let managed = ensure_managed_cert(dir, hosts)?;
            let ca_cert_path = Some(dir.join("phoenix-local-ca.pem"));
            (managed, ca_cert_path, "auto")
        }
    };

    let certs = load_certs(&paths.cert_path)?;
    let key = load_key(&paths.key_path)?;

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(LoadedConfig {
        server: config,
        cert_path: paths.cert_path,
        key_path: paths.key_path,
        ca_cert_path,
        mode,
    })
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, Box<dyn Error>> {
    let pem = fs::read(path)?;
    let certs: Result<Vec<_>, _> = CertificateDer::pem_slice_iter(&pem).collect();
    let certs = certs?;
    if certs.is_empty() {
        return Err(format!("no certificates found in {}", path.display()).into());
    }
    Ok(certs)
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>, Box<dyn Error>> {
    let pem = fs::read(path)?;
    PrivateKeyDer::from_pem_slice(&pem)
        .map_err(|e| format!("failed to load private key from {}: {e}", path.display()).into())
}

pub(crate) async fn wait_for_fatal_local_authority(
    receiver: &mut tokio::sync::watch::Receiver<Option<&'static str>>,
) -> &'static str {
    loop {
        if let Some(boundary) = *receiver.borrow() {
            return boundary;
        }
        match receiver.changed().await {
            Ok(()) => {
                if let Some(boundary) = *receiver.borrow() {
                    return boundary;
                }
            }
            Err(_) => std::future::pending::<()>().await,
        }
    }
}

pub(crate) async fn drain_concurrently<OwnerDrain, ConnectionDrain>(
    owner_drain: OwnerDrain,
    connection_drain: ConnectionDrain,
) where
    OwnerDrain: std::future::Future,
    ConnectionDrain: std::future::Future,
{
    let (_, _) = tokio::join!(owner_drain, connection_drain);
}

pub async fn serve_https(
    listener: TcpListener,
    app: Router,
    tls_config: ServerConfig,
    socket_activated: bool,
    mut fatal_local_authority_rx: tokio::sync::watch::Receiver<Option<&'static str>>,
    runtime: &crate::runtime::RuntimeManager,
    bash_handles: &crate::tools::bash::BashHandleRegistry,
) -> Result<(), Box<dyn Error>> {
    let local_addr = listener.local_addr()?;
    tracing::info!(
        addr = %local_addr,
        socket_activated,
        alpn = "h2,http/1.1",
        "Phoenix IDE HTTPS server listening"
    );

    let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));
    let server = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    let graceful = GracefulShutdown::new();
    let shutdown = crate::hot_restart::shutdown_signal();
    let mut shutdown = pin!(shutdown);

    loop {
        tokio::select! {
            () = &mut shutdown => {
                drop(listener);
                tracing::info!("HTTPS listener stopped accepting new connections");
                break;
            }
            boundary = wait_for_fatal_local_authority(&mut fatal_local_authority_rx) => {
                drop(listener);
                tracing::error!(?boundary, "fatal local SQLite authority loss; stopping HTTPS without database cleanup");
                let deadline = runtime
                    .fatal_local_authority_deadline()
                    .expect("fatal authority deadline must be set before HTTPS drain");
                let fatal_tail = async {
                    drain_concurrently(
                        runtime.fence_fatal_local_authority(),
                        graceful.shutdown(),
                    )
                    .await;
                    crate::tools::bash::shutdown_kill_tree_until(deadline, bash_handles).await;
                };
                let _ = bounded_post_shutdown_drain_until(
                    deadline,
                    fatal_tail,
                    "HTTPS fatal authority",
                )
                .await;
                return Err(crate::FatalLocalAuthorityExit.into());
            }
            accepted = listener.accept() => {
                let (stream, peer_addr) = match accepted {
                    Ok(conn) => conn,
                    Err(e) => {
                        tracing::warn!(error = %e, "HTTPS accept failed");
                        continue;
                    }
                };

                if let Err(e) = stream.set_nodelay(true) {
                    tracing::debug!(peer = %peer_addr, error = %e, "Failed to set TCP_NODELAY");
                }

                let app = app.clone();
                let tls_acceptor = tls_acceptor.clone();
                let server = server.clone();
                let watcher = graceful.watcher();

                tokio::spawn(async move {
                    let stream = match tls_acceptor.accept(stream).await {
                        Ok(stream) => stream,
                        Err(e) => {
                            tracing::debug!(peer = %peer_addr, error = %e, "TLS handshake failed");
                            return;
                        }
                    };

                    log_alpn(peer_addr, &stream);

                    let io = TokioIo::new(stream);
                    // Inject the real peer address as `ConnectInfo<SocketAddr>`
                    // into each request's extensions, mirroring what the plain
                    // path's `into_make_service_with_connect_info` does. Without
                    // this the `ConnectInfo` extractor (used by `auth_login` to
                    // key the login throttle on the unspoofable peer IP) would
                    // fail on every HTTPS request. Applied per-connection so the
                    // captured `peer_addr` is the address for this connection.
                    let app = app.layer(axum::Extension(ConnectInfo(peer_addr)));
                    let service = TowerToHyperService::new(app);
                    let conn = server.serve_connection_with_upgrades(io, service);
                    let conn = watcher.watch(conn);
                    if let Err(e) = conn.await {
                        tracing::debug!(peer = %peer_addr, error = %e, "HTTPS connection error");
                    }
                });
            }
        }
    }

    let _ = bounded_post_shutdown_drain(graceful.shutdown(), "HTTPS").await;

    Ok(())
}

fn log_alpn(
    peer_addr: SocketAddr,
    stream: &tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
) {
    let protocol = stream.get_ref().1.alpn_protocol().map_or_else(
        || "none".to_string(),
        |proto| String::from_utf8_lossy(proto).into_owned(),
    );
    tracing::debug!(peer = %peer_addr, alpn = %protocol, "TLS connection accepted");
}

fn tls_dir_from_env(db_path: &str) -> PathBuf {
    if let Some(path) = env::var_os("PHOENIX_TLS_DIR") {
        return PathBuf::from(path);
    }

    let db_parent = Path::new(db_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = db_parent {
        return parent.join("tls");
    }

    PhoenixRuntimeEnvironment::detect()
        .phoenix_home()
        .join("tls")
}

fn hosts_from_env() -> Vec<String> {
    // Insertion order is preserved (deduped): the cert's SAN order is
    // immaterial, but `external_host` takes the first non-loopback entry as the
    // canonical OAuth redirect host (REQ-MCP-020), so the operator's first
    // `PHOENIX_TLS_HOSTS` entry -- their intended primary name -- must win over
    // a later one. A `BTreeSet` would reorder `phoenix.example.com,10.0.0.5`
    // into `10.0.0.5` first.
    let mut hosts: Vec<String> = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];

    if let Ok(extra) = env::var("PHOENIX_TLS_HOSTS") {
        for host in extra
            .split(',')
            .map(str::trim)
            .filter(|host| !host.is_empty())
        {
            if !hosts.iter().any(|existing| existing == host) {
                hosts.push(host.to_string());
            }
        }
    }

    hosts
}

fn ensure_managed_cert(dir: &Path, hosts: &[String]) -> Result<Paths, Box<dyn Error>> {
    let cert_path = dir.join("phoenix-local-server.pem");
    let key_path = dir.join("phoenix-local-server-key.pem");
    let issued = phoenix_tls::issue_leaf(dir, &cert_path, &key_path, hosts)?;
    Ok(Paths {
        cert_path: issued.cert_path,
        key_path: issued.key_path,
    })
}

#[cfg(test)]
mod external_host_tests {
    #[tokio::test]
    async fn fatal_https_starts_connection_drain_while_owner_drain_is_pending() {
        let (owner_started_tx, owner_started_rx) = tokio::sync::oneshot::channel();
        let (connection_started_tx, connection_started_rx) = tokio::sync::oneshot::channel();
        let (release_owner_tx, release_owner_rx) = tokio::sync::oneshot::channel();
        let (release_connection_tx, release_connection_rx) = tokio::sync::oneshot::channel();

        let drain = tokio::spawn(super::drain_concurrently(
            async move {
                let _ = owner_started_tx.send(());
                let _ = release_owner_rx.await;
            },
            async move {
                let _ = connection_started_tx.send(());
                let _ = release_connection_rx.await;
            },
        ));

        owner_started_rx.await.expect("owner drain polled");
        connection_started_rx
            .await
            .expect("connection drain polled before owner completion");
        release_owner_tx.send(()).expect("release owner drain");
        release_connection_tx
            .send(())
            .expect("release connection drain");
        drain.await.expect("concurrent drains join");
    }

    use super::ConfigSource;
    use std::path::PathBuf;

    #[test]
    fn external_host_takes_the_first_non_loopback_in_order() {
        // The operator's first PHOENIX_TLS_HOSTS entry (their intended primary
        // name) must win over a later one; loopback defaults are skipped.
        let cfg = ConfigSource::Auto {
            dir: PathBuf::from("/tmp"),
            hosts: vec![
                "localhost".into(),
                "127.0.0.1".into(),
                "::1".into(),
                "phoenix.example.com".into(),
                "10.0.0.5".into(),
            ],
        };
        assert_eq!(cfg.external_host().as_deref(), Some("phoenix.example.com"));
    }

    #[test]
    fn external_host_is_none_without_a_configured_domain() {
        let cfg = ConfigSource::Auto {
            dir: PathBuf::from("/tmp"),
            hosts: vec!["localhost".into(), "127.0.0.1".into(), "::1".into()],
        };
        assert_eq!(cfg.external_host(), None);
    }
}

#[cfg(test)]
mod bounded_drain_tests {
    use super::{
        bounded_post_shutdown_drain, bounded_post_shutdown_drain_until,
        wait_for_fatal_local_authority,
    };

    /// A drain future that completes before the deadline forwards its
    /// inner value verbatim. The contract `Some(F::Output) iff fast
    /// enough` is what both server paths rely on to decide whether to
    /// force-abort: HTTP aborts the `axum::serve` `JoinHandle` when the
    /// result is `None`; HTTPS discards `None`.
    #[tokio::test]
    async fn fast_drain_returns_some_with_inner_value() {
        let result = bounded_post_shutdown_drain(async { "drained" }, "test").await;
        assert_eq!(result, Some("drained"));
    }

    /// A drain future that never completes returns `None` once the
    /// deadline elapses. We can't wait the full `SHUTDOWN_GRACE` in a
    /// unit test, so this exercises the timeout path with a pending
    /// future under `tokio::time::pause()` and an explicit
    /// `advance()` past the grace period.
    ///
    #[tokio::test]
    async fn fatal_local_authority_signal_is_retained_for_late_receiver() {
        let (tx, mut rx) = tokio::sync::watch::channel(None);
        tx.send_replace(Some("direct_turn_terminal_recovery"));

        assert_eq!(
            wait_for_fatal_local_authority(&mut rx).await,
            "direct_turn_terminal_recovery"
        );
    }

    /// The bug class this guards: the regression caught on PR #117 was
    /// that the timeout had been started at *server startup* rather
    /// than at the shutdown signal, causing the non-stuck server to be
    /// aborted every `SHUTDOWN_GRACE` seconds. Routing the bound
    /// through this helper makes that bug structurally unreachable —
    /// `bounded_post_shutdown_drain` *takes* the future, so the clock
    /// cannot start before the function is called.
    #[tokio::test(start_paused = true)]
    async fn slow_drain_returns_none_after_grace_period() {
        let drain = std::future::pending::<()>();
        let join = tokio::spawn(async move { bounded_post_shutdown_drain(drain, "test").await });
        // Yield so the spawned task is polled at least once and the
        // inner `timeout()` registers its sleep before we advance the
        // clock; otherwise the advance fires before the timer exists
        // and the join hangs.
        tokio::task::yield_now().await;
        // Advance just past SHUTDOWN_GRACE so the inner timeout fires.
        tokio::time::advance(super::SHUTDOWN_GRACE + std::time::Duration::from_secs(1)).await;
        let result = join.await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn absolute_deadline_uses_remaining_budget_instead_of_fresh_grace() {
        let deadline = tokio::time::Instant::now() + super::SHUTDOWN_GRACE;
        tokio::time::advance(std::time::Duration::from_secs(20)).await;

        let drain = std::future::pending::<()>();
        let join = tokio::spawn(async move {
            bounded_post_shutdown_drain_until(deadline, drain, "absolute-test").await
        });
        tokio::task::yield_now().await;

        tokio::time::advance(std::time::Duration::from_secs(9)).await;
        tokio::task::yield_now().await;
        assert!(
            !join.is_finished(),
            "remaining budget should still be active"
        );

        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        assert!(join.await.unwrap().is_none());
    }
}
