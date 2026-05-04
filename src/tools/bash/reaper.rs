//! Child process reaper.
//!
//! REQ-BASH-007: at startup, set the subreaper bit so descendants whose
//! parent dies (double-forks, setsid daemons) reparent to Phoenix rather
//! than init. At shutdown, walk the live handle table and SIGKILL every
//! process group as a final cleanup pass.

use std::time::Duration;

use super::registry::BashHandleRegistry;

/// Time Phoenix waits at shutdown for SIGKILL'd groups to exit before
/// returning control (REQ-BASH-007: `SHUTDOWN_KILL_GRACE_SECONDS`).
pub const SHUTDOWN_KILL_GRACE_SECONDS: u64 = 2;

/// Set `PR_SET_CHILD_SUBREAPER` on Linux; log-and-degrade elsewhere.
///
/// Must be called once at Phoenix startup, before any tool routes accept
/// calls. Idempotent — `prctl` with the same value is a no-op the second
/// time. Errors are logged at WARN level; the process continues either
/// way (a missing reaper bit only weakens the cleanup guarantee for
/// double-forked descendants).
pub fn install_reaper() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: prctl with PR_SET_CHILD_SUBREAPER and arg2=1 is a
        // process-level flag flip; no memory or fd implications.
        let rc = unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1u64, 0u64, 0u64, 0u64) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            tracing::warn!(
                error = %err,
                "PR_SET_CHILD_SUBREAPER failed; orphaned descendants will reparent to init \
                 instead of Phoenix — bash handle cleanup may leak escapees"
            );
        } else {
            tracing::info!("PR_SET_CHILD_SUBREAPER installed (subreaper bit set)");
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        tracing::warn!(
            "PR_SET_CHILD_SUBREAPER unavailable on this OS; descendants that escape \
             their process group may leak on Phoenix exit"
        );
    }
}

/// Walk the live handle table and SIGKILL every process group, then wait
/// briefly (up to [`SHUTDOWN_KILL_GRACE_SECONDS`]) for the kernel to
/// deliver before returning.
///
/// SIGKILL rather than SIGTERM (REQ-BASH-007 rationale): Phoenix is
/// exiting, so graceful-shutdown handlers in children would race with
/// Phoenix's own exit. Reaping is the goal; courtesy is not.
pub async fn shutdown_kill_tree(registry: &BashHandleRegistry) {
    let pgids = registry.snapshot_live_pgids().await;
    if pgids.is_empty() {
        tracing::debug!("shutdown_kill_tree: no live bash handles");
        return;
    }
    tracing::info!(
        count = pgids.len(),
        "shutdown_kill_tree: SIGKILLing live bash process groups"
    );
    #[cfg(unix)]
    for pgid in &pgids {
        // SAFETY: kill(2) with negative pid signals the process group;
        // no memory implications. Errors (ESRCH) are expected — the
        // group may have exited between snapshot and signal — and we
        // don't surface them.
        unsafe {
            let _ = libc::kill(-*pgid, libc::SIGKILL);
        }
    }
    tokio::time::sleep(Duration::from_secs(SHUTDOWN_KILL_GRACE_SECONDS)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_kill_tree_with_empty_registry_is_a_noop() {
        let registry = BashHandleRegistry::new();
        // No live handles → returns immediately without sleeping the full
        // grace period. We don't assert timing precisely; just that it
        // doesn't panic and returns.
        let start = std::time::Instant::now();
        shutdown_kill_tree(&registry).await;
        // The early-return path skips the sleep.
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn install_reaper_is_safe_to_call() {
        // Smoke test — must not panic. Real effect (subreaper bit set
        // on Linux) is process-global and not directly observable from
        // unit tests without spawning a subprocess; we leave that to
        // integration testing.
        install_reaper();
        // Double-call also fine (no panic).
        install_reaper();
    }
}
