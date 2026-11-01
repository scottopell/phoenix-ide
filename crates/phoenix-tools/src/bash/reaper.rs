//! Child process reaper.
//!
//! REQ-BASH-007: install the subreaper bit where supported. At shutdown,
//! attempt bounded, incarnation-bound SIGKILL for registered live handles;
//! this cannot guarantee descendant absence or complete tree reaping.

use std::time::Duration;

use super::registry::BashHandleRegistry;

/// Shared shutdown budget for snapshot, incarnation-bound signal attempts,
/// and handle-exit observation (REQ-BASH-007: `SHUTDOWN_KILL_GRACE_SECONDS`).
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

/// Attempt incarnation-bound SIGKILL for each live handle within one global
/// deadline, then observe handle exits while time remains. This best-effort
/// pass cannot prove descendant absence or that process groups were reaped.
pub async fn shutdown_kill_tree_until(
    deadline: tokio::time::Instant,
    registry: &BashHandleRegistry,
) {
    let teardown = async {
        let handles = registry.snapshot_handles_for_shutdown().await;
        let mut live_handles = Vec::new();
        for handle in handles {
            if handle.state().await.is_live() {
                live_handles.push(handle);
            }
        }
        if live_handles.is_empty() {
            tracing::debug!("shutdown_kill_tree: no live bash handles");
            return;
        }
        tracing::info!(
            count = live_handles.len(),
            "shutdown_kill_tree: attempting incarnation-bound SIGKILL for live bash handles"
        );

        let mut signaled_handles = Vec::new();
        for handle in live_handles {
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(
                    handle_id = %handle.handle_id,
                    "shutdown_kill_tree: global deadline expired before signaling this handle"
                );
                break;
            }
            match tokio::time::timeout_at(deadline, handle.signal_live_incarnation(libc::SIGKILL))
                .await
            {
                Ok(Ok(Some(_pgid))) => signaled_handles.push(handle),
                Ok(Ok(None)) => tracing::warn!(
                    handle_id = %handle.handle_id,
                    "shutdown_kill_tree: no live incarnation remained to signal"
                ),
                Ok(Err(error)) => tracing::warn!(
                    handle_id = %handle.handle_id,
                    error = %error,
                    "shutdown_kill_tree: incarnation-bound signal failed; handle remains unverified"
                ),
                Err(_) => {
                    tracing::warn!(
                        handle_id = %handle.handle_id,
                        "shutdown_kill_tree: global deadline expired during incarnation-bound signal"
                    );
                    break;
                }
            }
        }

        for handle in signaled_handles {
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(
                    handle_id = %handle.handle_id,
                    "shutdown_kill_tree: global deadline expired before handle exit observation"
                );
                break;
            }
            let mut exit = handle.exit_observer();
            if exit.borrow().is_some() {
                tracing::debug!(handle_id = %handle.handle_id, "shutdown_kill_tree: terminal handle exit observed");
                continue;
            }
            match tokio::time::timeout_at(deadline, exit.changed()).await {
                Ok(Ok(())) if exit.borrow().is_some() => {
                    tracing::debug!(handle_id = %handle.handle_id, "shutdown_kill_tree: handle exit observed after signal");
                }
                Ok(Ok(())) => {
                    tracing::warn!(handle_id = %handle.handle_id, "shutdown_kill_tree: exit notification changed without terminal state");
                }
                Ok(Err(_)) => {
                    tracing::warn!(handle_id = %handle.handle_id, "shutdown_kill_tree: handle exit observer closed before terminal state");
                }
                Err(_) => {
                    tracing::warn!(handle_id = %handle.handle_id, "shutdown_kill_tree: global deadline expired before handle exit was observed");
                }
            }
        }
    };
    if tokio::time::timeout_at(deadline, teardown).await.is_err() {
        tracing::warn!(
            "shutdown_kill_tree: global deadline expired during registry snapshot or shutdown pass; remaining handles are unverified"
        );
    }
}

pub async fn shutdown_kill_tree(registry: &BashHandleRegistry) {
    shutdown_kill_tree_until(
        tokio::time::Instant::now() + Duration::from_secs(SHUTDOWN_KILL_GRACE_SECONDS),
        registry,
    )
    .await;
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

    #[tokio::test]
    async fn shutdown_invalid_incarnation_retains_handle_and_unsettled_exit() {
        use crate::bash::handle::{Handle, HandleId, HandleState};
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};

        let registry = BashHandleRegistry::new();
        let scope = ResourceScopeKey::Work(WorkScopeId::parse("shutdown-invalid").unwrap());
        // An out-of-range PID fails before pidfd_open on Linux; other Unix
        // platforms return Unsupported. Neither path may target a real process.
        let handle = Handle::new_live(
            scope.clone(),
            HandleId::new("shutdown-invalid"),
            "not spawned".into(),
            None,
            std::path::PathBuf::from("/tmp"),
            i32::MAX,
            u32::MAX,
            1024,
        );
        let table = registry.get_or_create(&scope).await;
        table.write().await.insert(handle.clone());
        shutdown_kill_tree_until(
            tokio::time::Instant::now() + Duration::from_secs(1),
            &registry,
        )
        .await;
        assert!(matches!(
            handle.state().await.as_ref(),
            HandleState::Live(_)
        ));
        assert!(handle.exit_observer().borrow().is_none());
        let retained = registry.snapshot_handles_for_shutdown().await;
        assert_eq!(retained.len(), 1);
        assert!(std::sync::Arc::ptr_eq(&retained[0], &handle));
    }

    #[tokio::test]
    async fn shutdown_deadline_bounds_snapshot_lock_wait() {
        use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};

        let registry = BashHandleRegistry::new();
        let scope = ResourceScopeKey::Work(WorkScopeId::parse("shutdown-lock").unwrap());
        let table = registry.get_or_create(&scope).await;
        let _guard = table.write().await;
        // Keep the lock held until shutdown returns. The inner deadline must
        // cancel snapshot acquisition, not start a fresh per-handle budget.
        tokio::time::timeout(
            Duration::from_secs(1),
            shutdown_kill_tree_until(
                tokio::time::Instant::now() + Duration::from_millis(10),
                &registry,
            ),
        )
        .await
        .expect("global shutdown deadline must bound snapshot lock acquisition");
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
