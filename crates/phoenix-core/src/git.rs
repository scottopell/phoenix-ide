//! Host git interrogation helpers.
//!
//! Thin wrappers over the `git` CLI for repository-root and default-branch
//! resolution. These are host-interaction utilities (like [`crate::platform`]),
//! not domain types — they shell out and inspect the working copy. They live in
//! `phoenix-core` because both the persistence layer and the runtime need them.

use std::path::Path;

const NONINTERACTIVE_GIT_CONFIG: &[(&str, &str)] = &[
    ("commit.gpgsign", "false"),
    ("tag.gpgSign", "false"),
    ("core.editor", "true"),
    ("core.pager", "cat"),
];

const NONINTERACTIVE_GIT_ENV: &[(&str, &str)] = &[
    ("GIT_EDITOR", "true"),
    ("VISUAL", "true"),
    ("EDITOR", "true"),
    ("GIT_PAGER", "cat"),
    ("PAGER", "cat"),
    ("GIT_TERMINAL_PROMPT", "0"),
];

/// Construct a Git subprocess with Phoenix's process-level safety defaults.
///
/// The environment and command-line configuration overrides take precedence over
/// system, global, local, and inherited process configuration without modifying
/// any of them. Every statically spawned Git process in Phoenix goes through this
/// constructor so repository setup in tests and production Git probes cannot
/// invoke an interactive editor, pager, terminal prompt, or signing agent.
#[must_use]
pub fn command() -> std::process::Command {
    command_with_config(&[])
}

/// Construct a safe Git subprocess with additional process-level configuration.
///
/// Valid inherited `GIT_CONFIG_*` entries are preserved. Additional entries are
/// applied before Phoenix's noninteractive defaults as command-line `-c`
/// arguments, which take precedence over inherited `GIT_CONFIG_PARAMETERS`
/// without discarding unrelated parameters. Malformed inherited indexed
/// configuration is discarded as a unit.
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
    for &(key, value) in NONINTERACTIVE_GIT_ENV {
        command.env(key, value);
    }

    for &(key, value) in config.iter().chain(NONINTERACTIVE_GIT_CONFIG.iter()) {
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

const LOCAL_HEAD_MAX_ATTEMPTS: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalHeadQuery {
    SymbolicHead,
    RefCommit(String),
    HeadCommit,
    VerifyHead,
}

fn resolve_local_git_head(
    repository_identity: String,
    mut read: impl FnMut(LocalHeadQuery) -> Option<String>,
) -> LocalGitHeadObservation {
    let mut known_unborn_branch_name = None;
    for _ in 0..LOCAL_HEAD_MAX_ATTEMPTS {
        if let Some(full_ref) = read(LocalHeadQuery::SymbolicHead).filter(|name| !name.is_empty()) {
            let branch_name = full_ref
                .strip_prefix("refs/heads/")
                .unwrap_or(&full_ref)
                .to_string();
            known_unborn_branch_name = Some(branch_name.clone());
            let ref_commit = read(LocalHeadQuery::RefCommit(full_ref.clone()));
            if read(LocalHeadQuery::SymbolicHead).as_deref() != Some(full_ref.as_str()) {
                continue;
            }
            if let Some(head_oid) = ref_commit.filter(|oid| !oid.is_empty()) {
                return LocalGitHeadObservation::NamedBranch {
                    repository_identity,
                    branch_name,
                    head_oid,
                };
            }
            if read(LocalHeadQuery::VerifyHead).is_none() {
                return LocalGitHeadObservation::Unborn {
                    repository_identity,
                    branch_name: Some(branch_name),
                };
            }
            continue;
        }

        let first_oid = read(LocalHeadQuery::HeadCommit);
        let second_oid = read(LocalHeadQuery::HeadCommit);
        if read(LocalHeadQuery::SymbolicHead).is_none() {
            if let Some(head_oid) = first_oid.filter(|oid| Some(oid) == second_oid.as_ref()) {
                return LocalGitHeadObservation::Detached {
                    repository_identity,
                    head_oid,
                };
            }
        }
    }

    if read(LocalHeadQuery::VerifyHead).is_none() {
        return LocalGitHeadObservation::Unborn {
            repository_identity,
            branch_name: known_unborn_branch_name,
        };
    }

    LocalGitHeadObservation::Unavailable {
        repository_identity: Some(repository_identity),
        error: "unable to resolve HEAD state".to_string(),
    }
}

/// Observe the authoritative local Git HEAD state for a repository worktree.
///
/// Distinguishes a named branch from detached HEAD, unborn HEAD, and
/// unavailable/error states without parsing shell command intent.
#[must_use]
pub fn observe_local_git_head(path: &Path) -> LocalGitHeadObservation {
    let Some(repository_identity) = detect_git_repo_root(path) else {
        return LocalGitHeadObservation::Unavailable {
            repository_identity: None,
            error: "not a git repository".to_string(),
        };
    };

    resolve_local_git_head(repository_identity, |query| match query {
        LocalHeadQuery::SymbolicHead => git_capture(path, &["symbolic-ref", "--quiet", "HEAD"]),
        LocalHeadQuery::RefCommit(full_ref) => {
            git_capture(path, &["rev-parse", &format!("{full_ref}^{{commit}}")])
        }
        LocalHeadQuery::HeadCommit => git_capture(path, &["rev-parse", "HEAD^{commit}"]),
        LocalHeadQuery::VerifyHead => git_capture(path, &["rev-parse", "--verify", "HEAD"]),
    })
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

    fn noninteractive_config_args() -> Vec<std::ffi::OsString> {
        NONINTERACTIVE_GIT_CONFIG
            .iter()
            .flat_map(|(key, value)| ["-c".into(), format!("{key}={value}").into()])
            .collect()
    }

    fn command_sets_env(command: &std::process::Command, name: &str, value: &str) -> bool {
        command
            .get_envs()
            .any(|(key, env_value)| key == name && env_value == Some(std::ffi::OsStr::new(value)))
    }

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
    fn command_preserves_config_parameters_and_applies_noninteractive_config_on_command_line() {
        let command = command();
        assert!(!command
            .get_envs()
            .any(|(key, _)| key == "GIT_CONFIG_PARAMETERS"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            noninteractive_config_args()
        );
    }

    #[test]
    fn additional_config_is_preserved_and_signing_override_has_final_precedence() {
        let command = command_with_config(&[("fetch.prune", "true"), ("commit.gpgsign", "true")]);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "-c".into(),
                "fetch.prune=true".into(),
                "-c".into(),
                "commit.gpgsign=true".into(),
            ]
            .into_iter()
            .chain(noninteractive_config_args())
            .collect::<Vec<std::ffi::OsString>>()
        );
    }

    #[test]
    fn command_overrides_interactive_editor_pager_and_terminal_prompt_environment() {
        let command = command();
        for &(name, value) in NONINTERACTIVE_GIT_ENV {
            assert!(
                command_sets_env(&command, name, value),
                "expected {name}={value} override"
            );
        }
    }

    #[test]
    fn command_preserves_askpass_credential_helpers() {
        let command = command();
        assert!(!command
            .get_envs()
            .any(|(key, value)| (key == "GIT_ASKPASS" || key == "SSH_ASKPASS") && value.is_none()));
    }

    #[test]
    fn command_disables_hostile_local_editor_configuration() {
        let repo = tempfile::tempdir().expect("tempdir");
        let marker = repo.path().join("editor-ran");
        let hostile_editor = format!("sh -c 'echo editor-ran > {}'", marker.display());
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
        run(&["config", "core.editor", &hostile_editor]);
        let output = command()
            .args(["commit", "--allow-empty", "--quiet"])
            .current_dir(repo.path())
            .output()
            .expect("git runs");

        assert!(
            !output.status.success(),
            "empty message commit should abort"
        );
        assert!(!marker.exists(), "git invoked a repository-local editor");
    }

    #[test]
    fn command_disables_hostile_local_tag_signing_configuration() {
        let repo = tempfile::tempdir().expect("tempdir");
        let marker = repo.path().join("editor-ran");
        let hostile_editor = format!("sh -c 'echo editor-ran > {}'", marker.display());
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
        run(&["commit", "--allow-empty", "--quiet", "-m", "base"]);
        run(&["config", "core.editor", &hostile_editor]);
        run(&["config", "tag.gpgSign", "true"]);
        run(&["tag", "v1"]);

        assert!(!marker.exists(), "git invoked an editor while tagging");
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
    use std::collections::VecDeque;

    fn resolve_script(
        script: impl IntoIterator<Item = (LocalHeadQuery, Option<&'static str>)>,
    ) -> LocalGitHeadObservation {
        let mut script: VecDeque<_> = script.into_iter().collect();
        let observed = resolve_local_git_head("repo".to_string(), |query| {
            let (expected, value) = script.pop_front().expect("unexpected HEAD query");
            assert_eq!(expected, query);
            value.map(str::to_string)
        });
        assert!(script.is_empty(), "unconsumed HEAD queries: {script:?}");
        observed
    }

    fn named_attempt(
        first_ref: &'static str,
        oid: Option<&'static str>,
        second_ref: Option<&'static str>,
    ) -> [(LocalHeadQuery, Option<&'static str>); 3] {
        [
            (LocalHeadQuery::SymbolicHead, Some(first_ref)),
            (LocalHeadQuery::RefCommit(first_ref.to_string()), oid),
            (LocalHeadQuery::SymbolicHead, second_ref),
        ]
    }

    fn detached_attempt(
        first_oid: Option<&'static str>,
        second_oid: Option<&'static str>,
        symbolic_head: Option<&'static str>,
    ) -> [(LocalHeadQuery, Option<&'static str>); 4] {
        [
            (LocalHeadQuery::SymbolicHead, None),
            (LocalHeadQuery::HeadCommit, first_oid),
            (LocalHeadQuery::HeadCommit, second_oid),
            (LocalHeadQuery::SymbolicHead, symbolic_head),
        ]
    }

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
    fn deterministic_snapshot_accepts_stable_named_branch() {
        assert_eq!(
            resolve_script(named_attempt(
                "refs/heads/main",
                Some("main-oid"),
                Some("refs/heads/main")
            )),
            LocalGitHeadObservation::NamedBranch {
                repository_identity: "repo".to_string(),
                branch_name: "main".to_string(),
                head_oid: "main-oid".to_string(),
            }
        );
    }

    #[test]
    fn deterministic_snapshot_retries_branch_change_without_cross_pairing() {
        let script = named_attempt(
            "refs/heads/main",
            Some("main-oid"),
            Some("refs/heads/feature"),
        )
        .into_iter()
        .chain(named_attempt(
            "refs/heads/feature",
            Some("feature-oid"),
            Some("refs/heads/feature"),
        ));

        assert_eq!(
            resolve_script(script),
            LocalGitHeadObservation::NamedBranch {
                repository_identity: "repo".to_string(),
                branch_name: "feature".to_string(),
                head_oid: "feature-oid".to_string(),
            }
        );
    }

    #[test]
    fn deterministic_snapshot_accepts_only_stable_detached_oid() {
        assert_eq!(
            resolve_script(detached_attempt(Some("oid"), Some("oid"), None)),
            LocalGitHeadObservation::Detached {
                repository_identity: "repo".to_string(),
                head_oid: "oid".to_string(),
            }
        );
    }

    #[test]
    fn deterministic_snapshot_recognizes_unborn_named_head() {
        let script = named_attempt("refs/heads/topic", None, Some("refs/heads/topic"))
            .into_iter()
            .chain([(LocalHeadQuery::VerifyHead, None)]);
        assert_eq!(
            resolve_script(script),
            LocalGitHeadObservation::Unborn {
                repository_identity: "repo".to_string(),
                branch_name: Some("topic".to_string()),
            }
        );
    }

    #[test]
    fn deterministic_snapshot_retries_transient_failures() {
        let script = detached_attempt(None, None, None)
            .into_iter()
            .chain(named_attempt(
                "refs/heads/main",
                Some("oid"),
                Some("refs/heads/main"),
            ));
        assert_eq!(
            resolve_script(script),
            LocalGitHeadObservation::NamedBranch {
                repository_identity: "repo".to_string(),
                branch_name: "main".to_string(),
                head_oid: "oid".to_string(),
            }
        );
    }

    #[test]
    fn deterministic_snapshot_exhausts_six_attempts_before_unavailable() {
        let script = (0..LOCAL_HEAD_MAX_ATTEMPTS)
            .flat_map(|_| detached_attempt(Some("old"), Some("new"), None))
            .chain([(LocalHeadQuery::VerifyHead, Some("new"))]);
        assert_eq!(
            resolve_script(script),
            LocalGitHeadObservation::Unavailable {
                repository_identity: Some("repo".to_string()),
                error: "unable to resolve HEAD state".to_string(),
            }
        );
    }

    #[test]
    fn deterministic_snapshot_preserves_known_branch_after_retry_exhaustion_to_unborn() {
        let first = named_attempt("refs/heads/topic", Some("oid"), Some("refs/heads/other"));
        let rest = (1..LOCAL_HEAD_MAX_ATTEMPTS).flat_map(|_| detached_attempt(None, None, None));
        let script = first
            .into_iter()
            .chain(rest)
            .chain([(LocalHeadQuery::VerifyHead, None)]);
        assert_eq!(
            resolve_script(script),
            LocalGitHeadObservation::Unborn {
                repository_identity: "repo".to_string(),
                branch_name: Some("topic".to_string()),
            }
        );
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
                branch_name: Some("main".to_string()),
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

    #[test]
    fn observe_local_git_head_preserves_known_unborn_branch_name() {
        let repo = temp_dir();
        git(repo.path(), &["init"]);
        git(repo.path(), &["symbolic-ref", "HEAD", "refs/heads/topic"]);

        let observed = observe_local_git_head(repo.path());
        assert_eq!(
            observed,
            LocalGitHeadObservation::Unborn {
                repository_identity: std::fs::canonicalize(repo.path())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                branch_name: Some("topic".to_string()),
            }
        );
    }

    #[test]
    fn observe_local_git_head_eventually_reports_consistent_snapshot_during_checkout() {
        let repo = init_repo();
        commit_file(repo.path(), "base.txt", "base");
        git(repo.path(), &["checkout", "-b", "feature"]);
        let feature_oid = commit_file(repo.path(), "feature.txt", "feature");
        git(repo.path(), &["checkout", "main"]);
        let main_oid = git_out(repo.path(), &["rev-parse", "HEAD^{commit}"]);

        std::thread::scope(|scope| {
            let start = std::sync::Arc::new(std::sync::Barrier::new(2));
            let checkout_start = std::sync::Arc::clone(&start);
            let repo_path = repo.path();
            let handle = scope.spawn(move || {
                checkout_start.wait();
                for _ in 0..8 {
                    git(repo_path, &["checkout", "feature"]);
                    git(repo_path, &["checkout", "main"]);
                }
            });

            start.wait();
            let mut authoritative_snapshots = 0;
            for _ in 0..20 {
                match observe_local_git_head(repo.path()) {
                    LocalGitHeadObservation::NamedBranch {
                        branch_name,
                        head_oid,
                        ..
                    } => {
                        authoritative_snapshots += 1;
                        if branch_name == "main" {
                            assert_eq!(head_oid, main_oid);
                        } else if branch_name == "feature" {
                            assert_eq!(head_oid, feature_oid);
                        } else {
                            panic!("unexpected branch snapshot: {branch_name} {head_oid}");
                        }
                    }
                    LocalGitHeadObservation::Detached { head_oid, .. } => {
                        authoritative_snapshots += 1;
                        assert!(head_oid == main_oid || head_oid == feature_oid);
                    }
                    LocalGitHeadObservation::Unborn { .. }
                    | LocalGitHeadObservation::Unavailable { .. } => {}
                }
            }

            assert!(
                authoritative_snapshots > 0,
                "concurrent real-Git wiring produced no authoritative snapshot"
            );
            handle.join().unwrap();
        });
    }
}
