#![cfg(target_os = "macos")]
#[path = "support/native_sampling.rs"]
mod native_sampling;
use native_sampling::{sampling_rows, sampling_runner_command, settled_sampling_keys};
use phoenix_deployment_controller::{
    guardian_ipc::{WireSnapshot, WireState},
    native_guardian::GuardianClient,
    AttemptId, BirthToken, ControllerOs, ControllerSqliteStore, CpuContentionBasisPoints,
    DeploymentAttempt, IdentityObservation, MacOsControllerOs, MemoryPressure, MonotonicTick,
    NativeSessionState, Phase, ProcessId, ProcessIdentity, ResourcePolicy,
};
use std::num::{NonZeroU32, NonZeroUsize};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct GuardedGuardian {
    child: Child,
    armed: bool,
}

impl GuardedGuardian {
    fn new(child: Child) -> Self {
        Self { child, armed: true }
    }

    fn kill_owned_children(&self) {
        let Ok(output) = Command::new("/usr/bin/pgrep")
            .args(["-P", &self.child.id().to_string()])
            .output()
        else {
            return;
        };
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Ok(pid) = line.trim().parse::<i32>() {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
    }

    fn finish(&mut self) {
        self.kill_owned_children();
        self.child.kill().ok();
        self.child.wait().ok();
        self.armed = false;
    }
}

impl std::ops::Deref for GuardedGuardian {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl std::ops::DerefMut for GuardedGuardian {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

impl Drop for GuardedGuardian {
    fn drop(&mut self) {
        if self.armed {
            self.finish();
        }
    }
}

struct GuardedRunner {
    child: Child,
    armed: bool,
}

impl GuardedRunner {
    fn new(child: Child) -> Self {
        Self { child, armed: true }
    }

    fn finish(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
        self.armed = false;
    }
}

impl std::ops::Deref for GuardedRunner {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl std::ops::DerefMut for GuardedRunner {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

impl Drop for GuardedRunner {
    fn drop(&mut self) {
        if self.armed {
            self.finish();
        }
    }
}

fn wait_process_gone(pid: i32, timeout: Duration) {
    let until = Instant::now() + timeout;
    loop {
        let result = unsafe { libc::kill(pid, 0) };
        if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return;
        }
        assert!(Instant::now() < until, "process {pid} was not reaped");
        // test-timing-allow: bounded polling observes kernel process removal during teardown.
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_guardian_is_parent(child_pid: i32, guardian_pid: u32) {
    let parent_pid = Command::new("/bin/ps")
        .args(["-o", "ppid=", "-p", &child_pid.to_string()])
        .output()
        .unwrap();
    assert_eq!(parent_pid.status.code(), Some(0));
    assert_eq!(
        parent_pid
            .stdout
            .iter()
            .copied()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect::<Vec<_>>(),
        guardian_pid.to_string().into_bytes(),
        "guardian retains original parenthood"
    );
}

struct NativeProcessFixture {
    _parent: tempfile::TempDir,
    dir: PathBuf,
    path: PathBuf,
    attempt: AttemptId,
    guardian: Child,
    child_pid: i32,
    child_birth: BirthToken,
    runner: Child,
}

fn runner_command(dir: &Path, path: &Path, observe_seconds: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"));
    command.args([
        "--mode",
        "observe-only",
        "--socket-dir",
        dir.to_str().unwrap(),
        "--store",
        path.to_str().unwrap(),
        "--attempt",
        "64",
        "--observe-seconds",
        observe_seconds,
    ]);
    command
}

fn process_name(pid: i32) -> String {
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .unwrap();
    Path::new(String::from_utf8(output.stdout).unwrap().trim())
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn wait_for_process_name(pid: i32, expected: &str) {
    let until = Instant::now() + Duration::from_secs(2);
    while process_name(pid) != expected {
        assert!(Instant::now() < until, "child did not exec {expected}");
        // test-timing-allow: bounded polling observes the gated exec transition.
        thread::sleep(Duration::from_millis(10));
    }
}

fn start_fixture() -> NativeProcessFixture {
    let parent = tempfile::Builder::new()
        .prefix("sdc-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let dir = parent.path().join("sdc-disposable-process");
    let path = parent.path().join("controller.sqlite");
    let attempt = AttemptId::new(64);
    let deadline = MonotonicTick::native_now()
        .unwrap()
        .checked_add(Duration::from_secs(60))
        .unwrap();
    let mut store = ControllerSqliteStore::open(&path).unwrap();
    let policy = ResourcePolicy::new(
        NonZeroUsize::new(2).unwrap(),
        0,
        0,
        CpuContentionBasisPoints::new(0).unwrap(),
        MemoryPressure::Critical,
    );
    let root = ProcessIdentity::new(
        ProcessId::new(NonZeroU32::new(42).unwrap()),
        BirthToken::new(1, 0).unwrap(),
    );
    DeploymentAttempt::start(attempt, deadline, policy, root)
        .persist(&mut store)
        .unwrap();
    drop(store);
    let deadline_arg = deadline.get().to_string();
    let mut guardian = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-guardian"))
        .args([
            "--socket-dir",
            dir.to_str().unwrap(),
            "--child-seconds",
            "15",
            "--serve-seconds",
            "16",
            "--attempt",
            "64",
            "--deadline-nanoseconds",
            &deadline_arg,
            "--store",
            path.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let line = BufReader::new(guardian.stdout.take().unwrap())
        .lines()
        .next()
        .unwrap()
        .unwrap();
    let child_pid = line
        .strip_prefix("disposable child pid=")
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let birthday: Vec<_> = line.split("birth=").nth(1).unwrap().split(':').collect();
    let child_birth = BirthToken::new(
        birthday[0].parse::<u64>().unwrap(),
        birthday[1].parse::<u32>().unwrap(),
    )
    .unwrap();
    assert_eq!(process_name(child_pid), "sh");
    let mut runner = runner_command(&dir, &path, "5")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut runner_output = BufReader::new(runner.stdout.take().unwrap());
    let mut ready = String::new();
    runner_output.read_line(&mut ready).unwrap();
    assert_eq!(ready.trim(), "verified binding: InitialActive");
    wait_for_process_name(child_pid, "sleep");
    NativeProcessFixture {
        _parent: parent,
        dir,
        path,
        attempt,
        guardian,
        child_pid,
        child_birth,
        runner,
    }
}

fn wait_for_detached(dir: &Path) -> WireSnapshot {
    let reconnect_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(mut replacement) = GuardianClient::connect(dir) {
            if let Ok(snapshot) = replacement.hello() {
                if snapshot.state == WireState::DetachedFenced {
                    return snapshot;
                }
            }
        }
        assert!(
            Instant::now() < reconnect_deadline,
            "guardian did not fence"
        );
        // test-timing-allow: bounded polling observes the protocol state transition.
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn runner_death_does_not_kill_guardian_or_its_original_child() {
    let NativeProcessFixture {
        _parent,
        dir,
        path,
        attempt,
        mut guardian,
        child_pid,
        child_birth,
        mut runner,
    } = start_fixture();
    let mut store = ControllerSqliteStore::open_read_only(&path).unwrap();
    let binding = store.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(binding.identity().process.pid().get(), guardian.id());
    let mut observer = MacOsControllerOs::new(&dir).unwrap();
    assert!(
        matches!(
            observer.observe_identity(binding.identity().process),
            IdentityObservation::SameIdentity { .. }
        ),
        "guardian PID and birth token must match OS observation"
    );
    assert_eq!(binding.slots().len(), 1);
    assert_eq!(
        binding.slots()[0].process.pid().get(),
        u32::try_from(child_pid).unwrap()
    );
    assert_eq!(binding.slots()[0].process.birth(), child_birth);
    runner.kill().unwrap();
    runner.wait().unwrap();
    let snapshot = wait_for_detached(&dir);
    assert_eq!(snapshot.slots.len(), 1);
    assert_eq!(
        snapshot.native_binding.as_ref().unwrap().nonce,
        binding.identity().nonce.bytes()
    );
    assert!(guardian.try_wait().unwrap().is_none());
    assert_eq!(
        unsafe { libc::kill(child_pid, 0) },
        0,
        "child survived runner death"
    );
    assert_guardian_is_parent(child_pid, guardian.id());
    drop(store);
    let runner_status = runner_command(&dir, &path, "1").output().unwrap();
    let mut store = ControllerSqliteStore::open_read_only(&path).unwrap();
    assert!(
        runner_status.status.success(),
        "{}",
        String::from_utf8_lossy(&runner_status.stderr)
    );
    let output = String::from_utf8_lossy(&runner_status.stdout);
    assert!(
        output.contains("reconciling generation=2 interruption=2"),
        "{output}"
    );
    assert_eq!(
        store
            .load_native_binding(attempt)
            .unwrap()
            .unwrap()
            .current_session()
            .state,
        NativeSessionState::Reconciling
    );
    let durable = store.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(durable.current_session().generation.get(), 2);
    assert_eq!(
        durable.dispatch_floor(),
        store
            .load_attempt(attempt)
            .unwrap()
            .unwrap()
            .persistence_revision()
    );
    assert!(guardian.try_wait().unwrap().is_none());
    assert_eq!(unsafe { libc::kill(child_pid, 0) }, 0);
    assert_guardian_is_parent(child_pid, guardian.id());
    assert!(guardian.wait().unwrap().success());
}

#[allow(clippy::too_many_lines)]
fn sample_death_and_recover(lost_response: bool, finish_reconciliation: bool) {
    use std::{io::Read, net::Shutdown, os::unix::net::UnixStream};
    let parent = tempfile::Builder::new()
        .prefix("sdc-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let dir = parent.path().join("sdc-disposable-sampling-process");
    let path = parent.path().join("controller.sqlite");
    let attempt = AttemptId::new(64);
    let deadline = MonotonicTick::native_now()
        .unwrap()
        .checked_add(Duration::from_secs(60))
        .unwrap();
    drop(ControllerSqliteStore::open(&path).unwrap());
    let mut command = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-guardian"));
    let deadline_arg = deadline.get().to_string();
    command.args([
        "--socket-dir",
        dir.to_str().unwrap(),
        "--child-seconds",
        "25",
        "--serve-seconds",
        "30",
        "--attempt",
        "64",
        "--deadline-nanoseconds",
        &deadline_arg,
        "--store",
        path.to_str().unwrap(),
    ]);
    if lost_response {
        command.arg("--fixture-ack-response-barrier");
    }
    let mut guardian = GuardedGuardian::new(command.stdout(Stdio::piped()).spawn().unwrap());
    let line = BufReader::new(guardian.stdout.take().unwrap())
        .lines()
        .next()
        .unwrap()
        .unwrap();
    let child_pid: i32 = line
        .strip_prefix("disposable child pid=")
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let birth: Vec<_> = line.split("birth=").nth(1).unwrap().split(':').collect();
    let child_birth =
        BirthToken::new(birth[0].parse().unwrap(), birth[1].parse().unwrap()).unwrap();
    let mut control = lost_response.then(|| {
        let stream = UnixStream::connect(dir.join("ack-response-barrier.sock")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
    });
    let mut runner = GuardedRunner::new(
        sampling_runner_command(&dir, &path, deadline, !lost_response)
            .stdin(if lost_response {
                Stdio::null()
            } else {
                Stdio::piped()
            })
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    if let Some(stream) = control.as_mut() {
        let mut marker = [0];
        stream.read_exact(&mut marker).unwrap();
        assert_eq!(marker, [b'A'], "guardian stored receipt before responding");
    } else {
        let mut output = BufReader::new(runner.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "verified binding: InitialActive");
        line.clear();
        output.read_line(&mut line).unwrap();
        assert!(
            line.starts_with("application_committed_pending_ack revision="),
            "{line}"
        );
    }
    let mut observed = ControllerSqliteStore::open_read_only(&path).unwrap();
    let binding = observed.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(binding.identity().process.pid().get(), guardian.id());
    assert_eq!(
        binding.slots()[0].process.pid().get(),
        u32::try_from(child_pid).expect("child PID is positive")
    );
    assert_eq!(binding.slots()[0].process.birth(), child_birth);
    assert_eq!(binding.deadline(), deadline);
    let mut os = MacOsControllerOs::new(&dir).unwrap();
    assert!(matches!(
        os.observe_identity(binding.identity().process),
        IdentityObservation::SameIdentity { .. }
    ));
    let (key, pending) = observed
        .pending_native_sampling_ack(attempt)
        .unwrap()
        .unwrap();
    let phoenix_deployment_controller::NativeSamplingStatus::AppliedPendingAck {
        sample,
        applying_revision,
        ..
    } = pending
    else {
        panic!("expected successful pending sample");
    };
    let before = sampling_rows(&path);
    assert_eq!(before.0, 1);
    assert_eq!(before.1, 1);
    assert_eq!(before.2, 0);
    assert_eq!(sample.sequence, 0);
    assert_eq!(before.4, 0);
    drop(observed);
    runner.finish();
    if let Some(mut stream) = control {
        use std::io::Write;
        stream.write_all(b"R").unwrap();
        stream.shutdown(Shutdown::Both).unwrap();
    }
    let detached = wait_for_detached(&dir);
    assert_eq!(
        detached.native_binding.as_ref().unwrap().nonce,
        binding.identity().nonce.bytes()
    );
    assert_eq!(detached.deadline, deadline.get());
    assert_eq!(
        detached.native_binding.as_ref().unwrap().slots[0].identity,
        binding.slots()[0].process.into()
    );
    assert!(guardian.try_wait().unwrap().is_none());
    assert_eq!(unsafe { libc::kill(child_pid, 0) }, 0);
    assert_guardian_is_parent(child_pid, guardian.id());
    let replacement = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"))
        .args([
            "--mode",
            "recover-applied-ack",
            "--socket-dir",
            dir.to_str().unwrap(),
            "--store",
            path.to_str().unwrap(),
            "--attempt",
            "64",
        ])
        .output()
        .unwrap();
    assert!(
        replacement.status.success(),
        "{}",
        String::from_utf8_lossy(&replacement.stderr)
    );
    let output = String::from_utf8_lossy(&replacement.stdout);
    assert!(
        output.contains("reconciling generation=2 interruption="),
        "{output}"
    );
    assert!(
        output.contains("sample_ack_confirmed generation=2"),
        "{output}"
    );
    let after = sampling_rows(&path);
    assert_eq!(after, (1, 1, 1, before.3, before.4));
    let mut observed = ControllerSqliteStore::open_read_only(&path).unwrap();
    assert_eq!(
        observed
            .load_native_binding(attempt)
            .unwrap()
            .unwrap()
            .current_session()
            .state,
        NativeSessionState::Reconciling
    );
    assert!(matches!(observed.load_native_sampling_status(key).unwrap(),
        Some(phoenix_deployment_controller::NativeSamplingStatus::SampleAckConfirmed {
            sample: confirmed, applying_revision: revision, confirming_session_generation, ..
        }) if confirmed == sample && revision == applying_revision && confirming_session_generation.get() == 2));
    assert!(guardian.try_wait().unwrap().is_none());
    assert_guardian_is_parent(child_pid, guardian.id());
    if finish_reconciliation {
        let before_finish = observed.load_native_binding(attempt).unwrap().unwrap();
        let mut pending = GuardedRunner::new(
            Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"))
                .args([
                    "--mode",
                    "recover-and-finish",
                    "--socket-dir",
                    dir.to_str().unwrap(),
                    "--store",
                    path.to_str().unwrap(),
                    "--attempt",
                    "64",
                    "--fixture-pause-before-begin",
                ])
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut pending_output = BufReader::new(pending.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(
                pending_output.read_line(&mut line).unwrap(),
                0,
                "pending runner exited before its barrier"
            );
            if line.starts_with("pending_begin generation=3 interruption=") {
                break;
            }
        }
        let pending_binding = observed.load_native_binding(attempt).unwrap().unwrap();
        assert_eq!(pending_binding.current_session().generation.get(), 3);
        assert_eq!(
            pending_binding.current_session().state,
            NativeSessionState::Reconciling
        );
        assert!(
            pending_binding.current_session().interruption_revision
                > before_finish.current_session().interruption_revision
        );
        assert_eq!(pending_binding.identity(), binding.identity());
        assert_eq!(pending_binding.deadline(), deadline);
        assert_eq!(sampling_rows(&path), after);
        assert!(observed
            .read_native_reconciliation_evidence(
                attempt,
                pending_binding.current_session().generation
            )
            .unwrap()
            .is_none());
        assert!(observed
            .load_native_reconciliation_activation(
                attempt,
                pending_binding.current_session().generation
            )
            .unwrap()
            .is_none());
        pending.kill().unwrap();
        assert!(!pending.wait().unwrap().success());
        let pending_detached = wait_for_detached(&dir);
        assert_eq!(
            pending_detached.session.generation, 2,
            "guardian never accepted pending generation"
        );
        assert_eq!(pending_detached.instance, detached.instance);
        assert_eq!(pending_detached.deadline, deadline.get());
        assert_eq!(pending_detached.slots, detached.slots);
        assert!(guardian.try_wait().unwrap().is_none());
        assert_eq!(unsafe { libc::kill(child_pid, 0) }, 0);
        assert_guardian_is_parent(child_pid, guardian.id());
        let mut finisher = GuardedRunner::new(
            Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"))
                .args([
                    "--mode",
                    "recover-and-finish",
                    "--socket-dir",
                    dir.to_str().unwrap(),
                    "--store",
                    path.to_str().unwrap(),
                    "--attempt",
                    "64",
                    "--fixture-activation-barriers",
                ])
                .stdout(Stdio::piped())
                .stdin(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut output = BufReader::new(finisher.stdout.take().unwrap());
        let mut pending_status_seen = false;
        let mut abandoned_fenced = false;
        let mut prepared_seen = false;
        let mut durable_seen = false;
        let mut marker_seen = false;
        let active = loop {
            let mut line = String::new();
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "finisher exited without Active evidence"
            );
            if line.starts_with("verified binding: PendingBegin") {
                pending_status_seen = true;
            }
            if line.trim() == "pending_begin_fenced generation=3" {
                assert!(pending_status_seen);
                abandoned_fenced = true;
            }
            if let Some(json) = line.strip_prefix("prepared_snapshot=") {
                assert!(abandoned_fenced);
                let prepared: WireSnapshot = serde_json::from_str(json.trim()).unwrap();
                assert_eq!(prepared.state, WireState::Reconciling);
                assert_eq!(prepared.session.generation, 4);
                assert_eq!(prepared.instance, detached.instance);
                assert_eq!(prepared.deadline, deadline.get());
                assert_eq!(prepared.slots, detached.slots);
                let binding = observed.load_native_binding(attempt).unwrap().unwrap();
                assert_eq!(
                    binding.current_session().state,
                    NativeSessionState::Reconciling
                );
                assert!(observed
                    .read_native_reconciliation_evidence(
                        attempt,
                        binding.current_session().generation
                    )
                    .unwrap()
                    .is_some());
                assert!(observed
                    .load_native_reconciliation_activation(
                        attempt,
                        binding.current_session().generation
                    )
                    .unwrap()
                    .is_none());
                assert_eq!(sampling_rows(&path), after);
                assert_guardian_is_parent(child_pid, guardian.id());
                prepared_seen = true;
                finisher.stdin.as_mut().unwrap().write_all(b"!").unwrap();
            }
            if let Some(json) = line.strip_prefix("durable_activation_snapshot=") {
                assert!(prepared_seen);
                let durable: WireSnapshot = serde_json::from_str(json.trim()).unwrap();
                assert_eq!(durable.state, WireState::Reconciling);
                assert_eq!(durable.session.generation, 4);
                assert_eq!(durable.instance, detached.instance);
                assert_eq!(durable.deadline, deadline.get());
                assert_eq!(durable.slots, detached.slots);
                let binding = observed.load_native_binding(attempt).unwrap().unwrap();
                assert_eq!(binding.current_session().state, NativeSessionState::Active);
                let activation = observed
                    .load_native_reconciliation_activation(
                        attempt,
                        binding.current_session().generation,
                    )
                    .unwrap()
                    .unwrap();
                assert_eq!(activation.binding_revision(), binding.revision());
                assert_eq!(
                    binding.dispatch_floor(),
                    activation.completion().recovery_revision()
                );
                assert_eq!(sampling_rows(&path), after);
                assert_guardian_is_parent(child_pid, guardian.id());
                durable_seen = true;
                finisher.stdin.as_mut().unwrap().write_all(b"!").unwrap();
            }
            if line.starts_with("reconciliation_active generation=4 interruption=") {
                assert!(
                    abandoned_fenced && prepared_seen && durable_seen,
                    "catch-up, prepare, and durable activation must precede live Active"
                );
                let activation = observed
                    .load_native_reconciliation_activation(
                        attempt,
                        std::num::NonZeroU64::new(4).unwrap(),
                    )
                    .unwrap()
                    .expect("Active marker follows durable activation");
                assert_eq!(activation.completion().session_generation().get(), 4);
                assert_eq!(
                    observed
                        .load_native_binding(attempt)
                        .unwrap()
                        .unwrap()
                        .current_session()
                        .state,
                    NativeSessionState::Active
                );
                marker_seen = true;
            }
            if let Some(json) = line.strip_prefix("active_snapshot=") {
                assert!(marker_seen, "Active snapshot follows the finish marker");
                break serde_json::from_str::<WireSnapshot>(json.trim()).unwrap();
            }
        };
        assert_eq!(active.state, WireState::Active);
        assert_eq!(active.session.generation, 4);
        assert!(active.session.generation > pending_binding.current_session().generation.get());
        assert_eq!(active.instance, detached.instance);
        assert_eq!(active.deadline, deadline.get());
        assert_eq!(active.slots, detached.slots);
        let native = active.native_binding.as_ref().unwrap();
        assert_eq!(native.nonce, binding.identity().nonce.bytes());
        assert_eq!(native.guardian, binding.identity().process.into());
        assert_eq!(native.slots[0].identity, binding.slots()[0].process.into());
        assert!(active.retained.is_empty());
        assert!(finisher.wait().unwrap().success());
        let activation = observed
            .load_native_reconciliation_activation(attempt, std::num::NonZeroU64::new(4).unwrap())
            .unwrap()
            .unwrap();
        let completion = activation.completion();
        let checkpoint = observed.load_attempt(attempt).unwrap().unwrap();
        let completed_binding = observed.load_native_binding(attempt).unwrap().unwrap();
        assert_eq!(
            completed_binding.current_session().state,
            NativeSessionState::Active
        );
        assert_eq!(completed_binding.revision(), activation.binding_revision());
        assert_eq!(
            completed_binding.dispatch_floor(),
            completion.recovery_revision()
        );
        assert_eq!(
            completed_binding.sessions()[2].state,
            NativeSessionState::Detached
        );
        assert_eq!(
            completed_binding.sessions()[2].interruption_revision,
            pending_binding.current_session().interruption_revision
        );
        assert_eq!(
            completed_binding.sessions()[3].state,
            NativeSessionState::Active
        );
        assert!(
            completion.interruption_revision()
                > before_finish.current_session().interruption_revision
        );
        assert!(completion.recovery_revision() > completion.interruption_revision());
        assert_eq!(
            checkpoint.persistence_revision(),
            completion.recovery_revision()
        );
        assert_eq!(completed_binding.identity(), binding.identity());
        assert_eq!(sampling_rows(&path), after);
        let raw = rusqlite::Connection::open(&path).unwrap();
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
        let final_detached = wait_for_detached(&dir);
        assert_eq!(final_detached.state, WireState::DetachedFenced);
        assert_eq!(final_detached.session, active.session);
        assert_eq!(final_detached.instance, active.instance);
        assert_eq!(final_detached.deadline, active.deadline);
        let mut writable = ControllerSqliteStore::open_existing(&path).unwrap();
        assert_eq!(
            phoenix_deployment_controller::native_guardian::verify_native_binding(
                &mut writable,
                attempt,
                &final_detached
            )
            .unwrap(),
            phoenix_deployment_controller::native_guardian::VerifiedBindingStatus::Detached
        );
        let detached_binding = writable.load_native_binding(attempt).unwrap().unwrap();
        assert_eq!(
            detached_binding.current_session().state,
            NativeSessionState::Detached
        );
        assert_eq!(
            detached_binding.current_session().generation,
            completed_binding.current_session().generation
        );
        assert_eq!(
            detached_binding.revision(),
            completed_binding.revision().checked_next().unwrap()
        );
        assert_eq!(
            detached_binding.dispatch_floor(),
            completion.recovery_revision()
        );
        assert_eq!(sampling_rows(&path), after);
        assert!(guardian.try_wait().unwrap().is_none());
        assert_eq!(unsafe { libc::kill(child_pid, 0) }, 0);
        assert_guardian_is_parent(child_pid, guardian.id());
    }
    guardian.finish();
    wait_process_gone(child_pid, Duration::from_secs(5));
}

#[test]
fn application_commit_survives_runner_death_before_ack() {
    sample_death_and_recover(false, false);
}

#[test]
fn guardian_ack_receipt_survives_lost_response_and_runner_death() {
    sample_death_and_recover(true, false);
}

#[test]
fn fresh_runner_finishes_reconciliation_after_death_and_ack_recovery() {
    sample_death_and_recover(false, true);
}

fn line(output: &mut BufReader<std::process::ChildStdout>) -> String {
    if output.buffer().is_empty() {
        let mut fd = libc::pollfd {
            fd: std::os::fd::AsRawFd::as_raw_fd(output.get_ref()),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(
            unsafe { libc::poll(&raw mut fd, 1, 5000) },
            1,
            "runner marker timeout"
        );
    }
    let mut line = String::new();
    assert_ne!(output.read_line(&mut line).unwrap(), 0, "runner marker EOF");
    line.trim_end().to_owned()
}
fn success(child: &mut Child) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "runner failed: {status}");
            return;
        }
        assert!(Instant::now() < until, "runner exit timeout");
        // test-timing-allow: bounded polling observes disposable runner exit.
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
#[allow(clippy::too_many_lines)]
fn fresh_runner_continues_exactly_one_retained_sample_after_two_phase_activation() {
    let parent = tempfile::Builder::new()
        .prefix("sdc-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let dir = parent.path().join("sdc-disposable-continue-sample");
    let path = parent.path().join("controller.sqlite");
    let attempt = AttemptId::new(64);
    let deadline = MonotonicTick::native_now()
        .unwrap()
        .checked_add(Duration::from_secs(60))
        .unwrap();
    drop(ControllerSqliteStore::open(&path).unwrap());
    let mut command = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-guardian"));
    let deadline_arg = deadline.get().to_string();
    command.args([
        "--socket-dir",
        dir.to_str().unwrap(),
        "--child-seconds",
        "25",
        "--serve-seconds",
        "30",
        "--attempt",
        "64",
        "--deadline-nanoseconds",
        &deadline_arg,
        "--store",
        path.to_str().unwrap(),
    ]);
    let mut guardian = GuardedGuardian::new(command.stdout(Stdio::piped()).spawn().unwrap());
    let mut guardian_output = BufReader::new(guardian.stdout.take().unwrap());
    assert!(line(&mut guardian_output).starts_with("disposable child pid="));
    let mut initial = GuardedRunner::new(
        sampling_runner_command(&dir, &path, deadline, true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut initial_output = BufReader::new(initial.stdout.take().unwrap());
    assert_eq!(line(&mut initial_output), "verified binding: InitialActive");
    assert!(line(&mut initial_output).starts_with("application_committed_pending_ack revision="));
    assert_eq!(
        (
            sampling_rows(&path).0,
            sampling_rows(&path).1,
            sampling_rows(&path).2
        ),
        (1, 1, 0)
    );
    initial.finish();
    let original = wait_for_detached(&dir);
    assert_eq!(original.retained.len(), 1);
    let mut observed = ControllerSqliteStore::open_read_only(&path).unwrap();
    let binding = observed.load_native_binding(attempt).unwrap().unwrap();
    let child = binding.slots()[0].process;
    let child_pid = i32::try_from(child.pid().get()).unwrap();
    assert_eq!(binding.identity().process.pid().get(), guardian.id());
    assert_eq!(binding.deadline(), deadline);
    assert_eq!(
        original.native_binding.as_ref().unwrap().nonce,
        binding.identity().nonce.bytes()
    );
    assert_eq!(
        original.native_binding.as_ref().unwrap().guardian,
        binding.identity().process.into()
    );
    assert_eq!(
        original.native_binding.as_ref().unwrap().slots[0].identity,
        child.into()
    );
    let mut ack = GuardedRunner::new(
        Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"))
            .args([
                "--mode",
                "recover-applied-ack",
                "--socket-dir",
                dir.to_str().unwrap(),
                "--store",
                path.to_str().unwrap(),
                "--attempt",
                "64",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut ack_output = BufReader::new(ack.stdout.take().unwrap());
    assert!(line(&mut ack_output).starts_with("verified binding: Detached"));
    assert!(line(&mut ack_output).starts_with("reconciling generation=2 interruption="));
    assert_eq!(line(&mut ack_output), "sample_ack_confirmed generation=2");
    success(&mut ack);
    let after_ack = wait_for_detached(&dir);
    assert_eq!(after_ack.instance, original.instance);
    assert_eq!(after_ack.deadline, original.deadline);
    assert_eq!(after_ack.slots, original.slots);
    assert_eq!(after_ack.native_binding, original.native_binding);
    assert!(after_ack.retained.is_empty());
    let original = after_ack;
    let settled = sampling_rows(&path);
    assert_eq!((settled.0, settled.1, settled.2, settled.4), (1, 1, 1, 0));
    let settled_keys = settled_sampling_keys(&path);
    assert_eq!(settled_keys.len(), 1);
    let same_custody = |snapshot: &WireSnapshot| {
        assert_eq!(snapshot.instance, original.instance);
        assert_eq!(snapshot.attempt, original.attempt);
        assert_eq!(snapshot.deadline, original.deadline);
        assert_eq!(snapshot.slots, original.slots);
        assert_eq!(snapshot.native_binding, original.native_binding);
        assert!(snapshot.retained.is_empty());
        assert_guardian_is_parent(child_pid, guardian.id());
        let mut os = MacOsControllerOs::new(&dir).unwrap();
        assert!(matches!(
            os.observe_identity(binding.identity().process),
            IdentityObservation::SameIdentity { .. }
        ));
        assert!(matches!(
            os.observe_identity(child),
            IdentityObservation::SameIdentity { .. }
        ));
    };
    let mut runner = GuardedRunner::new(
        Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"))
            .args([
                "--mode",
                "recover-finish-and-sample",
                "--socket-dir",
                dir.to_str().unwrap(),
                "--store",
                path.to_str().unwrap(),
                "--attempt",
                "64",
                "--fixture-activation-and-sample-barriers",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    assert_ne!(runner.id(), initial.id());
    let mut output = BufReader::new(runner.stdout.take().unwrap());
    assert!(line(&mut output).starts_with("verified binding: Detached"));
    assert!(line(&mut output).starts_with("reconciling generation=3 interruption="));
    let prepared_line = line(&mut output);
    let prepared: WireSnapshot =
        serde_json::from_str(prepared_line.strip_prefix("prepared_snapshot=").unwrap()).unwrap();
    assert_eq!(prepared.state, WireState::Reconciling);
    assert_eq!(prepared.session.generation, 3);
    same_custody(&prepared);
    let generation = std::num::NonZeroU64::new(3).unwrap();
    let preparing_binding = observed.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(
        preparing_binding.current_session().state,
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
    runner.stdin.as_mut().unwrap().write_all(b"!").unwrap();
    let durable_line = line(&mut output);
    let durable: WireSnapshot = serde_json::from_str(
        durable_line
            .strip_prefix("durable_activation_snapshot=")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(durable.state, WireState::Reconciling);
    assert_eq!(durable.session, prepared.session);
    same_custody(&durable);
    let active_binding = observed.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(
        active_binding.current_session().state,
        NativeSessionState::Active
    );
    let activation = observed
        .load_native_reconciliation_activation(attempt, generation)
        .unwrap()
        .unwrap();
    let recovery = activation.completion().recovery_revision();
    assert_eq!(activation.binding_revision(), active_binding.revision());
    assert_eq!(active_binding.dispatch_floor(), recovery);
    assert_eq!(sampling_rows(&path), settled);
    runner.stdin.as_mut().unwrap().write_all(b"!").unwrap();
    assert!(line(&mut output).starts_with("reconciliation_active generation=3 interruption="));
    let active_line = line(&mut output);
    let active: WireSnapshot =
        serde_json::from_str(active_line.strip_prefix("active_snapshot=").unwrap()).unwrap();
    assert_eq!(active.state, WireState::Active);
    assert_eq!(active.session, prepared.session);
    same_custody(&active);
    assert_eq!(line(&mut output), "post_activation_sample_pending");
    assert_eq!(
        observed.load_native_binding(attempt).unwrap().unwrap(),
        active_binding
    );
    assert_eq!(sampling_rows(&path), settled);
    assert_eq!(settled_sampling_keys(&path), settled_keys);
    runner.stdin.as_mut().unwrap().write_all(b"!").unwrap();
    assert_eq!(
        line(&mut output),
        format!(
            "recovered_sample_applied_ack attempt=64 revision={} ordinal=0 sequence=1",
            recovery.get()
        )
    );
    success(&mut runner);
    let mut remaining = String::new();
    std::io::Read::read_to_string(&mut output, &mut remaining).unwrap();
    assert!(
        remaining.is_empty(),
        "no follow-up runner effect: {remaining}"
    );
    let continued = sampling_rows(&path);
    assert_eq!(
        (continued.0, continued.1, continued.2, continued.4),
        (2, 2, 2, 1)
    );
    assert_eq!(continued.3, recovery.checked_next().unwrap().get());
    let mut expected_keys = settled_keys;
    expected_keys.push((recovery.get(), 0, 1, 3, 3));
    assert_eq!(settled_sampling_keys(&path), expected_keys);
    assert!(observed
        .pending_native_sampling_ack(attempt)
        .unwrap()
        .is_none());
    let continued_attempt = observed.load_attempt(attempt).unwrap().unwrap();
    assert_eq!(continued_attempt.persistence_revision().get(), continued.3);
    assert_eq!(continued_attempt.phase(), Phase::OwnershipDiscovery);
    let raw =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    for table in [
        "native_reconciliation_completions",
        "native_reconciliation_activations",
    ] {
        assert_eq!(
            raw.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    let final_snapshot = wait_for_detached(&dir);
    assert_eq!(final_snapshot.session, active.session);
    same_custody(&final_snapshot);
    let mut writable = ControllerSqliteStore::open_existing(&path).unwrap();
    assert_eq!(
        phoenix_deployment_controller::native_guardian::verify_native_binding(
            &mut writable,
            attempt,
            &final_snapshot
        )
        .unwrap(),
        phoenix_deployment_controller::native_guardian::VerifiedBindingStatus::Detached
    );
    let detached_binding = writable.load_native_binding(attempt).unwrap().unwrap();
    assert_eq!(detached_binding.identity(), binding.identity());
    assert_eq!(detached_binding.slots(), binding.slots());
    assert_eq!(detached_binding.deadline(), deadline);
    assert_eq!(detached_binding.current_session().generation, generation);
    assert_eq!(
        detached_binding.current_session().state,
        NativeSessionState::Detached
    );
    assert_eq!(detached_binding.dispatch_floor(), recovery);
    assert_eq!(sampling_rows(&path), continued);
    assert!(MonotonicTick::native_now().unwrap() < deadline);
    let guardian_pid = i32::try_from(guardian.id()).unwrap();
    guardian.finish();
    wait_process_gone(child_pid, Duration::from_secs(5));
    wait_process_gone(guardian_pid, Duration::from_secs(5));
}
