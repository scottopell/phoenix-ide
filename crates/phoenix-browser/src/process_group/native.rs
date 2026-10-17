use phoenix_core::process_identity::{process_identity_matches, ProcessIdentity};
use std::io;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct OwnedChromeGroup {
    root: ProcessIdentity,
    drained: bool,
}

impl OwnedChromeGroup {
    pub(crate) fn adopt(
        root: ProcessIdentity,
        child: &mut tokio::process::Child,
    ) -> io::Result<Self> {
        if !process_identity_matches(root) {
            return Err(io::Error::other(
                "Chrome launch birth identity is not proven",
            ));
        }
        let pid = libc::pid_t::try_from(root.pid).map_err(io::Error::other)?;
        // SAFETY: this is the exact live birth identity supplied by the owner.
        if unsafe { libc::getpgid(pid) } != pid {
            return Err(io::Error::other("Chrome launch did not isolate its group"));
        }
        let owned = Self {
            root,
            drained: false,
        };
        owned.root_exited(child)?;
        Ok(owned)
    }

    pub(crate) fn is_drained(&self) -> bool {
        self.drained
    }

    fn waitable_exit(&self) -> io::Result<bool> {
        let pid = libc::pid_t::try_from(self.root.pid).map_err(io::Error::other)?;
        let mut status: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: status is writable and WNOWAIT retains the child/PID anchor.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.root.pid,
                &raw mut status,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: waitid initialized the siginfo payload, or left its zero PID.
        Ok(unsafe { status.si_pid() } == pid)
    }

    #[cfg(target_os = "macos")]
    fn darwin_wait_diagnostic(&self) -> String {
        let mut status: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let options = libc::WEXITED | libc::WNOHANG | libc::WNOWAIT;
        // SAFETY: another non-consuming observation, only on the error path.
        let result = unsafe { libc::waitid(libc::P_PID, self.root.pid, &raw mut status, options) };
        let wait_error = (result != 0).then(io::Error::last_os_error);
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: a null new action observes SIGCHLD disposition without changing it.
        let action_result =
            unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &raw mut action) };
        let action_error = (action_result != 0).then(io::Error::last_os_error);
        format!(
            "waitid_result={result} waitid_errno={wait_error:?} options={options} p_pid={} siginfo_size={} si_signo={} si_errno={} si_code={} si_pid={} si_uid={} si_status={} sigaction_result={action_result} sigaction_errno={action_error:?} sigchld_handler={} sigchld_flags={} sig_ign={} sa_nocldwait={}",
            libc::P_PID, std::mem::size_of::<libc::siginfo_t>(), status.si_signo,
            status.si_errno, status.si_code, status.si_pid, status.si_uid,
            status.si_status, action.sa_sigaction, action.sa_flags, libc::SIG_IGN,
            libc::SA_NOCLDWAIT,
        )
    }

    pub(crate) fn root_exited(&self, child: &mut tokio::process::Child) -> io::Result<bool> {
        if child.id() != Some(self.root.pid) {
            return Err(io::Error::other(
                "owned Chrome child was reaped or replaced",
            ));
        }
        if self.waitable_exit()? {
            // A waitable, unreaped child still reserves its PID even when macOS
            // no longer supplies PROC_PIDTBSDINFO for that zombie.
            return Ok(true);
        }
        let pid = libc::pid_t::try_from(self.root.pid).map_err(io::Error::other)?;
        let observed_identity =
            phoenix_core::process_identity::current_process_identity(self.root.pid);
        // SAFETY: getpgid only observes this candidate process; no signal here.
        let observed_group = unsafe { libc::getpgid(pid) };
        let group_error = (observed_group == -1).then(io::Error::last_os_error);
        let live_matches = observed_identity == Some(self.root) && observed_group == pid;
        if live_matches {
            return Ok(false);
        }
        // Exit can occur between waitid's live observation and proc_pidinfo.
        if self.waitable_exit()? {
            return Ok(true);
        }
        #[cfg(target_os = "macos")]
        eprintln!("owned_chrome_darwin_wait {}", self.darwin_wait_diagnostic());
        Err(io::Error::other(format!(
            "live Chrome root birth identity or launch group changed: expected={:?} observed={observed_identity:?} child_pid={:?} pgid={observed_group} pgid_error={group_error:?} waitable_exit=false",
            self.root, child.id(),
        )))
    }

    fn signal(&self, child: &mut tokio::process::Child, signal: libc::c_int) -> io::Result<()> {
        self.root_exited(child)?;
        let pid = libc::pid_t::try_from(self.root.pid).map_err(io::Error::other)?;
        // SAFETY: the unreaped owned child anchors this isolated launch group.
        if unsafe { libc::kill(-pid, signal) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(crate) async fn drain(
        &mut self,
        child: &mut tokio::process::Child,
        budget: Duration,
    ) -> io::Result<()> {
        if self.drained {
            return Ok(());
        }
        let started = Instant::now();
        let deadline = started + budget;
        let escalate = started + budget.min(Duration::from_millis(250));
        self.signal(child, libc::SIGTERM)?;
        let mut killed = false;
        loop {
            let exited = self.root_exited(child)?;
            if exited && !group_has_live_members(self.root.pid)? {
                self.drained = true;
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Chrome launch group did not drain",
                ));
            }
            if !killed && now >= escalate {
                self.signal(child, libc::SIGKILL)?;
                killed = true;
            }
            // This observes kernel process completion, never retries deletion.
            tokio::task::yield_now().await;
        }
    }
}

impl Drop for OwnedChromeGroup {
    fn drop(&mut self) {
        if !self.drained {
            tracing::warn!(
                "Chrome group dropped without retained child drain authority; profile retained"
            );
        }
    }
}

#[cfg(target_os = "linux")]
fn group_has_live_members(group: u32) -> io::Result<bool> {
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .is_none()
        {
            continue;
        }
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let tail = stat
            .rsplit_once(')')
            .ok_or_else(|| io::Error::other("invalid process stat"))?
            .1;
        let mut fields = tail.split_whitespace();
        let state = fields
            .next()
            .ok_or_else(|| io::Error::other("missing process state"))?;
        let _parent = fields.next();
        let actual = fields
            .next()
            .and_then(|field| field.parse::<u32>().ok())
            .ok_or_else(|| io::Error::other("missing process group"))?;
        if actual == group && state != "Z" && state != "X" {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
fn group_has_live_members(group: u32) -> io::Result<bool> {
    // PROC_PGRP_ONLY = 2, as defined by libproc.h.
    let size = unsafe { libc::proc_listpids(2, group, std::ptr::null_mut(), 0) };
    if size <= 0 {
        return Err(io::Error::other("could not size Chrome process group"));
    }
    let mut pids = vec![
        0 as libc::pid_t;
        usize::try_from(size).map_err(io::Error::other)?
            / std::mem::size_of::<libc::pid_t>()
            + 32
    ];
    let bytes = libc::c_int::try_from(pids.len() * std::mem::size_of::<libc::pid_t>())
        .map_err(io::Error::other)?;
    let count = unsafe { libc::proc_listpids(2, group, pids.as_mut_ptr().cast(), bytes) };
    if count <= 0 || count >= bytes {
        return Err(io::Error::other(
            "could not enumerate complete Chrome process group",
        ));
    }
    for pid in pids.into_iter().take(
        usize::try_from(count).map_err(io::Error::other)? / std::mem::size_of::<libc::pid_t>(),
    ) {
        if pid == 0 {
            continue;
        }
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
            .map_err(io::Error::other)?;
        let read = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        if read != size {
            if io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                continue;
            }
            return Err(io::Error::other("could not inspect Chrome group member"));
        }
        if info.pbi_pgid == group && info.pbi_status != libc::SZOMB {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::process_identity::current_process_identity;
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

    fn sentinel() -> tokio::process::Child {
        tokio::process::Command::new("/bin/sh")
            .args(["-c", "exec cat >/dev/null"])
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }

    #[cfg(target_os = "macos")]
    async fn diagnose_exit_transition(group: &OwnedChromeGroup, child: &mut tokio::process::Child) {
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut samples = 0;
        loop {
            let pid = libc::pid_t::try_from(group.root.pid).unwrap();
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).unwrap();
            // SAFETY: bounded observation of the retained owned child's PID only.
            let read = unsafe {
                libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
            };
            let read_error = (read != size).then(io::Error::last_os_error);
            // SAFETY: observes the same retained owned child's process group.
            let pgid = unsafe { libc::getpgid(pid) };
            let pgid_error = (pgid == -1).then(io::Error::last_os_error);
            eprintln!("owned_chrome_transition sample={samples} child={:?} proc_rc={read} proc_errno={read_error:?} proc_status={} proc_group={} pgid={pgid} pgid_errno={pgid_error:?} {}", child.id(), info.pbi_status, info.pbi_pgid, group.darwin_wait_diagnostic());
            samples += 1;
            if group.waitable_exit().is_ok_and(|exited| exited)
                || Instant::now() >= deadline
                || samples >= 32
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn exited_root_drains_stderr_closed_writer_before_profile_removal() {
        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().to_path_buf();
        let mut unrelated = sentinel();
        let script = r#"
/bin/sh -c 'trap "" TERM; exec 2>/dev/null; : > "$1/writer"; echo ready; while :; do printf x > "$1/writer"; done' writer "$1" </dev/null &
read release
exit 0
"#;
        let mut root = tokio::process::Command::new("/bin/sh")
            .args(["-c", script, "root"])
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdout = tokio::io::BufReader::new(root.stdout.take().unwrap());
        let mut ready = String::new();
        tokio::time::timeout(Duration::from_secs(5), stdout.read_line(&mut ready))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        let identity = current_process_identity(root.id().unwrap()).unwrap();
        let mut group = OwnedChromeGroup::adopt(identity, &mut root).unwrap();
        root.stdin
            .take()
            .unwrap()
            .write_all(b"release\n")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match group.root_exited(&mut root) {
                    Ok(true) => break,
                    Ok(false) => tokio::task::yield_now().await,
                    Err(error) => {
                        #[cfg(target_os = "macos")]
                        diagnose_exit_transition(&group, &mut root).await;
                        panic!("strict owned-root observation failed: {error}");
                    }
                }
            }
        })
        .await
        .unwrap();
        assert!(
            group.root_exited(&mut root).unwrap(),
            "WNOWAIT must retain the same exited-child anchor"
        );
        assert!(
            group.root_exited(&mut root).unwrap(),
            "observing exit must not reap the root"
        );
        assert!(path.join("writer").exists());
        assert!(
            group_has_live_members(identity.pid).unwrap(),
            "root exit must not imply writer completion"
        );
        assert!(unrelated.try_wait().unwrap().is_none());
        group
            .drain(&mut root, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(group.is_drained());
        assert!(!group_has_live_members(identity.pid).unwrap());
        assert!(
            unrelated.try_wait().unwrap().is_none(),
            "unrelated sentinel must not be signalled"
        );
        root.wait().await.unwrap();
        profile.close().unwrap();
        assert!(!path.exists());
        unrelated.kill().await.unwrap();
    }

    #[test]
    fn escaped_writer_fixture() {
        use std::io::{BufRead as _, Write as _};
        let Ok(role) = std::env::var("PHOENIX_GROUP_TEST_ROLE") else {
            return;
        };
        if role == "writer" {
            assert_eq!(unsafe { libc::setsid() }, unsafe { libc::getpid() });
            let path = std::env::var_os("PHOENIX_GROUP_TEST_PROFILE").unwrap();
            let _file =
                std::fs::File::create(std::path::PathBuf::from(path).join("escaped-writer"))
                    .unwrap();
            println!("writer-ready:{}", std::process::id());
            std::io::stdout().flush().unwrap();
            loop {
                unsafe {
                    libc::pause();
                }
            }
        }
        assert_eq!(role, "root");
        let mut writer = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "process_group::native::tests::escaped_writer_fixture",
                "--nocapture",
            ])
            .env("PHOENIX_GROUP_TEST_ROLE", "writer")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = std::io::BufReader::new(writer.stdout.take().unwrap());
        for line in stdout.lines() {
            if let Some((_, pid)) = line.unwrap().split_once("writer-ready:") {
                println!("root-ready:{}", pid.trim());
                std::io::stdout().flush().unwrap();
                break;
            }
        }
        let mut release = String::new();
        std::io::stdin().read_line(&mut release).unwrap();
        std::process::exit(0);
    }

    #[tokio::test]
    async fn group_drain_does_not_claim_an_escaped_writer() {
        let profile = tempfile::tempdir().unwrap();
        let mut root = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "process_group::native::tests::escaped_writer_fixture",
                "--nocapture",
            ])
            .env("PHOENIX_GROUP_TEST_ROLE", "root")
            .env("PHOENIX_GROUP_TEST_PROFILE", profile.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdout = tokio::io::BufReader::new(root.stdout.take().unwrap());
        let writer_pid = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut line = String::new();
                assert_ne!(stdout.read_line(&mut line).await.unwrap(), 0);
                if let Some((_, pid)) = line.split_once("root-ready:") {
                    break pid.trim().parse::<u32>().unwrap();
                }
            }
        })
        .await
        .unwrap();
        let writer_identity = current_process_identity(writer_pid).unwrap();
        let mut group = OwnedChromeGroup::adopt(
            current_process_identity(root.id().unwrap()).unwrap(),
            &mut root,
        )
        .unwrap();
        root.stdin
            .take()
            .unwrap()
            .write_all(b"release\n")
            .await
            .unwrap();
        group
            .drain(&mut root, Duration::from_secs(5))
            .await
            .unwrap();
        root.wait().await.unwrap();
        assert!(
            process_identity_matches(writer_identity),
            "group drain is not escaped-process completion"
        );
        assert_eq!(
            unsafe { libc::getpgid(i32::try_from(writer_pid).unwrap()) },
            i32::try_from(writer_pid).unwrap()
        );
        assert!(profile.path().join("escaped-writer").exists());
        // Only the fixture's recorded exact identity is signalled, not an old group.
        assert!(process_identity_matches(writer_identity));
        assert_eq!(
            unsafe { libc::kill(i32::try_from(writer_pid).unwrap(), libc::SIGKILL) },
            0
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while group_has_live_members(writer_pid).unwrap() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn reaped_or_mismatched_root_cannot_authorize_old_group_signals() {
        let profile = tempfile::tempdir().unwrap();
        let mut unrelated = sentinel();
        let mut root = tokio::process::Command::new("/bin/sh")
            .args(["-c", "echo ready; read release"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdout = tokio::io::BufReader::new(root.stdout.take().unwrap());
        let mut ready = String::new();
        tokio::time::timeout(Duration::from_secs(5), stdout.read_line(&mut ready))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        let identity = current_process_identity(root.id().unwrap()).unwrap();
        let mut group = OwnedChromeGroup::adopt(identity, &mut root).unwrap();
        assert!(
            group.root_exited(&mut unrelated).is_err(),
            "a different child handle cannot authorize this group"
        );
        assert!(group.signal(&mut unrelated, libc::SIGTERM).is_err());
        assert!(unrelated.try_wait().unwrap().is_none());
        root.stdin
            .take()
            .unwrap()
            .write_all(b"release\n")
            .await
            .unwrap();
        root.wait().await.unwrap();
        assert!(
            group.root_exited(&mut root).is_err(),
            "reaped root must lose wait authority"
        );
        assert!(group
            .drain(&mut root, Duration::from_secs(1))
            .await
            .is_err());
        assert!(!group.is_drained());
        assert!(profile.path().exists());
        let mut reused = current_process_identity(unrelated.id().unwrap()).unwrap();
        reused.start_time += 1;
        assert!(OwnedChromeGroup::adopt(reused, &mut unrelated).is_err());
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.kill().await.unwrap();
    }
}
