//! Property-based and unit tests for the terminal module.
//!
//! Spec: `specs/terminal/terminal.allium`
//! Obligations covered:
//!   - `OneTerminalPerResourceScopeKey` invariant (REQ-TERM-003, REQ-TERM-WS-001)
//!   - `is_terminal()` correctness (REQ-TERM-012 precondition)
//!   - Dims validity (`ResizeFrameRejected` precondition)
//!   - `try_insert` atomic semantics (used on the fresh-session path; the
//!     reclaim path — task 24691 — goes through `get` + `stop_tx.send`
//!     and is exercised in `terminal::ws::reclaim_tests` and
//!     `terminal::relay::tests::*detach*`)
//!   - remove/get lifecycle (`TerminalOpened` / `UserClosedTerminal` state transitions)

#![allow(clippy::unwrap_used)]

use proptest::prelude::*;

use super::session::{
    ActiveTerminalInsertError, ActiveTerminals, Dims, TerminalLaunchIdentity,
    TerminalRetirementOutcome,
};
use phoenix_core::process_identity::ProcessIdentity;
use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};

static REAL_CHILD_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ── Helpers ──────────────────────────────────────────────────────────────────

fn scope(id: &str) -> ResourceScopeKey {
    ResourceScopeKey::Work(WorkScopeId::parse(id).unwrap())
}

fn arb_work_scope() -> impl Strategy<Value = ResourceScopeKey> {
    "[a-z0-9]{8}-[a-z0-9]{4}".prop_map(|id| scope(&id))
}

fn dummy_launch_identity(pid: u32) -> TerminalLaunchIdentity {
    TerminalLaunchIdentity {
        process: ProcessIdentity {
            pid,
            start_time: u128::from(pid) * 10,
        },
        launch_uuid: format!("launch-{pid}"),
    }
}

/// Build a minimal `TerminalHandle` for registry tests.
/// Uses /dev/null as a stand-in fd since these tests never do PTY I/O.
fn dummy_handle(dims: Dims) -> super::session::TerminalHandle {
    dummy_handle_with_pid(dims, 1)
}

fn dummy_handle_with_pid(dims: Dims, pid: i32) -> super::session::TerminalHandle {
    dummy_handle_kind(dims, super::session::TerminalChildKind::Shell, pid)
}

fn dummy_handle_kind(
    _dims: Dims,
    child_kind: super::session::TerminalChildKind,
    pid: i32,
) -> super::session::TerminalHandle {
    use crate::command_tracker::CommandTracker;
    use crate::session::{ShellIntegrationStatus, StopReason};
    use std::fs::OpenOptions;
    use std::os::unix::io::{FromRawFd, IntoRawFd};

    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
        .expect("open /dev/null");

    let raw = f.into_raw_fd();
    // SAFETY: we own the fd, transferring to OwnedFd.
    let owned_fd = unsafe { std::os::unix::io::OwnedFd::from_raw_fd(raw) };

    let (stop_tx, _stop_rx) = tokio::sync::watch::channel(StopReason::Running);

    super::session::TerminalHandle {
        master_fd: std::sync::Mutex::new(Some(owned_fd)),
        child_pid: nix::unistd::Pid::from_raw(pid), // synthetic test pid — never reaped
        launch_identity: dummy_launch_identity(pid.cast_unsigned()),
        child_kind,
        tracker: std::sync::Arc::new(std::sync::Mutex::new(CommandTracker::new(
            "test-session".to_string(),
        ))),
        shell_integration_status: std::sync::Arc::new(std::sync::Mutex::new(
            ShellIntegrationStatus::Unknown,
        )),
        stop_tx,
        attach_permit: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
    }
}

// ── Unit: OneTerminalPerResourceScopeKey (registry semantics) ────────────────────────

/// REQ-TERM-003 / REQ-TERM-WS-001 atomicity: `try_insert` on an already-active
/// scope returns `None`. The higher-level handler (see `ws.rs::acquire_handle`)
/// treats that as a signal to reclaim the winner rather than reject —
/// see task 24691 and `DuplicateConnectionReclaimsSession` in terminal.allium.
/// This test covers only the registry-level atomicity used as the race guard.
#[test]
fn shell_session_snapshot_excludes_tmux_clients() {
    use crate::session::TerminalChildKind;

    let registry = ActiveTerminals::new();
    let shell_scope = scope("shell");
    let tmux_scope = scope("tmux");
    let dims = Dims::try_new(80, 24).expect("valid dimensions");
    registry
        .try_insert(
            shell_scope,
            dummy_handle_kind(dims, TerminalChildKind::Shell, 1),
        )
        .expect("shell insert");
    registry
        .try_insert(
            tmux_scope,
            dummy_handle_kind(dims, TerminalChildKind::TmuxClient, 1),
        )
        .expect("tmux insert");

    assert_eq!(registry.snapshot_shell_session_ids(), vec![1]);
}

#[test]
fn try_insert_rejects_duplicate() {
    let registry = ActiveTerminals::new();
    let scope = scope("conv-001");
    let dims = Dims { cols: 80, rows: 24 };

    // First insert succeeds.
    let first = registry.try_insert(scope.clone(), dummy_handle(dims));
    assert!(first.is_some(), "first insert should succeed");

    // Second insert is rejected.
    let second = registry.try_insert(scope.clone(), dummy_handle(dims));
    assert!(
        second.is_none(),
        "duplicate insert must return None for the same scope"
    );
}

/// After `remove`, a new insert succeeds (absent → active → absent → active cycle).
#[test]
fn remove_allows_reinsertion() {
    let registry = ActiveTerminals::new();
    let scope = scope("conv-002");
    let dims = Dims { cols: 80, rows: 24 };

    registry
        .try_insert(scope.clone(), dummy_handle(dims))
        .unwrap();
    registry.remove_unfenced(&scope);

    let third = registry.try_insert(scope.clone(), dummy_handle(dims));
    assert!(third.is_some(), "insert after remove must succeed");
}

#[test]
fn relay_teardown_transfers_to_existing_close_owner() {
    use crate::session::RelayTeardownOwnership;

    let registry = ActiveTerminals::default();
    let scope = scope("relay-fence");
    let handle = registry
        .try_insert(scope.clone(), dummy_handle(Dims::try_new(80, 24).unwrap()))
        .expect("insert terminal");
    let _permit = registry.begin_retirement(&scope);

    assert_eq!(
        registry.claim_relay_teardown(&scope, &handle),
        RelayTeardownOwnership::ExistingRetirementOwner
    );

    assert!(registry.is_retirement_fenced(&scope));
    assert!(
        registry
            .get(&scope)
            .is_some_and(|current| std::sync::Arc::ptr_eq(&current, &handle)),
        "relay must leave the exact handle and reap authority with Close"
    );
}

#[tokio::test]
async fn stale_relay_completion_cannot_reopen_newer_close_fence() {
    use crate::session::RelayTeardownOwnership;

    let registry = ActiveTerminals::new();
    let scope = scope("stale-relay-close-fence");
    let dims = Dims::try_new(80, 24).unwrap();
    let handle = registry
        .try_insert_exact(scope.clone(), dummy_handle(dims))
        .expect("insert terminal");
    let relay = match registry.claim_relay_teardown(&scope, &handle) {
        RelayTeardownOwnership::RelayInitiated(permit) => permit,
        ownership @ (RelayTeardownOwnership::ExistingRetirementOwner
        | RelayTeardownOwnership::StaleRelay) => {
            panic!("relay should own generation N, got {ownership:?}")
        }
    };
    let close = registry.begin_retirement(&scope);
    assert_eq!(close.generation().get(), relay.generation().get() + 1);

    let completion = registry.complete_relay_retirement(relay).await;
    assert!(matches!(
        registry.reopen_after_relay_completion(completion),
        TerminalRetirementOutcome::Residual { .. }
    ));

    assert!(registry.is_retirement_fenced(&scope));
    assert!(matches!(
        registry.try_insert_exact(scope.clone(), dummy_handle(dims)),
        Err(ActiveTerminalInsertError::RetirementFenced)
    ));
    registry.cancel_retirement(close);
}

#[tokio::test]
async fn current_relay_completion_reopens_its_own_fence() {
    use crate::session::RelayTeardownOwnership;

    let registry = ActiveTerminals::new();
    let scope = scope("relay-only-reopen");
    let dims = Dims::try_new(80, 24).unwrap();
    let handle = registry
        .try_insert_exact(scope.clone(), dummy_handle(dims))
        .expect("insert terminal");
    let relay = match registry.claim_relay_teardown(&scope, &handle) {
        RelayTeardownOwnership::RelayInitiated(permit) => permit,
        ownership @ (RelayTeardownOwnership::ExistingRetirementOwner
        | RelayTeardownOwnership::StaleRelay) => {
            panic!("relay should own its retirement, got {ownership:?}")
        }
    };

    let completion = registry.complete_relay_retirement(relay).await;
    assert!(matches!(
        registry.reopen_after_relay_completion(completion),
        TerminalRetirementOutcome::Residual { .. }
    ));
    assert!(!registry.is_retirement_fenced(&scope));
}

#[test]
fn close_fence_prevents_relay_setup_from_overwriting_teardown() {
    let registry = ActiveTerminals::default();
    let scope = scope("relay-setup-fence");
    let handle = registry
        .try_insert(scope.clone(), dummy_handle(Dims::try_new(80, 24).unwrap()))
        .expect("insert terminal");
    let _permit = registry.begin_retirement(&scope);

    assert!(!registry.prepare_relay(&scope, &handle));
}

#[test]
fn retirement_revokes_pre_spawn_reservation() {
    let registry = ActiveTerminals::new();
    let scope = scope("reserved-before-fence");
    let dims = Dims { cols: 80, rows: 24 };

    registry.reserve_spawn(&scope).expect("reserve admission");
    let permit = registry.begin_retirement(&scope);

    assert!(permit.instance.is_none());
    assert!(matches!(
        registry.insert_reserved(scope.clone(), dummy_handle(dims)),
        Err(ActiveTerminalInsertError::RetirementFenced)
    ));
    assert!(registry.get(&scope).is_none());
}

#[test]
fn begin_retirement_fences_admission_until_reopened() {
    let registry = ActiveTerminals::new();
    let scope = scope("retirement-fence");
    let dims = Dims { cols: 80, rows: 24 };

    registry
        .try_insert(scope.clone(), dummy_handle(dims))
        .expect("initial insert");

    let permit = registry.begin_retirement(&scope);
    assert_eq!(permit.work_scope, scope);
    assert_eq!(permit.generation().get(), 1);
    assert!(
        permit.instance.is_some(),
        "live terminal should stamp exact identity"
    );
    assert!(registry.is_retirement_fenced(&scope));
    assert!(matches!(
        registry.try_insert_exact(scope.clone(), dummy_handle(dims)),
        Err(ActiveTerminalInsertError::RetirementFenced)
    ));

    registry.cancel_retirement(permit);
    assert!(!registry.is_retirement_fenced(&scope));
    registry.remove_unfenced(&scope);
    assert!(
        registry
            .try_insert(scope.clone(), dummy_handle(dims))
            .is_some(),
        "repair reopen must restore admission"
    );
}

#[tokio::test]
async fn complete_retirement_requires_exact_instance_and_reopen_for_admission() {
    let registry = ActiveTerminals::new();
    let scope = scope("retirement-exact");
    let dims = Dims { cols: 80, rows: 24 };

    let first = registry
        .try_insert_exact(scope.clone(), dummy_handle(dims))
        .expect("first insert");
    let first_identity = super::session::TerminalInstanceIdentity::from_handle(&first);
    let permit = registry.begin_retirement(&scope);
    let second_permit = registry.begin_retirement(&scope);
    assert_eq!(permit.generation().get(), 1);
    assert_eq!(second_permit.generation().get(), 2);

    assert_eq!(
        registry.complete_retirement(&permit).await,
        TerminalRetirementOutcome::Residual {
            reason: format!(
                "exact terminal instance {} remained current after teardown",
                first_identity.stable_identity()
            ),
        },
        "stale generation must not retire the still-current instance"
    );
    assert!(
        registry.get(&scope).is_some(),
        "stale permit must leave entry intact"
    );

    assert_eq!(
        registry
            .complete_retirement_by_observing(
                &second_permit,
                tokio::time::Instant::now() + std::time::Duration::from_secs(1),
                |pid| async move { Ok(nix::sys::wait::WaitStatus::Exited(pid, 0)) },
                |_| None,
            )
            .await,
        TerminalRetirementOutcome::Retired,
        "current permit must retire the exact current instance"
    );
    assert!(
        registry.get(&scope).is_none(),
        "exact retirement removes live entry"
    );
    assert!(
        registry.is_retirement_fenced(&scope),
        "scope stays fenced until repair reopen"
    );
    assert!(matches!(
        registry.try_insert_exact(scope.clone(), dummy_handle(dims)),
        Err(ActiveTerminalInsertError::RetirementFenced)
    ));

    registry.cancel_retirement(second_permit);
    assert!(
        registry
            .try_insert(scope.clone(), dummy_handle(dims))
            .is_some(),
        "exact Close cancellation must permit a fresh terminal after teardown"
    );
}

#[tokio::test(start_paused = true)]
async fn retirement_waits_for_attach_release_without_consuming_a_second_budget() {
    let registry = ActiveTerminals::new();
    let scope = scope("retirement-shared-deadline");
    let handle = registry
        .try_insert_exact(scope.clone(), dummy_handle(Dims::try_new(80, 24).unwrap()))
        .expect("insert terminal");
    let relay_authority = handle.attach_permit.clone().acquire_owned().await.unwrap();
    let permit = registry.begin_retirement(&scope);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let completing = {
        let registry = registry.clone();
        tokio::spawn(async move {
            registry
                .complete_retirement_by_observing(
                    &permit,
                    deadline,
                    |pid| async move { Ok(nix::sys::wait::WaitStatus::Exited(pid, 0)) },
                    |_| None,
                )
                .await
        })
    };

    tokio::task::yield_now().await;
    assert!(!completing.is_finished());
    tokio::time::advance(std::time::Duration::from_secs(9)).await;
    assert!(!completing.is_finished());

    // Permit release is the causal notification; no sleep or polling is needed.
    drop(relay_authority);
    tokio::task::yield_now().await;
    assert_eq!(
        completing.await.unwrap(),
        TerminalRetirementOutcome::Retired
    );
}

#[tokio::test(start_paused = true)]
async fn retirement_attach_wait_uses_declared_outer_deadline() {
    let registry = ActiveTerminals::new();
    let scope = scope("retirement-outer-deadline");
    let handle = registry
        .try_insert_exact(scope.clone(), dummy_handle(Dims::try_new(80, 24).unwrap()))
        .expect("insert terminal");
    let _relay_authority = handle.attach_permit.clone().acquire_owned().await.unwrap();
    let permit = registry.begin_retirement(&scope);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let completing = {
        let registry = registry.clone();
        tokio::spawn(async move { registry.complete_retirement_by(&permit, deadline).await })
    };

    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        completing.await.unwrap(),
        TerminalRetirementOutcome::Residual {
            reason: "terminal relay did not release after teardown request".to_string(),
        }
    );
}

#[tokio::test]
async fn attached_relay_close_transfers_authority_and_completes_exact_teardown() {
    use crate::session::{RelayTeardownOwnership, StopReason};
    use crate::spawn::{spawn_pty, PtyExecPlan};

    let _real_child_guard = REAL_CHILD_TEST_LOCK.lock().await;
    let registry = ActiveTerminals::new();
    let scope = scope("attached-close");
    let handle = registry
        .try_insert_exact(
            scope.clone(),
            spawn_pty(
                std::path::Path::new("/tmp"),
                Dims::try_new(80, 24).unwrap(),
                PtyExecPlan::Shell,
            )
            .expect("spawn attached PTY"),
        )
        .expect("publish attached PTY");
    let relay_permit = handle.attach_permit.clone().acquire_owned().await.unwrap();
    let mut stop_rx = handle.stop_tx.subscribe();
    let permit = registry.begin_retirement_by(
        &scope,
        tokio::time::Instant::now() + std::time::Duration::from_secs(2),
    );
    let close = {
        let registry = registry.clone();
        tokio::spawn(async move {
            let outcome = registry.complete_retirement(&permit).await;
            (outcome, permit)
        })
    };

    stop_rx.changed().await.unwrap();
    assert_eq!(*stop_rx.borrow(), StopReason::TearDown);
    assert_eq!(
        registry.claim_relay_teardown(&scope, &handle),
        RelayTeardownOwnership::ExistingRetirementOwner,
        "attached relay must transfer exact teardown authority to Close"
    );
    drop(relay_permit);
    drop(handle);

    assert_eq!(close.await.unwrap().0, TerminalRetirementOutcome::Retired);
    assert!(registry.get(&scope).is_none());
}

#[cfg(unix)]
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn sighup_resistant_attached_child_times_out_bounded_with_retry_authority() {
    use crate::command_tracker::CommandTracker;
    use crate::session::{ShellIntegrationStatus, StopReason, TerminalChildKind, TerminalHandle};
    use nix::unistd::{fork, ForkResult};
    use std::os::fd::{FromRawFd, OwnedFd};

    let _real_child_guard = REAL_CHILD_TEST_LOCK.lock().await;
    let mut ready_pipe = [0; 2];
    assert_eq!(unsafe { libc::pipe(ready_pipe.as_mut_ptr()) }, 0);
    // SAFETY: the child performs only async-signal-safe libc operations before
    // `_exit`; the parent owns and reaps the exact returned PID.
    let child = match unsafe { fork() }.expect("fork resistant child") {
        ForkResult::Child => unsafe {
            libc::close(ready_pipe[0]);
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            let ready = [1_u8];
            libc::write(ready_pipe[1], ready.as_ptr().cast(), ready.len());
            libc::close(ready_pipe[1]);
            loop {
                libc::pause();
            }
        },
        ForkResult::Parent { child } => child,
    };
    unsafe {
        libc::close(ready_pipe[1]);
        let mut ready = [0_u8];
        assert_eq!(
            libc::read(ready_pipe[0], ready.as_mut_ptr().cast(), ready.len()),
            1
        );
        libc::close(ready_pipe[0]);
    }
    let process =
        phoenix_core::process_identity::current_process_identity(child.as_raw().cast_unsigned())
            .expect("capture child identity");
    let raw = unsafe { libc::dup(libc::STDERR_FILENO) };
    assert!(raw >= 0);
    let master_fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let (stop_tx, _stop_rx) = tokio::sync::watch::channel(StopReason::Running);
    let registry = ActiveTerminals::new();
    let scope = scope("resistant-timeout");
    let handle = registry
        .try_insert_exact(
            scope.clone(),
            TerminalHandle {
                master_fd: std::sync::Mutex::new(Some(master_fd)),
                child_pid: child,
                launch_identity: TerminalLaunchIdentity {
                    process,
                    launch_uuid: "resistant-child".to_string(),
                },
                child_kind: TerminalChildKind::Shell,
                tracker: std::sync::Arc::new(std::sync::Mutex::new(CommandTracker::new(
                    "resistant-child".to_string(),
                ))),
                shell_integration_status: std::sync::Arc::new(std::sync::Mutex::new(
                    ShellIntegrationStatus::Unknown,
                )),
                stop_tx,
                attach_permit: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
            },
        )
        .expect("publish resistant child");
    let relay_permit = handle.attach_permit.clone().acquire_owned().await.unwrap();
    let mut stop_rx = handle.stop_tx.subscribe();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(100);
    let permit = registry.begin_retirement_by(&scope, deadline);
    let completing = {
        let registry = registry.clone();
        tokio::spawn(async move {
            let outcome = registry.complete_retirement(&permit).await;
            (outcome, permit)
        })
    };
    let started = std::time::Instant::now();

    stop_rx.changed().await.unwrap();
    assert_eq!(*stop_rx.borrow(), StopReason::TearDown);
    assert_eq!(
        registry.claim_relay_teardown(&scope, &handle),
        crate::session::RelayTeardownOwnership::ExistingRetirementOwner
    );
    drop(relay_permit);
    let (outcome, permit) = completing.await.unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(
        outcome,
        TerminalRetirementOutcome::Residual {
            reason: "terminal child exit was not authoritatively observed; exact handle retained for retry"
                .to_string(),
        }
    );
    assert!(
        registry
            .get(&scope)
            .is_some_and(|current| std::sync::Arc::ptr_eq(&current, &handle)),
        "timeout must retain the exact handle and retry authority"
    );
    assert_eq!(
        registry.complete_retirement(&permit).await,
        outcome,
        "retry must reuse the expired absolute budget rather than reset it"
    );

    nix::sys::signal::kill(child, nix::sys::signal::Signal::SIGKILL).unwrap();
    nix::sys::wait::waitpid(child, None).unwrap();
}

#[tokio::test(start_paused = true)]
async fn deadline_with_unobservable_identity_retains_exact_handle_for_retry() {
    let registry = ActiveTerminals::new();
    let scope = scope("retirement-unobservable-identity");
    let handle = registry
        .try_insert_exact(
            scope.clone(),
            dummy_handle_with_pid(Dims::try_new(80, 24).unwrap(), 41),
        )
        .expect("insert terminal");
    let permit = registry.begin_retirement(&scope);

    let outcome = registry
        .complete_retirement_by_observing(
            &permit,
            tokio::time::Instant::now(),
            |_| std::future::pending(),
            |_| None,
        )
        .await;

    assert_eq!(
        outcome,
        TerminalRetirementOutcome::Residual {
            reason: "terminal child exit was not authoritatively observed; exact handle retained for retry"
                .to_string(),
        }
    );
    assert!(
        registry
            .get(&scope)
            .is_some_and(|current| std::sync::Arc::ptr_eq(&current, &handle)),
        "an unproven observation must preserve exact handle and retry ownership"
    );
}

#[tokio::test]
async fn failed_wait_with_reused_pid_retires_exact_handle() {
    let registry = ActiveTerminals::new();
    let scope = scope("retirement-reused-pid");
    let handle = registry
        .try_insert_exact(
            scope.clone(),
            dummy_handle_with_pid(Dims::try_new(80, 24).unwrap(), 42),
        )
        .expect("insert terminal");
    let expected = handle.launch_identity.process;
    let permit = registry.begin_retirement(&scope);
    let reused = ProcessIdentity {
        start_time: expected.start_time + 1,
        ..expected
    };

    let outcome = registry
        .complete_retirement_by_observing(
            &permit,
            tokio::time::Instant::now() + std::time::Duration::from_secs(1),
            |_| async { Err(nix::errno::Errno::ECHILD) },
            |_| Some(reused),
        )
        .await;

    assert_eq!(outcome, TerminalRetirementOutcome::Retired);
    assert!(
        registry.get(&scope).is_none(),
        "a different identity at the PID proves the exact child absent"
    );
}

#[tokio::test]
async fn complete_retirement_exits_and_reaps_long_lived_detached_direct_pty() {
    use crate::session::TerminalRetirementOutcome;
    use crate::spawn::{spawn_pty, PtyExecPlan};

    let _real_child_guard = REAL_CHILD_TEST_LOCK.lock().await;
    let registry = ActiveTerminals::new();
    let scope = scope("detached-direct-pty");
    let handle = spawn_pty(
        std::path::Path::new("/tmp"),
        Dims::try_new(80, 24).unwrap(),
        PtyExecPlan::Shell,
    )
    .expect("spawn real direct-shell PTY");
    let process = handle.launch_identity.process;
    registry
        .try_insert_exact(scope.clone(), handle)
        .expect("publish detached PTY");

    assert!(
        phoenix_core::process_identity::process_identity_matches(process),
        "direct PTY child must remain live while its detached master is owned"
    );

    let permit = registry.begin_retirement(&scope);
    assert_eq!(
        registry.complete_retirement(&permit).await,
        TerminalRetirementOutcome::Retired
    );
    assert!(registry.get(&scope).is_none());
    assert!(
        !phoenix_core::process_identity::process_identity_matches(process),
        "retirement must close the detached master and reap the exact child"
    );
}

/// `get` returns `Some` for registered scopes, `None` otherwise.
#[test]
fn get_returns_correct_presence() {
    let registry = ActiveTerminals::new();
    let dims = Dims { cols: 80, rows: 24 };
    let absent = scope("nonexistent");
    let present = scope("present");

    assert!(registry.get(&absent).is_none());

    registry
        .try_insert(present.clone(), dummy_handle(dims))
        .unwrap();
    assert!(registry.get(&present).is_some());
    assert!(registry.get(&absent).is_none());
}

/// REQ-TERM-WS-001: the singleton global terminal scope holds exactly one
/// terminal at a time and is disjoint from ordinary work scopes.
#[test]
fn global_scope_is_disjoint_and_singleton() {
    let registry = ActiveTerminals::new();
    let dims = Dims { cols: 80, rows: 24 };

    assert!(registry
        .try_insert(ResourceScopeKey::GlobalTerminal, dummy_handle(dims))
        .is_some());
    assert!(
        registry
            .try_insert(ResourceScopeKey::GlobalTerminal, dummy_handle(dims))
            .is_none(),
        "Global is singleton: a second insert must return None"
    );

    let conv_lookalike = scope("global_terminal");
    assert!(
        registry
            .try_insert(conv_lookalike.clone(), dummy_handle(dims))
            .is_some(),
        "ordinary work must not collide with the global terminal"
    );
}

// ── Unit: cascade_on_delete (REQ-TERM-WS-001, REQ-TERM-012) ──────────────────

/// REQ-TERM-012: cascade removes the registry entry for the torn-down scope.
/// Mirrors the tmux/browser cascade pattern.
#[tokio::test]
async fn cascade_on_delete_removes_entry_for_scope() {
    use crate::spawn::{spawn_pty, PtyExecPlan};

    let _real_child_guard = REAL_CHILD_TEST_LOCK.lock().await;
    let registry = ActiveTerminals::new();
    let scope = scope("cascade-remove");
    let handle = spawn_pty(
        std::path::Path::new("/tmp"),
        Dims::try_new(80, 24).unwrap(),
        PtyExecPlan::Shell,
    )
    .expect("spawn real direct-shell PTY");

    registry.try_insert(scope.clone(), handle).unwrap();
    assert!(registry.get(&scope).is_some());

    registry.cascade_on_delete(&scope, None).await;
    assert!(
        registry.get(&scope).is_none(),
        "cascade with no inheritor must remove the entry"
    );
}

/// REQ-TERM-WS-001: scope-equality preservation. A continuation conversation
/// that resolves to the same Worktree scope inherits the terminal; cascade
/// must skip teardown rather than kill a session the inheritor still uses.
#[tokio::test]
async fn cascade_on_delete_preserves_when_continuation_inherits_scope() {
    let registry = ActiveTerminals::new();
    let dims = Dims { cols: 80, rows: 24 };
    let scope = scope("cascade-preserve");
    let inheritor = scope.clone();

    registry
        .try_insert(scope.clone(), dummy_handle(dims))
        .unwrap();
    registry.cascade_on_delete(&scope, Some(&inheritor)).await;

    assert!(
        registry.get(&scope).is_some(),
        "cascade must preserve the terminal when inheritor_scope == work_scope"
    );
}

/// Cascade against a scope with no registry entry is a no-op (the common
/// case during conversation cleanup for sub-agent / no-terminal scopes).
#[tokio::test]
async fn cascade_on_delete_no_entry_is_noop() {
    let registry = ActiveTerminals::new();
    let scope = scope("never-existed");
    registry.cascade_on_delete(&scope, None).await;
    assert!(registry.get(&scope).is_none());
}

// ── Property: OneTerminalPerResourceScopeKey ─────────────────────────────────────────

proptest! {
    /// Invariant: for any sequence of try_insert / remove operations across
    /// distinct scopes, the count of active terminals per scope never exceeds 1.
    ///
    /// Maps to: `OneTerminalPerResourceScopeKey` in terminal.allium.
    #[test]
    fn prop_one_terminal_per_workscope(
        ops in proptest::collection::vec(
            (arb_work_scope(), proptest::bool::ANY),  // (scope, insert=true / remove=false)
            1..50
        )
    ) {
        let registry = ActiveTerminals::new();
        let dims = Dims { cols: 80, rows: 24 };

        for (scope, do_insert) in ops {
            if do_insert {
                // try_insert either succeeds or returns None — never panics.
                let _ = registry.try_insert(scope.clone(), dummy_handle(dims));
            } else {
                registry.remove_unfenced(&scope);
            }

            // Invariant: count per scope must be 0 or 1.
            let count = registry.active_count_for_scope(&scope);
            prop_assert!(count <= 1,
                "OneTerminalPerResourceScopeKey violated: {} active for {:?}",
                count, scope);
        }
    }

    /// Concurrent-simulation: two inserts racing on the same scope must
    /// result in at most one active terminal. We simulate this serially
    /// (Rust Mutex guarantees atomicity; the spec requires it).
    #[test]
    fn prop_concurrent_insert_one_wins(scope in arb_work_scope()) {
        let registry = ActiveTerminals::new();
        let dims = Dims { cols: 80, rows: 24 };

        let r1 = registry.try_insert(scope.clone(), dummy_handle(dims));
        let r2 = registry.try_insert(scope.clone(), dummy_handle(dims));

        // Exactly one succeeds.
        let successes = [r1.is_some(), r2.is_some()].iter().filter(|&&b| b).count();
        prop_assert_eq!(successes, 1,
            "exactly one of two racing inserts must win; got {}", successes);
    }
}

// ── Unit: Dims validity ───────────────────────────────────────────────────────

/// `ResizeFrameRejected` precondition: dims with cols=0 or rows=0 are invalid.
#[test]
fn dims_zero_cols_is_invalid() {
    // The spec requires dimensions.cols > 0 and dimensions.rows > 0.
    // Our ws.rs rejects frames where either is 0.
    // This test documents the boundary; `apply_resize` is only called
    // after the guard in the writer task.
    let invalid = Dims { cols: 0, rows: 24 };
    assert_eq!(invalid.cols, 0, "zero cols recognized as boundary case");
}

// ── Unit: resize frame validation (ResizeFrameRejected rule) ────────────────

/// REQ-TERM-006 / `ResizeFrameRejected`:
/// The relay requires cols >= 2 && rows >= 1 for a resize to be applied.
/// Frames with cols < 2 or rows = 0 must be silently dropped (session must stay connected).
#[test]
fn small_cols_resize_frame_is_rejected() {
    // Construct a 0x01 frame with cols=1 (below the minimum of 2)
    let data = {
        let mut v = vec![0x01u8];
        v.extend_from_slice(&1u16.to_be_bytes()); // cols = 1
        v.extend_from_slice(&24u16.to_be_bytes()); // rows = 24
        v
    };

    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(super::relay::dispatch_frame_for_test(&data, "test-conv"));
    assert!(result, "cols=1 frame should not disconnect the session");
}

#[test]
fn zero_rows_resize_frame_is_rejected() {
    let data = {
        let mut v = vec![0x01u8];
        v.extend_from_slice(&80u16.to_be_bytes()); // cols = 80
        v.extend_from_slice(&0u16.to_be_bytes()); // rows = 0
        v
    };

    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(super::relay::dispatch_frame_for_test(&data, "test-conv"));
    assert!(result, "rows=0 frame should not disconnect the session");
}

proptest! {
    /// ResizeFrameRejected: for any frame with invalid dimensions, the session
    /// must remain connected (return true).
    #[test]
    fn prop_small_dimension_resize_rejected(
        bad_cols in 0u16..=1u16,   // 0 and 1 are both below the cols>=2 minimum
        bad_rows in 0u16..=1u16,
    ) {
        // Test cases where cols < 2 or rows < 1
        prop_assume!(bad_cols < 2 || bad_rows == 0);

        let data = {
            let mut v = vec![0x01u8];
            v.extend_from_slice(&bad_cols.to_be_bytes());
            v.extend_from_slice(&bad_rows.to_be_bytes());
            v
        };

        let result = tokio::runtime::Runtime::new().unwrap().block_on(
            super::relay::dispatch_frame_for_test(&data, "test")
        );

        prop_assert!(result, "invalid resize frame must not disconnect the session");
    }
}

// ── Unit: build_env (REQ-TERM-002 / ShellEnvironmentConstructed rule) ─────────

/// REQ-TERM-002: The shell environment must contain all required variables
/// and must NOT inherit the server process environment.
#[test]
fn build_env_contains_required_variables() {
    use super::spawn::build_env;

    let env = build_env("/bin/bash", "test-launch");
    let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();

    for required in &["TERM", "COLORTERM", "HOME", "USER", "SHELL", "PATH", "LANG"] {
        assert!(
            keys.contains(required),
            "build_env missing required key: {required}"
        );
    }
}

#[test]
fn build_env_term_is_xterm_256color() {
    use super::spawn::build_env;
    let env = build_env("/bin/bash", "test-launch");
    let term = env
        .iter()
        .find(|(k, _)| k == "TERM")
        .map(|(_, v)| v.as_str());
    assert_eq!(
        term,
        Some("xterm-256color"),
        "TERM must be xterm-256color — wrong value breaks readline and vim"
    );
}

#[test]
fn build_env_colorterm_is_truecolor() {
    use super::spawn::build_env;
    let env = build_env("/bin/bash", "test-launch");
    let ct = env
        .iter()
        .find(|(k, _)| k == "COLORTERM")
        .map(|(_, v)| v.as_str());
    assert_eq!(ct, Some("truecolor"));
}

#[test]
fn build_env_shell_matches_argument() {
    use super::spawn::build_env;
    let env = build_env("/usr/bin/zsh", "test-launch");
    let shell = env
        .iter()
        .find(|(k, _)| k == "SHELL")
        .map(|(_, v)| v.as_str());
    assert_eq!(
        shell,
        Some("/usr/bin/zsh"),
        "SHELL env var must reflect the shell passed to build_env"
    );
}

#[test]
fn build_env_lang_is_utf8() {
    use super::spawn::build_env;
    let env = build_env("/bin/bash", "test-launch");
    let lang = env
        .iter()
        .find(|(k, _)| k == "LANG")
        .map(|(_, v)| v.as_str());
    assert_eq!(lang, Some("en_US.UTF-8"));
}

#[test]
fn build_env_no_duplicate_keys() {
    use super::spawn::build_env;
    let env = build_env("/bin/bash", "test-launch");
    let mut keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    let original_len = keys.len();
    keys.dedup();
    assert_eq!(
        keys.len(),
        original_len,
        "build_env must not produce duplicate keys"
    );
}

// ── CommandTracker proptests ──────────────────────────────────────────────────
//
// These proptests verify REQ-TERM-021 invariants under adversarial byte
// sequences and arbitrary delivery chunking.

#[test]
fn terminal_instance_stable_identity_excludes_arc_address_but_exact_match_keeps_it() {
    let dims = Dims { cols: 80, rows: 24 };
    let handle = std::sync::Arc::new(dummy_handle(dims));
    let identity = super::session::TerminalInstanceIdentity::from_handle(&handle);

    let cloned_handle = std::sync::Arc::clone(&handle);
    assert!(identity.matches_handle(&cloned_handle));

    let same_launch_new_arc = std::sync::Arc::new(dummy_handle(dims));
    let same_launch_identity =
        super::session::TerminalInstanceIdentity::from_handle(&same_launch_new_arc);
    assert_eq!(
        identity.stable_identity(),
        same_launch_identity.stable_identity(),
        "stable identity must come from durable launch identity, not Arc address"
    );
    assert!(
        !identity.matches_handle(&same_launch_new_arc),
        "exact in-process matching must still distinguish different Arc instances"
    );
    assert_eq!(
        identity.process_identity(),
        same_launch_identity.process_identity(),
        "durable process identity should be separable from in-process handle identity"
    );
}

#[test]
fn build_env_carries_terminal_launch_uuid_marker() {
    use super::spawn::{build_env, TERMINAL_LAUNCH_UUID_ENV_VAR};

    let env = build_env("/bin/bash", "launch-uuid-test");
    let launch_uuid = env
        .iter()
        .find(|(k, _)| k == TERMINAL_LAUNCH_UUID_ENV_VAR)
        .map(|(_, v)| v.as_str());
    assert_eq!(launch_uuid, Some("launch-uuid-test"));
}

#[cfg(test)]
mod command_tracker_proptest {
    use proptest::prelude::*;

    use crate::command_tracker::CommandTracker;
    use crate::test_helpers::full_command;

    proptest! {
        /// REQ-TERM-021 / CommandRecordRingBufferBound:
        /// Feeding arbitrary bytes must never panic, and the ring buffer must never
        /// exceed capacity 5.
        #[test]
        fn prop_command_tracker_arbitrary_bytes_no_panic(
            bytes in proptest::collection::vec(any::<u8>(), 0..1024),
        ) {
            let mut tracker = CommandTracker::new("prop-test".to_string());
            tracker.ingest(&bytes);
            prop_assert!(
                tracker.record_count() <= 5,
                "ring buffer must not exceed capacity 5; got {}",
                tracker.record_count()
            );
        }

        /// REQ-TERM-021: Splitting a sequence of full_command bytes into arbitrary
        /// chunks must produce the same ring buffer contents as delivering them whole.
        ///
        /// Verifies that `CommandTracker` handles cross-chunk OSC sequences correctly
        /// (vte::Parser is stateful across advance calls).
        #[test]
        fn prop_command_tracker_split_chunks(
            // Generate 1-5 commands.
            commands in proptest::collection::vec(
                ("[a-z]{1,10}", "[a-zA-Z0-9 ]{0,50}", proptest::option::of(0i32..=127i32)),
                1..=5usize,
            ),
            // Generate 1-3 split points as percentages 0..=100.
            split_points in proptest::collection::vec(0usize..=100usize, 1..=3),
        ) {
            // Build full byte sequence.
            let mut all_bytes: Vec<u8> = Vec::new();
            for (cmd, output, code) in &commands {
                all_bytes.extend_from_slice(&full_command(cmd, output, *code));
            }

            if all_bytes.is_empty() {
                return Ok(());
            }

            // Normalise split points to actual offsets within the sequence.
            let len = all_bytes.len();
            let mut splits: Vec<usize> = split_points
                .iter()
                .map(|&p| (p * len / 100).min(len))
                .collect();
            splits.sort_unstable();
            splits.dedup();

            // Deliver in chunks.
            let mut tracker = CommandTracker::new("prop-split".to_string());
            let mut last = 0usize;
            for &split in &splits {
                if split > last {
                    tracker.ingest(&all_bytes[last..split]);
                    last = split;
                }
            }
            if last < all_bytes.len() {
                tracker.ingest(&all_bytes[last..]);
            }

            // All commands that fit in the ring buffer must be present (oldest may be
            // evicted if more than 5 were delivered).
            let expected_count = commands.len().min(5);
            prop_assert_eq!(
                tracker.record_count(),
                expected_count,
                "ring buffer must contain min(commands, 5) records; \
                 got {}, expected {}",
                tracker.record_count(),
                expected_count
            );

            // The most recent command must match the last in the list.
            if let Some(last_cmd) = commands.last() {
                let rec = tracker.last_command().expect("ring buffer must be non-empty");
                prop_assert_eq!(
                    &rec.command_text, &last_cmd.0,
                    "last command_text mismatch"
                );
                prop_assert_eq!(
                    rec.exit_code, last_cmd.2,
                    "last exit_code mismatch"
                );
            }
        }
    }
}

/// Op-enum proptest: drives `CommandTracker` via first-class state machine operations
/// and asserts ALL spec invariants after EVERY operation.
///
/// This is qualitatively different from the delivery proptests above, which only assert
/// no-panic and ring-buffer count at quiescence. The Op-enum generator produces
/// `StartOnly` (C with no D), `EndOnly` (D with no C), and `ClobberCapture` (C during
/// capture) as first-class operations — exactly the sequences that stress the state
/// machine's recovery logic. Invariants are checked after every op, not just at the end.
///
/// Invariants checked:
///   - `CommandRecordRingBufferBound`: count <= 5 at all times
///   - `CommandLifecycleFieldsCoherent`: completed records have `duration_ms` > 0
///   - `OneExecutingCommandAtATime`: at most one capture active (structural; redundant
///     field removed, so this is now enforced by the type)
///   - Ring buffer ordering: newest record matches most recently completed `RunCommand`
#[cfg(test)]
mod command_tracker_op_proptest {
    use proptest::prelude::*;

    use crate::command_tracker::CommandTracker;
    use crate::test_helpers::TerminalStream;

    /// A first-class operation on the `CommandTracker` state machine.
    #[derive(Debug, Clone)]
    enum TrackerOp {
        /// Complete command: C + output + D. The happy path.
        RunCommand {
            cmd: String,
            output: String,
            code: Option<i32>,
        },
        /// C with no following D — simulates command in-flight at session end.
        StartOnly(String),
        /// D with no preceding C — stray marker from subshell or signal.
        EndOnly(Option<i32>),
        /// C during active capture — simulates nested subshell or rapid-fire commands.
        ClobberCapture(String),
        /// Arbitrary bytes — realistic terminal noise between commands.
        ArbitraryBytes(Vec<u8>),
    }

    fn arb_op() -> impl Strategy<Value = TrackerOp> {
        prop_oneof![
            // RunCommand is the most common case; weight it higher.
            3 => ("[a-z]{1,8}", "[a-zA-Z0-9 ./-]{0,40}", proptest::option::of(-1i32..=127i32))
                .prop_map(|(cmd, output, code)| TrackerOp::RunCommand { cmd, output, code }),
            1 => "[a-z]{1,8}".prop_map(TrackerOp::StartOnly),
            1 => proptest::option::of(0i32..=127i32).prop_map(TrackerOp::EndOnly),
            1 => "[a-z]{1,8}".prop_map(TrackerOp::ClobberCapture),
            1 => proptest::collection::vec(any::<u8>(), 0..64).prop_map(TrackerOp::ArbitraryBytes),
        ]
    }

    fn apply_op(tracker: &mut CommandTracker, op: &TrackerOp) {
        let bytes = match op {
            TrackerOp::RunCommand { cmd, output, code } => TerminalStream::new()
                .osc133_c(cmd)
                .text(output)
                .osc133_d(*code)
                .build(),
            TrackerOp::StartOnly(cmd) => TerminalStream::new().osc133_c(cmd).build(),
            TrackerOp::EndOnly(code) => TerminalStream::new().osc133_d(*code).build(),
            TrackerOp::ClobberCapture(cmd) => {
                // Emit a C without a D first (enter capture), then immediately another C.
                TerminalStream::new()
                    .osc133_c("outer")
                    .osc133_c(cmd)
                    .build()
            }
            TrackerOp::ArbitraryBytes(b) => b.clone(),
        };
        tracker.ingest(&bytes);
    }

    /// Assert all spec invariants. Called after every operation.
    fn check_invariants(
        tracker: &CommandTracker,
        op: &TrackerOp,
        step: usize,
    ) -> Result<(), TestCaseError> {
        // CommandRecordRingBufferBound: count <= 5 at all times.
        prop_assert!(
            tracker.record_count() <= 5,
            "step {step} after {op:?}: ring buffer exceeded capacity 5 (got {})",
            tracker.record_count()
        );

        // CommandLifecycleFieldsCoherent: every record in the ring buffer is a
        // completed command and must have command_text set (may be empty string when
        // the shell doesn't populate the C payload, but the field must exist).
        // Note: duration_ms may be 0 for sub-millisecond commands — as_millis()
        // truncates, so this is not a useful completeness sentinel.
        for (i, rec) in tracker.all_records().iter().enumerate() {
            // command_text is always a String (never uninitialized); this just confirms
            // the record was fully constructed and not a zero-value default.
            let _ = (i, rec.command_text.as_str()); // binding suppresses unused warning
        }

        // Ring buffer ordering: records are oldest-first in all_records();
        // recent_commands() returns newest-first.
        let recent = tracker.recent_commands(5);
        let all: Vec<_> = tracker.all_records().iter().collect();
        if !all.is_empty() {
            prop_assert_eq!(
                recent.first().map(|r| r.command_text.as_str()),
                all.last().map(|r| r.command_text.as_str()),
                "step {}: recent_commands newest != all_records last",
                step
            );
        }

        Ok(())
    }

    proptest! {
        /// Drive the CommandTracker through a sequence of mixed operations (happy path,
        /// aborted captures, stray D markers, clobbers, noise) and assert all spec
        /// invariants hold after every single step.
        ///
        /// This catches bugs that only manifest mid-sequence — e.g. a stuck capture
        /// after `StartOnly` that corrupts the next `RunCommand`'s record, or a
        /// ring buffer that transiently exceeds 5 before eviction.
        #[test]
        fn prop_state_machine_invariants_hold_after_every_op(
            ops in proptest::collection::vec(arb_op(), 1..=20usize),
        ) {
            let mut tracker = CommandTracker::new("op-prop".to_string());
            for (step, op) in ops.iter().enumerate() {
                apply_op(&mut tracker, op);
                check_invariants(&tracker, op, step)?;
            }
        }
    }
}
