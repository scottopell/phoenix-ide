#![cfg(target_os = "macos")]

#[path = "support/launchd.rs"]
mod launchd;
#[path = "support/native_sampling.rs"]
mod native_sampling;

use launchd::{
    bounded_text, poll_pause, private_file, CleanupFailures, DisposableLaunchdGuardian, ATTEMPT,
};
use native_sampling::{sampling_rows, sampling_runner_command};
use phoenix_deployment_controller::{
    guardian_ipc::{WireSnapshot, WireState},
    native_guardian::{verify_native_binding, GuardianClient, VerifiedBindingStatus},
    ControllerSqliteStore, MonotonicTick, NativeSamplingStatus, NativeSessionState,
};
use std::{
    io::Write,
    num::NonZeroU64,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const WAIT: Duration = Duration::from_secs(10);

struct Runner {
    child: Child,
    out: PathBuf,
    err: PathBuf,
    consumed: usize,
    cleanup_failures: CleanupFailures,
}

impl Runner {
    fn start(mut command: Command, guardian: &DisposableLaunchdGuardian, name: &str) -> Self {
        let out = guardian.root().join(format!("{name}.stdout"));
        let err = guardian.root().join(format!("{name}.stderr"));
        let child = command
            .stdin(Stdio::piped())
            .stdout(private_file(&out).unwrap())
            .stderr(private_file(&err).unwrap())
            .spawn()
            .unwrap();
        Self {
            child,
            out,
            err,
            consumed: 0,
            cleanup_failures: guardian.cleanup_failures(),
        }
    }

    fn take_line(&mut self, text: &str) -> Option<String> {
        let tail = text.get(self.consumed..)?;
        let relative = tail.find('\n')?;
        let line = tail.get(..relative)?.trim_end().to_owned();
        self.consumed += relative + 1;
        Some(line)
    }

    fn line(&mut self) -> String {
        let until = Instant::now() + WAIT;
        loop {
            let text = bounded_text(&self.out).unwrap();
            if let Some(line) = self.take_line(&text) {
                return line;
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                // Exit can race the first file read; reread after reaping before declaring EOF.
                let final_text = bounded_text(&self.out).unwrap();
                if let Some(line) = self.take_line(&final_text) {
                    return line;
                }
                panic!(
                    "runner exited ({status}) without marker: {}",
                    bounded_text(&self.err).unwrap()
                );
            }
            assert!(
                Instant::now() < until,
                "runner output timed out: {}",
                bounded_text(&self.err).unwrap()
            );
            poll_pause();
        }
    }

    fn marker(&mut self, prefix: &str) -> String {
        // Finite output as well as a wall-clock bound: an unexpected line fails rather than spins.
        let line = self.line();
        assert!(line.starts_with(prefix), "expected {prefix}, got {line}");
        line
    }

    fn release(&mut self) {
        self.child.stdin.as_mut().unwrap().write_all(b"!").unwrap();
    }

    fn success(&mut self) {
        let until = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "runner failed: {}",
                    bounded_text(&self.err).unwrap()
                );
                return;
            }
            assert!(Instant::now() < until, "runner exit timed out");
            poll_pause();
        }
    }

    fn kill(&mut self) {
        assert!(
            self.child.try_wait().unwrap().is_none(),
            "runner must still be paused before forced death"
        );
        self.child.kill().unwrap();
        self.reaped();
        assert!(!self.child.try_wait().unwrap().unwrap().success());
    }

    fn reaped(&mut self) {
        let until = Instant::now() + WAIT;
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < until, "runner reaping timed out");
            poll_pause();
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        // Child is an unreaped handle created by this test, never a discovered/historical PID.
        self.child.kill().ok();
        let until = Instant::now() + WAIT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < until => poll_pause(),
                result => {
                    self.cleanup_failures
                        .record(format!("disposable runner cleanup unverified: {result:?}"));
                    return;
                }
            }
        }
    }
}

fn recovery_command(guardian: &DisposableLaunchdGuardian, mode: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"));
    command.args([
        "--mode",
        mode,
        "--socket-dir",
        guardian.socket_dir().to_str().unwrap(),
        "--store",
        guardian.store().to_str().unwrap(),
        "--attempt",
        "64",
    ]);
    command
}

fn detached(guardian: &mut DisposableLaunchdGuardian) -> WireSnapshot {
    let until = Instant::now() + WAIT;
    loop {
        if let Ok(mut client) = GuardianClient::connect(&guardian.socket_dir()) {
            if let Ok(snapshot) = client.hello() {
                if snapshot.state == WireState::DetachedFenced {
                    guardian.assert_original_parent().unwrap();
                    return snapshot;
                }
            }
        }
        assert!(Instant::now() < until, "guardian did not fence runner exit");
        poll_pause();
    }
}

fn same_custody(current: &WireSnapshot, original: &WireSnapshot) {
    assert_eq!(current.instance, original.instance);
    assert_eq!(current.attempt, original.attempt);
    assert_eq!(current.deadline, original.deadline);
    assert_eq!(current.slots, original.slots);
    let current = current.native_binding.as_ref().unwrap();
    let original = original.native_binding.as_ref().unwrap();
    assert_eq!(current.guardian, original.guardian);
    assert_eq!(current.nonce, original.nonce);
    assert_eq!(current.slots, original.slots);
}

fn snapshot(line: &str, prefix: &str) -> WireSnapshot {
    serde_json::from_str(line.strip_prefix(prefix).unwrap()).unwrap()
}

#[test]
#[allow(clippy::too_many_lines)]
fn launchd_retains_original_parent_through_three_independent_runners() {
    // Missing gui/<current uid> is a qualification failure with diagnostics, not a skip
    // or permission to fall back to a user/system domain or touch an installed agent.
    let mut guardian = DisposableLaunchdGuardian::start().unwrap();
    let path = guardian.store();
    let attempt = ATTEMPT;
    let deadline = guardian.deadline();
    let raw =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(
        raw.pragma_query_value::<u32, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        5
    );
    drop(raw);
    guardian.assert_original_parent().unwrap();
    let mut initial = Runner::start(
        sampling_runner_command(&guardian.socket_dir(), &path, deadline, true),
        &guardian,
        "initial",
    );
    let initial_pid = initial.child.id();
    initial.marker("verified binding: InitialActive");
    initial.marker("application_committed_pending_ack revision=");
    let mut observed = ControllerSqliteStore::open_read_only(&path).unwrap();
    let binding = observed.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(binding.identity().process, guardian.guardian());
    assert_eq!(binding.slots().len(), 1);
    assert_eq!(binding.slots()[0].process, guardian.child());
    assert_eq!(binding.deadline(), deadline);
    let (key, pending) = observed
        .pending_native_sampling_ack(attempt)
        .unwrap()
        .unwrap();
    let NativeSamplingStatus::AppliedPendingAck {
        sample,
        applying_revision,
        ..
    } = pending
    else {
        panic!("expected exactly one applied pending ACK");
    };
    let before = sampling_rows(&path);
    assert_eq!((before.0, before.1, before.2, before.4), (1, 1, 0, 0));
    assert_eq!(sample.sequence, 0);
    initial.kill();
    let original = detached(&mut guardian);
    assert_eq!(original.deadline, deadline.get());
    assert_eq!(
        original.native_binding.as_ref().unwrap().guardian,
        binding.identity().process.into()
    );
    assert_eq!(
        original.native_binding.as_ref().unwrap().nonce,
        binding.identity().nonce.bytes()
    );
    assert_eq!(
        original.native_binding.as_ref().unwrap().slots[0].identity,
        guardian.child().into()
    );
    assert_eq!(
        original.retained.len(),
        1,
        "result retained after runner death"
    );

    let mut ack = Runner::start(
        recovery_command(&guardian, "recover-applied-ack"),
        &guardian,
        "ack",
    );
    assert_ne!(ack.child.id(), initial_pid);
    ack.success();
    let ack_output = bounded_text(&ack.out).unwrap();
    assert!(ack_output.contains("reconciling generation=2 interruption="));
    assert!(ack_output.contains("sample_ack_confirmed generation=2"));
    let settled = (1, 1, 1, before.3, before.4);
    assert_eq!(sampling_rows(&path), settled);
    assert!(matches!(observed.load_native_sampling_status(key).unwrap(),
        Some(NativeSamplingStatus::SampleAckConfirmed {
            sample: confirmed, applying_revision: revision, confirming_session_generation, ..
        }) if confirmed == sample && revision == applying_revision && confirming_session_generation.get() == 2));
    let ack_binding = observed.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(
        ack_binding.current_session().state,
        NativeSessionState::Reconciling
    );
    let after_ack = detached(&mut guardian);
    same_custody(&after_ack, &original);
    assert_eq!(after_ack.session.generation, 2);
    assert!(after_ack.retained.is_empty());

    let mut command = recovery_command(&guardian, "recover-and-finish");
    command.arg("--fixture-activation-barriers");
    let mut finish = Runner::start(command, &guardian, "finish");
    assert_ne!(finish.child.id(), initial_pid);
    assert_ne!(finish.child.id(), ack.child.id());
    finish.marker("verified binding: Detached");
    finish.marker("reconciling generation=3 interruption=");
    let generation = NonZeroU64::new(3).unwrap();
    let prepared = snapshot(&finish.marker("prepared_snapshot="), "prepared_snapshot=");
    assert_eq!(prepared.state, WireState::Reconciling);
    assert_eq!(prepared.session.generation, 3);
    same_custody(&prepared, &original);
    let prepared_binding = observed.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(
        prepared_binding.current_session().state,
        NativeSessionState::Reconciling
    );
    assert!(observed
        .read_native_reconciliation_evidence(attempt, generation)
        .unwrap()
        .is_some());
    assert!(observed
        .load_native_reconciliation_activation(attempt, generation)
        .unwrap()
        .is_none());
    assert_eq!(sampling_rows(&path), settled);
    guardian.assert_original_parent().unwrap();
    finish.release();

    let durable = snapshot(
        &finish.marker("durable_activation_snapshot="),
        "durable_activation_snapshot=",
    );
    assert_eq!(
        durable.state,
        WireState::Reconciling,
        "no live Active before durable activation"
    );
    assert_eq!(durable.session, prepared.session);
    same_custody(&durable, &original);
    let active_binding = observed.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(
        active_binding.current_session().state,
        NativeSessionState::Active
    );
    let activation = observed
        .load_native_reconciliation_activation(attempt, generation)
        .unwrap()
        .unwrap();
    assert_eq!(activation.binding_revision(), active_binding.revision());
    assert!(
        activation.completion().interruption_revision()
            > ack_binding.current_session().interruption_revision
    );
    assert!(
        activation.completion().recovery_revision()
            > activation.completion().interruption_revision()
    );
    assert_eq!(
        active_binding.dispatch_floor(),
        activation.completion().recovery_revision()
    );
    assert_eq!(sampling_rows(&path), settled);
    guardian.assert_original_parent().unwrap();
    finish.release();
    finish.marker("reconciliation_active generation=3 interruption=");
    assert!(observed
        .load_native_reconciliation_activation(attempt, generation)
        .unwrap()
        .is_some());
    let active = snapshot(&finish.marker("active_snapshot="), "active_snapshot=");
    assert_eq!(active.state, WireState::Active);
    assert_eq!(active.session, prepared.session);
    assert!(active.retained.is_empty());
    same_custody(&active, &original);
    finish.success();
    let final_snapshot = detached(&mut guardian);
    assert_eq!(final_snapshot.session, active.session);
    same_custody(&final_snapshot, &original);
    let mut writable = ControllerSqliteStore::open_existing(&path).unwrap();
    assert_eq!(
        verify_native_binding(&mut writable, attempt, &final_snapshot).unwrap(),
        VerifiedBindingStatus::Detached
    );
    let final_binding = writable.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(final_binding.identity(), binding.identity());
    assert_eq!(final_binding.slots(), binding.slots());
    assert_eq!(final_binding.deadline(), deadline);
    assert_eq!(final_binding.current_session().generation, generation);
    assert_eq!(
        final_binding.current_session().state,
        NativeSessionState::Detached
    );
    assert_eq!(
        final_binding.dispatch_floor(),
        activation.completion().recovery_revision()
    );
    assert_eq!(
        writable
            .load_attempt(attempt)
            .unwrap()
            .unwrap()
            .persistence_revision(),
        activation.completion().recovery_revision()
    );
    assert_eq!(
        sampling_rows(&path),
        settled,
        "no duplicate native execution or reapplication"
    );
    let raw =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    for table in [
        "native_reconciliation_completions",
        "native_reconciliation_activations",
    ] {
        assert_eq!(
            raw.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    guardian.assert_original_parent().unwrap();
    assert!(MonotonicTick::native_now().unwrap() < deadline);
    drop((initial, ack, finish, raw, observed, writable));
    // Consuming finish confirms exact-label absence AND original guardian/child disappearance.
    guardian.finish().unwrap();
}
