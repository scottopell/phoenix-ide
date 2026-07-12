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
/// Valid inherited `GIT_CONFIG_*` entries are preserved. Additional entries are
/// appended, then `commit.gpgsign=false` is appended last so no inherited or
/// caller-provided entry can re-enable interactive signing. Malformed inherited
/// configuration is discarded as a unit rather than passed through to Git.
#[must_use]
pub fn command_with_config(config: &[(&str, &str)]) -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command.env_remove("GIT_CONFIG_PARAMETERS");
    let inherited = inherited_config_count();

    if inherited.is_none() {
        clear_inherited_config(&mut command);
    }

    let mut index = inherited.unwrap_or(0);
    for &(key, value) in config
        .iter()
        .chain(std::iter::once(&("commit.gpgsign", "false")))
    {
        command
            .env(format!("GIT_CONFIG_KEY_{index}"), key)
            .env(format!("GIT_CONFIG_VALUE_{index}"), value);
        index += 1;
    }
    command.env("GIT_CONFIG_COUNT", index.to_string());
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
    fn command_removes_higher_precedence_config_parameters() {
        let command = command();
        assert!(command
            .get_envs()
            .any(|(key, value)| { key == "GIT_CONFIG_PARAMETERS" && value.is_none() }));
    }

    #[test]
    fn additional_config_is_preserved_and_signing_override_has_final_precedence() {
        let command = command_with_config(&[("fetch.prune", "true"), ("commit.gpgsign", "true")]);
        let environment = command
            .get_envs()
            .filter_map(|(key, value)| Some((key.to_str()?, value?.to_str()?)))
            .collect::<std::collections::HashMap<_, _>>();

        let count = environment["GIT_CONFIG_COUNT"].parse::<usize>().unwrap();
        assert!(count >= 3);
        assert_eq!(
            environment[format!("GIT_CONFIG_KEY_{}", count - 3).as_str()],
            "fetch.prune"
        );
        assert_eq!(
            environment[format!("GIT_CONFIG_VALUE_{}", count - 3).as_str()],
            "true"
        );
        assert_eq!(
            environment[format!("GIT_CONFIG_KEY_{}", count - 2).as_str()],
            "commit.gpgsign"
        );
        assert_eq!(
            environment[format!("GIT_CONFIG_VALUE_{}", count - 2).as_str()],
            "true"
        );
        assert_eq!(
            environment[format!("GIT_CONFIG_KEY_{}", count - 1).as_str()],
            "commit.gpgsign"
        );
        assert_eq!(
            environment[format!("GIT_CONFIG_VALUE_{}", count - 1).as_str()],
            "false"
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
