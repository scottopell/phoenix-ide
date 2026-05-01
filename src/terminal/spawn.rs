//! PTY spawn path — REQ-TERM-001, REQ-TERM-002.
//!
//! With tmux integration (REQ-TMUX-004), the PTY child runs `tmux -S
//! <conv-sock> attach -t main` instead of `$SHELL -i` when the tmux
//! binary is available and the conversation has a live server. The
//! fork → setsid → TIOCSCTTY → dup2 sequence is identical between the
//! two paths; only the argv differs.

use super::command_tracker::CommandTracker;
use super::session::{Dims, ShellIntegrationStatus, StopReason, TerminalHandle};
use nix::{
    pty::openpty,
    unistd::{close, dup2, execve, fork, setsid, ForkResult},
};
use std::{
    ffi::CString,
    os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// What the PTY child should exec into.
///
/// `Tmux` carries the conversation's resolved socket path; the child
/// will run `tmux -S <socket> attach -t main` with `TMUX` removed from
/// its environment to avoid outer-tmux nesting refusal (REQ-TMUX-004).
///
/// `Shell` is the v1 fallback: `$SHELL -i` (REQ-TERM-001/002). It runs
/// when the tmux binary isn't available on the host or when the caller
/// chose not to attach (e.g. tests).
#[derive(Debug, Clone)]
pub enum PtyExecPlan {
    Tmux { socket_path: PathBuf },
    Shell,
}

/// Spawn a PTY-backed interactive shell in `cwd`.
///
/// Returns a `TerminalHandle` on success. Must be called from a
/// `spawn_blocking` context — `fork` + `exec` cannot run on the tokio
/// executor.
///
/// The argv branching (REQ-TMUX-004) is materialised before fork by the
/// caller and passed in as `plan` so this function stays synchronous.
pub fn spawn_pty(
    cwd: &Path,
    initial_dims: Dims,
    plan: PtyExecPlan,
) -> Result<TerminalHandle, String> {
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

            // The tmux branch builds an env that strips `TMUX` so an
            // outer-tmux `phoenix-ide` invocation doesn't trigger
            // tmux's nesting refusal ("sessions should be nested with
            // care"). The shell branch keeps the v1 env unchanged.
            let env_pairs = match &plan {
                PtyExecPlan::Tmux { .. } => build_env_for_tmux(&shell_path),
                PtyExecPlan::Shell => build_env(&shell_path),
            };
            let env_cstrings: Vec<CString> = env_pairs
                .iter()
                .filter_map(|(k, v)| CString::new(format!("{k}={v}")).ok())
                .collect();

            match plan {
                PtyExecPlan::Tmux { socket_path } => {
                    let tmux_c = CString::new("tmux").unwrap();
                    let dash_s = CString::new("-S").unwrap();
                    let sock_c = CString::new(socket_path.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| CString::new("").unwrap());
                    let attach = CString::new("attach").unwrap();
                    let dash_t = CString::new("-t").unwrap();
                    let main_c = CString::new("main").unwrap();
                    // `tmux` may not be on a known absolute path; use
                    // `execvpe` to traverse PATH. nix exposes execvp;
                    // we want envp control too, so call libc directly.
                    let argv: Vec<CString> =
                        vec![tmux_c.clone(), dash_s, sock_c, attach, dash_t, main_c];
                    exec_via_path(&tmux_c, &argv, &env_cstrings);
                    eprintln!("execvpe tmux: {}", std::io::Error::last_os_error());
                    unsafe { libc::_exit(1) };
                }
                PtyExecPlan::Shell => {
                    let shell_c = CString::new(shell_path.clone())
                        .unwrap_or_else(|_| CString::new("/bin/bash").unwrap());
                    let arg_i = CString::new("-i").unwrap();
                    let _ = execve(&shell_c, &[shell_c.clone(), arg_i], &env_cstrings);
                    eprintln!("execve {shell_path}: {}", std::io::Error::last_os_error());
                    unsafe { libc::_exit(1) };
                }
            }
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
                // 1 permit: the attached relay holds it for its lifetime; a
                // reclaimer must wait for the sitting relay to drop the permit
                // before proceeding. See `TerminalHandle::attach_permit`.
                attach_permit: Arc::new(tokio::sync::Semaphore::new(1)),
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

/// Same as [`build_env`] but explicitly omits `TMUX`. When Phoenix is
/// launched from inside an outer tmux session, the inherited `TMUX`
/// env var causes the inner tmux to refuse to nest by default
/// ("sessions should be nested with care"). REQ-TMUX-004 / spec
/// design.md §"TMUX env handling".
pub(crate) fn build_env_for_tmux(shell_path: &str) -> Vec<(String, String)> {
    // build_env() never includes TMUX in the first place — its caller
    // populates the env from a fixed list. We re-use it verbatim and
    // belt-and-braces drop any TMUX-prefixed key just in case the list
    // grows in the future.
    build_env(shell_path)
        .into_iter()
        .filter(|(k, _)| k != "TMUX" && k != "TMUX_PANE")
        .collect()
}

/// Path-resolving `execve` for the tmux attach branch. Walks `PATH`
/// from the supplied env, attempting `execve` against each candidate.
/// Returns only on failure (the successful path replaces the process
/// image).
fn exec_via_path(prog: &CString, argv: &[CString], envp: &[CString]) {
    // First, try execve() against the literal name in case it's already
    // an absolute path or the user shelled out from a directory tmux
    // happens to live in.
    let _ = execve(prog, argv, envp);

    // Fallback: walk PATH. Pull from the env we're about to hand the
    // child rather than the parent's env so the resolution matches what
    // the child will see.
    let path = envp
        .iter()
        .find_map(|cstr| {
            let s = cstr.to_string_lossy();
            s.strip_prefix("PATH=").map(str::to_owned)
        })
        .unwrap_or_else(|| "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into());

    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = std::path::PathBuf::from(dir).join(prog.to_string_lossy().as_ref());
        let Ok(c_candidate) = CString::new(candidate.to_string_lossy().into_owned()) else {
            continue;
        };
        let _ = execve(&c_candidate, argv, envp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_env_for_tmux_omits_tmux_keys() {
        let env = build_env_for_tmux("/bin/bash");
        for (k, _) in &env {
            assert_ne!(k, "TMUX", "TMUX env var must be stripped on tmux path");
            assert_ne!(k, "TMUX_PANE", "TMUX_PANE env var must be stripped");
        }
        // Belt-and-braces: even though build_env doesn't add TMUX,
        // verify standard keys do still come through. If a future
        // refactor accidentally drops them this fails loudly.
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"PATH"));
        assert!(keys.contains(&"HOME"));
        assert!(keys.contains(&"TERM"));
    }

    #[test]
    fn pty_exec_plan_tmux_carries_socket_path() {
        let plan = PtyExecPlan::Tmux {
            socket_path: PathBuf::from("/tmp/phoenix-ide/tmux-sockets/conv-test.sock"),
        };
        match plan {
            PtyExecPlan::Tmux { socket_path } => {
                assert!(socket_path.to_string_lossy().contains("conv-test"));
            }
            PtyExecPlan::Shell => panic!("expected Tmux variant"),
        }
    }

    #[test]
    fn pty_exec_plan_shell_is_fallback_carrier() {
        let plan = PtyExecPlan::Shell;
        match plan {
            PtyExecPlan::Shell => {}
            PtyExecPlan::Tmux { .. } => panic!("expected Shell variant"),
        }
    }
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
