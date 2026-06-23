use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use nono::{AccessMode, CapabilitySet, Sandbox};
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;

const TASK_DIRS_ENV: &str = "PHOENIX_SANDBOX_TASK_DIRS";
const SENSITIVE_DIRS_ENV: &str = "PHOENIX_SANDBOX_SENSITIVE_DIRS";
const REPO_ROOT_ENV: &str = "PHOENIX_SANDBOX_REPO_ROOT";
const SCRATCH_ENV: &str = "PHOENIX_SANDBOX_SCRATCH";
const HOME_ENV: &str = "PHOENIX_SANDBOX_HOME";
const PLATFORM_TEMP_ENV: &str = "PHOENIX_SANDBOX_PLATFORM_TEMP";
const TASK_WRITES_ENV: &str = "PHOENIX_SANDBOX_TASK_WRITES";
const LIST_SEPARATOR: &str = "\u{1f}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExploreSandboxTaskWrites {
    Allow,
    Deny,
}

impl ExploreSandboxTaskWrites {
    fn as_env(self) -> &'static str {
        match self {
            Self::Allow => "1",
            Self::Deny => "0",
        }
    }

    fn from_env() -> Self {
        match std::env::var_os(TASK_WRITES_ENV).as_deref() {
            Some(value) if value == "1" => Self::Allow,
            _ => Self::Deny,
        }
    }

    fn allows(self) -> bool {
        matches!(self, Self::Allow)
    }
}

#[derive(Debug, Clone)]
pub struct ExploreReadOnlyPolicy {
    repo_root: PathBuf,
    task_dirs: Vec<PathBuf>,
    scratch_dir: PathBuf,
    sandbox_home: PathBuf,
    platform_temp: PathBuf,
    sensitive_dirs: Vec<PathBuf>,
    task_writes: ExploreSandboxTaskWrites,
    path: OsString,
}

impl ExploreReadOnlyPolicy {
    /// Build an Explore read-only policy for `working_dir`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the working directory cannot be canonicalized
    /// or the scratch/home directories cannot be created.
    pub fn discover(
        working_dir: &Path,
        task_writes: ExploreSandboxTaskWrites,
    ) -> std::io::Result<Self> {
        let runtime_env = PhoenixRuntimeEnvironment::detect();
        let repo_root = working_dir.canonicalize()?;
        let mut task_dirs: Vec<PathBuf> = taskmd_core::discover::candidates(&repo_root)
            .into_iter()
            .filter_map(|name| {
                let path = repo_root.join(name);
                let canonical = path.canonicalize().ok()?;
                canonical.starts_with(&repo_root).then_some(canonical)
            })
            .filter(|path| path.is_dir())
            .collect();
        task_dirs.sort();
        task_dirs.dedup();
        let scratch_root = runtime_env.tmp_subdir("explore-bash")?;
        let scratch_dir = scratch_root.join(uuid::Uuid::new_v4().to_string());
        let sandbox_home = scratch_dir.join("home");
        std::fs::create_dir_all(&sandbox_home)?;
        let platform_temp = platform_temp_dir(&repo_root, &scratch_dir, runtime_env.tmp_root());
        std::fs::create_dir_all(&platform_temp)?;
        let path = inherited_path();
        let sensitive_dirs = sensitive_dirs_for_home(runtime_env.home());
        Ok(Self {
            repo_root,
            task_dirs,
            scratch_dir,
            sandbox_home,
            platform_temp,
            sensitive_dirs,
            task_writes,
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
        command.env(TASK_WRITES_ENV, self.task_writes.as_env());
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
        let task_writes = ExploreSandboxTaskWrites::from_env();
        let path = inherited_path();
        Ok(Self {
            repo_root,
            task_dirs,
            scratch_dir,
            sandbox_home,
            platform_temp,
            sensitive_dirs,
            task_writes,
            path,
        })
    }

    fn capability_set(&self) -> Result<CapabilitySet, String> {
        let mut caps = CapabilitySet::new()
            .allow_path("/", AccessMode::Read)
            .map_err(|e| e.to_string())?
            .allow_path(&self.scratch_dir, AccessMode::ReadWrite)
            .map_err(|e| e.to_string())?;

        for path in system_read_write_files() {
            if path.exists() {
                caps.allow_file_mut(path, AccessMode::ReadWrite)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        if self.platform_temp.is_dir() {
            caps = caps
                .allow_path(&self.platform_temp, AccessMode::ReadWrite)
                .map_err(|e| format!("{}: {e}", self.platform_temp.display()))?;
        }
        if self.task_writes.allows() {
            for path in &self.task_dirs {
                if path.is_dir() {
                    caps = caps
                        .allow_path(path, AccessMode::ReadWrite)
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                }
            }
        }
        #[cfg(target_os = "macos")]
        add_sensitive_denies(&mut caps, &self.sensitive_dirs)?;
        #[cfg(not(target_os = "macos"))]
        add_sensitive_denies(&mut caps, &self.sensitive_dirs);

        Ok(caps.block_network())
    }
}

pub struct ExploreSandboxCommand {
    pub command: Command,
    pub scratch_dir: PathBuf,
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
    pub fn command(
        cmd: &str,
        working_dir: &Path,
        task_writes: ExploreSandboxTaskWrites,
    ) -> Result<ExploreSandboxCommand, String> {
        let policy = ExploreReadOnlyPolicy::discover(working_dir, task_writes)
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
        let scratch_dir = policy.scratch_dir.clone();
        policy.to_command_env(&mut command);
        Ok(ExploreSandboxCommand {
            command,
            scratch_dir,
        })
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
        let mut command = Command::new("bash");
        command
            .arg("-c")
            .arg(cmd)
            .current_dir(&policy.repo_root)
            .env_clear();
        policy.apply_child_env(&mut command);
        let err = command.exec();
        Err(format!("failed to exec bash from PATH: {err}"))
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

fn platform_temp_dir(repo_root: &Path, scratch_dir: &Path, tmp_root: &Path) -> PathBuf {
    let temp = tmp_root.parent().unwrap_or(tmp_root);
    let temp = temp.canonicalize().unwrap_or_else(|_| temp.to_path_buf());
    if repo_root.starts_with(&temp) || temp.starts_with(repo_root) {
        scratch_dir.join("platform-temp")
    } else {
        temp
    }
}

fn sensitive_dirs_for_home(home: &Path) -> Vec<PathBuf> {
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
fn add_sensitive_denies(_caps: &mut CapabilitySet, _sensitive_dirs: &[PathBuf]) {}

#[cfg(target_os = "macos")]
fn seatbelt_escape_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn system_read_write_files() -> &'static [PathBuf] {
    use std::sync::OnceLock;
    static PATHS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    PATHS.get_or_init(|| {
        ["/dev/null", "/dev/zero", "/dev/random", "/dev/urandom"]
            .into_iter()
            .map(PathBuf::from)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_rejects_task_dir_symlink_outside_repo() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let repo = temp.path().join("repo");
        let external = temp.path().join("external-tasks");
        std::fs::create_dir_all(&repo).expect("repo");
        std::fs::create_dir_all(&external).expect("external tasks");
        std::fs::write(
            external.join(taskmd_core::constants::TEMPLATE_FILENAME),
            "# Template\n",
        )
        .expect("template");
        make_symlink(&external, &repo.join("tasks"));

        let policy = ExploreReadOnlyPolicy::discover(&repo, ExploreSandboxTaskWrites::Allow)
            .expect("policy");
        assert!(
            policy.task_dirs.is_empty(),
            "task dirs must not include symlink targets outside repo: {:?}",
            policy.task_dirs
        );
    }

    #[test]
    fn discover_accepts_task_dir_symlink_inside_repo() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let repo = temp.path().join("repo");
        let real_tasks = repo.join("real-tasks");
        std::fs::create_dir_all(&real_tasks).expect("real tasks");
        std::fs::write(
            real_tasks.join(taskmd_core::constants::TEMPLATE_FILENAME),
            "# Template\n",
        )
        .expect("template");
        make_symlink(&real_tasks, &repo.join("tasks"));

        let policy = ExploreReadOnlyPolicy::discover(&repo, ExploreSandboxTaskWrites::Allow)
            .expect("policy");
        assert_eq!(policy.task_dirs, vec![real_tasks.canonicalize().unwrap()]);
    }

    #[test]
    fn platform_temp_falls_back_under_scratch_when_repo_is_under_temp() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let host_temp = temp.path().join("host-temp");
        let repo = host_temp.join(format!("phoenix-repo-{}", uuid::Uuid::new_v4()));
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&repo).expect("repo");

        assert_eq!(
            platform_temp_dir(
                &repo.canonicalize().unwrap(),
                &scratch,
                &host_temp.join("phoenix-ide")
            ),
            scratch.join("platform-temp")
        );
    }

    #[cfg(unix)]
    fn make_symlink(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).expect("symlink");
    }

    #[cfg(windows)]
    fn make_symlink(target: &Path, link: &Path) {
        std::os::windows::fs::symlink_dir(target, link).expect("symlink");
    }
}
