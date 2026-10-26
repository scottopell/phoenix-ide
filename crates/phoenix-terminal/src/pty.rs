use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub(crate) struct PtyPair {
    pub master: OwnedFd,
    pub slave: OwnedFd,
}

pub(crate) fn allocate() -> Result<PtyPair, String> {
    allocate_pair().map_err(|error| format!("allocate isolated PTY: {error}"))
}

fn open_endpoint(path: &CStr) -> io::Result<OwnedFd> {
    // SAFETY: path is NUL-terminated; no mode argument is needed without O_CREAT.
    let raw = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: open returned a new descriptor owned by this function.
    let endpoint = unsafe { OwnedFd::from_raw_fd(raw) };
    if raw >= 3 {
        return Ok(endpoint);
    }
    // SAFETY: endpoint remains owned during the atomic, close-on-exec duplication.
    let relocated = unsafe { libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, 3) };
    if relocated < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl returned a new descriptor distinct from endpoint.
    Ok(unsafe { OwnedFd::from_raw_fd(relocated) })
}

fn allocate_pair() -> io::Result<PtyPair> {
    let master = open_endpoint(c"/dev/ptmx")?;
    // SAFETY: master owns an open PTY master descriptor.
    if unsafe { libc::grantpt(master.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: master remains owned while the slave is unlocked and opened.
    if unsafe { libc::unlockpt(master.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let slave = open_endpoint(&slave_name(&master)?)?;
    Ok(PtyPair { master, slave })
}

#[cfg(target_os = "macos")]
fn slave_name(master: &OwnedFd) -> io::Result<CString> {
    // TIOCPTYGNAME's ABI is a 128-byte char buffer, independent of PATH_MAX.
    let mut name = [0_u8; 128];
    // SAFETY: name has the exact output-buffer size encoded by TIOCPTYGNAME.
    if unsafe {
        libc::ioctl(
            master.as_raw_fd(),
            libc::TIOCPTYGNAME.into(),
            name.as_mut_ptr(),
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    CStr::from_bytes_until_nul(&name)
        .map(CStr::to_owned)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(target_os = "linux")]
fn slave_name(master: &OwnedFd) -> io::Result<CString> {
    let mut name = [0_u8; libc::PATH_MAX as usize];
    // SAFETY: name is a writable buffer of the advertised length.
    let result =
        unsafe { libc::ptsname_r(master.as_raw_fd(), name.as_mut_ptr().cast(), name.len()) };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    CStr::from_bytes_until_nul(&name)
        .map(CStr::to_owned)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::allocate;
    use std::ffi::CString;
    use std::os::fd::AsRawFd;

    #[test]
    fn allocated_pair_is_close_on_exec() {
        let pair = allocate().expect("allocate PTY");
        for descriptor in [&pair.master, &pair.slave] {
            // SAFETY: the pair owns both descriptors throughout this assertion.
            let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) };
            assert!(flags >= 0);
            assert_ne!(flags & libc::FD_CLOEXEC, 0, "PTY descriptor is inheritable");
        }
    }

    #[test]
    fn allocation_without_standard_descriptors() {
        const CHILD: &str = "PHOENIX_PTY_TEST_CLOSE_STDIO";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "pty::tests::allocation_without_standard_descriptors",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .status()
                .expect("run isolated closed-stdio probe");
            assert!(status.success(), "closed-stdio allocation failed: {status}");
            return;
        }
        // SAFETY: only this isolated probe process loses its standard descriptors.
        for fd in [0, 1, 2] {
            unsafe { libc::close(fd) };
        }
        let Ok(pair) = allocate() else {
            std::process::exit(100);
        };
        for descriptor in [&pair.master, &pair.slave] {
            if descriptor.as_raw_fd() < 3 {
                std::process::exit(101);
            }
            let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) };
            if flags < 0 || flags & libc::FD_CLOEXEC == 0 {
                std::process::exit(102);
            }
        }
        drop(pair);
        std::process::exit(0);
    }

    fn exec_observes_terminal(fd: std::os::fd::RawFd, clear_close_on_exec: bool) -> bool {
        let shell = CString::new("/bin/sh").unwrap();
        let dash_c = CString::new("-c").unwrap();
        let script = CString::new("test -t \"$1\"").unwrap();
        let arg_zero = CString::new("pty-inheritance-probe").unwrap();
        let descriptor = CString::new(fd.to_string()).unwrap();
        let arguments = [
            shell.as_ptr(),
            dash_c.as_ptr(),
            script.as_ptr(),
            arg_zero.as_ptr(),
            descriptor.as_ptr(),
            std::ptr::null(),
        ];
        let environment = [std::ptr::null()];
        // SAFETY: the child only invokes execve/_exit with buffers prepared before fork.
        let child = unsafe { libc::fork() };
        assert!(child >= 0, "fork descriptor probe");
        if child == 0 {
            unsafe {
                if clear_close_on_exec && libc::fcntl(fd, libc::F_SETFD, 0) != 0 {
                    libc::_exit(127);
                }
                libc::execve(shell.as_ptr(), arguments.as_ptr(), environment.as_ptr());
                libc::_exit(127);
            }
        }
        let mut status = 0;
        loop {
            // SAFETY: child is our exact child; this call reaps it before returning.
            let waited = unsafe { libc::waitpid(child, &raw mut status, 0) };
            if waited < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            assert_eq!(waited, child);
            break;
        }
        assert!(
            libc::WIFEXITED(status),
            "descriptor probe terminated abnormally"
        );
        let exit = libc::WEXITSTATUS(status);
        assert!(exit == 0 || exit == 1, "descriptor probe failed: {exit}");
        exit == 0
    }

    #[test]
    fn unrelated_exec_cannot_retain_either_pty_endpoint() {
        let pair = allocate().expect("allocate PTY");
        for descriptor in [&pair.master, &pair.slave] {
            assert!(unsafe { libc::isatty(descriptor.as_raw_fd()) } == 1);
            // Only the forked probe clears the flag, leaving parallel tests isolated.
            assert!(exec_observes_terminal(descriptor.as_raw_fd(), true));
            assert!(
                !exec_observes_terminal(descriptor.as_raw_fd(), false),
                "unrelated exec retained PTY endpoint"
            );
        }
    }
}
