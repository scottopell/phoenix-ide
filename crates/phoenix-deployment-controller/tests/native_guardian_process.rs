#![cfg(target_os = "macos")]
use phoenix_deployment_controller::{guardian_ipc::WireState, native_guardian::GuardianClient};
use std::{
    io::{BufRead, BufReader},
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[test]
fn runner_death_does_not_kill_guardian_or_its_original_child() {
    let parent = tempfile::Builder::new()
        .prefix("sdc-")
        .tempdir_in("/tmp")
        .unwrap();
    std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let dir = parent.path().join("sdc-disposable-process");
    let mut guardian = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-guardian"))
        .args([
            "--socket-dir",
            dir.to_str().unwrap(),
            "--child-seconds",
            "7",
            "--serve-seconds",
            "8",
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
    let until = Instant::now() + Duration::from_secs(2);
    let mut client = loop {
        if let Ok(client) = GuardianClient::connect(&dir) {
            break client;
        }
        assert!(Instant::now() < until, "guardian socket not ready");
        // test-timing-allow: bounded polling waits for socket readiness.
        thread::sleep(Duration::from_millis(25));
    };
    let original = client.hello().unwrap();
    assert_eq!(original.slots.len(), 1);
    drop(client);
    let mut runner = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"))
        .args([
            "--socket-dir",
            dir.to_str().unwrap(),
            "--observe-seconds",
            "5",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut runner_output = BufReader::new(runner.stdout.take().unwrap());
    let mut ready = String::new();
    runner_output.read_line(&mut ready).unwrap();
    assert!(ready.starts_with("guardian instance="));
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
    assert_eq!(snapshot.instance, original.instance);
    assert_eq!(snapshot.slots, original.slots);
    assert!(guardian.try_wait().unwrap().is_none());
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        0,
        "child survived runner death"
    );
    let parent_pid = Command::new("/bin/ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    assert_eq!(parent_pid.status.code(), Some(0));
    assert_eq!(
        parent_pid
            .stdout
            .iter()
            .copied()
            .filter(|b| !b.is_ascii_whitespace())
            .collect::<Vec<_>>(),
        guardian.id().to_string().into_bytes(),
        "guardian retains original parenthood"
    );
    let runner_status = Command::new(env!("CARGO_BIN_EXE_phoenix-deployment-runner"))
        .args([
            "--socket-dir",
            dir.to_str().unwrap(),
            "--observe-seconds",
            "1",
        ])
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(runner_status.success());
    assert!(guardian.wait().unwrap().success());
}
