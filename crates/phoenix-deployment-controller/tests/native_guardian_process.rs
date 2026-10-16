#![cfg(target_os = "macos")]
use phoenix_deployment_controller::{
    guardian_ipc::WireState, native_guardian::GuardianClient, AttemptId, BirthToken, ControllerOs,
    ControllerSqliteStore, CpuContentionBasisPoints, DeploymentAttempt, IdentityObservation,
    MacOsControllerOs, MemoryPressure, MonotonicTick, NativeSessionState, ProcessId,
    ProcessIdentity, ResourcePolicy,
};
use std::num::{NonZeroU32, NonZeroUsize};
use std::{
    io::{BufRead, BufReader},
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

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

#[test]
fn runner_death_does_not_kill_guardian_or_its_original_child() {
    let parent = tempfile::Builder::new()
        .prefix("sdc-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let dir = parent.path().join("sdc-disposable-process");
    let path = parent.path().join("controller.sqlite");
    let attempt = AttemptId::new(64);
    let deadline = MonotonicTick::new(u64::MAX);
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
            "--deadline",
            "18446744073709551615",
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
    let pid: i32 = line
        .strip_prefix("disposable child pid=")
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let mut runner = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"))
        .args([
            "--socket-dir",
            dir.to_str().unwrap(),
            "--observe-seconds",
            "5",
            "--store",
            path.to_str().unwrap(),
            "--attempt",
            "64",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut runner_output = BufReader::new(runner.stdout.take().unwrap());
    let mut ready = String::new();
    runner_output.read_line(&mut ready).unwrap();
    assert_eq!(ready.trim(), "verified binding: InitialActive");
    let mut store = ControllerSqliteStore::open_existing(&path).unwrap();
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
    assert_eq!(binding.slots()[0].process.pid().get(), pid as u32);
    let birthday: Vec<_> = line.split("birth=").nth(1).unwrap().split(':').collect();
    assert_eq!(
        binding.slots()[0].process.birth().seconds(),
        birthday[0].parse::<u64>().unwrap()
    );
    assert_eq!(
        binding.slots()[0].process.birth().nanos(),
        birthday[1].parse::<u32>().unwrap()
    );
    runner.kill().unwrap();
    runner.wait().unwrap();
    let reconnect_deadline = Instant::now() + Duration::from_secs(5);
    let snapshot = loop {
        if let Ok(mut replacement) = GuardianClient::connect(&dir) {
            if let Ok(snapshot) = replacement.hello() {
                if snapshot.state == WireState::DetachedFenced {
                    break snapshot;
                }
            }
        }
        assert!(
            Instant::now() < reconnect_deadline,
            "guardian did not fence"
        );
        // test-timing-allow: bounded polling observes the protocol state transition.
        thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(snapshot.slots.len(), 1);
    assert_eq!(
        snapshot.native_binding.as_ref().unwrap().nonce,
        binding.identity().nonce.bytes()
    );
    assert!(guardian.try_wait().unwrap().is_none());
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        0,
        "child survived runner death"
    );
    assert_guardian_is_parent(pid, guardian.id());
    let runner_status = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"))
        .args([
            "--socket-dir",
            dir.to_str().unwrap(),
            "--observe-seconds",
            "1",
            "--store",
            path.to_str().unwrap(),
            "--attempt",
            "64",
        ])
        .output()
        .unwrap();
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
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
    assert_guardian_is_parent(pid, guardian.id());
    assert!(guardian.wait().unwrap().success());
}
