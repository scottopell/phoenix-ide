#![cfg(target_os = "macos")]

use std::{
    fs,
    mem::MaybeUninit,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use phoenix_deployment_controller::{
    ControllerOs, IdentityObservation, MacOsControllerOs, ResourceObservationFailure, Signal,
    SignalOutcome, SupervisedChild, SupervisionRisk,
};

#[test]
fn supervised_child_has_stable_identity_and_bounded_group_cleanup() {
    let mut os = MacOsControllerOs::new(PathBuf::from("/tmp"))
        .expect("native resource baseline is observable");
    let mut command = Command::new("/bin/sleep");
    command.arg("30");
    let mut child = os
        .spawn_supervised(&mut command)
        .expect("isolated harmless child starts in its own process group");

    assert!(matches!(
        os.observe_identity(child.identity()),
        IdentityObservation::SameIdentity { .. }
    ));
    assert_eq!(
        os.signal_exact_identity(child.identity(), Signal::Kill),
        SignalOutcome::TemporarilyUnavailable,
        "observation alone never grants PID mutation authority"
    );
    assert!(child
        .try_finalize()
        .expect("live leader is observable")
        .is_none());
    assert!(os
        .list_children(child.identity())
        .expect("owned child enumeration succeeds")
        .is_empty());
    assert_eq!(child.process_group(), child.identity().pid());
    assert_eq!(
        MacOsControllerOs::supervision_risks(),
        [
            SupervisionRisk::DescendantMayEscapeProcessGroup,
            SupervisionRisk::SameSessionProcessMayJoinProcessGroup,
            SupervisionRisk::ControllerExitLosesParentAuthority,
        ]
    );

    assert_eq!(
        child.signal_group(Signal::Terminate),
        SignalOutcome::Delivered
    );
    let status = cleanup_before_deadline(&mut child);
    assert!(!status.success());
    assert_eq!(
        child.signal_group(Signal::Kill),
        SignalOutcome::TemporarilyUnavailable,
        "reaped leader no longer reserves a signalable PGID"
    );
}

#[test]
fn exited_leader_cannot_be_finalized_while_a_same_group_descendant_remains() {
    let directory = FixtureDirectory::new();
    let os = MacOsControllerOs::new("/tmp").expect("native resource baseline is observable");
    let mut command = fixture_command("leave-descendant", &directory.0);
    let mut child = os
        .spawn_supervised(&mut command)
        .expect("isolated fixture starts");
    fs::write(directory.0.join("go"), b"").expect("release leader fixture");
    assert!(poll_until(|| directory
        .0
        .join("descendant-started")
        .exists()));
    assert!(poll_until(|| leader_exited_without_reaping(
        child.identity().pid().get()
    )));
    assert!(child.try_finalize().expect("group is observable").is_none());
    assert_eq!(child.signal_group(Signal::Kill), SignalOutcome::Delivered);
    let status = cleanup_before_deadline(&mut child);
    assert!(
        status.success(),
        "natural leader exit is distinct from descendant cleanup"
    );
}

#[test]
fn drop_is_bounded_and_attempts_direct_child_cleanup() {
    let mut os = MacOsControllerOs::new("/tmp").expect("native resource baseline is observable");
    let mut command = Command::new("/bin/sleep");
    command.arg("15");
    let child = os
        .spawn_supervised(&mut command)
        .expect("isolated harmless child starts");
    let identity = child.identity();
    let started = Instant::now();
    drop(child);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "drop must not wait for natural exit"
    );
    assert!(poll_until(|| match os.observe_identity(identity) {
        IdentityObservation::ExactIdentityAbsent | IdentityObservation::PidReused(_) => true,
        IdentityObservation::SameIdentity { .. } => {
            // Drop may exhaust its best-effort reap budget before SIGKILL completes.
            // SAFETY: this exact test child still belongs to this parent; WNOHANG only
            // reaps it if already exited and never waits for exit.
            let mut status = 0;
            let pid = libc::pid_t::try_from(identity.pid().get()).expect("native child PID");
            let reaped = unsafe { libc::waitpid(pid, &raw mut status, libc::WNOHANG) };
            reaped == pid
        }
        IdentityObservation::Unresolved(_) => false,
    }));
}

// Invoked only in an isolated test subprocess, never against an existing process.
#[test]
#[allow(clippy::zombie_processes)]
fn native_child_fixture() {
    let Some(directory) = std::env::var_os("PHOENIX_TEST_SUPERVISION_DIRECTORY") else {
        return;
    };
    let directory = PathBuf::from(directory);
    assert!(poll_until(|| directory.join("go").exists()));
    match std::env::var("PHOENIX_TEST_SUPERVISION_MODE").as_deref() {
        Ok("leave-descendant") => {
            let descendant = Command::new("/bin/sleep")
                .arg("15")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("harmless bounded descendant starts");
            fs::write(directory.join("descendant-started"), b"")
                .expect("publish descendant readiness");
            // The scenario deliberately exits the leader without reaping its descendant.
            drop(descendant);
        }
        Ok(mode) => panic!("unexpected isolated fixture mode: {mode}"),
        Err(error) => panic!("isolated fixture mode is missing: {error}"),
    }
}

fn fixture_command(mode: &str, directory: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("native test executable exists"));
    command
        .args(["--exact", "native_child_fixture", "--nocapture"])
        .env("PHOENIX_TEST_SUPERVISION_MODE", mode)
        .env("PHOENIX_TEST_SUPERVISION_DIRECTORY", directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn cleanup_before_deadline(child: &mut SupervisedChild) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(report) = child.try_cleanup().expect("cleanup remains observable") {
            assert_eq!(
                report.unresolved_risks,
                MacOsControllerOs::supervision_risks()
            );
            return report.leader_status;
        }
        assert!(
            Instant::now() < deadline,
            "native cleanup must finish within the test budget"
        );
        // test-timing-allow: the retained child exposes no notification API; each retry rechecks observable cleanup state.
        thread::sleep(Duration::from_millis(10));
    }
}

fn leader_exited_without_reaping(pid: u32) -> bool {
    let mut info = MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: pid belongs to this test's retained child. WNOWAIT does not reap it;
    // WNOHANG does not block, and info points to writable siginfo_t storage.
    if unsafe {
        libc::waitid(
            libc::P_PID,
            pid,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
        )
    } != 0
    {
        return false;
    }
    // SAFETY: the zero-initialized siginfo_t was filled by successful waitid.
    let info = unsafe { info.assume_init() };
    u32::try_from(info.si_pid).ok() == Some(pid)
}

fn poll_until(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if ready() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        // test-timing-allow: fixture readiness is observed on every retry; the sleep only bounds polling load.
        thread::sleep(Duration::from_millis(10));
    }
}

struct FixtureDirectory(PathBuf);

impl FixtureDirectory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("valid test time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "phoenix-test-supervision-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("unique harmless test directory is created");
        Self(path)
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn native_resource_sample_reports_capacity_and_measured_contention() {
    let mut os = MacOsControllerOs::new(PathBuf::from("/tmp"))
        .expect("native resource baseline is observable");
    let mut observed = None;
    for sequence in 0..50 {
        // test-timing-allow: elapsed time is required for cumulative native CPU counters to advance.
        thread::sleep(Duration::from_millis(20));
        match os.sample_resources(sequence) {
            Ok(sample) => {
                observed = Some(sample);
                break;
            }
            Err(ResourceObservationFailure::TemporarilyUnavailable) => {}
            Err(error) => panic!("native resource observation failed: {error:?}"),
        }
    }
    let sample = observed.expect("CPU counters advance within the bounded observation loop");
    assert!(sample.capacity.available_memory_bytes > 0);
    assert!(sample.capacity.available_storage_bytes > 0);
    assert!(sample.contention.cpu.get() <= 10_000);
}
