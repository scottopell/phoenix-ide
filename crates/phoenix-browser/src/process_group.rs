//! Launch-owned Chrome process groups and the native executable shim.

use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;

const EXEC_ENV: &str = "PHOENIX_INTERNAL_CHROME_EXEC";
static SHIM: OnceLock<ChromeExecShim> = OnceLock::new();

/// Capability installed only by an executable implementing `enter_chrome_exec`.
#[derive(Clone, Debug)]
pub struct ChromeExecShim(PathBuf);

impl ChromeExecShim {
    /// Call from the executable entry point, before creating threads.
    ///
    /// # Errors
    /// Returns an error on unsupported platforms or executable discovery failure.
    pub fn install() -> io::Result<()> {
        if !cfg!(any(target_os = "linux", target_os = "macos")) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Chrome process groups require Linux or macOS",
            ));
        }
        SHIM.set(Self(std::env::current_exe()?)).map_err(|_| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Chrome exec shim already installed",
            )
        })
    }

    pub(crate) fn registered() -> Option<Self> {
        SHIM.get().cloned()
    }

    pub(crate) fn executable(&self) -> &std::path::Path {
        &self.0
    }

    #[cfg(unix)]
    pub(crate) fn encoded_target(path: &std::path::Path) -> String {
        use base64::Engine as _;
        use std::os::unix::ffi::OsStrExt as _;
        base64::engine::general_purpose::STANDARD.encode(path.as_os_str().as_bytes())
    }

    pub(crate) fn environment_key() -> &'static str {
        EXEC_ENV
    }
}

/// Dispatch before parsing application arguments or creating a runtime.
///
/// # Errors
/// A present shim request never falls through to application startup on failure.
pub fn enter_chrome_exec() -> io::Result<()> {
    let Some(target) = std::env::var_os(EXEC_ENV) else {
        return Ok(());
    };
    #[cfg(unix)]
    {
        use base64::Engine as _;
        use std::os::unix::ffi::OsStringExt as _;
        use std::os::unix::process::CommandExt as _;
        let target = target.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid Chrome exec request")
        })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(target)
            .map_err(io::Error::other)?;
        let target = PathBuf::from(std::ffi::OsString::from_vec(bytes));
        if !target.is_absolute() || target == std::env::current_exe()? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid Chrome exec target",
            ));
        }
        // SAFETY: this entry point is called before threads or a runtime exist.
        if unsafe { libc::setpgid(0, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let error = std::process::Command::new(target)
            .args(std::env::args_os().skip(1))
            .env_remove(EXEC_ENV)
            .exec();
        Err(error)
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Chrome exec shim is Unix-only",
        ))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod native;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) use native::{OwnedChromeGroup, RootObservation};
