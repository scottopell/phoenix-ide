//! Durable, OS-verifiable process identity.
//!
//! A PID is reusable. Phoenix therefore binds destructive authority to the
//! process birth observation supplied by the host kernel as well as the PID.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time: u128,
}

/// Returns the current exact identity for `pid`, or `None` when the host cannot
/// prove it. Callers must treat `None` as unproven authority, never as absence.
#[must_use]
pub fn current_process_identity(pid: u32) -> Option<ProcessIdentity> {
    Some(ProcessIdentity {
        pid,
        start_time: process_start_time(pid)?,
    })
}

/// Whether the current host process is exactly `expected` rather than a reused
/// PID. This performs one host observation; callers that signal afterwards must
/// re-observe before accepting completion.
#[must_use]
pub fn process_identity_matches(expected: ProcessIdentity) -> bool {
    current_process_identity(expected.pid) == Some(expected)
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Option<u128> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    let tail = stat.get(close + 1..)?;
    tail.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_os = "macos")]
fn process_start_time(pid: u32) -> Option<u128> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
    let rc = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::addr_of_mut!(info).cast::<libc::c_void>(),
            size,
        )
    };
    if rc != size {
        return None;
    }
    u128::from(info.pbi_start_tvsec)
        .checked_mul(1_000_000)
        .and_then(|seconds| seconds.checked_add(u128::from(info.pbi_start_tvusec)))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_start_time(_pid: u32) -> Option<u128> {
    None
}

#[cfg(test)]
mod tests {
    use super::{current_process_identity, process_identity_matches, ProcessIdentity};

    #[test]
    fn current_process_identity_is_exact() {
        let identity = current_process_identity(std::process::id())
            .expect("supported host provides current process identity");
        assert!(process_identity_matches(identity));
        assert!(!process_identity_matches(ProcessIdentity {
            start_time: identity.start_time.saturating_add(1),
            ..identity
        }));
    }
}
