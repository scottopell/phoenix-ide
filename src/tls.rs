use axum::Router;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::graceful::GracefulShutdown,
    service::TowerToHyperService,
};
use rustls::ServerConfig;
use rustls_pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use std::{
    collections::BTreeSet,
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

const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

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

impl ConfigSource {
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

pub async fn serve_https(
    listener: TcpListener,
    app: Router,
    tls_config: ServerConfig,
    socket_activated: bool,
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

    tokio::select! {
        () = graceful.shutdown() => {}
        () = tokio::time::sleep(SHUTDOWN_GRACE) => {
            tracing::warn!(
                timeout_seconds = SHUTDOWN_GRACE.as_secs(),
                "Timed out waiting for HTTPS connections to drain"
            );
        }
    }

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

    let home = env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    home.join(".phoenix-ide").join("tls")
}

fn hosts_from_env() -> Vec<String> {
    let mut hosts = BTreeSet::from([
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ]);

    if let Ok(extra) = env::var("PHOENIX_TLS_HOSTS") {
        hosts.extend(
            extra
                .split(',')
                .map(str::trim)
                .filter(|host| !host.is_empty())
                .map(ToOwned::to_owned),
        );
    }

    hosts.into_iter().collect()
}

fn ensure_managed_cert(dir: &Path, hosts: &[String]) -> Result<Paths, Box<dyn Error>> {
    let cert_path = dir.join("phoenix-local-server.pem");
    let key_path = dir.join("phoenix-local-server-key.pem");
    let issued = crate::tls_certs::issue_leaf(dir, &cert_path, &key_path, hosts)?;
    Ok(Paths {
        cert_path: issued.cert_path,
        key_path: issued.key_path,
    })
}
