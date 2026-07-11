//! Host git interrogation helpers.
//!
//! Thin wrappers over the `git` CLI for repository-root and default-branch
//! resolution. These are host-interaction utilities (like [`crate::platform`]),
//! not domain types — they shell out and inspect the working copy. They live in
//! `phoenix-core` because both the persistence layer and the runtime need them.

use std::path::Path;

/// Construct a Git subprocess with Phoenix's process-level safety defaults.
///
/// The environment override takes precedence over system, global, and local
/// Git configuration without modifying any of them. Every statically spawned
/// Git process in Phoenix goes through this constructor so even repository
/// setup in tests cannot invoke an interactive signing agent.
#[must_use]
pub fn command() -> std::process::Command {
    command_with_config(&[])
}

/// Construct a safe Git subprocess with additional process-level configuration.
///
/// Valid inherited `GIT_CONFIG_*` entries are preserved. Additional entries and
/// `commit.gpgsign=false` are applied as command-line `-c` arguments, which take
/// precedence over inherited `GIT_CONFIG_PARAMETERS` without discarding unrelated
/// parameters. Malformed inherited indexed configuration is discarded as a unit.
#[must_use]
pub fn command_with_config(config: &[(&str, &str)]) -> std::process::Command {
    let mut command = std::process::Command::new("git");
    let inherited = inherited_config_count();

    if inherited.is_none() {
        clear_inherited_config(&mut command);
    }

    if let Some(count) = inherited.filter(|count| *count > 0) {
        command.env("GIT_CONFIG_COUNT", count.to_string());
    }
    for &(key, value) in config
        .iter()
        .chain(std::iter::once(&("commit.gpgsign", "false")))
    {
        command.arg("-c").arg(format!("{key}={value}"));
    }
    command
}

fn inherited_config_count() -> Option<usize> {
    let Some(raw_count) = std::env::var_os("GIT_CONFIG_COUNT") else {
        return Some(0);
    };
    let count = raw_count.to_str()?.parse::<usize>().ok()?;
    (0..count)
        .all(|index| {
            std::env::var(format!("GIT_CONFIG_KEY_{index}"))
                .ok()
                .is_some_and(|key| is_valid_config_key(&key))
                && std::env::var_os(format!("GIT_CONFIG_VALUE_{index}")).is_some()
        })
        .then_some(count)
}

fn is_valid_config_key(key: &str) -> bool {
    let Some(first_dot) = key.find('.') else {
        return false;
    };
    let Some(last_dot) = key.rfind('.') else {
        return false;
    };
    let Some(section) = key.get(..first_dot) else {
        return false;
    };
    let Some(name) = key.get(last_dot + 1..) else {
        return false;
    };
    let valid_component = |component: &str| {
        !component.is_empty()
            && component
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
    };

    section
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
        && valid_component(section)
        && name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && valid_component(name)
        && !key.contains(['\0', '\n'])
}

fn clear_inherited_config(command: &mut std::process::Command) {
    command.env_remove("GIT_CONFIG_COUNT");
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(|name| {
            name.starts_with("GIT_CONFIG_KEY_") || name.starts_with("GIT_CONFIG_VALUE_")
        }) {
            command.env_remove(name);
        }
    }
}

use crate::domain::observed_branch::LocalGitHeadObservation;

/// Detect the git repository root for a given directory path.
///
/// Returns `None` if the path is not inside a git repository.
#[must_use]
pub fn detect_git_repo_root(path: &Path) -> Option<String> {
    let output = command()
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(path)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Resolve a repository's default branch — the project's `main_ref` and the
/// canonical fork base (REQ-PROJ-034a, Allium `GitDirectoryDetected`).
///
/// The remote's default branch (cached `refs/remotes/origin/HEAD`, no network)
/// when detectable, else the repository's checked-out branch. Returns `None`
/// when neither can be determined (e.g. `path` is not a git repository, or HEAD
/// is detached with no remote default) so the caller can decide its own
/// fallback.
#[must_use]
pub fn resolve_default_branch(path: &Path) -> Option<String> {
    if let Some(remote) = resolve_remote_default_branch(path) {
        return Some(remote);
    }

    // Else the checked-out branch. A detached HEAD reports the literal "HEAD",
    // which is not a real branch name.
    match git_capture(path, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Some(current) if current != "HEAD" && !current.is_empty() => Some(current),
        _ => None,
    }
}

/// Resolve a repository's *remote* default branch — the authoritative fork base
/// signal for `main_ref` reconciliation (REQ-PROJ-034a).
///
/// Reads the cached `refs/remotes/origin/HEAD` symbolic ref (no network) and
/// strips the `refs/remotes/origin/` prefix. Returns `None` when there is no
/// remote HEAD (e.g. a local/no-origin repository). Unlike
/// `resolve_default_branch`, there is deliberately no current-branch fallback:
/// the current checkout is not an authoritative default and must never be used
/// to overwrite an immutable stored `main_ref`.
#[must_use]
pub fn resolve_remote_default_branch(path: &Path) -> Option<String> {
    let remote = git_capture(path, &["symbolic-ref", "refs/remotes/origin/HEAD"])?;
    let branch = remote.strip_prefix("refs/remotes/origin/")?;
    if branch.is_empty() {
        return None;
    }
    Some(branch.to_string())
}

/// Observe the authoritative local Git HEAD state for a repository worktree.
///
/// Distinguishes a named branch from detached HEAD, unborn HEAD, and
/// unavailable/error states without parsing shell command intent.
#[must_use]
pub fn observe_local_git_head(path: &Path) -> LocalGitHeadObservation {
    let repo_root = detect_git_repo_root(path);
    let Some(repository_identity) = repo_root else {
        return LocalGitHeadObservation::Unavailable {
            repository_identity: None,
            error: "not a git repository".to_string(),
        };
    };

    match git_capture(path, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Some(branch_name) if !branch_name.is_empty() => {
            if let Some(head_oid) =
                git_capture(path, &["rev-parse", "HEAD^{commit}"]).filter(|oid| !oid.is_empty())
            {
                return LocalGitHeadObservation::NamedBranch {
                    repository_identity,
                    branch_name,
                    head_oid,
                };
            }
        }
        _ => {}
    }

    if let Some(head_oid) = git_capture(path, &["rev-parse", "HEAD^{commit}"]) {
        if !head_oid.is_empty() {
            return LocalGitHeadObservation::Detached {
                repository_identity,
                head_oid,
            };
        }
    }

    let branch_name = git_capture(path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .filter(|name| !name.is_empty() && name != "HEAD");
    let head_exists = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(path)
        .output()
        .ok()
        .is_some_and(|output| output.status.success());
    if !head_exists {
        return LocalGitHeadObservation::Unborn {
            repository_identity,
            branch_name,
        };
    }

    LocalGitHeadObservation::Unavailable {
        repository_identity: Some(repository_identity),
        error: "unable to resolve HEAD state".to_string(),
    }
}

/// Run `git <args>` in `path`, returning trimmed stdout on success.
fn git_capture(path: &Path, args: &[&str]) -> Option<String> {
    let output = command().args(args).current_dir(path).output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;

    #[test]
    fn config_key_validation_rejects_keys_git_cannot_parse() {
        assert!(is_valid_config_key("commit.gpgsign"));
        assert!(is_valid_config_key("http.https://example.com.proxy"));
        assert!(!is_valid_config_key("bad key"));
        assert!(!is_valid_config_key("nosection"));
        assert!(!is_valid_config_key("section.9name"));
        assert!(!is_valid_config_key("section.name\nother.value"));
    }

    #[test]
    fn command_preserves_config_parameters_and_overrides_signing_on_command_line() {
        let command = command();
        assert!(!command
            .get_envs()
            .any(|(key, _)| key == "GIT_CONFIG_PARAMETERS"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["-c", "commit.gpgsign=false"]
        );
    }

    #[test]
    fn additional_config_is_preserved_and_signing_override_has_final_precedence() {
        let command = command_with_config(&[("fetch.prune", "true"), ("commit.gpgsign", "true")]);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "-c",
                "fetch.prune=true",
                "-c",
                "commit.gpgsign=true",
                "-c",
                "commit.gpgsign=false",
            ]
        );
    }

    #[test]
    fn command_disables_hostile_commit_signing_configuration() {
        let repo = tempfile::tempdir().expect("tempdir");
        let run = |args: &[&str]| {
            let output = command()
                .args(args)
                .current_dir(repo.path())
                .output()
                .expect("git runs");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };

        run(&["init", "--quiet"]);
        run(&["config", "user.email", "test@phoenix"]);
        run(&["config", "user.name", "Phoenix Test"]);
        run(&["config", "commit.gpgsign", "true"]);
        run(&["config", "gpg.format", "ssh"]);
        run(&[
            "config",
            "gpg.ssh.program",
            "signing-program-must-never-run",
        ]);
        run(&["commit", "--allow-empty", "--quiet", "-m", "unsigned"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(path: &std::path::Path, args: &[&str]) {
        let status = command()
            .args(args)
            .current_dir(path)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    fn git_out(path: &std::path::Path, args: &[&str]) -> String {
        let output = command()
            .args(args)
            .current_dir(path)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = temp_dir();
        git(dir.path(), &["init", "-b", "main"]);
        git(dir.path(), &["config", "user.name", "Phoenix Test"]);
        git(dir.path(), &["config", "user.email", "phoenix@example.com"]);
        dir
    }

    fn commit_file(path: &std::path::Path, name: &str, body: &str) -> String {
        std::fs::write(path.join(name), body).expect("write file");
        git(path, &["add", name]);
        git(path, &["commit", "-m", "commit"]);
        git_out(path, &["rev-parse", "HEAD^{commit}"])
    }

    #[test]
    fn observe_local_git_head_reports_named_branch_and_head_oid() {
        let repo = init_repo();
        let head_oid = commit_file(repo.path(), "f.txt", "hi");

        let observed = observe_local_git_head(repo.path());
        assert_eq!(
            observed,
            LocalGitHeadObservation::NamedBranch {
                repository_identity: std::fs::canonicalize(repo.path())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                branch_name: "main".to_string(),
                head_oid,
            }
        );
    }

    #[test]
    fn observe_local_git_head_reports_detached_head() {
        let repo = init_repo();
        let head_oid = commit_file(repo.path(), "f.txt", "hi");
        git(repo.path(), &["checkout", "--detach", "HEAD"]);

        let observed = observe_local_git_head(repo.path());
        assert_eq!(
            observed,
            LocalGitHeadObservation::Detached {
                repository_identity: std::fs::canonicalize(repo.path())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                head_oid,
            }
        );
    }

    #[test]
    fn observe_local_git_head_reports_unborn_head() {
        let repo = init_repo();

        let observed = observe_local_git_head(repo.path());
        assert_eq!(
            observed,
            LocalGitHeadObservation::Unborn {
                repository_identity: std::fs::canonicalize(repo.path())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                branch_name: None,
            }
        );
    }

    #[test]
    fn observe_local_git_head_reports_unavailable_for_non_repo() {
        let dir = temp_dir();

        let observed = observe_local_git_head(dir.path());
        assert_eq!(
            observed,
            LocalGitHeadObservation::Unavailable {
                repository_identity: None,
                error: "not a git repository".to_string(),
            }
        );
    }
}
