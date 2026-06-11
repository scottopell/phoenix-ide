use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use nono::{AccessMode, CapabilitySet, Sandbox};

const TASK_DIRS_ENV: &str = "PHOENIX_SANDBOX_TASK_DIRS";
const REPO_ROOT_ENV: &str = "PHOENIX_SANDBOX_REPO_ROOT";
const SCRATCH_ENV: &str = "PHOENIX_SANDBOX_SCRATCH";
const LIST_SEPARATOR: &str = "\u{1f}";

#[derive(Debug, Clone)]
pub struct ExploreReadOnlyPolicy {
    repo_root: PathBuf,
    task_dirs: Vec<PathBuf>,
    scratch_dir: PathBuf,
}

impl ExploreReadOnlyPolicy {
    /// Build an Explore read-only policy for `working_dir`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the working directory cannot be canonicalized
    /// or the scratch directory cannot be created.
    pub fn discover(working_dir: &Path) -> std::io::Result<Self> {
        let repo_root = working_dir.canonicalize()?;
        let task_dirs = taskmd_core::discover::candidates(&repo_root)
            .into_iter()
            .map(|name| repo_root.join(name))
            .filter(|path| path.is_dir())
            .collect();
        let scratch_dir = std::env::temp_dir()
            .join("phoenix-ide")
            .join("explore-bash")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&scratch_dir)?;
        Ok(Self {
            repo_root,
            task_dirs,
            scratch_dir,
        })
    }

    fn to_command_env(&self, command: &mut Command) {
        command.env(REPO_ROOT_ENV, &self.repo_root);
        command.env(SCRATCH_ENV, &self.scratch_dir);
        command.env("PHOENIX_SANDBOX_SCRATCH", &self.scratch_dir);
        command.env("TMPDIR", &self.scratch_dir);
        command.env("HOME", &self.scratch_dir);
        command.env("PATH", safe_path());
        command.env("GIT_CONFIG_NOSYSTEM", "1");
        command.env("GIT_TERMINAL_PROMPT", "0");
        command.env("PAGER", "cat");
        command.env("NO_COLOR", "1");
        command.env(TASK_DIRS_ENV, join_paths(&self.task_dirs));
    }

    fn from_env() -> Result<Self, String> {
        let repo_root = env_path(REPO_ROOT_ENV)?;
        let scratch_dir = env_path(SCRATCH_ENV)?;
        let task_dirs = std::env::var_os(TASK_DIRS_ENV)
            .map(|value| split_paths(&value))
            .unwrap_or_default();
        Ok(Self {
            repo_root,
            task_dirs,
            scratch_dir,
        })
    }

    fn capability_set(&self) -> Result<CapabilitySet, String> {
        let mut caps = CapabilitySet::new()
            .allow_path(&self.repo_root, AccessMode::Read)
            .map_err(|e| e.to_string())?
            .allow_path(&self.scratch_dir, AccessMode::ReadWrite)
            .map_err(|e| e.to_string())?;

        for path in system_read_paths() {
            if path.is_dir() {
                caps = caps
                    .allow_path(path, AccessMode::Read)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        for path in system_read_write_paths() {
            if path.is_dir() {
                caps = caps
                    .allow_path(path, AccessMode::ReadWrite)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        for path in &self.task_dirs {
            if path.is_dir() {
                caps = caps
                    .allow_path(path, AccessMode::ReadWrite)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }

        Ok(caps.block_network())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExploreSandboxLauncher;

impl ExploreSandboxLauncher {
    /// Build the Phoenix child-process command that applies the Explore
    /// sandbox and execs `bash -c cmd`.
    ///
    /// # Errors
    ///
    /// Returns an error when the policy cannot be constructed or the current
    /// Phoenix executable cannot be resolved.
    pub fn command(cmd: &str, working_dir: &Path) -> Result<Command, String> {
        let policy = ExploreReadOnlyPolicy::discover(working_dir)
            .map_err(|e| format!("failed to create explore sandbox policy: {e}"))?;
        let exe = std::env::current_exe()
            .map_err(|e| format!("failed to resolve phoenix executable: {e}"))?;
        let mut command = Command::new(exe);
        command
            .arg("--sandbox-exec")
            .arg("--")
            .arg(cmd)
            .current_dir(&policy.repo_root)
            .env_clear();
        policy.to_command_env(&mut command);
        Ok(command)
    }

    #[must_use]
    pub fn supported() -> bool {
        Sandbox::support_info().is_supported
    }
}

pub fn apply_explore_read_only_from_env(cmd: &str) -> ! {
    match apply_and_exec(cmd) {
        Ok(never) => match never {},
        Err(e) => {
            eprintln!("phoenix explore sandbox failed: {e}");
            std::process::exit(126);
        }
    }
}

fn apply_and_exec(cmd: &str) -> Result<std::convert::Infallible, String> {
    let policy = ExploreReadOnlyPolicy::from_env()?;
    let caps = policy.capability_set()?;
    Sandbox::apply(&caps).map_err(|e| format!("failed to apply nono sandbox: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = Command::new("/bin/bash")
            .arg("-c")
            .arg(cmd)
            .current_dir(&policy.repo_root)
            .env_clear()
            .env("HOME", &policy.scratch_dir)
            .env("TMPDIR", &policy.scratch_dir)
            .env("PHOENIX_SANDBOX_SCRATCH", &policy.scratch_dir)
            .env("PATH", safe_path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("PAGER", "cat")
            .env("NO_COLOR", "1")
            .exec();
        Err(format!("failed to exec /bin/bash: {err}"))
    }

    #[cfg(not(unix))]
    {
        let _ = cmd;
        Err("explore bash sandbox launcher requires Unix exec".to_string())
    }
}

fn env_path(name: &str) -> Result<PathBuf, String> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {name}"))
}

fn join_paths(paths: &[PathBuf]) -> OsString {
    paths
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join(LIST_SEPARATOR)
        .into()
}

fn split_paths(value: &OsString) -> Vec<PathBuf> {
    value
        .to_string_lossy()
        .split(LIST_SEPARATOR)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn safe_path() -> &'static str {
    "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/opt/homebrew/sbin"
}

fn system_read_paths() -> &'static [PathBuf] {
    use std::sync::OnceLock;
    static PATHS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    PATHS.get_or_init(|| {
        [
            "/bin",
            "/usr/bin",
            "/usr/lib",
            "/usr/libexec",
            "/usr/share",
            "/usr/sbin",
            "/System",
            "/Library",
            "/opt/homebrew/bin",
            "/opt/homebrew/lib",
            "/opt/homebrew/share",
            "/opt/homebrew/Cellar",
            "/private/var/db",
            "/var/db",
            "/etc",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect()
    })
}
fn system_read_write_paths() -> &'static [PathBuf] {
    use std::sync::OnceLock;
    static PATHS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    PATHS.get_or_init(|| ["/dev"].into_iter().map(PathBuf::from).collect())
}
