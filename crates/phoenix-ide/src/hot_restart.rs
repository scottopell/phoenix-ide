//! Hot restart support for zero-downtime deployments.
//!
//! Supports three modes:
//!
//! 1. **Socket activation** (recommended for production):
//!    - systemd or launchd owns the socket
//!    - On SIGHUP, we exit cleanly; the service manager restarts us with the same socket
//!    - Zero-downtime: socket never closes during upgrade
//!
//! 2. **Dev mode** (normal binding):
//!    - If no activation socket is passed, bind fresh on startup
//!    - SIGHUP triggers graceful shutdown without restart
//!
//! 3. **Daemon mode** (for non-socket-activated environments):
//!    - Detached deploy flow handles stop/copy/start
//!
//! The mode is auto-detected from the service manager's socket-passing contract.

use chrono::{DateTime, Utc};
use std::net::SocketAddr;
#[cfg(target_os = "macos")]
use std::os::fd::FromRawFd;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

/// Flag indicating SIGHUP was received (reload requested)
static HOT_RESTART_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Tracks process start time for uptime reporting on shutdown
static START_TIME: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Tracks process start wall-clock time for absolute start-time reporting.
static START_WALL: std::sync::OnceLock<DateTime<Utc>> = std::sync::OnceLock::new();

/// Call at startup to record the process start time.
pub fn record_start_time() {
    START_TIME.get_or_init(Instant::now);
    START_WALL.get_or_init(Utc::now);
}

/// Process uptime in seconds, or 0 if the start time was never recorded.
pub fn uptime_secs() -> u64 {
    START_TIME.get().map_or(0, |t| t.elapsed().as_secs())
}

/// Wall-clock time the process started, if it was recorded.
pub fn started_at() -> Option<DateTime<Utc>> {
    START_WALL.get().copied()
}

/// Tracks how the listener was acquired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    None,
    Systemd,
    Launchd,
}

impl Activation {
    fn as_u8(self) -> u8 {
        match self {
            Activation::None => 0,
            Activation::Systemd => 1,
            Activation::Launchd => 2,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Activation::Systemd,
            2 => Activation::Launchd,
            _ => Activation::None,
        }
    }
}

/// Tracks whether the listener came from socket activation.
static ACTIVATION: AtomicU8 = AtomicU8::new(0);

/// Get a TCP listener, either from socket activation or freshly bound.
///
/// Systemd socket activation is detected via `LISTEN_FDS` / `LISTEN_PID`.
/// macOS launchd activation is detected by attempting to activate the `Listeners`
/// socket name from the launchd plist.
pub async fn get_listener(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let mut listenfd = listenfd::ListenFd::from_env();

    if listenfd.len() > 0 {
        tracing::info!(
            fd_count = listenfd.len(),
            "Detected systemd socket activation"
        );

        if let Some(std_listener) = listenfd.take_tcp_listener(0)? {
            tracing::info!("Using systemd-provided TCP listener");
            ACTIVATION.store(Activation::Systemd.as_u8(), Ordering::SeqCst);
            std_listener.set_nonblocking(true)?;
            let listener = TcpListener::from_std(std_listener)?;
            configure_tcp_options(&listener)?;
            return Ok(listener);
        }

        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "LISTEN_FDS set but no TCP listener at FD 3",
        ));
    }

    #[cfg(target_os = "macos")]
    if let Some(std_listener) = take_launchd_listener()? {
        tracing::info!("Using launchd-provided TCP listener");
        ACTIVATION.store(Activation::Launchd.as_u8(), Ordering::SeqCst);
        std_listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(std_listener)?;
        configure_tcp_options(&listener)?;
        return Ok(listener);
    }

    tracing::debug!(addr = %addr, "Binding fresh listener (no socket activation)");
    let listener = TcpListener::bind(addr).await?;
    configure_tcp_options(&listener)?;
    Ok(listener)
}

#[cfg(target_os = "macos")]
const LAUNCHD_SOCKET_NAME: &str = "Listeners";

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn launch_activate_socket(
        name: *const libc::c_char,
        fds: *mut *mut libc::c_int,
        cnt: *mut libc::size_t,
    ) -> libc::c_int;
}

#[cfg(target_os = "macos")]
fn take_launchd_listener() -> std::io::Result<Option<std::net::TcpListener>> {
    let name = std::ffi::CString::new(LAUNCHD_SOCKET_NAME).expect("static socket name has no nul");
    let mut fds: *mut libc::c_int = std::ptr::null_mut();
    let mut count: libc::size_t = 0;

    let rc = unsafe { launch_activate_socket(name.as_ptr(), &raw mut fds, &raw mut count) };
    if rc == libc::ESRCH || rc == libc::ENOENT {
        return Ok(None);
    }
    if rc != 0 {
        return Err(std::io::Error::from_raw_os_error(rc));
    }
    if count == 0 {
        if !fds.is_null() {
            unsafe { libc::free(fds.cast()) };
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "launchd activated the socket name but returned no sockets",
        ));
    }
    if fds.is_null() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "launchd returned sockets without an fd array",
        ));
    }

    if count > 1 {
        let fd_count = count;
        unsafe {
            let fd_slice = std::slice::from_raw_parts(fds, count);
            for fd in fd_slice {
                libc::close(*fd);
            }
            libc::free(fds.cast());
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "launchd returned {fd_count} sockets for {LAUNCHD_SOCKET_NAME}; Phoenix requires a single listener"
            ),
        ));
    }

    tracing::info!(
        socket_name = LAUNCHD_SOCKET_NAME,
        fd_count = count,
        "Detected launchd socket activation"
    );

    let listener = unsafe {
        let fd = *fds;
        let listener = std::net::TcpListener::from_raw_fd(fd);
        libc::free(fds.cast());
        listener
    };

    Ok(Some(listener))
}

/// Set TCP keepalive and user timeout on the listener socket.
///
/// Options are inherited by all accepted connections, so stale clients
/// (e.g. SSE streams whose TCP FIN was lost in transit) are reaped within
/// ~90s instead of the OS default of ~2 hours.
///
/// - `SO_KEEPALIVE` + `TCP_KEEPIDLE`/`INTVL`: reap truly idle connections
/// - `TCP_USER_TIMEOUT`: reap connections where a write (e.g. SSE ping) goes
///   unacknowledged — the critical path for our 15s app-level keepalive
fn configure_tcp_options(listener: &TcpListener) -> std::io::Result<()> {
    use socket2::{SockRef, TcpKeepalive};

    let sock = SockRef::from(listener);

    let keepalive = TcpKeepalive::new()
        .with_time(Duration::from_secs(60))
        .with_interval(Duration::from_secs(10));

    #[cfg(target_os = "linux")]
    let keepalive = keepalive.with_retries(3);

    sock.set_tcp_keepalive(&keepalive)?;

    #[cfg(target_os = "linux")]
    sock.set_tcp_user_timeout(Some(Duration::from_secs(60)))?;

    Ok(())
}

/// Check which socket activation mechanism supplied the listener.
pub fn activation() -> Activation {
    Activation::from_u8(ACTIVATION.load(Ordering::SeqCst))
}

/// Check if running under socket activation.
pub fn is_socket_activated() -> bool {
    activation() != Activation::None
}

/// Signal handler that triggers shutdown.
/// Returns when the server should shut down.
///
/// - SIGHUP: For socket-activated mode, exits immediately (systemd restarts with same socket).
///   For non-socket mode, triggers graceful shutdown.
/// - SIGTERM/SIGINT: Triggers graceful shutdown.
pub async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sighup = signal(SignalKind::hangup()).expect("Failed to install SIGHUP handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to install SIGINT handler");

    tokio::select! {
        _ = sighup.recv() => {
            HOT_RESTART_REQUESTED.store(true, Ordering::SeqCst);
            if is_socket_activated() {
                tracing::info!(uptime_secs = uptime_secs(), "Received SIGHUP (socket-activated) - exiting immediately");
                std::process::exit(0);
            } else {
                tracing::info!(uptime_secs = uptime_secs(), "Received SIGHUP (non-socket-activated) - graceful shutdown");
            }
        }
        _ = sigterm.recv() => {
            tracing::info!(uptime_secs = uptime_secs(), signal = "SIGTERM", "Shutting down (likely deploy or manual stop)");
        }
        _ = sigint.recv() => {
            tracing::info!(uptime_secs = uptime_secs(), signal = "SIGINT", "Shutting down (interactive interrupt)");
        }
    }
}

/// Called after graceful shutdown completes.
/// Just logs the shutdown reason.
pub fn maybe_perform_hot_restart() {
    // Note: For socket-activated SIGHUP, we exit immediately in shutdown_signal(),
    // so this function is only reached for SIGTERM/SIGINT or non-socket SIGHUP.
    tracing::info!("Graceful shutdown complete");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn test_initial_state() {
        // Note: These tests may be affected by global state from other tests,
        // so we just verify the atomics are readable without panicking.
        let _requested = HOT_RESTART_REQUESTED.load(Ordering::SeqCst);
    }

    #[test]
    fn test_socket_activation_flag() {
        ACTIVATION.store(Activation::None.as_u8(), Ordering::SeqCst);
        assert_eq!(activation(), Activation::None);
        assert!(!is_socket_activated());

        ACTIVATION.store(Activation::Systemd.as_u8(), Ordering::SeqCst);
        assert_eq!(activation(), Activation::Systemd);
        assert!(is_socket_activated());

        ACTIVATION.store(Activation::Launchd.as_u8(), Ordering::SeqCst);
        assert_eq!(activation(), Activation::Launchd);
        assert!(is_socket_activated());

        ACTIVATION.store(Activation::None.as_u8(), Ordering::SeqCst);
    }

    #[tokio::test]
    async fn test_get_listener_without_socket_activation() {
        // Without LISTEN_FDS env var, should bind fresh
        std::env::remove_var("LISTEN_FDS");
        std::env::remove_var("LISTEN_PID");

        ACTIVATION.store(Activation::None.as_u8(), Ordering::SeqCst);

        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let listener = get_listener(addr).await.expect("Should bind successfully");

        // Should NOT be socket activated
        assert!(!is_socket_activated());

        // Should have bound to some port
        let local_addr = listener.local_addr().expect("Should have local addr");
        assert!(local_addr.port() > 0);

        drop(listener);
    }

    #[tokio::test]
    async fn test_get_listener_with_invalid_listen_fds() {
        // Set LISTEN_FDS but with invalid count
        std::env::set_var("LISTEN_FDS", "0");
        std::env::set_var("LISTEN_PID", std::process::id().to_string());

        ACTIVATION.store(Activation::None.as_u8(), Ordering::SeqCst);

        // Should fall back to normal binding since count is 0
        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let result = get_listener(addr).await;

        // Clean up env vars
        std::env::remove_var("LISTEN_FDS");
        std::env::remove_var("LISTEN_PID");

        // listenfd with 0 FDs means no socket activation, falls through to bind
        assert!(result.is_ok());
        assert!(!is_socket_activated());
    }
}
