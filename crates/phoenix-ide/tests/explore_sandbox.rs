use std::process::Command;

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[allow(clippy::too_many_lines)]
fn explore_sandbox_enforces_read_only_policy() {
    if !phoenix_core::platform::PlatformCapability::detect().has_sandbox() {
        eprintln!("skipping: nono sandbox backend is unavailable");
        return;
    }

    let fixture_root = std::env::current_dir()
        .expect("cwd")
        .join("target")
        .join("explore-sandbox-tests")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&fixture_root).expect("fixture root");
    let _cleanup = RemoveDirOnDrop(fixture_root.clone());
    let repo = fixture_root.join("repo");
    let tasks = repo.join("tasks");
    let scratch = fixture_root.join("scratch");
    let sandbox_home = scratch.join("home");
    let platform_temp = std::env::temp_dir();
    let outside = fixture_root.join("outside.txt");
    let sensitive = fixture_root.join("sensitive");
    std::fs::create_dir_all(&tasks).expect("tasks dir");
    std::fs::create_dir_all(&sandbox_home).expect("sandbox home");
    std::fs::create_dir_all(&sensitive).expect("sensitive dir");
    std::fs::write(tasks.join("_TEMPLATE.md"), "# Template\n").expect("template");
    std::fs::write(repo.join("file.txt"), "hello\n").expect("source file");
    std::fs::write(&outside, "outside\n").expect("outside file");
    std::fs::write(sensitive.join("secret.txt"), "secret\n").expect("sensitive file");

    run_host_git(&repo, &["init", "-q"]);
    run_host_git(&repo, &["config", "user.email", "test@example.com"]);
    run_host_git(&repo, &["config", "user.name", "Test User"]);
    run_host_git(&repo, &["add", "file.txt", "tasks/_TEMPLATE.md"]);
    run_host_git(&repo, &["commit", "-qm", "init"]);

    let bin = env!("CARGO_BIN_EXE_phoenix_ide");

    let read = sandbox_run(
        bin,
        &SandboxFixture {
            repo: &repo,
            tasks: &tasks,
            scratch: &scratch,
            sandbox_home: &sandbox_home,
            platform_temp: &platform_temp,
            sensitive: &sensitive,
        },
        "git --no-pager log --oneline -1 && git status --short && cat file.txt",
    );
    assert!(read.status.success(), "read failed: {read:?}");
    assert!(read.stderr.is_empty(), "stderr: {}", read.stderr);
    assert!(read.stdout.contains("init"), "stdout: {}", read.stdout);
    assert!(read.stdout.contains("hello"), "stdout: {}", read.stdout);

    let outside_read = sandbox_run(
        bin,
        &SandboxFixture {
            repo: &repo,
            tasks: &tasks,
            scratch: &scratch,
            sandbox_home: &sandbox_home,
            platform_temp: &platform_temp,
            sensitive: &sensitive,
        },
        &format!("cat {}", shell_quote(&outside)),
    );
    assert!(
        outside_read.status.success(),
        "outside read failed: {outside_read:?}"
    );
    assert!(outside_read.stdout.contains("outside"));

    let denied = sandbox_run(
        bin,
        &SandboxFixture {
            repo: &repo,
            tasks: &tasks,
            scratch: &scratch,
            sandbox_home: &sandbox_home,
            platform_temp: &platform_temp,
            sensitive: &sensitive,
        },
        "echo bad > file.txt",
    );
    assert!(
        !denied.status.success(),
        "source write unexpectedly succeeded"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("file.txt")).expect("read host file"),
        "hello\n"
    );

    let task_write = sandbox_run(
        bin,
        &SandboxFixture {
            repo: &repo,
            tasks: &tasks,
            scratch: &scratch,
            sandbox_home: &sandbox_home,
            platform_temp: &platform_temp,
            sensitive: &sensitive,
        },
        "echo ok > tasks/34001-p2-ready--sandbox-test.md && cat tasks/34001-p2-ready--sandbox-test.md",
    );
    assert!(
        task_write.status.success(),
        "task write failed: {task_write:?}"
    );
    assert!(task_write.stdout.contains("ok"));

    let scratch_write = sandbox_run(
        bin,
        &SandboxFixture {
            repo: &repo,
            tasks: &tasks,
            scratch: &scratch,
            sandbox_home: &sandbox_home,
            platform_temp: &platform_temp,
            sensitive: &sensitive,
        },
        "echo ok > \"$PHOENIX_SANDBOX_SCRATCH/scratch.txt\" && cat \"$PHOENIX_SANDBOX_SCRATCH/scratch.txt\"",
    );
    assert!(
        scratch_write.status.success(),
        "scratch write failed: {scratch_write:?}"
    );
    assert!(scratch_write.stdout.contains("ok"));

    let env_probe = sandbox_run(
        bin,
        &SandboxFixture {
            repo: &repo,
            tasks: &tasks,
            scratch: &scratch,
            sandbox_home: &sandbox_home,
            platform_temp: &platform_temp,
            sensitive: &sensitive,
        },
        "printf 'home=%s\npsh=%s\nscratch=%s\ntmp=%s\ngh=%s\n' \"$HOME\" \"$PHOENIX_SANDBOX_HOME\" \"$PHOENIX_SANDBOX_SCRATCH\" \"$TMPDIR\" \"${GH_TOKEN-unset}\"",
    );
    assert!(
        env_probe.status.success(),
        "env probe failed: {env_probe:?}"
    );
    assert!(env_probe
        .stdout
        .contains(&format!("home={}", sandbox_home.display())));
    assert!(env_probe
        .stdout
        .contains(&format!("psh={}", sandbox_home.display())));
    assert!(env_probe
        .stdout
        .contains(&format!("scratch={}", scratch.display())));
    assert!(env_probe
        .stdout
        .contains(&format!("tmp={}", platform_temp.display())));
    assert!(env_probe.stdout.contains("gh=unset"));

    #[cfg(target_os = "macos")]
    {
        let sensitive_read = sandbox_run(
            bin,
            &SandboxFixture {
                repo: &repo,
                tasks: &tasks,
                scratch: &scratch,
                sandbox_home: &sandbox_home,
                platform_temp: &platform_temp,
                sensitive: &sensitive,
            },
            &format!("cat {}", shell_quote(&sensitive.join("secret.txt"))),
        );
        assert!(
            !sensitive_read.status.success(),
            "sensitive read unexpectedly succeeded: {sensitive_read:?}"
        );
    }

    let network = sandbox_run(
        bin,
        &SandboxFixture {
            repo: &repo,
            tasks: &tasks,
            scratch: &scratch,
            sandbox_home: &sandbox_home,
            platform_temp: &platform_temp,
            sensitive: &sensitive,
        },
        "exec 3<>/dev/tcp/example.com/443",
    );
    assert!(!network.status.success(), "network unexpectedly succeeded");
}

struct RemoveDirOnDrop(std::path::PathBuf);

impl Drop for RemoveDirOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_host_git(cwd: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {args:?} failed");
}

#[derive(Debug)]
struct SandboxOutput {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

struct SandboxFixture<'a> {
    repo: &'a std::path::Path,
    tasks: &'a std::path::Path,
    scratch: &'a std::path::Path,
    sandbox_home: &'a std::path::Path,
    platform_temp: &'a std::path::Path,
    sensitive: &'a std::path::Path,
}

fn sandbox_run(bin: &str, fixture: &SandboxFixture<'_>, cmd: &str) -> SandboxOutput {
    let output = Command::new(bin)
        .args(["--sandbox-exec", "--", cmd])
        .env("PHOENIX_SANDBOX_REPO_ROOT", fixture.repo)
        .env("PHOENIX_SANDBOX_SCRATCH", fixture.scratch)
        .env("PHOENIX_SANDBOX_HOME", fixture.sandbox_home)
        .env("PHOENIX_SANDBOX_PLATFORM_TEMP", fixture.platform_temp)
        .env("PHOENIX_SANDBOX_TASK_DIRS", fixture.tasks)
        .env("PHOENIX_SANDBOX_SENSITIVE_DIRS", fixture.sensitive)
        .env("GH_TOKEN", "should-not-leak")
        .output()
        .expect("run sandbox child");
    SandboxOutput {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn shell_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}
