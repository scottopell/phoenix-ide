use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use nono::{AccessMode, CapabilitySet, Sandbox};

const TASK_DIRS_ENV: &str = "PHOENIX_SANDBOX_TASK_DIRS";
const SENSITIVE_DIRS_ENV: &str = "PHOENIX_SANDBOX_SENSITIVE_DIRS";
const REPO_ROOT_ENV: &str = "PHOENIX_SANDBOX_REPO_ROOT";
const SCRATCH_ENV: &str = "PHOENIX_SANDBOX_SCRATCH";
const HOME_ENV: &str = "PHOENIX_SANDBOX_HOME";
const PLATFORM_TEMP_ENV: &str = "PHOENIX_SANDBOX_PLATFORM_TEMP";
const LIST_SEPARATOR: &str = "\u{1f}";

#[derive(Debug, Clone)]
pub struct ExploreReadOnlyPolicy {
    repo_root: PathBuf,
    task_dirs: Vec<PathBuf>,
    scratch_dir: PathBuf,
    sandbox_home: PathBuf,
    platform_temp: PathBuf,
    sensitive_dirs: Vec<PathBuf>,
    path: OsString,
}

impl ExploreReadOnlyPolicy {
    /// Build an Explore read-only policy for `working_dir`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the working directory cannot be canonicalized
    /// or the scratch/home directories cannot be created.
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
        let sandbox_home = scratch_dir.join("home");
        std::fs::create_dir_all(&sandbox_home)?;
        let platform_temp = platform_temp_dir();
        let path = inherited_path();
        let sensitive_dirs = sensitive_dirs_for_home(std::env::var_os("HOME").as_deref());
        Ok(Self {
            repo_root,
            task_dirs,
            scratch_dir,
            sandbox_home,
            platform_temp,
            sensitive_dirs,
            path,
        })
    }

    fn to_command_env(&self, command: &mut Command) {
        command.env(REPO_ROOT_ENV, &self.repo_root);
        command.env(SCRATCH_ENV, &self.scratch_dir);
        command.env(HOME_ENV, &self.sandbox_home);
        command.env(PLATFORM_TEMP_ENV, &self.platform_temp);
        command.env(TASK_DIRS_ENV, join_paths(&self.task_dirs));
        command.env(SENSITIVE_DIRS_ENV, join_paths(&self.sensitive_dirs));
        self.apply_child_env(command);
    }

    fn apply_child_env(&self, command: &mut Command) {
        command.env("PHOENIX_SANDBOX_SCRATCH", &self.scratch_dir);
        command.env("PHOENIX_SANDBOX_HOME", &self.sandbox_home);
        command.env("HOME", &self.sandbox_home);
        command.env("TMPDIR", &self.platform_temp);
        command.env("PATH", &self.path);
        command.env("GIT_CONFIG_NOSYSTEM", "1");
        command.env("GIT_TERMINAL_PROMPT", "0");
        command.env("GIT_OPTIONAL_LOCKS", "0");
        command.env("PAGER", "cat");
        command.env("NO_COLOR", "1");
    }

    fn from_env() -> Result<Self, String> {
        let repo_root = env_path(REPO_ROOT_ENV)?;
        let scratch_dir = env_path(SCRATCH_ENV)?;
        let sandbox_home = env_path(HOME_ENV)?;
        let platform_temp = env_path(PLATFORM_TEMP_ENV)?;
        let task_dirs = std::env::var_os(TASK_DIRS_ENV)
            .map(|value| split_paths(&value))
            .unwrap_or_default();
        let sensitive_dirs = std::env::var_os(SENSITIVE_DIRS_ENV)
            .map(|value| split_paths(&value))
            .unwrap_or_default();
        let path = inherited_path();
        Ok(Self {
            repo_root,
            task_dirs,
            scratch_dir,
            sandbox_home,
            platform_temp,
            sensitive_dirs,
            path,
        })
    }

    fn capability_set(&self) -> Result<CapabilitySet, String> {
        let mut caps = CapabilitySet::new()
            .allow_path("/", AccessMode::Read)
            .map_err(|e| e.to_string())?
            .allow_path(&self.scratch_dir, AccessMode::ReadWrite)
            .map_err(|e| e.to_string())?;

        for path in system_read_write_paths() {
            if path.is_dir() {
                caps = caps
                    .allow_path(path, AccessMode::ReadWrite)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        if self.platform_temp.is_dir() {
            caps = caps
                .allow_path(&self.platform_temp, AccessMode::ReadWrite)
                .map_err(|e| format!("{}: {e}", self.platform_temp.display()))?;
        }
        for path in &self.task_dirs {
            if path.is_dir() {
                caps = caps
                    .allow_path(path, AccessMode::ReadWrite)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        add_sensitive_denies(&mut caps, &self.sensitive_dirs)?;

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
        let mut command = Command::new("/bin/bash");
        command
            .arg("-c")
            .arg(cmd)
            .current_dir(&policy.repo_root)
            .env_clear();
        policy.apply_child_env(&mut command);
        let err = command.exec();
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

fn inherited_path() -> OsString {
    std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/usr/bin:/bin:/usr/sbin:/sbin"))
}

fn platform_temp_dir() -> PathBuf {
    std::env::temp_dir()
}

fn sensitive_dirs_for_home(home: Option<&OsStr>) -> Vec<PathBuf> {
    let Some(home) = home else {
        return Vec::new();
    };
    let home = PathBuf::from(home);
    [
        ".ssh",
        ".aws",
        ".gnupg",
        ".password-store",
        ".config/password-store",
        ".config/phoenix-ide",
        ".phoenix-ide",
    ]
    .into_iter()
    .map(|relative| home.join(relative))
    .filter(|path| path.exists())
    .collect()
}

#[cfg(target_os = "macos")]
fn add_sensitive_denies(
    caps: &mut CapabilitySet,
    sensitive_dirs: &[PathBuf],
) -> Result<(), String> {
    for path in sensitive_dirs {
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        let rule = format!(
            "(deny file-read* (subpath \"{}\"))",
            seatbelt_escape_path(&canonical)
        );
        caps.add_platform_rule(rule).map_err(|e| {
            format!(
                "failed to add sensitive-path deny for {}: {e}",
                path.display()
            )
        })?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn add_sensitive_denies(
    _caps: &mut CapabilitySet,
    _sensitive_dirs: &[PathBuf],
) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn seatbelt_escape_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn system_read_write_paths() -> &'static [PathBuf] {
    use std::sync::OnceLock;
    static PATHS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    PATHS.get_or_init(|| ["/dev"].into_iter().map(PathBuf::from).collect())
}
