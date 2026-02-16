//! Hot restart support for zero-downtime deployments.
//!
//! On SIGHUP, this module:
//! 1. Prepares the listening socket for inheritance (clears `FD_CLOEXEC`)
//! 2. Sets `PHOENIX_LISTEN_FD` environment variable
//! 3. Calls `execve()` to replace the process with the new binary
//!
//! The new process detects `PHOENIX_LISTEN_FD` and inherits the socket,
//! maintaining the same PID and never dropping the TCP listener.

use std::net::SocketAddr;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use tokio::net::TcpListener;

/// Environment variable for passing listener FD across exec boundary
const LISTEN_FD_ENV: &str = "PHOENIX_LISTEN_FD";

/// Stored listener FD for hot restart (set before axum consumes the listener)
static LISTENER_FD: AtomicI32 = AtomicI32::new(-1);

/// Flag indicating SIGHUP was received and we should exec after graceful shutdown
static HOT_RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Get a TCP listener, either inherited from a previous process or freshly bound.
pub async fn get_listener(addr: SocketAddr) -> std::io::Result<TcpListener> {
    if let Some(listener) = try_inherit_listener()? {
        tracing::info!("Inherited listener from previous process (hot restart)");
        Ok(listener)
    } else {
        tracing::debug!("Binding fresh listener");
        TcpListener::bind(addr).await
    }
}

/// Store the listener's raw FD for later use in hot restart.
/// Must be called before passing the listener to `axum::serve`.
pub fn store_listener_fd(listener: &TcpListener) {
    let fd = listener.as_raw_fd();
    LISTENER_FD.store(fd, Ordering::SeqCst);
    tracing::debug!(fd, "Stored listener FD for potential hot restart");
}

/// Check if hot restart was requested and perform it if so.
/// Call this after the server has shut down gracefully.
/// Does not return if hot restart is performed.
pub fn maybe_perform_hot_restart() {
    if !HOT_RESTART_REQUESTED.load(Ordering::SeqCst) {
        tracing::info!("Normal shutdown (no hot restart requested)");
        return;
    }

    let fd = LISTENER_FD.load(Ordering::SeqCst);
    if fd < 0 {
        tracing::error!("Hot restart requested but no listener FD stored - performing normal exit");
        return;
    }

    tracing::info!(fd, "Performing hot restart");

    if let Err(e) = prepare_listener_for_exec(fd) {
        tracing::error!(error = %e, "Failed to prepare listener for exec - performing normal exit");
        return;
    }

    // This only returns if execve fails
    let Err(e) = perform_hot_restart(fd);
    tracing::error!(error = %e, "execve failed - performing normal exit");
}

/// Try to inherit a listener from a previous process via `PHOENIX_LISTEN_FD`.
fn try_inherit_listener() -> std::io::Result<Option<TcpListener>> {
    let Ok(fd_str) = std::env::var(LISTEN_FD_ENV) else {
        return Ok(None);
    };

    let fd: RawFd = fd_str.parse().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Invalid {LISTEN_FD_ENV} value '{fd_str}': {e}"),
        )
    })?;

    tracing::info!(fd, "Inheriting listener from file descriptor");

    // SAFETY: We trust that the previous process passed us a valid TCP listener FD.
    // This is set by our own hot_restart logic, not external input.
    let std_listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
    std_listener.set_nonblocking(true)?;

    // Clear the env var so child processes don't try to inherit it
    std::env::remove_var(LISTEN_FD_ENV);

    TcpListener::from_std(std_listener).map(Some)
}

/// Signal handler that triggers hot restart on SIGHUP.
/// Returns when the server should shut down (either for restart or termination).
pub async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sighup = signal(SignalKind::hangup()).expect("Failed to install SIGHUP handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to install SIGINT handler");

    tokio::select! {
        _ = sighup.recv() => {
            tracing::info!("Received SIGHUP - will perform hot restart after graceful shutdown");
            HOT_RESTART_REQUESTED.store(true, Ordering::SeqCst);
        }
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM - shutting down");
        }
        _ = sigint.recv() => {
            tracing::info!("Received SIGINT - shutting down");
        }
    }
}

/// Prepare a listener for inheritance across exec boundary.
/// Clears `FD_CLOEXEC` so the FD survives `execve()`.
fn prepare_listener_for_exec(fd: RawFd) -> std::io::Result<()> {
    use nix::fcntl::{fcntl, FcntlArg, FdFlag};

    // Clear FD_CLOEXEC so the FD survives exec
    fcntl(fd, FcntlArg::F_SETFD(FdFlag::empty()))
        .map_err(|e| std::io::Error::other(format!("Failed to clear FD_CLOEXEC: {e}")))?;

    tracing::debug!(fd, "Cleared FD_CLOEXEC on listener");
    Ok(())
}

/// Perform hot restart by exec'ing the current binary.
/// This function does not return on success.
fn perform_hot_restart(listener_fd: RawFd) -> std::io::Result<std::convert::Infallible> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let binary_path = std::env::current_exe()?;
    let binary_cstr = CString::new(binary_path.as_os_str().as_bytes()).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Invalid binary path: {e}"),
        )
    })?;

    // Collect current args
    let args: Vec<CString> = std::env::args_os()
        .map(|arg| CString::new(arg.as_bytes()).unwrap())
        .collect();

    // Collect current env, adding PHOENIX_LISTEN_FD
    let mut env: Vec<CString> = std::env::vars_os()
        .filter(|(k, _)| k != LISTEN_FD_ENV) // Remove old value if present
        .map(|(k, v)| {
            let mut s = k.as_bytes().to_vec();
            s.push(b'=');
            s.extend_from_slice(v.as_bytes());
            CString::new(s).unwrap()
        })
        .collect();

    // Add the listener FD
    env.push(CString::new(format!("{LISTEN_FD_ENV}={listener_fd}")).unwrap());

    tracing::info!(fd = listener_fd, binary = %binary_path.display(), "Executing new binary");

    // Point of no return - this replaces the process image
    nix::unistd::execve(&binary_cstr, &args, &env)
        .map_err(|e| std::io::Error::other(format!("execve failed: {e}")))?;

    unreachable!("execve returned")
}
