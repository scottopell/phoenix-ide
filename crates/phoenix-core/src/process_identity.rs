//! Durable, OS-verifiable process identity.
//!
//! A PID is reusable. Phoenix therefore binds destructive authority to the
//! process birth observation supplied by the host kernel as well as the PID.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time: u128,
}

/// A bounded, non-sensitive reason why the host could not prove process identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityObservationFailure {
    InvalidPid,
    MissingProcess { errno: Option<i32> },
    IoError { errno: Option<i32> },
    ShortBsdRead { rc: i32, errno: Option<i32> },
    InvalidBirth,
    Unsupported,
}

impl IdentityObservationFailure {
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::InvalidPid => "invalid_pid",
            Self::MissingProcess { .. } => "missing_process",
            Self::IoError { .. } => "io_error",
            Self::ShortBsdRead { .. } => "short_bsd_read",
            Self::InvalidBirth => "invalid_birth",
            Self::Unsupported => "unsupported",
        }
    }

    #[must_use]
    pub const fn errno(self) -> Option<i32> {
        match self {
            Self::MissingProcess { errno }
            | Self::IoError { errno }
            | Self::ShortBsdRead { errno, .. } => errno,
            Self::InvalidPid | Self::InvalidBirth | Self::Unsupported => None,
        }
    }

    #[must_use]
    pub const fn rc(self) -> Option<i32> {
        match self {
            Self::ShortBsdRead { rc, .. } => Some(rc),
            Self::InvalidPid
            | Self::MissingProcess { .. }
            | Self::IoError { .. }
            | Self::InvalidBirth
            | Self::Unsupported => None,
        }
    }
}

/// Returns exact identity or a typed, bounded observation failure.
///
/// # Errors
///
/// Returns [`IdentityObservationFailure`] when the host cannot prove the process identity.
pub fn observe_process_identity(pid: u32) -> Result<ProcessIdentity, IdentityObservationFailure> {
    if pid == 0 {
        return Err(IdentityObservationFailure::InvalidPid);
    }
    let start_time = process_start_time(pid)?;
    Ok(ProcessIdentity { pid, start_time })
}

/// Returns the current exact identity for `pid`, or `None` when the host cannot
/// prove it. Callers must treat `None` as unproven authority, never as absence.
#[must_use]
pub fn current_process_identity(pid: u32) -> Option<ProcessIdentity> {
    observe_process_identity(pid).ok()
}

/// Whether the current host process is exactly `expected` rather than a reused
/// PID. This performs one host observation; callers that signal afterwards must
/// re-observe before accepting completion.
#[must_use]
pub fn process_identity_matches(expected: ProcessIdentity) -> bool {
    current_process_identity(expected.pid) == Some(expected)
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Result<u128, IdentityObservationFailure> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|error| classify_io_error(&error))?;
    parse_linux_start_time(&stat)
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_start_time(stat: &str) -> Result<u128, IdentityObservationFailure> {
    let close = stat
        .rfind(')')
        .ok_or(IdentityObservationFailure::InvalidBirth)?;
    let tail = stat
        .get(close + 1..)
        .ok_or(IdentityObservationFailure::InvalidBirth)?;
    tail.split_whitespace()
        .nth(19)
        .ok_or(IdentityObservationFailure::InvalidBirth)?
        .parse()
        .map_err(|_| IdentityObservationFailure::InvalidBirth)
}

#[cfg(any(target_os = "linux", test))]
fn classify_io_error(error: &std::io::Error) -> IdentityObservationFailure {
    let errno = error.raw_os_error();
    if error.kind() == std::io::ErrorKind::NotFound || errno == Some(libc::ESRCH) {
        IdentityObservationFailure::MissingProcess { errno }
    } else {
        IdentityObservationFailure::IoError { errno }
    }
}

#[cfg(target_os = "macos")]
fn process_start_time(pid: u32) -> Result<u128, IdentityObservationFailure> {
    let pid = libc::c_int::try_from(pid).map_err(|_| IdentityObservationFailure::InvalidPid)?;
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
        .map_err(|_| IdentityObservationFailure::InvalidBirth)?;
    let rc = unsafe {
        // proc_pidinfo may return a short result without setting errno.
        *libc::__error() = 0;
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::addr_of_mut!(info).cast::<libc::c_void>(),
            size,
        )
    };
    let errno = std::io::Error::last_os_error().raw_os_error();
    if let Some(failure) = bsdinfo_failure(rc, size, errno) {
        return Err(failure);
    }
    u128::from(info.pbi_start_tvsec)
        .checked_mul(1_000_000)
        .and_then(|seconds| seconds.checked_add(u128::from(info.pbi_start_tvusec)))
        .ok_or(IdentityObservationFailure::InvalidBirth)
}

#[cfg(any(target_os = "macos", test))]
fn bsdinfo_failure(
    rc: i32,
    expected: i32,
    errno: Option<i32>,
) -> Option<IdentityObservationFailure> {
    if rc == expected {
        None
    } else if rc <= 0 && errno == Some(libc::ESRCH) {
        Some(IdentityObservationFailure::MissingProcess { errno })
    } else if rc < 0 || (rc == 0 && errno.is_some_and(|value| value != 0)) {
        Some(IdentityObservationFailure::IoError { errno })
    } else {
        Some(IdentityObservationFailure::ShortBsdRead { rc, errno })
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_start_time(_pid: u32) -> Result<u128, IdentityObservationFailure> {
    Err(IdentityObservationFailure::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::{ProcessIdentity, current_process_identity, process_identity_matches};

    #[test]
    fn bsdinfo_short_denied_and_missing_results_have_typed_causes() {
        use super::IdentityObservationFailure as Failure;
        let size = 136;
        assert_eq!(
            super::bsdinfo_failure(-1, size, Some(libc::EPERM)),
            Some(Failure::IoError {
                errno: Some(libc::EPERM)
            })
        );
        assert_eq!(
            super::bsdinfo_failure(-1, size, Some(libc::ESRCH)),
            Some(Failure::MissingProcess {
                errno: Some(libc::ESRCH)
            })
        );
        assert_eq!(
            super::bsdinfo_failure(size - 1, size, Some(0)),
            Some(Failure::ShortBsdRead {
                rc: size - 1,
                errno: Some(0)
            })
        );
        for errno in [libc::EPERM, libc::EACCES] {
            assert_eq!(
                super::bsdinfo_failure(0, size, Some(errno)),
                Some(Failure::IoError { errno: Some(errno) })
            );
        }
        assert_eq!(
            super::bsdinfo_failure(0, size, Some(libc::ESRCH)),
            Some(Failure::MissingProcess {
                errno: Some(libc::ESRCH)
            })
        );
        assert_eq!(
            super::bsdinfo_failure(0, size, Some(0)),
            Some(Failure::ShortBsdRead {
                rc: 0,
                errno: Some(0)
            })
        );
        assert_eq!(super::bsdinfo_failure(size, size, Some(0)), None);
    }

    #[test]
    fn linux_stat_parse_rejects_truncated_and_malformed_birth_fields() {
        assert_eq!(
            super::parse_linux_start_time("123 (name) S"),
            Err(super::IdentityObservationFailure::InvalidBirth)
        );
        let malformed = format!("123 (name) S {}", "1 ".repeat(18));
        for invalid in ["not-a-number", "340282366920938463463374607431768211456"] {
            let stat = format!("123 (name) S {}{invalid}", "1 ".repeat(18));
            assert_eq!(
                super::parse_linux_start_time(&stat),
                Err(super::IdentityObservationFailure::InvalidBirth)
            );
        }
        let valid = format!("123 (name with ) parentheses)) S {}987", "1 ".repeat(18));
        assert_eq!(super::parse_linux_start_time(&valid), Ok(987));
        assert_eq!(
            super::parse_linux_start_time(&malformed),
            Err(super::IdentityObservationFailure::InvalidBirth)
        );
    }

    #[test]
    fn io_errors_distinguish_missing_from_denied_or_other_io() {
        use super::IdentityObservationFailure as Failure;
        let missing = std::io::Error::from_raw_os_error(libc::ESRCH);
        assert_eq!(
            super::classify_io_error(&missing),
            Failure::MissingProcess {
                errno: Some(libc::ESRCH)
            }
        );
        let denied = std::io::Error::from_raw_os_error(libc::EACCES);
        assert_eq!(
            super::classify_io_error(&denied),
            Failure::IoError {
                errno: Some(libc::EACCES)
            }
        );
    }

    #[test]
    fn zero_pid_is_invalid_and_synthetic_esrch_is_classified_as_missing() {
        assert_eq!(
            super::observe_process_identity(0),
            Err(super::IdentityObservationFailure::InvalidPid)
        );
        assert_eq!(
            super::classify_io_error(&std::io::Error::from_raw_os_error(libc::ESRCH)),
            super::IdentityObservationFailure::MissingProcess {
                errno: Some(libc::ESRCH)
            }
        );
    }

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
