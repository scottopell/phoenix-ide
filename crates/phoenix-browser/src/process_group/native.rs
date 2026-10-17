use phoenix_core::process_identity::{process_identity_matches, ProcessIdentity};
use std::io;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootObservation {
    Live,
    #[cfg(any(target_os = "macos", test))]
    ExitPending,
    Exited,
}

impl RootObservation {
    fn permits_signal(self) -> bool {
        matches!(self, Self::Live | Self::Exited)
    }

    fn permits_completion(self) -> bool {
        self == Self::Exited
    }
}

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
        owned.observe_root(child)?;
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

    pub(crate) fn observe_root(
        &self,
        child: &mut tokio::process::Child,
    ) -> io::Result<RootObservation> {
        if child.id() != Some(self.root.pid) {
            return Err(io::Error::other(
                "owned Chrome child was reaped or replaced",
            ));
        }
        if self.waitable_exit()? {
            // A waitable, unreaped child still reserves its PID even when macOS
            // no longer supplies PROC_PIDTBSDINFO for that zombie.
            return Ok(RootObservation::Exited);
        }
        let pid = libc::pid_t::try_from(self.root.pid).map_err(io::Error::other)?;
        let observed_identity =
            phoenix_core::process_identity::current_process_identity(self.root.pid);
        // SAFETY: getpgid only observes this candidate process; no signal here.
        let observed_group = unsafe { libc::getpgid(pid) };
        let group_error = (observed_group == -1).then(io::Error::last_os_error);
        let live_matches = observed_identity == Some(self.root) && observed_group == pid;
        if live_matches {
            return Ok(RootObservation::Live);
        }
        // Exit can occur between waitid's live observation and proc_pidinfo.
        if self.waitable_exit()? {
            return Ok(RootObservation::Exited);
        }
        #[cfg(target_os = "macos")]
        if observed_identity.is_none()
            && observed_group == -1
            && group_error
                .as_ref()
                .is_some_and(|error| error.raw_os_error() == Some(libc::ESRCH))
        {
            return Ok(RootObservation::ExitPending);
        }
        #[cfg(target_os = "macos")]
        eprintln!("owned_chrome_darwin_wait {}", self.darwin_wait_diagnostic());
        Err(io::Error::other(format!(
            "live Chrome root birth identity or launch group changed: expected={:?} observed={observed_identity:?} child_pid={:?} pgid={observed_group} pgid_error={group_error:?} waitable_exit=false",
            self.root, child.id(),
        )))
    }

    fn signal(&self, child: &mut tokio::process::Child, signal: libc::c_int) -> io::Result<bool> {
        if !self.observe_root(child)?.permits_signal() {
            return Ok(false);
        }
        let pid = libc::pid_t::try_from(self.root.pid).map_err(io::Error::other)?;
        // SAFETY: the unreaped owned child anchors this isolated launch group.
        if unsafe { libc::kill(-pid, signal) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(true)
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
        let mut terminated = false;
        let mut killed = false;
        loop {
            let observation = self.observe_root(child)?;
            if observation.permits_completion() {
                let live_members = group_has_live_members(self.root.pid).inspect_err(|error| {
                    #[cfg(target_os = "macos")]
                    eprintln!("owned_chrome_group_enumeration child={:?} root={:?} observation={observation:?} error={error} {}", child.id(), self.root, self.darwin_wait_diagnostic());
                    #[cfg(not(target_os = "macos"))]
                    let _ = error;
                })?;
                if !live_members && self.observe_root(child)? == RootObservation::Exited {
                    self.drained = true;
                    return Ok(());
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Chrome launch group did not drain",
                ));
            }
            if !terminated {
                terminated = self.signal(child, libc::SIGTERM)?;
            } else if !killed && now >= escalate {
                killed = self.signal(child, libc::SIGKILL)?;
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

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy)]
struct GroupQueryOutcome {
    returned_bytes: libc::c_int,
    errno: libc::c_int,
}

#[cfg(any(target_os = "macos", test))]
impl GroupQueryOutcome {
    fn complete_pid_count(self, capacity_bytes: usize) -> io::Result<usize> {
        if self.errno != 0 {
            return Err(io::Error::from_raw_os_error(self.errno));
        }
        let returned = usize::try_from(self.returned_bytes).map_err(io::Error::other)?;
        let pid_size = std::mem::size_of::<libc::pid_t>();
        if capacity_bytes == 0 || returned >= capacity_bytes || !returned.is_multiple_of(pid_size) {
            return Err(io::Error::other(format!(
                "incomplete Chrome process group query: returned_bytes={returned} capacity_bytes={capacity_bytes} pid_size={pid_size}",
            )));
        }
        Ok(returned / pid_size)
    }
}

#[cfg(target_os = "macos")]
fn query_group(group: u32, buffer: Option<&mut [libc::pid_t]>) -> io::Result<GroupQueryOutcome> {
    let (pointer, capacity) = match buffer {
        Some(buffer) => (
            buffer.as_mut_ptr().cast(),
            libc::c_int::try_from(std::mem::size_of_val(buffer)).map_err(io::Error::other)?,
        ),
        None => (std::ptr::null_mut(), 0),
    };
    // SAFETY: __error is thread-local; no other call intervenes before capture.
    let outcome = unsafe {
        *libc::__error() = 0;
        let returned_bytes = libc::proc_listpids(2, group, pointer, capacity);
        let errno = *libc::__error();
        GroupQueryOutcome {
            returned_bytes,
            errno,
        }
    };
    #[cfg(test)]
    GROUP_QUERY_TRACE.with(|count| {
        if count.get() < 32 {
            eprintln!("owned_group_query sample={} group={group} capacity_bytes={capacity} returned_bytes={} errno={}", count.get(), outcome.returned_bytes, outcome.errno);
            count.set(count.get() + 1);
        }
    });
    Ok(outcome)
}

#[cfg(all(target_os = "macos", test))]
thread_local! {
    static GROUP_QUERY_TRACE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(target_os = "macos")]
fn group_has_live_members(group: u32) -> io::Result<bool> {
    // PROC_PGRP_ONLY = 2, as defined by libproc.h.
    let sizing = query_group(group, None)?;
    let size = sizing.returned_bytes;
    if size <= 0 || sizing.errno != 0 {
        return Err(io::Error::other(format!(
            "could not size Chrome process group: group={group} size={size} errno={:?}",
            sizing.errno
        )));
    }
    let mut pids = vec![
        0 as libc::pid_t;
        usize::try_from(size).map_err(io::Error::other)?
            / std::mem::size_of::<libc::pid_t>()
            + 32
    ];
    let bytes = std::mem::size_of_val(pids.as_slice());
    let listing = query_group(group, Some(&mut pids))?;
    let count = listing.complete_pid_count(bytes).map_err(|error| {
        io::Error::other(format!(
            "could not enumerate complete Chrome process group: group={group} size={size} capacity_bytes={bytes} returned_bytes={} errno={}: {error}", listing.returned_bytes, listing.errno,
        ))
    })?;
    for pid in pids.into_iter().take(count) {
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

    #[test]
    fn pending_exit_authorizes_neither_signal_nor_completion() {
        assert!(!RootObservation::ExitPending.permits_signal());
        assert!(!RootObservation::ExitPending.permits_completion());
        assert!(RootObservation::Live.permits_signal());
        assert!(!RootObservation::Live.permits_completion());
        assert!(RootObservation::Exited.permits_signal());
        assert!(RootObservation::Exited.permits_completion());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn owned_empty_group_query_contract_before_and_after_root_reap() {
        GROUP_QUERY_TRACE.with(|count| count.set(0));
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
        let group = OwnedChromeGroup::adopt(identity, &mut root).unwrap();
        root.stdin
            .take()
            .unwrap()
            .write_all(b"release\n")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while group.observe_root(&mut root).unwrap() != RootObservation::Exited {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for phase in ["unreaped", "reaped"] {
            if phase == "reaped" {
                root.wait().await.unwrap();
            }
            let sizing = query_group(identity.pid, None).unwrap();
            assert!(sizing.returned_bytes > 0);
            assert_eq!(sizing.errno, 0);
            let mut pids = vec![
                0;
                usize::try_from(sizing.returned_bytes).unwrap()
                    / std::mem::size_of::<libc::pid_t>()
                    + 32
            ];
            let listing = query_group(identity.pid, Some(&mut pids)).unwrap();
            eprintln!("owned_empty_group phase={phase} root={identity:?} child={:?} sizing={sizing:?} listing={listing:?}", root.id());
            assert_eq!(
                listing.errno, 0,
                "owned-group query must not mask a syscall error"
            );
            assert!(listing.returned_bytes >= 0);
            assert!(
                usize::try_from(listing.returned_bytes).unwrap()
                    < std::mem::size_of_val(pids.as_slice())
            );
        }
        assert!(
            group.observe_root(&mut root).is_err(),
            "post-reap enumeration grants no signal authority"
        );
    }

    #[test]
    fn group_query_decoder_distinguishes_empty_error_and_truncation() {
        let pid_bytes = std::mem::size_of::<libc::pid_t>();
        let capacity = pid_bytes * 8;
        assert_eq!(
            GroupQueryOutcome {
                returned_bytes: 0,
                errno: 0
            }
            .complete_pid_count(capacity)
            .unwrap(),
            0
        );
        assert_eq!(
            GroupQueryOutcome {
                returned_bytes: i32::try_from(pid_bytes * 2).unwrap(),
                errno: 0
            }
            .complete_pid_count(capacity)
            .unwrap(),
            2
        );
        for outcome in [
            GroupQueryOutcome {
                returned_bytes: 0,
                errno: libc::EPERM,
            },
            GroupQueryOutcome {
                returned_bytes: i32::try_from(pid_bytes).unwrap(),
                errno: libc::ESRCH,
            },
            GroupQueryOutcome {
                returned_bytes: -1,
                errno: 0,
            },
            GroupQueryOutcome {
                returned_bytes: 1,
                errno: 0,
            },
            GroupQueryOutcome {
                returned_bytes: i32::try_from(capacity).unwrap(),
                errno: 0,
            },
            GroupQueryOutcome {
                returned_bytes: i32::try_from(capacity + pid_bytes).unwrap(),
                errno: 0,
            },
        ] {
            assert!(
                outcome.complete_pid_count(capacity).is_err(),
                "invalid query accepted: {outcome:?}"
            );
        }
        assert!(GroupQueryOutcome {
            returned_bytes: 0,
            errno: 0
        }
        .complete_pid_count(0)
        .is_err());
    }

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
            let observed_group = unsafe { libc::getpgid(pid) };
            let group_error = (observed_group == -1).then(io::Error::last_os_error);
            eprintln!("owned_chrome_transition sample={samples} child={:?} proc_rc={read} proc_errno={read_error:?} proc_status={} proc_group={} pgid={observed_group} pgid_errno={group_error:?} {}", child.id(), info.pbi_status, info.pbi_pgid, group.darwin_wait_diagnostic());
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
                match group.observe_root(&mut root) {
                    Ok(RootObservation::Exited) => break,
                    Ok(RootObservation::Live | RootObservation::ExitPending) => {
                        tokio::task::yield_now().await;
                    }
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
        assert_eq!(
            group.observe_root(&mut root).unwrap(),
            RootObservation::Exited,
            "WNOWAIT must retain the same exited-child anchor"
        );
        assert_eq!(
            group.observe_root(&mut root).unwrap(),
            RootObservation::Exited,
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
            group.observe_root(&mut unrelated).is_err(),
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
            group.observe_root(&mut root).is_err(),
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
