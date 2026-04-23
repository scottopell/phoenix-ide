//! PTY spawn path — REQ-TERM-001, REQ-TERM-002

use super::command_tracker::CommandTracker;
use super::session::{Dims, ShellIntegrationStatus, StopReason, TerminalHandle};
use nix::{
    pty::openpty,
    unistd::{close, dup2, execve, fork, setsid, ForkResult},
};
use std::{
    ffi::CString,
    os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::Path,
    sync::{Arc, Mutex},
};

/// Spawn a PTY-backed interactive shell in `cwd`.
///
/// Returns a `TerminalHandle` on success. Must be called from a
/// `spawn_blocking` context — `fork` + `exec` cannot run on the tokio executor.
pub fn spawn_pty(cwd: &Path, initial_dims: Dims) -> Result<TerminalHandle, String> {
    // --- PTY creation --------------------------------------------------------
    let pty = openpty(None, None).map_err(|e| format!("openpty: {e}"))?;

    // Extract raw fds before consuming OwnedFd wrappers.
    let master_raw: RawFd = pty.master.as_raw_fd();
    let slave_raw: RawFd = pty.slave.as_raw_fd();

    // Set initial window size on the PTY before the child starts (REQ-TERM-005).
    set_winsize_raw(master_raw, initial_dims).map_err(|e| format!("TIOCSWINSZ (initial): {e}"))?;

    // --- Fork ----------------------------------------------------------------
    // SAFETY: standard POSIX fork + exec pattern.
    let fork_result = unsafe { fork() }.map_err(|e| format!("fork: {e}"))?;

    match fork_result {
        // ---- Child -----------------------------------------------------------
        ForkResult::Child => {
            // New session; child becomes session leader.
            setsid().unwrap_or_else(|e| {
                eprintln!("setsid: {e}");
                unsafe { libc::_exit(1) };
            });

            // Make slave fd the controlling terminal.
            // SAFETY: TIOCSCTTY on a valid slave fd; standard POSIX.
            #[allow(clippy::useless_conversion)] // TIOCSCTTY type varies across glibc targets
            let ret = unsafe { libc::ioctl(slave_raw, libc::TIOCSCTTY.into(), 0) };
            if ret != 0 {
                eprintln!("TIOCSCTTY failed: {}", std::io::Error::last_os_error());
                unsafe { libc::_exit(1) };
            }

            // Wire slave to std{in,out,err}.
            for std_fd in [0, 1, 2] {
                dup2(slave_raw, std_fd).unwrap_or_else(|e| {
                    eprintln!("dup2({slave_raw}, {std_fd}): {e}");
                    unsafe { libc::_exit(1) };
                });
            }

            // Close the original slave fd now that it's been dup'd.
            let _ = close(slave_raw);

            // Close master in child — child doesn't use it, and holding it
            // open prevents EIO delivery when the child exits.
            let _ = close(master_raw);

            // Change to conversation working directory.
            if let Err(e) = std::env::set_current_dir(cwd) {
                eprintln!("chdir {}: {e}", cwd.display());
                // Non-fatal: shell will start in an unspecified directory.
            }

            let shell_path = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_owned());

            let env_pairs = build_env(&shell_path);
            let env_cstrings: Vec<CString> = env_pairs
                .iter()
                .filter_map(|(k, v)| CString::new(format!("{k}={v}")).ok())
                .collect();

            let shell_c = CString::new(shell_path.clone())
                .unwrap_or_else(|_| CString::new("/bin/bash").unwrap());
            let arg_i = CString::new("-i").unwrap();

            let _ = execve(&shell_c, &[shell_c.clone(), arg_i], &env_cstrings);
            eprintln!("execve {shell_path}: {}", std::io::Error::last_os_error());
            unsafe { libc::_exit(1) };
        }

        // ---- Parent ----------------------------------------------------------
        ForkResult::Parent { child } => {
            // Parent must close slave fd — if parent holds slave open,
            // EIO will never fire on the master when the child exits.
            let _ = close(slave_raw);
            // Forget the OwnedFd wrappers to avoid double-close.
            // master_raw is now owned by the OwnedFd below.
            std::mem::forget(pty.slave);

            // SAFETY: master_raw is a valid fd; we take ownership here.
            let master_fd = unsafe { OwnedFd::from_raw_fd(master_raw) };
            // Forget the openpty OwnedFd to avoid double-close.
            std::mem::forget(pty.master);

            // Use conversation_id placeholder; ws.rs updates tracker session when
            // the handle is registered. The conversation_id is not available at spawn
            // time, so we use the child PID as a unique session identifier.
            let session_id = child.to_string();

            let (stop_tx, _stop_rx) = tokio::sync::watch::channel(StopReason::Running);

            Ok(TerminalHandle {
                master_fd,
                child_pid: child,
                tracker: Arc::new(Mutex::new(CommandTracker::new(session_id))),
                shell_integration_status: Arc::new(Mutex::new(ShellIntegrationStatus::Unknown)),
                stop_tx,
                detached: Arc::new(tokio::sync::Notify::new()),
            })
        }
    }
}

/// Apply window size to a PTY master fd (raw).
pub fn set_winsize_raw(fd: RawFd, dims: Dims) -> Result<(), String> {
    let ws = libc::winsize {
        ws_col: dims.cols,
        ws_row: dims.rows,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: fd is a valid PTY master; TIOCSWINSZ is safe on Linux.
    let ret = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
    if ret == 0 {
        Ok(())
    } else {
        Err(format!(
            "ioctl TIOCSWINSZ: {}",
            std::io::Error::last_os_error()
        ))
    }
}

/// Construct an explicit minimal environment for the shell (REQ-TERM-002).
///
/// Never inherits the API server's environment — prevents secret leakage.
pub(crate) fn build_env(shell_path: &str) -> Vec<(String, String)> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_owned());
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "user".to_owned());
    let path = std::env::var("PATH").unwrap_or_else(|_| {
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_owned()
    });

    vec![
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("HOME".into(), home),
        ("USER".into(), user.clone()),
        ("LOGNAME".into(), user),
        ("SHELL".into(), shell_path.to_owned()),
        ("PATH".into(), path),
        ("LANG".into(), "en_US.UTF-8".into()),
        // Shell integration hints: tell prompts that OSC 133 is expected.
        // powerlevel10k specifically gates its A/B emission on
        // $ITERM_SHELL_INTEGRATION_INSTALLED=Yes; setting this makes p10k
        // emit prompt markers without requiring a separate iTerm2 script.
        // TERM_PROGRAM is set for forward compatibility with prompts that
        // detect the host terminal by name rather than by env-var sniffing.
        ("ITERM_SHELL_INTEGRATION_INSTALLED".into(), "Yes".into()),
        ("TERM_PROGRAM".into(), "phoenix-ide".into()),
    ]
}

/// Set a file descriptor to non-blocking mode.
pub fn set_nonblocking(fd: libc::c_int) -> Result<(), std::io::Error> {
    // SAFETY: `fcntl` syscall on a valid fd.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let ret = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if ret == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
