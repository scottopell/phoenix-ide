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
            let error = io::Error::last_os_error();
            #[cfg(target_os = "macos")]
            eprintln!(
                "owned_chrome_waitid_failed root={:?} error={error} {}",
                self.root,
                self.darwin_wait_diagnostic()
            );
            return Err(error);
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
        if darwin_exit_is_pending(
            self.root,
            observed_identity,
            observed_group,
            group_error.as_ref().and_then(io::Error::raw_os_error),
        ) {
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
        let result = if unsafe { libc::kill(-pid, signal) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        };
        signal_delivery(result).inspect_err(|error| {
            #[cfg(target_os = "macos")]
            {
                let observed_identity = phoenix_core::process_identity::current_process_identity(self.root.pid);
                let observed_group = unsafe { libc::getpgid(pid) };
                let group_error = (observed_group == -1).then(io::Error::last_os_error);
                eprintln!("owned_chrome_signal_failed signal={signal} root={:?} child={:?} observed_identity={observed_identity:?} getpgid={observed_group} group_error={group_error:?} error={error} {}", self.root, child.id(), self.darwin_wait_diagnostic());
            }
            #[cfg(not(target_os = "macos"))]
            let _ = error;
        })
    }

    pub(crate) async fn drain(
        &mut self,
        child: &mut tokio::process::Child,
        budget: Duration,
    ) -> io::Result<()> {
        self.drain_with_signal(child, budget, Self::signal).await
    }

    async fn drain_with_signal(
        &mut self,
        child: &mut tokio::process::Child,
        budget: Duration,
        mut signal: impl FnMut(&Self, &mut tokio::process::Child, libc::c_int) -> io::Result<bool>,
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
                terminated = signal(self, child, libc::SIGTERM)?;
            } else if !killed && now >= escalate {
                killed = signal(self, child, libc::SIGKILL)?;
            }
            // This observes kernel process completion, never retries deletion.
            tokio::task::yield_now().await;
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn darwin_exit_is_pending(
    root: ProcessIdentity,
    observed_identity: Option<ProcessIdentity>,
    observed_group: libc::pid_t,
    group_errno: Option<libc::c_int>,
) -> bool {
    (observed_identity.is_none() || observed_identity == Some(root))
        && observed_group == -1
        && group_errno == Some(libc::ESRCH)
}

fn signal_delivery(result: io::Result<()>) -> io::Result<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(false),
        Err(error) => Err(error),
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
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<libc::pid_t>().ok())
            .filter(|pid| *pid > 0)
        else {
            continue;
        };
        if linux_process_is_live_member(pid, group, query_process_group, || {
            std::fs::read_to_string(entry.path().join("stat"))
        })? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(target_os = "linux")]
fn query_process_group(pid: libc::pid_t) -> io::Result<libc::pid_t> {
    let group = unsafe { libc::getpgid(pid) };
    if group < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(group)
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_process_is_live_member(
    pid: libc::pid_t,
    group: u32,
    mut query_group: impl FnMut(libc::pid_t) -> io::Result<libc::pid_t>,
    read_stat: impl FnOnce() -> io::Result<String>,
) -> io::Result<bool> {
    let mut is_member = || match query_group(pid) {
        Ok(actual) => Ok(u32::try_from(actual).map_err(io::Error::other)? == group),
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(false),
        Err(error) => Err(error),
    };
    if !is_member()? {
        return Ok(false);
    }
    let stat = match read_stat() {
        Ok(stat) => stat,
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                || error.raw_os_error() == Some(libc::ESRCH) =>
        {
            return if is_member()? { Err(error) } else { Ok(false) };
        }
        Err(error) => return Err(error),
    };
    let parsed = (|| {
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
        Ok(actual == group && state != "Z" && state != "X")
    })();
    match parsed {
        Ok(live) => Ok(live),
        Err(original) => match is_member() {
            Ok(false) => Ok(false),
            Ok(true) | Err(_) => Err(original),
        },
    }
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
    fn linux_scan_skips_only_kernel_proven_nonmembers_without_reading_stat() {
        for membership in [
            Ok(0),
            Ok(99),
            Err(io::Error::from_raw_os_error(libc::ESRCH)),
        ] {
            let mut membership = Some(membership);
            assert!(!linux_process_is_live_member(
                42,
                42,
                |_| membership.take().unwrap(),
                || panic!("unrelated or vanished process must not require procfs access"),
            )
            .unwrap());
        }
        for invalid in [-1, libc::pid_t::MIN] {
            assert!(linux_process_is_live_member(
                42,
                42,
                |_| Ok(invalid),
                || panic!("invalid process group must not authorize a stat read"),
            )
            .is_err());
        }
        let denied = linux_process_is_live_member(
            42,
            42,
            |_| Err(io::Error::from_raw_os_error(libc::EPERM)),
            || panic!("unknown membership must fail before stat"),
        )
        .unwrap_err();
        assert_eq!(denied.raw_os_error(), Some(libc::EPERM));
        let owned = linux_process_is_live_member(
            42,
            42,
            |_| Ok(42),
            || Err(io::Error::from(io::ErrorKind::PermissionDenied)),
        )
        .unwrap_err();
        assert_eq!(owned.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn linux_scan_reclassifies_missing_candidate_stat_without_assuming_exit() {
        for after in [
            Ok(0),
            Ok(99),
            Err(io::Error::from_raw_os_error(libc::ESRCH)),
        ] {
            let mut observations = [Ok(42), after].into_iter();
            assert!(!linux_process_is_live_member(
                42,
                42,
                |_| observations.next().unwrap(),
                || Err(io::Error::from(io::ErrorKind::NotFound)),
            )
            .unwrap());
            assert!(observations.next().is_none());
        }
        for after in [Ok(42), Err(io::Error::from_raw_os_error(libc::EPERM))] {
            let mut observations = [Ok(42), after].into_iter();
            assert!(linux_process_is_live_member(
                42,
                42,
                |_| observations.next().unwrap(),
                || Err(io::Error::from(io::ErrorKind::NotFound)),
            )
            .is_err());
        }
    }

    #[test]
    fn linux_scan_reclassifies_stat_esrch_without_assuming_exit() {
        for after in [
            Ok(0),
            Ok(99),
            Err(io::Error::from_raw_os_error(libc::ESRCH)),
        ] {
            let mut observations = [Ok(42), after].into_iter();
            assert!(!linux_process_is_live_member(
                42,
                42,
                |_| observations.next().unwrap(),
                || Err(io::Error::from_raw_os_error(libc::ESRCH)),
            )
            .unwrap());
            assert!(observations.next().is_none());
        }
        for (after, expected_errno) in [
            (Ok(42), libc::ESRCH),
            (Err(io::Error::from_raw_os_error(libc::EPERM)), libc::EPERM),
            (Err(io::Error::from_raw_os_error(libc::EIO)), libc::EIO),
        ] {
            let mut observations = [Ok(42), after].into_iter();
            let error = linux_process_is_live_member(
                42,
                42,
                |_| observations.next().unwrap(),
                || Err(io::Error::from_raw_os_error(libc::ESRCH)),
            )
            .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(expected_errno));
            assert!(observations.next().is_none());
        }
        for errno in [libc::EPERM, libc::EIO] {
            let mut observations = [Ok(42), Ok(99)].into_iter();
            let error = linux_process_is_live_member(
                42,
                42,
                |_| observations.next().unwrap(),
                || Err(io::Error::from_raw_os_error(errno)),
            )
            .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(errno));
            assert!(
                observations.next().is_some(),
                "unknown stat error must not reclassify"
            );
        }
    }

    #[test]
    fn linux_scan_reclassifies_malformed_stat_only_after_known_membership_loss() {
        for stat in [
            "invalid",
            "42 (Chrome)",
            "42 (Chrome) S 1",
            "42 (Chrome) S 1 invalid",
        ] {
            for after in [
                Ok(0),
                Ok(99),
                Err(io::Error::from_raw_os_error(libc::ESRCH)),
            ] {
                let mut observations = [Ok(42), after].into_iter();
                let mut reads = 0;
                let live = linux_process_is_live_member(
                    42,
                    42,
                    |_| {
                        observations
                            .next()
                            .expect("exactly two membership observations")
                    },
                    || {
                        reads += 1;
                        Ok(stat.to_owned())
                    },
                )
                .unwrap();
                assert!(!live, "{stat:?}");
                assert_eq!(reads, 1);
                assert!(observations.next().is_none());
            }
        }
    }

    #[test]
    fn linux_scan_preserves_parse_error_when_fresh_membership_is_owned_or_unknown() {
        for (stat, message) in [
            ("invalid", "invalid process stat"),
            ("42 (Chrome)", "missing process state"),
            ("42 (Chrome) S 1", "missing process group"),
            ("42 (Chrome) S 1 invalid", "missing process group"),
        ] {
            for after in [
                Ok(42),
                Ok(-1),
                Err(io::Error::from_raw_os_error(libc::EPERM)),
                Err(io::Error::from_raw_os_error(libc::EIO)),
            ] {
                let mut observations = [Ok(42), after].into_iter();
                let mut reads = 0;
                let error = linux_process_is_live_member(
                    42,
                    42,
                    |_| {
                        observations
                            .next()
                            .expect("exactly two membership observations")
                    },
                    || {
                        reads += 1;
                        Ok(stat.to_owned())
                    },
                )
                .unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::Other);
                assert_eq!(error.to_string(), message);
                assert_eq!(error.raw_os_error(), None);
                assert_eq!(reads, 1);
                assert!(observations.next().is_none());
            }
        }
    }

    #[test]
    fn linux_scan_requires_readable_candidate_state_and_rechecks_stat_group() {
        for (stat, live) in [
            ("42 (Chrome worker) S 1 42", true),
            ("42 (Chrome worker) Z 1 42", false),
            ("42 (Chrome worker) X 1 42", false),
            ("42 (replacement) S 1 99", false),
        ] {
            assert_eq!(
                linux_process_is_live_member(42, 42, |_| Ok(42), || Ok(stat.to_owned())).unwrap(),
                live,
            );
        }
        for stat in ["invalid", "42 (Chrome)", "42 (Chrome) S 1 invalid"] {
            assert!(
                linux_process_is_live_member(42, 42, |_| Ok(42), || Ok(stat.to_owned())).is_err()
            );
        }
    }

    #[test]
    fn missing_signal_target_requires_reobservation_not_completion() {
        assert!(!signal_delivery(Err(io::Error::from_raw_os_error(libc::ESRCH))).unwrap());
        assert!(signal_delivery(Ok(())).unwrap());
        for errno in [libc::EPERM, libc::EINVAL] {
            assert_eq!(
                signal_delivery(Err(io::Error::from_raw_os_error(errno)))
                    .unwrap_err()
                    .raw_os_error(),
                Some(errno),
            );
        }
        assert!(!RootObservation::Live.permits_completion());
        assert!(!RootObservation::ExitPending.permits_completion());
    }

    #[test]
    fn darwin_pending_exit_preserves_birth_identity_and_requires_missing_group() {
        let root = ProcessIdentity {
            pid: 42,
            start_time: 100,
        };
        let replacement = ProcessIdentity {
            pid: 42,
            start_time: 101,
        };
        let other_pid = ProcessIdentity {
            pid: 43,
            start_time: 100,
        };
        for (identity, consistent_birth) in [
            (None, true),
            (Some(root), true),
            (Some(replacement), false),
            (Some(other_pid), false),
        ] {
            for (group, errno, missing_group) in [
                (-1, Some(libc::ESRCH), true),
                (-1, Some(libc::EPERM), false),
                (-1, Some(libc::EINVAL), false),
                (-1, None, false),
                (42, None, false),
                (43, None, false),
                (0, None, false),
                (42, Some(libc::ESRCH), false),
            ] {
                assert_eq!(
                    darwin_exit_is_pending(root, identity, group, errno),
                    consistent_birth && missing_group,
                    "identity={identity:?} group={group} errno={errno:?}",
                );
            }
        }
        assert!(!RootObservation::ExitPending.permits_signal());
        assert!(!RootObservation::ExitPending.permits_completion());
    }

    #[tokio::test]
    async fn signal_exit_race_reobserves_owned_child_before_draining() {
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "echo ready; read release"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut output = tokio::io::BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        tokio::time::timeout(Duration::from_secs(5), output.read_line(&mut ready))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ready.trim(), "ready");
        let identity = current_process_identity(child.id().unwrap()).unwrap();
        let mut group = OwnedChromeGroup::adopt(identity, &mut child).unwrap();
        let mut attempts = 0;
        group
            .drain_with_signal(
                &mut child,
                Duration::from_secs(5),
                |owner, retained_child, signal| {
                    attempts += 1;
                    if attempts == 1 {
                        assert_eq!(
                            owner.observe_root(retained_child).unwrap(),
                            RootObservation::Live
                        );
                        assert!(!owner.is_drained());
                        signal_delivery(Err(io::Error::from_raw_os_error(libc::ESRCH)))
                    } else {
                        assert!(attempts >= 2);
                        owner.signal(retained_child, signal)
                    }
                },
            )
            .await
            .unwrap();
        assert!(attempts >= 2, "ESRCH must not mark the group drained");
        assert!(group.is_drained());
        assert_eq!(
            group.observe_root(&mut child).unwrap(),
            RootObservation::Exited
        );
        child.wait().await.unwrap();
    }

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

    struct EscapedWriterChild(std::process::Child);

    impl Drop for EscapedWriterChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let deadline = Instant::now() + Duration::from_secs(2);
            while matches!(self.0.try_wait(), Ok(None)) && Instant::now() < deadline {
                // test-timing-allow: bounded reap of this fixture's killed direct child.
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    struct EscapedLaunchCleanup {
        root: tokio::process::Child,
        writer: Option<ProcessIdentity>,
    }

    impl Drop for EscapedLaunchCleanup {
        fn drop(&mut self) {
            if let Some(writer) = self.writer {
                if process_identity_matches(writer) {
                    if let Ok(pid) = libc::pid_t::try_from(writer.pid) {
                        // SAFETY: the fixture supplied this exact owned birth identity.
                        let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
                    }
                }
            }
            // EOF lets a not-yet-released root kill and reap its owned writer.
            drop(self.root.stdin.take());
            let deadline = Instant::now() + Duration::from_secs(3);
            while matches!(self.root.try_wait(), Ok(None)) && Instant::now() < deadline {
                // test-timing-allow: await the owned fixture's EOF cleanup before kill fallback.
                std::thread::sleep(Duration::from_millis(10));
            }
            if matches!(self.root.try_wait(), Ok(None)) {
                let _ = self.root.start_kill();
                let deadline = Instant::now() + Duration::from_secs(2);
                while matches!(self.root.try_wait(), Ok(None)) && Instant::now() < deadline {
                    // test-timing-allow: bounded reap of the fixture's killed direct root child.
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            if let Some(writer) = self.writer {
                observe_escaped_writer_stopped(writer);
            }
        }
    }

    fn observe_escaped_writer_stopped(writer: ProcessIdentity) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while process_identity_matches(writer) {
            match group_has_live_members(writer.pid) {
                Ok(false) => return,
                observation if Instant::now() >= deadline => {
                    eprintln!("escaped fixture emergency cleanup unresolved: writer={writer:?} observation={observation:?}");
                    return;
                }
                _ => {
                    // test-timing-allow: observe termination of the exact escaped fixture, not reap a non-child.
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    #[test]
    fn escaped_writer_fixture() {
        use std::io::{BufRead as _, Write as _};
        enum Event {
            Ready(String),
            Release,
            Cancel,
        }
        let Ok(role) = std::env::var("PHOENIX_GROUP_TEST_ROLE") else {
            return;
        };
        if role == "writer" {
            assert_eq!(unsafe { libc::setsid() }, unsafe { libc::getpid() });
            let path = std::env::var_os("PHOENIX_GROUP_TEST_PROFILE").unwrap();
            let _file =
                std::fs::File::create(std::path::PathBuf::from(path).join("escaped-writer"))
                    .unwrap();
            if std::env::var_os("PHOENIX_GROUP_TEST_PENDING_WRITER").is_some() {
                let identity = current_process_identity(std::process::id()).unwrap();
                let profile = std::path::PathBuf::from(
                    std::env::var_os("PHOENIX_GROUP_TEST_PROFILE").unwrap(),
                );
                std::fs::write(
                    profile.join("pending-writer"),
                    format!("{} {}", identity.pid, identity.start_time),
                )
                .unwrap();
                loop {
                    unsafe {
                        libc::pause();
                    }
                }
            }
            println!("writer-ready:{}", std::process::id());
            std::io::stdout().flush().unwrap();
            loop {
                unsafe {
                    libc::pause();
                }
            }
        }
        assert_eq!(role, "root");
        let mut writer = EscapedWriterChild(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "process_group::native::tests::escaped_writer_fixture",
                    "--nocapture",
                ])
                .env("PHOENIX_GROUP_TEST_ROLE", "writer")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        println!("writer-spawned:{}", writer.0.id());
        std::io::stdout().flush().unwrap();
        let stdout = std::io::BufReader::new(writer.0.stdout.take().unwrap());
        let (events, receiver) = std::sync::mpsc::channel();
        let readiness = events.clone();
        std::thread::spawn(move || {
            for line in stdout.lines() {
                if let Some((_, pid)) = line.unwrap().split_once("writer-ready:") {
                    let _ = readiness.send(Event::Ready(pid.trim().to_owned()));
                    return;
                }
            }
            let _ = readiness.send(Event::Cancel);
        });
        std::thread::spawn(move || {
            let mut release = String::new();
            let event = match std::io::stdin().read_line(&mut release) {
                Ok(_) if release.trim() == "release" => Event::Release,
                _ => Event::Cancel,
            };
            let _ = events.send(event);
        });
        let mut ready = false;
        loop {
            match receiver.recv_timeout(Duration::from_secs(5)).unwrap() {
                Event::Ready(pid) => {
                    ready = true;
                    println!("root-ready:{pid}");
                    std::io::stdout().flush().unwrap();
                }
                Event::Release if ready => std::process::exit(0),
                Event::Release | Event::Cancel => break,
            }
        }
        drop(writer);
        let profile =
            std::path::PathBuf::from(std::env::var_os("PHOENIX_GROUP_TEST_PROFILE").unwrap());
        std::fs::write(profile.join("root-cleaned-writer"), b"cleaned").unwrap();
    }

    fn escaped_spawn(profile: &std::path::Path, pending_writer: bool) -> EscapedLaunchCleanup {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        if pending_writer {
            command.env("PHOENIX_GROUP_TEST_PENDING_WRITER", "1");
        }
        let root = command
            .args([
                "--exact",
                "process_group::native::tests::escaped_writer_fixture",
                "--nocapture",
            ])
            .env("PHOENIX_GROUP_TEST_ROLE", "root")
            .env("PHOENIX_GROUP_TEST_PROFILE", profile)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        EscapedLaunchCleanup { root, writer: None }
    }

    async fn escaped_launch(profile: &std::path::Path) -> EscapedLaunchCleanup {
        let mut cleanup = escaped_spawn(profile, false);
        let mut stdout = tokio::io::BufReader::new(cleanup.root.stdout.take().unwrap());
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut line = String::new();
                assert_ne!(stdout.read_line(&mut line).await.unwrap(), 0);
                if let Some((_, pid)) = line.split_once("writer-spawned:") {
                    let pid = pid.trim().parse::<u32>().unwrap();
                    cleanup.writer = current_process_identity(pid);
                    assert!(cleanup.writer.is_some(), "spawned writer identity missing");
                }
                if let Some((_, pid)) = line.split_once("root-ready:") {
                    assert_eq!(
                        cleanup.writer.unwrap().pid,
                        pid.trim().parse::<u32>().unwrap()
                    );
                    break;
                }
            }
        })
        .await
        .unwrap();
        cleanup
    }

    #[tokio::test]
    async fn escaped_writer_unwind_before_identity_cancels_pending_readiness() {
        let profile = tempfile::tempdir().unwrap();
        let cleanup = escaped_spawn(profile.path(), true);
        let root_identity = current_process_identity(cleanup.root.id().unwrap()).unwrap();
        let writer_identity = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(profile.path().join("pending-writer"))
                {
                    let mut fields = contents.split_whitespace();
                    if let (Some(pid), Some(start_time)) = (fields.next(), fields.next()) {
                        break ProcessIdentity {
                            pid: pid.parse().unwrap(),
                            start_time: start_time.parse().unwrap(),
                        };
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(cleanup.writer.is_none());
        assert!(!profile.path().join("root-cleaned-writer").exists());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _cleanup = cleanup;
            panic!("injected failure before identity or readiness handoff");
        }));
        assert!(result.is_err());
        assert!(
            profile.path().join("root-cleaned-writer").exists(),
            "root cleanup must finish before outer fallback"
        );
        assert!(!process_identity_matches(writer_identity));
        assert!(!process_identity_matches(root_identity));
    }

    #[tokio::test]
    async fn escaped_writer_unwind_cleans_owned_fixture_before_release() {
        let profile = tempfile::tempdir().unwrap();
        let cleanup = escaped_launch(profile.path()).await;
        let root_identity = current_process_identity(cleanup.root.id().unwrap()).unwrap();
        let writer_identity = cleanup.writer.unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _cleanup = cleanup;
            panic!("injected failure before root release");
        }));
        assert!(result.is_err());
        tokio::time::timeout(Duration::from_secs(5), async {
            while process_identity_matches(root_identity)
                || process_identity_matches(writer_identity)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn group_drain_does_not_claim_an_escaped_writer() {
        let profile = tempfile::tempdir().unwrap();
        let mut cleanup = escaped_launch(profile.path()).await;
        let writer_identity = cleanup.writer.unwrap();
        let writer_pid = writer_identity.pid;
        let root = &mut cleanup.root;
        let mut group =
            OwnedChromeGroup::adopt(current_process_identity(root.id().unwrap()).unwrap(), root)
                .unwrap();
        root.stdin
            .take()
            .unwrap()
            .write_all(b"release\n")
            .await
            .unwrap();
        group.drain(root, Duration::from_secs(5)).await.unwrap();
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
