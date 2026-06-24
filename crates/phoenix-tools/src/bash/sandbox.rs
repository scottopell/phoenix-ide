use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use nono::{AccessMode, CapabilitySet, Sandbox};
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;

const REPO_ROOT_ENV: &str = "PHOENIX_SANDBOX_REPO_ROOT";
const SCRATCH_ENV: &str = "PHOENIX_SANDBOX_SCRATCH";
const HOME_ENV: &str = "PHOENIX_SANDBOX_HOME";
const PLATFORM_TEMP_ENV: &str = "PHOENIX_SANDBOX_PLATFORM_TEMP";

#[derive(Debug, Clone)]
pub struct ExploreReadOnlyPolicy {
    repo_root: PathBuf,
    scratch_dir: PathBuf,
    sandbox_home: PathBuf,
    platform_temp: PathBuf,
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
        let runtime_env = PhoenixRuntimeEnvironment::detect();
        let repo_root = working_dir.canonicalize()?;
        let git_dirs = git_state_dirs(&repo_root);
        let mut protected_dirs = Vec::with_capacity(5 + git_dirs.len());
        protected_dirs.push(repo_root.clone());
        protected_dirs.extend(git_dirs);
        protected_dirs.extend(runtime_protected_dirs(&runtime_env));
        protected_dirs.sort();
        protected_dirs.dedup();
        let scratch_root = runtime_env.tmp_subdir("explore-bash")?;
        let scratch_dir = scratch_dir(&scratch_root, &protected_dirs)?;
        let sandbox_home = scratch_dir.join("home");
        std::fs::create_dir_all(&sandbox_home)?;
        let platform_temp =
            platform_temp_dir(&protected_dirs, &scratch_dir, runtime_env.tmp_root());
        std::fs::create_dir_all(&platform_temp)?;
        let path = inherited_path();
        Ok(Self {
            repo_root,
            scratch_dir,
            sandbox_home,
            platform_temp,
            path,
        })
    }

    fn to_command_env(&self, command: &mut Command) {
        command.env(REPO_ROOT_ENV, &self.repo_root);
        command.env(SCRATCH_ENV, &self.scratch_dir);
        command.env(HOME_ENV, &self.sandbox_home);
        command.env(PLATFORM_TEMP_ENV, &self.platform_temp);
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
        let path = inherited_path();
        Ok(Self {
            repo_root: repo_root.clone(),
            scratch_dir,
            sandbox_home,
            platform_temp,
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
    pub fn command(cmd: &str, working_dir: &Path) -> Result<ExploreSandboxCommand, String> {
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

fn inherited_path() -> OsString {
    std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/usr/bin:/bin:/usr/sbin:/sbin"))
}

fn scratch_dir(scratch_root: &Path, protected_dirs: &[PathBuf]) -> std::io::Result<PathBuf> {
    let scratch_root = scratch_root
        .canonicalize()
        .unwrap_or_else(|_| scratch_root.to_path_buf());
    if protected_dirs
        .iter()
        .any(|protected| paths_overlap(&scratch_root, protected))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "explore bash scratch root {} overlaps protected repository/Git/Phoenix state",
                scratch_root.display()
            ),
        ));
    }
    Ok(scratch_root.join(uuid::Uuid::new_v4().to_string()))
}

fn platform_temp_dir(protected_dirs: &[PathBuf], scratch_dir: &Path, tmp_root: &Path) -> PathBuf {
    let temp = tmp_root.parent().unwrap_or(tmp_root);
    let temp = temp.canonicalize().unwrap_or_else(|_| temp.to_path_buf());
    if protected_dirs
        .iter()
        .any(|protected| paths_overlap(&temp, protected))
    {
        scratch_dir.join("platform-temp")
    } else {
        temp
    }
}

fn paths_overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn runtime_protected_dirs(runtime_env: &PhoenixRuntimeEnvironment) -> Vec<PathBuf> {
    let mut dirs = vec![
        runtime_env.home().to_path_buf(),
        runtime_env.codex_home().to_path_buf(),
        runtime_env.data_dir().to_path_buf(),
        runtime_env.phoenix_home(),
    ];
    if let Some(parent) = runtime_env.db_path().parent() {
        dirs.push(parent.to_path_buf());
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

fn git_state_dirs(repo_root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for arg in ["--absolute-git-dir", "--git-common-dir"] {
        if let Some(path) = git_rev_parse_path(repo_root, arg) {
            dirs.push(path);
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

fn git_rev_parse_path(repo_root: &Path, arg: &str) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", arg])
        .current_dir(repo_root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    let absolute = if path.is_absolute() {
        path
    } else {
        repo_root.join(path)
    };
    absolute.canonicalize().ok()
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
    fn platform_temp_falls_back_under_scratch_when_repo_is_under_temp() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let host_temp = temp.path().join("host-temp");
        let repo = host_temp.join(format!("phoenix-repo-{}", uuid::Uuid::new_v4()));
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&repo).expect("repo");

        assert_eq!(
            platform_temp_dir(
                &[repo.canonicalize().unwrap()],
                &scratch,
                &host_temp.join("phoenix-ide")
            ),
            scratch.join("platform-temp")
        );
    }

    #[test]
    fn platform_temp_falls_back_under_scratch_when_git_dir_is_under_temp() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let repo = temp.path().join("repo");
        let host_temp = temp.path().join("host-temp");
        let git_dir = host_temp.join("git-dir");
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&repo).expect("repo");
        std::fs::create_dir_all(&git_dir).expect("git dir");

        assert_eq!(
            platform_temp_dir(
                &[
                    repo.canonicalize().unwrap(),
                    git_dir.canonicalize().unwrap()
                ],
                &scratch,
                &host_temp.join("phoenix-ide")
            ),
            scratch.join("platform-temp")
        );
    }

    #[test]
    fn platform_temp_falls_back_under_scratch_when_runtime_state_is_under_temp() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let host_temp = temp.path().join("host-temp");
        let phoenix_data = host_temp.join("phoenix-data");
        let scratch = temp.path().join("scratch");
        std::fs::create_dir_all(&phoenix_data).expect("phoenix data");

        assert_eq!(
            platform_temp_dir(
                &[phoenix_data.canonicalize().unwrap()],
                &scratch,
                &host_temp.join("phoenix-ide")
            ),
            scratch.join("platform-temp")
        );
    }

    #[test]
    fn scratch_dir_rejects_root_inside_protected_repo() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let repo = temp.path().join("repo");
        let scratch_root = repo.join(".tmp").join("phoenix-ide").join("explore-bash");
        std::fs::create_dir_all(&scratch_root).expect("scratch root");

        let err = scratch_dir(&scratch_root, &[repo.canonicalize().unwrap()]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    }
}
