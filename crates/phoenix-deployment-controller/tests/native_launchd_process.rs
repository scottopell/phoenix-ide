#![cfg(target_os = "macos")]

#[path = "support/launchd.rs"]
mod launchd;
#[path = "support/native_sampling.rs"]
mod native_sampling;

use launchd::{
    bounded_text, poll_pause, private_file, CleanupFailures, DisposableLaunchdGuardian,
    DisposableLaunchdInstallation, DisposableLaunchdRunnerJob, ATTEMPT,
};
use native_sampling::{sampling_rows, sampling_runner_command, settled_sampling_keys};
use phoenix_deployment_controller::{
    guardian_ipc::{
        WireCompletion, WireIdentityObservation, WireMemoryPressure, WireObservation, WireSnapshot,
        WireState,
    },
    native_guardian::{verify_native_binding, GuardianClient, VerifiedBindingStatus},
    CleanupOutcome, ControllerOs, ControllerSqliteStore, IdentityObservation, MacOsControllerOs,
    MonotonicTick, NativeChildListStatus, NativeSamplingStatus, NativeSessionState, Phase,
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

fn sampling_table_counts(path: &std::path::Path) -> [i64; 5] {
    let counts = || {
        let db =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        [
            "native_sampling_issues",
            "native_sampling_applications",
            "native_sampling_ack_confirmations",
            "native_reconciliation_completions",
            "native_reconciliation_activations",
        ]
        .map(|table| {
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
        })
    };
    counts()
}

fn child_list_table_counts(path: &std::path::Path) -> [i64; 4] {
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    [
        "native_child_list_issues",
        "native_child_list_applications",
        "native_child_list_ack_confirmations",
        "native_child_list_result_children",
    ]
    .map(|table| {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap()
    })
}

fn identity_observation_table_counts(path: &std::path::Path) -> [i64; 3] {
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    [
        "native_identity_observation_issues",
        "native_identity_observation_applications",
        "native_identity_observation_ack_confirmations",
    ]
    .map(|table| {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap()
    })
}

fn child_list_issue_parent(
    path: &std::path::Path,
) -> phoenix_deployment_controller::ProcessIdentity {
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let (pid, seconds, nanos): (i64, Vec<u8>, i64) = db
        .query_row(
            "SELECT parent_pid,parent_birth_seconds,parent_birth_nanos FROM native_child_list_issues",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    phoenix_deployment_controller::ProcessIdentity::new(
        phoenix_deployment_controller::ProcessId::new(
            std::num::NonZeroU32::new(u32::try_from(pid).unwrap()).unwrap(),
        ),
        phoenix_deployment_controller::BirthToken::new(
            u64::from_be_bytes(seconds.try_into().unwrap()),
            u32::try_from(nanos).unwrap(),
        )
        .unwrap(),
    )
}

fn assert_initial_applied_ack(
    runner: &mut DisposableLaunchdRunnerJob<'_>,
    path: &std::path::Path,
    original_guardian: phoenix_deployment_controller::ProcessIdentity,
    original_child: phoenix_deployment_controller::ProcessIdentity,
    deadline: MonotonicTick,
) -> (
    ControllerSqliteStore,
    phoenix_deployment_controller::DispatchKey,
    NativeSamplingStatus,
) {
    let applied_marker = runner
        .marker("application_committed_pending_ack revision=")
        .unwrap();
    runner.assert_live().unwrap();
    runner.guardian().assert_original_parent().unwrap();
    assert_eq!(runner.guardian().guardian(), original_guardian);
    assert_eq!(runner.guardian().child(), original_child);
    assert_eq!(runner.guardian().deadline(), deadline);
    assert!(MonotonicTick::native_now().unwrap() < deadline);
    runner.assert_witnesses().unwrap();
    let mut store = ControllerSqliteStore::open_read_only(path).unwrap();
    let (key, pending) = store.pending_native_sampling_ack(ATTEMPT).unwrap().unwrap();
    assert_eq!(
        applied_marker,
        format!(
            "application_committed_pending_ack revision={}",
            key.revision().get()
        )
    );
    (store, key, pending)
}

fn assert_fenced_sample(fenced: &WireSnapshot, revision: u64, deadline: MonotonicTick) {
    assert_eq!(fenced.state, WireState::DetachedFenced);
    assert_eq!(fenced.deadline, deadline.get());
    assert_eq!(fenced.retained.len(), 1);
    assert_eq!(fenced.retained[0].key.attempt, ATTEMPT.get());
    assert_eq!(fenced.retained[0].key.revision, revision);
    assert_eq!(fenced.retained[0].key.ordinal, 0);
    assert!(matches!(
        &fenced.retained[0].completion,
        WireCompletion::SampleResources {
            sequence: 0,
            result: Ok(result)
        } if result.sequence == 0
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn installed_two_job_launchd_reaches_ready_after_natural_root_exit_without_signals() {
    let mut installation = DisposableLaunchdInstallation::start_with_timing(30, 60, 70).unwrap();
    installation.assert_witnesses().unwrap();
    let root_path = installation.guardian().root().to_path_buf();
    let path = installation.guardian().store();
    let deadline = installation.guardian().deadline();
    let original_guardian = installation.guardian().guardian();
    let original_root = installation.guardian().child();
    installation.guardian().assert_original_parent().unwrap();

    let mut runner = DisposableLaunchdRunnerJob::start(&mut installation).unwrap();
    let label = runner.label().to_owned();
    runner.assert_live().unwrap();
    let initial_pid = runner.pid();
    assert_eq!(
        runner.marker("verified binding: InitialActive").unwrap(),
        "verified binding: InitialActive"
    );
    let (mut store, key, pending) = assert_initial_applied_ack(
        &mut runner,
        &path,
        original_guardian,
        original_root,
        deadline,
    );
    let NativeSamplingStatus::AppliedPendingAck {
        sample,
        applying_revision,
        ..
    } = pending
    else {
        panic!("expected durable applied sample pending ACK");
    };
    assert_eq!(sample.sequence, 0);
    assert_eq!(sampling_table_counts(&path), [1, 1, 0, 0, 0]);
    let before = store.load_native_binding(ATTEMPT).unwrap().unwrap();
    assert_eq!(before.identity().process, original_guardian);
    assert_eq!(before.slots().len(), 1);
    assert_eq!(before.slots()[0].process, original_root);
    assert_eq!(before.deadline(), deadline);
    assert!(MonotonicTick::native_now().unwrap() < deadline);
    runner.assert_witnesses().unwrap();
    runner.kill().unwrap();
    let fenced = detached(runner.guardian());
    assert_fenced_sample(&fenced, key.revision().get(), deadline);
    runner.assert_initial_generation_evidence().unwrap();
    runner.recover_applied_ack().unwrap();
    runner.assert_archived_generation(1).unwrap();
    assert_eq!(runner.label(), label);
    runner.assert_live().unwrap();
    let ack_pid = runner.pid();
    assert_ne!(ack_pid, initial_pid);
    assert_eq!(
        runner.marker("verified binding: Detached").unwrap(),
        "verified binding: Detached"
    );
    runner
        .marker("reconciling generation=2 interruption=")
        .unwrap();
    assert_eq!(
        runner.marker("sample_ack_confirmed generation=2").unwrap(),
        "sample_ack_confirmed generation=2"
    );
    runner.assert_live().unwrap();
    runner.guardian().assert_original_parent().unwrap();
    runner.assert_witnesses().unwrap();
    runner.release().unwrap();
    runner.success().unwrap();
    assert_eq!(sampling_rows(&path), (1, 1, 1, applying_revision.get(), 0));
    assert!(matches!(store.load_native_sampling_status(key).unwrap(),
        Some(NativeSamplingStatus::SampleAckConfirmed { sample: confirmed, applying_revision: revision, confirming_session_generation, .. })
        if confirmed == sample && revision == applying_revision && confirming_session_generation.get() == 2));
    assert!(store
        .pending_native_sampling_ack(ATTEMPT)
        .unwrap()
        .is_none());
    let settled = detached(runner.guardian());
    same_custody(&settled, &fenced);
    assert_eq!(settled.session.generation, 2);
    assert!(settled.retained.is_empty());
    runner.guardian().assert_original_parent().unwrap();
    assert_eq!(runner.guardian().guardian(), original_guardian);
    assert_eq!(runner.guardian().child(), original_root);
    assert_eq!(runner.guardian().deadline(), deadline);
    assert!(MonotonicTick::native_now().unwrap() < deadline);
    assert_eq!(
        store.load_native_binding(ATTEMPT).unwrap().unwrap().slots(),
        before.slots()
    );
    runner.assert_witnesses().unwrap();

    runner.recover_finish_and_ready().unwrap();
    runner.assert_archived_generation(2).unwrap();
    assert_eq!(runner.label(), label);
    runner.assert_live().unwrap();
    let ready_pid = runner.pid();
    assert_ne!(ready_pid, ack_pid);
    assert_ne!(ready_pid, initial_pid);
    runner.guardian().assert_guardian_job().unwrap();
    assert_eq!(
        runner.marker("verified binding: Detached").unwrap(),
        "verified binding: Detached"
    );
    runner
        .marker("reconciling generation=3 interruption=")
        .unwrap();
    let prepared = snapshot(
        &runner.marker("prepared_snapshot=").unwrap(),
        "prepared_snapshot=",
    );
    assert_eq!(prepared.session.generation, 3);
    same_custody(&prepared, &fenced);
    let durable = snapshot(
        &runner.marker("durable_activation_snapshot=").unwrap(),
        "durable_activation_snapshot=",
    );
    assert_eq!(durable.session, prepared.session);
    same_custody(&durable, &fenced);
    runner
        .marker("reconciliation_active generation=3 interruption=")
        .unwrap();
    let active = snapshot(
        &runner.marker("active_snapshot=").unwrap(),
        "active_snapshot=",
    );
    assert_eq!(active.state, WireState::Active);
    assert_eq!(active.session.generation, 3);
    same_custody(&active, &fenced);
    assert!(active.retained.is_empty());
    let second_sample = runner
        .marker("recovered_sample_applied_ack attempt=64")
        .unwrap();
    let second_revision: u64 = second_sample
        .split_once(" revision=")
        .unwrap()
        .1
        .split_once(" ordinal=0 sequence=1")
        .unwrap()
        .0
        .parse()
        .unwrap();
    runner.assert_witnesses().unwrap();

    let ready = runner
        .marker_with_timeout(
            "recover_finish_and_ready attempt=64",
            Duration::from_secs(65),
        )
        .unwrap();
    assert!(ready.contains("generation=3 phase=ReadyForDisruption cleanup=VerifiedClean effects=0 signal_exact_identity=0"), "{ready}");
    runner.success().unwrap();
    runner.guardian().assert_guardian_job().unwrap();
    runner.assert_witnesses().unwrap();
    assert_eq!(runner.label(), label);
    assert_eq!(child_list_table_counts(&path), [1, 1, 1, 0]);
    assert_eq!(child_list_issue_parent(&path), original_root);
    assert_eq!(sampling_table_counts(&path), [2, 2, 2, 1, 1]);
    let continued = sampling_rows(&path);
    assert_eq!(
        (continued.0, continued.1, continued.2, continued.4),
        (2, 2, 2, 1)
    );
    assert_eq!(
        settled_sampling_keys(&path),
        vec![
            (key.revision().get(), 0, 0, 1, 2),
            (second_revision, 0, 1, 3, 3),
        ]
    );
    let db =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let count = |table: &str| -> i64 {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    };
    assert_eq!(
        [
            count("native_identity_observation_issues"),
            count("native_identity_observation_applications"),
            count("native_identity_observation_ack_confirmations"),
        ],
        [1, 1, 1]
    );
    let (outcome, pid, birth_seconds, birth_nanos): (String, i64, Vec<u8>, i64) = db.query_row(
        "SELECT a.outcome,i.target_pid,i.target_birth_seconds,i.target_birth_nanos FROM native_identity_observation_issues i JOIN native_identity_observation_applications a ON a.attempt_id=i.attempt_id AND a.dispatch_revision=i.dispatch_revision AND a.ordinal=i.ordinal",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap();
    assert_eq!(outcome, "exact_identity_absent");
    assert_eq!(pid, i64::from(original_root.pid().get()));
    assert_eq!(
        u64::from_be_bytes(birth_seconds.try_into().unwrap()),
        original_root.birth().seconds()
    );
    assert_eq!(birth_nanos, i64::from(original_root.birth().nanos()));
    assert_eq!(
        MacOsControllerOs::new("/tmp")
            .unwrap()
            .observe_identity(original_root),
        IdentityObservation::ExactIdentityAbsent
    );
    let final_attempt = store.load_attempt(ATTEMPT).unwrap().unwrap();
    assert_eq!(final_attempt.phase(), Phase::ReadyForDisruption);
    assert_eq!(
        final_attempt.cleanup_outcome(),
        CleanupOutcome::VerifiedClean
    );
    assert_eq!(final_attempt.deadline(), deadline);
    let final_binding = store.load_native_binding(ATTEMPT).unwrap().unwrap();
    assert_eq!(final_binding.identity(), before.identity());
    assert_eq!(final_binding.slots(), before.slots());
    assert_eq!(final_binding.deadline(), before.deadline());
    assert!(store
        .pending_native_child_list_ack(ATTEMPT)
        .unwrap()
        .is_none());
    assert!(store
        .pending_native_identity_observation_ack(ATTEMPT)
        .unwrap()
        .is_none());
    let final_snapshot = GuardianClient::connect(&runner.guardian().socket_dir())
        .unwrap()
        .hello()
        .unwrap();
    assert_eq!(final_snapshot.state, WireState::DetachedFenced);
    assert_eq!(final_snapshot.session.generation, 3);
    assert!(final_snapshot.retained.is_empty());
    assert_eq!(final_snapshot.instance, fenced.instance);
    assert_eq!(final_snapshot.attempt, fenced.attempt);
    assert_eq!(final_snapshot.deadline, fenced.deadline);
    assert_eq!(final_snapshot.slots, fenced.slots);
    let final_native = final_snapshot.native_binding.as_ref().unwrap();
    let fenced_native = fenced.native_binding.as_ref().unwrap();
    assert_eq!(final_native.guardian, fenced_native.guardian);
    assert_eq!(final_native.nonce, fenced_native.nonce);
    assert_eq!(final_native.slots.len(), 1);
    assert_eq!(
        final_native.slots[0].identity,
        fenced_native.slots[0].identity
    );
    assert_eq!(final_native.slots[0].liveness, WireObservation::Exited);
    runner.finish().unwrap();
    installation.assert_witnesses().unwrap();
    drop((store, db));
    installation.finish().unwrap();
    assert!(
        !root_path.exists(),
        "installed fixture root was not torn down"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn installed_two_job_launchd_recovers_completed_unapplied_identity_without_reobservation() {
    let mut installation = DisposableLaunchdInstallation::start_with_timing(30, 60, 70).unwrap();
    installation.assert_witnesses().unwrap();
    let root_path = installation.guardian().root().to_path_buf();
    let path = installation.guardian().store();
    let deadline = installation.guardian().deadline();
    let original_guardian = installation.guardian().guardian();
    let original_root = installation.guardian().child();
    installation.guardian().assert_original_parent().unwrap();

    let mut runner = DisposableLaunchdRunnerJob::start(&mut installation).unwrap();
    runner.assert_live().unwrap();
    let first_pid = runner.pid();
    assert_eq!(
        runner.marker("verified binding: InitialActive").unwrap(),
        "verified binding: InitialActive"
    );
    runner
        .marker("application_committed_pending_ack revision=")
        .unwrap();
    runner.kill().unwrap();
    let initial_detached = detached(runner.guardian());
    assert_eq!(initial_detached.retained.len(), 1);

    runner.recover_applied_ack().unwrap();
    runner.assert_archived_generation(1).unwrap();
    let second_pid = runner.pid();
    assert_ne!(second_pid, first_pid);
    assert_eq!(
        runner.marker("verified binding: Detached").unwrap(),
        "verified binding: Detached"
    );
    runner
        .marker("reconciling generation=2 interruption=")
        .unwrap();
    assert_eq!(
        runner.marker("sample_ack_confirmed generation=2").unwrap(),
        "sample_ack_confirmed generation=2"
    );
    runner.release().unwrap();
    runner.success().unwrap();
    let settled_initial = detached(runner.guardian());
    same_custody(&settled_initial, &initial_detached);
    assert!(settled_initial.retained.is_empty());

    runner
        .recover_finish_and_ready_before_identity_application()
        .unwrap();
    runner.assert_archived_generation(2).unwrap();
    let third_pid = runner.pid();
    assert_ne!(third_pid, second_pid);
    assert_ne!(third_pid, first_pid);
    assert_eq!(
        runner.marker("verified binding: Detached").unwrap(),
        "verified binding: Detached"
    );
    runner
        .marker("reconciling generation=3 interruption=")
        .unwrap();
    runner.marker("prepared_snapshot=").unwrap();
    runner.marker("durable_activation_snapshot=").unwrap();
    runner
        .marker("reconciliation_active generation=3 interruption=")
        .unwrap();
    runner.marker("active_snapshot=").unwrap();
    runner
        .marker("recovered_sample_applied_ack attempt=64")
        .unwrap();
    let observed = runner
        .marker_with_timeout(
            "identity_observation_completed_pending_application revision=",
            Duration::from_secs(45),
        )
        .unwrap();
    let issue_revision = observed
        .strip_prefix("identity_observation_completed_pending_application revision=")
        .unwrap()
        .parse::<u64>()
        .unwrap();
    runner.assert_live().unwrap();
    runner.assert_witnesses().unwrap();
    assert_eq!(runner.guardian().guardian(), original_guardian);
    assert_eq!(runner.guardian().child(), original_root);
    assert_eq!(runner.guardian().deadline(), deadline);
    assert!(MonotonicTick::native_now().unwrap() < deadline);
    assert_eq!(identity_observation_table_counts(&path), [1, 0, 0]);
    assert_eq!(child_list_table_counts(&path), [1, 1, 1, 0]);
    let observed_sampling = sampling_rows(&path);
    let mut store = ControllerSqliteStore::open_read_only(&path).unwrap();
    let binding = store.load_native_binding(ATTEMPT).unwrap().unwrap();
    assert_eq!(binding.identity().process, original_guardian);
    assert_eq!(binding.slots().len(), 1);
    assert_eq!(binding.slots()[0].process, original_root);
    assert_eq!(binding.deadline(), deadline);
    assert!(store
        .pending_native_identity_observation_ack(ATTEMPT)
        .unwrap()
        .is_none());

    runner.kill().unwrap();
    let crashed = detached(runner.guardian());
    same_custody(&crashed, &settled_initial);
    assert_eq!(crashed.retained.len(), 1);
    let retained = &crashed.retained[0];
    assert_eq!(retained.key.attempt, ATTEMPT.get());
    assert_eq!(retained.key.revision, issue_revision);
    assert_eq!(retained.key.ordinal, 0);
    assert!(matches!(
        &retained.completion,
        WireCompletion::ObserveIdentity {
            identity,
            observation: WireIdentityObservation::ExactIdentityAbsent,
        } if *identity == original_root.into()
    ));
    assert_eq!(identity_observation_table_counts(&path), [1, 0, 0]);
    assert_eq!(sampling_rows(&path), observed_sampling);

    runner.recover_unapplied_identity().unwrap();
    runner.assert_archived_generation(3).unwrap();
    assert_ne!(runner.pid(), third_pid);
    assert_ne!(runner.pid(), second_pid);
    assert_ne!(runner.pid(), first_pid);
    assert_eq!(
        runner.marker("verified binding: Detached").unwrap(),
        "verified binding: Detached"
    );
    runner
        .marker("reconciling generation=4 interruption=")
        .unwrap();
    assert_eq!(
        runner
            .marker("recovered_identity_verified_pending_application revision=")
            .unwrap(),
        format!("recovered_identity_verified_pending_application revision={issue_revision}")
    );
    runner.assert_live().unwrap();
    runner.assert_witnesses().unwrap();
    assert_eq!(identity_observation_table_counts(&path), [1, 0, 0]);
    assert_eq!(sampling_rows(&path), observed_sampling);
    runner.release().unwrap();
    assert_eq!(
        runner
            .marker("recovered_identity_applied_ack attempt=64")
            .unwrap(),
        format!("recovered_identity_applied_ack attempt=64 revision={issue_revision} ordinal=0 effects=0 observe_identity=0 signal_exact_identity=0")
    );
    runner.success().unwrap();
    runner.assert_witnesses().unwrap();
    let confirmed = detached(runner.guardian());
    same_custody(&confirmed, &crashed);
    assert!(confirmed.retained.is_empty());
    assert_eq!(identity_observation_table_counts(&path), [1, 1, 1]);
    assert_eq!(sampling_rows(&path), observed_sampling);
    assert!(store
        .pending_native_identity_observation_ack(ATTEMPT)
        .unwrap()
        .is_none());
    let final_attempt = store.load_attempt(ATTEMPT).unwrap().unwrap();
    assert_eq!(final_attempt.phase(), Phase::Interrupted);
    assert_ne!(final_attempt.phase(), Phase::ReadyForDisruption);
    assert_eq!(final_attempt.deadline(), deadline);
    let final_binding = store.load_native_binding(ATTEMPT).unwrap().unwrap();
    assert_eq!(final_binding.identity(), binding.identity());
    assert_eq!(final_binding.slots(), binding.slots());
    assert_eq!(final_binding.deadline(), binding.deadline());
    let db =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let (outcome, confirming_generation): (String, Vec<u8>) = db
        .query_row(
            "SELECT a.outcome,c.confirming_session_generation FROM native_identity_observation_applications a JOIN native_identity_observation_ack_confirmations c USING(attempt_id,dispatch_revision,ordinal)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(outcome, "exact_identity_absent");
    assert_eq!(
        u64::from_be_bytes(confirming_generation.try_into().unwrap()),
        4
    );
    assert_eq!(
        MacOsControllerOs::new("/tmp")
            .unwrap()
            .observe_identity(original_root),
        IdentityObservation::ExactIdentityAbsent
    );
    runner.finish().unwrap();
    installation.assert_witnesses().unwrap();
    drop((store, db));
    installation.finish().unwrap();
    assert!(
        !root_path.exists(),
        "qualified fixture root was not torn down"
    );
}

#[test]
fn installed_two_job_launchd_recovers_applied_ack_without_touching_application() {
    let mut installation = DisposableLaunchdInstallation::start().unwrap();
    installation.assert_witnesses().unwrap();
    let guardian = installation.guardian();
    guardian.assert_original_parent().unwrap();
    let path = guardian.store();
    let deadline = guardian.deadline();
    let original_guardian = guardian.guardian();
    let original_child = guardian.child();
    let mut runner = DisposableLaunchdRunnerJob::start(&mut installation).unwrap();
    runner.assert_live().unwrap();
    let initial_pid = runner.pid();
    assert_eq!(
        runner.marker("verified binding: InitialActive").unwrap(),
        "verified binding: InitialActive"
    );
    let (mut store, key, pending) = assert_initial_applied_ack(
        &mut runner,
        &path,
        original_guardian,
        original_child,
        deadline,
    );
    let NativeSamplingStatus::AppliedPendingAck {
        sample,
        applying_revision,
        ..
    } = pending
    else {
        panic!("expected durable applied pending ACK");
    };
    assert_eq!(sample.sequence, 0);
    assert_eq!(sampling_rows(&path), (1, 1, 0, applying_revision.get(), 0));

    assert_eq!(sampling_table_counts(&path), [1, 1, 0, 0, 0]);
    let before = store.load_native_binding(ATTEMPT).unwrap().unwrap();
    assert_eq!(before.identity().process, original_guardian);
    assert_eq!(before.slots()[0].process, original_child);
    assert_eq!(before.deadline(), deadline);
    runner.kill().unwrap();
    let fenced = detached(runner.guardian());
    assert_fenced_sample(&fenced, key.revision().get(), deadline);
    assert_eq!(sampling_table_counts(&path), [1, 1, 0, 0, 0]);
    runner.assert_witnesses().unwrap();
    runner.assert_initial_generation_evidence().unwrap();
    runner.recover_applied_ack().unwrap();
    runner.assert_initial_generation_evidence().unwrap();
    runner.assert_live().unwrap();
    assert_ne!(runner.pid(), initial_pid);
    assert_eq!(
        runner.marker("verified binding: Detached").unwrap(),
        "verified binding: Detached"
    );
    runner
        .marker("reconciling generation=2 interruption=")
        .unwrap();
    assert_eq!(
        runner.marker("sample_ack_confirmed generation=2").unwrap(),
        "sample_ack_confirmed generation=2"
    );
    runner.assert_live().unwrap();
    runner.guardian().assert_original_parent().unwrap();
    runner.assert_witnesses().unwrap();
    runner.release().unwrap();
    runner.success().unwrap();
    assert_eq!(sampling_table_counts(&path), [1, 1, 1, 0, 0]);
    assert_eq!(sampling_rows(&path), (1, 1, 1, applying_revision.get(), 0));
    assert!(matches!(store.load_native_sampling_status(key).unwrap(),
        Some(NativeSamplingStatus::SampleAckConfirmed { sample: confirmed, applying_revision: revision, confirming_session_generation, .. })
        if confirmed == sample && revision == applying_revision && confirming_session_generation.get() == 2));
    assert!(store
        .pending_native_sampling_ack(ATTEMPT)
        .unwrap()
        .is_none());
    let after = store.load_native_binding(ATTEMPT).unwrap().unwrap();
    assert_eq!(after.identity(), before.identity());
    assert_eq!(after.slots(), before.slots());
    assert_eq!(after.deadline(), before.deadline());
    let settled = detached(runner.guardian());
    same_custody(&settled, &fenced);
    assert!(settled.retained.is_empty());
    assert_eq!(settled.state, WireState::DetachedFenced);
    runner.guardian().assert_original_parent().unwrap();
    assert!(MonotonicTick::native_now().unwrap() < deadline);
    runner.assert_witnesses().unwrap();
    runner.finish().unwrap();
    installation.assert_witnesses().unwrap();
    drop(store);
    installation.finish().unwrap();
}

#[test]
#[allow(clippy::too_many_lines)]
fn installed_two_job_launchd_recovers_child_list_ack_without_reenumeration_or_mutation() {
    let mut installation = DisposableLaunchdInstallation::start().unwrap();
    installation.assert_witnesses().unwrap();
    let root_path = installation.guardian().root().to_path_buf();
    let path = installation.guardian().store();
    let deadline = installation.guardian().deadline();
    let original_guardian = installation.guardian().guardian();
    let original_child = installation.guardian().child();
    installation.guardian().assert_original_parent().unwrap();

    let mut runner = DisposableLaunchdRunnerJob::start(&mut installation).unwrap();
    runner.assert_live().unwrap();
    assert_eq!(
        runner.marker("verified binding: InitialActive").unwrap(),
        "verified binding: InitialActive"
    );
    runner
        .marker("application_committed_pending_ack revision=")
        .unwrap();
    runner.assert_witnesses().unwrap();
    runner.kill().unwrap();
    let initial_detached = detached(runner.guardian());
    assert_eq!(initial_detached.retained.len(), 1);

    runner.recover_applied_ack().unwrap();
    runner.assert_archived_generation(1).unwrap();
    assert_eq!(
        runner.marker("verified binding: Detached").unwrap(),
        "verified binding: Detached"
    );
    runner
        .marker("reconciling generation=2 interruption=")
        .unwrap();
    assert_eq!(
        runner.marker("sample_ack_confirmed generation=2").unwrap(),
        "sample_ack_confirmed generation=2"
    );
    runner.assert_live().unwrap();
    runner.guardian().assert_original_parent().unwrap();
    runner.release().unwrap();
    runner.success().unwrap();
    runner.assert_witnesses().unwrap();

    runner.recover_finish_and_child_list().unwrap();
    runner.assert_archived_generation(2).unwrap();
    assert_eq!(
        runner.marker("verified binding: Detached").unwrap(),
        "verified binding: Detached"
    );
    runner
        .marker("reconciling generation=3 interruption=")
        .unwrap();
    runner.marker("prepared_snapshot=").unwrap();
    runner.marker("durable_activation_snapshot=").unwrap();
    runner
        .marker("reconciliation_active generation=3 interruption=")
        .unwrap();
    runner.marker("active_snapshot=").unwrap();
    let sample_marker = runner
        .marker("recovered_sample_applied_ack attempt=64")
        .unwrap();
    assert!(sample_marker.ends_with("sequence=1"), "{sample_marker}");
    let child_marker = runner
        .marker("child_list_application_committed_pending_ack revision=")
        .unwrap();
    assert!(child_marker.contains("revision="), "{child_marker}");
    runner.assert_live().unwrap();
    runner.guardian().assert_original_parent().unwrap();
    runner.assert_witnesses().unwrap();
    assert_eq!(child_list_table_counts(&path), [1, 1, 0, 0]);
    assert_eq!(child_list_issue_parent(&path), original_child);
    let mut before_ack = ControllerSqliteStore::open_read_only(&path).unwrap();
    let (child_key, child_status) = before_ack
        .pending_native_child_list_ack(ATTEMPT)
        .unwrap()
        .unwrap();
    assert!(matches!(
        child_status,
        NativeChildListStatus::AppliedPendingAck { children, .. } if children.is_empty()
    ));

    runner.kill().unwrap();
    let fenced = detached(runner.guardian());
    assert_eq!(fenced.retained.len(), 1);
    assert!(matches!(
        &fenced.retained[0].completion,
        WireCompletion::ListChildren {
            parent,
            result: Ok(children),
        } if *parent == original_child.into() && children.is_empty()
    ));
    assert_eq!(child_list_table_counts(&path), [1, 1, 0, 0]);
    runner.guardian().assert_original_parent().unwrap();
    runner.assert_witnesses().unwrap();

    runner.recover_child_list_ack().unwrap();
    runner.assert_archived_generation(3).unwrap();
    assert_eq!(
        runner.marker("verified binding: Detached").unwrap(),
        "verified binding: Detached"
    );
    runner
        .marker("reconciling generation=4 interruption=")
        .unwrap();
    let ack_marker = runner
        .marker("child_list_ack_confirmed attempt=64")
        .unwrap();
    assert!(ack_marker.contains("phase=Interrupted"), "{ack_marker}");
    runner.assert_live().unwrap();
    runner.guardian().assert_original_parent().unwrap();
    runner.assert_witnesses().unwrap();
    assert_eq!(child_list_table_counts(&path), [1, 1, 1, 0]);

    runner.release().unwrap();
    runner.success().unwrap();
    let confirmed_detached = detached(runner.guardian());
    assert!(confirmed_detached.retained.is_empty());
    let mut store = ControllerSqliteStore::open_read_only(&path).unwrap();
    assert_eq!(child_list_table_counts(&path), [1, 1, 1, 0]);
    assert_eq!(child_list_issue_parent(&path), original_child);
    let final_binding = store.load_native_binding(ATTEMPT).unwrap().unwrap();
    assert_eq!(final_binding.identity().process, original_guardian);
    assert_eq!(final_binding.slots()[0].process, original_child);
    assert_eq!(
        store.load_attempt(ATTEMPT).unwrap().unwrap().phase(),
        Phase::Interrupted
    );
    assert!(matches!(
        store.load_native_child_list_status(child_key).unwrap(),
        Some(NativeChildListStatus::ChildrenAckConfirmed { children, .. })
            if children.is_empty()
    ));
    assert_eq!(sampling_rows(&path).0, 2);
    assert_eq!(sampling_rows(&path).1, 2);
    assert_eq!(sampling_rows(&path).2, 2);
    assert!(unsafe { libc::kill(i32::try_from(original_child.pid().get()).unwrap(), 0) } == 0);
    runner.guardian().assert_original_parent().unwrap();
    runner.finish().unwrap();
    installation.assert_witnesses().unwrap();
    drop(store);
    installation.finish().unwrap();
    assert!(
        !root_path.exists(),
        "qualified fixture root was not torn down"
    );
    assert!(MonotonicTick::native_now().unwrap() < deadline);
}

#[test]
#[allow(clippy::too_many_lines)]
fn installed_two_job_launchd_recovers_completed_unapplied_child_list_without_reenumeration() {
    let mut installation = DisposableLaunchdInstallation::start().unwrap();
    installation.assert_witnesses().unwrap();
    let root_path = installation.guardian().root().to_path_buf();
    let path = installation.guardian().store();
    let deadline = installation.guardian().deadline();
    let original_guardian = installation.guardian().guardian();
    let original_child = installation.guardian().child();
    installation.guardian().assert_original_parent().unwrap();

    let mut runner = DisposableLaunchdRunnerJob::start(&mut installation).unwrap();
    runner.assert_live().unwrap();
    let first_pid = runner.pid();
    assert_eq!(
        runner.marker("verified binding: InitialActive").unwrap(),
        "verified binding: InitialActive"
    );
    runner
        .marker("application_committed_pending_ack revision=")
        .unwrap();
    runner.assert_witnesses().unwrap();
    runner.kill().unwrap();
    let initial_detached = detached(runner.guardian());
    assert_eq!(initial_detached.retained.len(), 1);

    runner.recover_applied_ack().unwrap();
    runner.assert_archived_generation(1).unwrap();
    let second_pid = runner.pid();
    assert_ne!(second_pid, first_pid);
    assert_eq!(
        runner.marker("verified binding: Detached").unwrap(),
        "verified binding: Detached"
    );
    runner
        .marker("reconciling generation=2 interruption=")
        .unwrap();
    assert_eq!(
        runner.marker("sample_ack_confirmed generation=2").unwrap(),
        "sample_ack_confirmed generation=2"
    );
    runner.assert_live().unwrap();
    runner.guardian().assert_original_parent().unwrap();
    runner.release().unwrap();
    runner.success().unwrap();
    runner.assert_witnesses().unwrap();
    let settled_initial = detached(runner.guardian());
    same_custody(&settled_initial, &initial_detached);
    assert!(settled_initial.retained.is_empty());
    assert_eq!(child_list_table_counts(&path), [0, 0, 0, 0]);
    assert_eq!(sampling_table_counts(&path), [1, 1, 1, 0, 0]);

    runner
        .recover_finish_and_child_list_before_application()
        .unwrap();
    runner.assert_archived_generation(2).unwrap();
    let third_pid = runner.pid();
    assert_ne!(third_pid, second_pid);
    assert_ne!(third_pid, first_pid);
    assert_eq!(
        runner.marker("verified binding: Detached").unwrap(),
        "verified binding: Detached"
    );
    runner
        .marker("reconciling generation=3 interruption=")
        .unwrap();
    runner.marker("prepared_snapshot=").unwrap();
    runner.marker("durable_activation_snapshot=").unwrap();
    runner
        .marker("reconciliation_active generation=3 interruption=")
        .unwrap();
    runner.marker("active_snapshot=").unwrap();
    let sample_marker = runner
        .marker("recovered_sample_applied_ack attempt=64")
        .unwrap();
    assert!(sample_marker.ends_with("sequence=1"), "{sample_marker}");
    let observed = runner
        .marker("child_list_observed_pending_application revision=")
        .unwrap();
    let issue_revision = observed
        .strip_prefix("child_list_observed_pending_application revision=")
        .unwrap()
        .parse::<u64>()
        .unwrap();
    runner.assert_live().unwrap();
    runner.guardian().assert_original_parent().unwrap();
    assert_eq!(runner.guardian().guardian(), original_guardian);
    assert_eq!(runner.guardian().child(), original_child);
    assert_eq!(runner.guardian().deadline(), deadline);
    assert!(MonotonicTick::native_now().unwrap() < deadline);
    runner.assert_witnesses().unwrap();
    assert_eq!(child_list_table_counts(&path), [1, 0, 0, 0]);
    assert_eq!(child_list_issue_parent(&path), original_child);
    let observed_sampling = sampling_rows(&path);
    assert_eq!(sampling_table_counts(&path), [2, 2, 2, 1, 1]);
    let evidence =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let (revision, ordinal, generation): (Vec<u8>, i64, Vec<u8>) = evidence
        .query_row(
            "SELECT dispatch_revision, ordinal, session_generation FROM native_child_list_issues",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        u64::from_be_bytes(revision.try_into().unwrap()),
        issue_revision
    );
    assert_eq!(ordinal, 0);
    assert_eq!(u64::from_be_bytes(generation.try_into().unwrap()), 3);
    let mut store = ControllerSqliteStore::open_read_only(&path).unwrap();
    assert!(store
        .pending_native_child_list_ack(ATTEMPT)
        .unwrap()
        .is_none());
    let binding = store.load_native_binding(ATTEMPT).unwrap().unwrap();
    assert_eq!(binding.identity().process, original_guardian);
    assert_eq!(binding.slots().len(), 1);
    assert_eq!(binding.slots()[0].process, original_child);
    assert_eq!(binding.deadline(), deadline);

    runner.kill().unwrap();
    let crashed = detached(runner.guardian());
    same_custody(&crashed, &settled_initial);
    assert_eq!(crashed.retained.len(), 1);
    let retained = &crashed.retained[0];
    assert_eq!(retained.key.attempt, ATTEMPT.get());
    assert_eq!(retained.key.revision, issue_revision);
    assert_eq!(retained.key.ordinal, 0);
    assert!(matches!(
        &retained.completion,
        WireCompletion::ListChildren {
            parent,
            result: Ok(children),
        } if *parent == original_child.into() && children.is_empty()
    ));
    assert_eq!(child_list_table_counts(&path), [1, 0, 0, 0]);
    assert_eq!(child_list_issue_parent(&path), original_child);
    assert_eq!(sampling_rows(&path), observed_sampling);
    runner.guardian().assert_original_parent().unwrap();
    runner.assert_witnesses().unwrap();

    runner.recover_unapplied_child_list().unwrap();
    runner.assert_archived_generation(3).unwrap();
    assert_ne!(runner.pid(), third_pid);
    assert_ne!(runner.pid(), second_pid);
    assert_ne!(runner.pid(), first_pid);
    assert_eq!(
        runner.marker("verified binding: Detached").unwrap(),
        "verified binding: Detached"
    );
    runner
        .marker("reconciling generation=4 interruption=")
        .unwrap();
    assert_eq!(
        runner
            .marker("recovered_child_list_verified_pending_application revision=")
            .unwrap(),
        format!(
            "recovered_child_list_verified_pending_application revision={issue_revision} parent={}",
            original_child.pid().get()
        )
    );
    runner.assert_live().unwrap();
    runner.guardian().assert_original_parent().unwrap();
    runner.assert_witnesses().unwrap();
    assert_eq!(child_list_table_counts(&path), [1, 0, 0, 0]);
    assert_eq!(sampling_rows(&path), observed_sampling);
    assert!(store
        .pending_native_child_list_ack(ATTEMPT)
        .unwrap()
        .is_none());
    runner.release().unwrap();
    assert_eq!(
        runner
            .marker("recovered_child_list_applied_ack attempt=64")
            .unwrap(),
        format!("recovered_child_list_applied_ack attempt=64 revision={issue_revision} ordinal=0")
    );
    runner.success().unwrap();
    runner.assert_witnesses().unwrap();
    let confirmed = detached(runner.guardian());
    same_custody(&confirmed, &crashed);
    assert!(confirmed.retained.is_empty());
    assert_eq!(child_list_table_counts(&path), [1, 1, 1, 0]);
    assert_eq!(child_list_issue_parent(&path), original_child);
    assert_eq!(sampling_rows(&path), observed_sampling);
    let confirming_generation: Vec<u8> = evidence
        .query_row(
            "SELECT confirming_session_generation FROM native_child_list_ack_confirmations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        u64::from_be_bytes(confirming_generation.try_into().unwrap()),
        4
    );
    assert!(store
        .pending_native_child_list_ack(ATTEMPT)
        .unwrap()
        .is_none());
    let final_binding = store.load_native_binding(ATTEMPT).unwrap().unwrap();
    assert_eq!(final_binding.identity(), binding.identity());
    assert_eq!(final_binding.slots(), binding.slots());
    assert_eq!(final_binding.deadline(), binding.deadline());
    assert_eq!(
        store.load_attempt(ATTEMPT).unwrap().unwrap().phase(),
        Phase::Interrupted
    );
    let identity_issues: i64 = evidence
        .query_row(
            "SELECT count(*) FROM native_identity_observation_issues",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(identity_issues, 0);
    assert_eq!(
        unsafe { libc::kill(i32::try_from(original_child.pid().get()).unwrap(), 0) },
        0
    );
    runner.guardian().assert_original_parent().unwrap();
    runner.finish().unwrap();
    installation.assert_witnesses().unwrap();
    drop(store);
    drop(evidence);
    installation.finish().unwrap();
    assert!(
        !root_path.exists(),
        "qualified fixture root was not torn down"
    );
    assert!(MonotonicTick::native_now().unwrap() < deadline);
}

#[test]
fn launchd_retains_original_parent_through_three_independent_runners() {
    launchd_journey(false);
}

#[test]
fn launchd_continues_one_sample_through_three_independent_runners() {
    launchd_journey(true);
}

#[test]
#[allow(clippy::too_many_lines)]
fn launchd_recovers_completed_but_unapplied_initial_sample() {
    let mut guardian = DisposableLaunchdGuardian::start().unwrap();
    let path = guardian.store();
    let deadline = guardian.deadline();
    let attempt = ATTEMPT;
    guardian.assert_original_parent().unwrap();

    let mut command = sampling_runner_command(&guardian.socket_dir(), &path, deadline, false);
    command.arg("--fixture-pause-after-observation");
    let mut initial = Runner::start(command, &guardian, "unapplied-initial");
    let initial_pid = initial.child.id();
    initial.marker("verified binding: InitialActive");
    let marker = initial.marker("sample_observed_pending_application revision=");
    let (prefix, json) = marker.split_once(" snapshot=").expect("observation marker");
    let active: WireSnapshot = serde_json::from_str(json).unwrap();
    assert_eq!(active.state, WireState::Active);
    assert_eq!(active.session.generation, 1);
    assert_eq!(active.attempt, attempt.get());
    assert_eq!(active.deadline, deadline.get());
    assert_eq!(active.retained.len(), 1);
    let retained = &active.retained[0];
    assert_eq!(retained.key.attempt, attempt.get());
    assert_eq!(retained.key.ordinal, 0);
    let WireCompletion::SampleResources {
        sequence: 0,
        result: Ok(wire_sample),
    } = retained.completion
    else {
        panic!("expected one successful retained seq0 sample: {retained:?}");
    };
    assert_eq!(wire_sample.sequence, 0);
    assert_eq!(
        prefix,
        format!(
            "sample_observed_pending_application revision={} sequence=0",
            retained.key.revision
        )
    );
    assert!(initial.child.try_wait().unwrap().is_none());

    let raw =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let count = |table: &str| -> i64 {
        raw.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    };
    let counts = || {
        [
            "native_sampling_issues",
            "native_sampling_applications",
            "native_sampling_ack_confirmations",
            "native_reconciliation_completions",
            "native_reconciliation_activations",
        ]
        .map(count)
    };
    assert_eq!(counts(), [1, 0, 0, 0, 0]);
    let mut observed = ControllerSqliteStore::open_read_only(&path).unwrap();
    let binding = observed.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(binding.identity().process, guardian.guardian());
    assert_eq!(binding.slots().len(), 1);
    assert_eq!(binding.slots()[0].process, guardian.child());
    assert_eq!(binding.deadline(), deadline);
    assert_eq!(
        active.native_binding.as_ref().unwrap().guardian,
        binding.identity().process.into()
    );
    assert_eq!(
        active.native_binding.as_ref().unwrap().nonce,
        binding.identity().nonce.bytes()
    );
    assert_eq!(
        active.native_binding.as_ref().unwrap().slots[0].identity,
        guardian.child().into()
    );
    let issue_revision = phoenix_deployment_controller::PersistenceRevision::new(
        NonZeroU64::new(retained.key.revision).unwrap(),
    );
    let issued: (Vec<u8>, Vec<u8>, i64, Vec<u8>, Vec<u8>) = raw
        .query_row(
            "SELECT attempt_id, dispatch_revision, ordinal, sequence, session_generation FROM native_sampling_issues",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        u128::from_be_bytes(issued.0.try_into().unwrap()),
        attempt.get()
    );
    assert_eq!(
        u64::from_be_bytes(issued.1.try_into().unwrap()),
        issue_revision.get()
    );
    assert_eq!(issued.2, 0);
    assert_eq!(u64::from_be_bytes(issued.3.try_into().unwrap()), 0);
    assert_eq!(u64::from_be_bytes(issued.4.try_into().unwrap()), 1);
    assert!(observed
        .pending_native_sampling_ack(attempt)
        .unwrap()
        .is_none());
    let checkpoint = observed.load_attempt(attempt).unwrap().unwrap();
    assert_eq!(checkpoint.phase(), Phase::ResourceSampling);
    assert_eq!(checkpoint.persistence_revision(), issue_revision);
    assert_eq!(checkpoint.deadline(), deadline);
    guardian.assert_original_parent().unwrap();

    initial.kill();
    let fenced = detached(&mut guardian);
    assert_eq!(fenced.state, WireState::DetachedFenced);
    assert_eq!(fenced.session, active.session);
    assert_eq!(fenced.retained, active.retained);
    same_custody(&fenced, &active);
    assert_eq!(counts(), [1, 0, 0, 0, 0]);
    let still_pending = observed.load_attempt(attempt).unwrap().unwrap();
    assert_eq!(still_pending.phase(), checkpoint.phase());
    assert_eq!(
        still_pending.persistence_revision(),
        checkpoint.persistence_revision()
    );
    assert_eq!(still_pending.deadline(), checkpoint.deadline());

    let mut paused_command = recovery_command(&guardian, "recover-unapplied-sample");
    paused_command.arg("--fixture-pause-after-recovery-verification");
    let mut replacement = Runner::start(paused_command, &guardian, "recover-unapplied-paused");
    assert_ne!(replacement.child.id(), initial_pid);
    replacement.marker("verified binding: Detached");
    let interruption = issue_revision.checked_next().unwrap();
    assert_eq!(
        replacement.marker("reconciling generation=2 interruption="),
        format!(
            "reconciling generation=2 interruption={}",
            interruption.get()
        )
    );
    assert_eq!(
        replacement.marker("recovered_sample_verified_pending_application "),
        format!(
            "recovered_sample_verified_pending_application revision={} sequence=0",
            issue_revision.get()
        )
    );
    assert!(replacement.child.try_wait().unwrap().is_none());
    assert_eq!(counts(), [1, 0, 0, 0, 0]);
    let pending_issues: i64 = raw.query_row(
        "SELECT count(*) FROM native_sampling_issues i WHERE NOT EXISTS (SELECT 1 FROM native_sampling_applications a WHERE a.attempt_id = i.attempt_id AND a.dispatch_revision = i.dispatch_revision AND a.ordinal = i.ordinal)",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(pending_issues, 1, "exact issued-pending-application row");
    let still_issued: (Vec<u8>, Vec<u8>, i64, Vec<u8>, Vec<u8>) = raw.query_row(
        "SELECT attempt_id, dispatch_revision, ordinal, sequence, session_generation FROM native_sampling_issues",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    ).unwrap();
    assert_eq!(
        u128::from_be_bytes(still_issued.0.try_into().unwrap()),
        attempt.get()
    );
    assert_eq!(
        u64::from_be_bytes(still_issued.1.try_into().unwrap()),
        issue_revision.get()
    );
    assert_eq!(still_issued.2, 0);
    assert_eq!(u64::from_be_bytes(still_issued.3.try_into().unwrap()), 0);
    assert_eq!(u64::from_be_bytes(still_issued.4.try_into().unwrap()), 1);
    assert!(observed
        .pending_native_sampling_ack(attempt)
        .unwrap()
        .is_none());
    let pending = observed.load_attempt(attempt).unwrap().unwrap();
    assert_eq!(pending.phase(), Phase::Interrupted);
    assert_eq!(pending.persistence_revision(), interruption);
    assert_eq!(pending.deadline(), deadline);
    let pending_binding = observed.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(pending_binding.identity(), binding.identity());
    assert_eq!(pending_binding.slots(), binding.slots());
    assert_eq!(pending_binding.deadline(), deadline);
    assert_eq!(
        pending_binding.current_session().state,
        NativeSessionState::Reconciling
    );
    assert_eq!(pending_binding.current_session().generation.get(), 2);
    assert_eq!(
        pending_binding.current_session().interruption_revision,
        interruption
    );
    guardian.assert_original_parent().unwrap();
    replacement.kill();
    let fenced_again = detached(&mut guardian);
    assert_eq!(fenced_again.state, WireState::DetachedFenced);
    assert_eq!(fenced_again.session.generation, 2);
    assert_eq!(fenced_again.retained, active.retained);
    same_custody(&fenced_again, &active);
    assert_eq!(counts(), [1, 0, 0, 0, 0]);

    let mut final_runner = Runner::start(
        recovery_command(&guardian, "recover-unapplied-sample"),
        &guardian,
        "recover-unapplied-final",
    );
    assert_ne!(final_runner.child.id(), replacement.child.id());
    assert_ne!(final_runner.child.id(), initial_pid);
    final_runner.marker("verified binding: Detached");
    let second_interruption = interruption.checked_next().unwrap();
    assert_eq!(
        final_runner.marker("reconciling generation=3 interruption="),
        format!(
            "reconciling generation=3 interruption={}",
            second_interruption.get()
        )
    );
    assert_eq!(
        final_runner.marker("recovered_sample_applied_ack "),
        format!(
            "recovered_sample_applied_ack attempt=64 revision={} ordinal=0 sequence=0",
            issue_revision.get()
        )
    );
    final_runner.success();
    let final_output = bounded_text(&final_runner.out).unwrap();
    assert_eq!(
        final_runner.consumed,
        final_output.len(),
        "recovery runner must not emit a follow-up effect: {:?}",
        final_output
            .get(final_runner.consumed..)
            .unwrap_or_default()
    );

    assert_eq!(counts(), [1, 1, 1, 0, 0]);
    let applied_revision = second_interruption.checked_next().unwrap();
    assert_eq!(sampling_rows(&path), (1, 1, 1, applied_revision.get(), 0));
    assert_eq!(
        settled_sampling_keys(&path),
        vec![(issue_revision.get(), 0, 0, 1, 3)]
    );
    let application: (Vec<u8>, Vec<u8>, Vec<u8>, i64, String) = raw
        .query_row(
            "SELECT applying_revision, available_memory, available_storage, cpu, pressure FROM native_sampling_applications",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        u64::from_be_bytes(application.0.try_into().unwrap()),
        applied_revision.get()
    );
    assert_eq!(
        u64::from_be_bytes(application.1.try_into().unwrap()),
        wire_sample.memory_bytes
    );
    assert_eq!(
        u64::from_be_bytes(application.2.try_into().unwrap()),
        wire_sample.storage_bytes
    );
    assert_eq!(application.3, i64::from(wire_sample.cpu_basis_points));
    assert_eq!(
        application.4,
        match wire_sample.memory_pressure {
            WireMemoryPressure::Normal => "nominal",
            WireMemoryPressure::Elevated => "warning",
            WireMemoryPressure::Critical => "critical",
        }
    );
    let confirmation: (Vec<u8>, Vec<u8>, i64, Vec<u8>, Vec<u8>) = raw
        .query_row(
            "SELECT attempt_id, dispatch_revision, ordinal, applying_revision, confirming_session_generation FROM native_sampling_ack_confirmations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        u128::from_be_bytes(confirmation.0.try_into().unwrap()),
        attempt.get()
    );
    assert_eq!(
        u64::from_be_bytes(confirmation.1.try_into().unwrap()),
        issue_revision.get()
    );
    assert_eq!(confirmation.2, 0);
    assert_eq!(
        u64::from_be_bytes(confirmation.3.try_into().unwrap()),
        applied_revision.get()
    );
    assert_eq!(u64::from_be_bytes(confirmation.4.try_into().unwrap()), 3);
    assert!(observed
        .pending_native_sampling_ack(attempt)
        .unwrap()
        .is_none());
    let final_attempt = observed.load_attempt(attempt).unwrap().unwrap();
    assert_eq!(final_attempt.phase(), Phase::Interrupted);
    assert_eq!(final_attempt.persistence_revision(), applied_revision);
    assert_eq!(final_attempt.deadline(), deadline);
    let after_binding = observed.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(after_binding.identity(), binding.identity());
    assert_eq!(after_binding.slots(), binding.slots());
    assert_eq!(after_binding.deadline(), deadline);
    assert_eq!(
        after_binding.current_session().state,
        NativeSessionState::Reconciling
    );
    assert_eq!(after_binding.current_session().generation.get(), 3);
    assert_eq!(
        after_binding.current_session().interruption_revision,
        second_interruption
    );
    let final_snapshot = detached(&mut guardian);
    assert_eq!(final_snapshot.state, WireState::DetachedFenced);
    assert_eq!(final_snapshot.session.generation, 3);
    assert!(final_snapshot.retained.is_empty());
    same_custody(&final_snapshot, &active);
    assert_eq!(counts(), [1, 1, 1, 0, 0]);
    guardian.assert_original_parent().unwrap();
    assert!(MonotonicTick::native_now().unwrap() < deadline);
    drop((initial, replacement, final_runner, observed, raw));
    guardian.finish().unwrap();
}

#[allow(clippy::too_many_lines)]
fn launchd_journey(continuation: bool) {
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
        7
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

    let settled_keys = settled_sampling_keys(&path);
    assert_eq!(settled_keys.len(), 1);
    let command = if continuation {
        let mut command = recovery_command(&guardian, "recover-finish-and-sample");
        command.arg("--fixture-activation-and-sample-barriers");
        command
    } else {
        let mut command = recovery_command(&guardian, "recover-and-finish");
        command.arg("--fixture-activation-barriers");
        command
    };
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
    let expected_final_rows;
    let expected_final_revision;
    if continuation {
        finish.marker("post_activation_sample_pending");
        assert_eq!(sampling_rows(&path), settled);
        assert_eq!(settled_sampling_keys(&path), settled_keys);
        assert_eq!(
            observed.load_native_binding(attempt).unwrap().unwrap(),
            active_binding
        );
        guardian.assert_original_parent().unwrap();
        finish.release();
        assert_eq!(
            finish.marker("recovered_sample_applied_ack "),
            format!(
                "recovered_sample_applied_ack attempt=64 revision={} ordinal=0 sequence=1",
                activation.completion().recovery_revision().get()
            )
        );
        finish.success();
        assert!(
            finish
                .take_line(&bounded_text(&finish.out).unwrap())
                .is_none(),
            "no follow-up runner effect"
        );
        let continued = sampling_rows(&path);
        assert_eq!(
            (continued.0, continued.1, continued.2, continued.4),
            (2, 2, 2, 1)
        );
        assert_eq!(
            continued.3,
            activation
                .completion()
                .recovery_revision()
                .checked_next()
                .unwrap()
                .get()
        );
        let mut expected_keys = settled_keys;
        expected_keys.push((
            activation.completion().recovery_revision().get(),
            0,
            1,
            3,
            3,
        ));
        assert_eq!(settled_sampling_keys(&path), expected_keys);
        expected_final_rows = continued;
        expected_final_revision = activation
            .completion()
            .recovery_revision()
            .checked_next()
            .unwrap();
    } else {
        finish.success();
        assert!(
            finish
                .take_line(&bounded_text(&finish.out).unwrap())
                .is_none(),
            "activation-only runner must never dispatch its retained ticket"
        );
        assert_eq!(sampling_rows(&path), settled);
        assert_eq!((settled.0, settled.1, settled.2, settled.4), (1, 1, 1, 0));
        assert_eq!(settled_sampling_keys(&path), settled_keys);
        expected_final_rows = settled;
        expected_final_revision = activation.completion().recovery_revision();
    }
    assert!(observed
        .pending_native_sampling_ack(attempt)
        .unwrap()
        .is_none());
    let final_snapshot = detached(&mut guardian);
    assert!(
        final_snapshot.retained.is_empty(),
        "no extra retained native effect"
    );
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
    let final_attempt = writable.load_attempt(attempt).unwrap().unwrap();
    assert_eq!(
        final_attempt.persistence_revision(),
        expected_final_revision
    );
    if continuation {
        assert!(matches!(
            final_attempt.phase(),
            Phase::OwnershipDiscovery | Phase::ResourceBlocked
        ));
    } else {
        assert_eq!(final_attempt.phase(), Phase::ResourceSampling);
    }
    assert_eq!(
        sampling_rows(&path),
        expected_final_rows,
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
