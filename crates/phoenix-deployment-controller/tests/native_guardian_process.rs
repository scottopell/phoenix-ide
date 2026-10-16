#![cfg(target_os = "macos")]
use phoenix_deployment_controller::{
    guardian_ipc::{WireSnapshot, WireState},
    native_guardian::GuardianClient,
    AttemptId, BirthToken, ControllerOs, ControllerSqliteStore, CpuContentionBasisPoints,
    DeploymentAttempt, IdentityObservation, MacOsControllerOs, MemoryPressure, MonotonicTick,
    NativeSessionState, ProcessId, ProcessIdentity, ResourcePolicy,
};
use std::num::{NonZeroU32, NonZeroUsize};
use std::{
    io::{BufRead, BufReader},
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

fn sampling_runner_command(
    dir: &Path,
    path: &Path,
    deadline: MonotonicTick,
    pause: bool,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"));
    let deadline_arg = deadline.get().to_string();
    command.args([
        "--mode",
        "initial-sample",
        "--socket-dir",
        dir.to_str().unwrap(),
        "--store",
        path.to_str().unwrap(),
        "--attempt",
        "64",
        "--deadline-nanoseconds",
        &deadline_arg,
        "--root-pid",
        "42",
        "--root-birth-seconds",
        "1",
        "--root-birth-nanos",
        "0",
    ]);
    if pause {
        command.arg("--fixture-pause-after-apply");
    }
    command
}

fn sampling_rows(path: &Path) -> (i64, i64, i64, u64, u64) {
    use rusqlite::Connection;
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let count = |table: &str| -> i64 {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    };
    let (revision, sequence): (Vec<u8>, Vec<u8>) = db.query_row(
        "SELECT applying_revision, i.sequence FROM native_sampling_applications a JOIN native_sampling_issues i USING(attempt_id,dispatch_revision,ordinal)",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    (
        count("native_sampling_issues"),
        count("native_sampling_applications"),
        count("native_sampling_ack_confirmations"),
        u64::from_be_bytes(revision.try_into().unwrap()),
        u64::from_be_bytes(sequence.try_into().unwrap()),
    )
}

fn sample_death_and_recover(lost_response: bool) {
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
    assert_eq!(binding.slots()[0].process.pid().get(), child_pid as u32);
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
    guardian.finish();
    wait_process_gone(child_pid, Duration::from_secs(5));
}

#[test]
fn application_commit_survives_runner_death_before_ack() {
    sample_death_and_recover(false);
}

#[test]
fn guardian_ack_receipt_survives_lost_response_and_runner_death() {
    sample_death_and_recover(true);
}
