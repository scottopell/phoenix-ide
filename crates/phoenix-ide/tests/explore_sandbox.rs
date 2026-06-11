use std::process::Command;

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn explore_sandbox_enforces_read_only_policy() {
    if !phoenix_core::platform::PlatformCapability::detect().has_sandbox() {
        eprintln!("skipping: nono sandbox backend is unavailable");
        return;
    }

    let temp = tempfile::TempDir::new().expect("tempdir");
    let repo = temp.path().join("repo");
    let tasks = repo.join("tasks");
    let scratch = temp.path().join("scratch");
    std::fs::create_dir_all(&tasks).expect("tasks dir");
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    std::fs::write(tasks.join("_TEMPLATE.md"), "# Template\n").expect("template");
    std::fs::write(repo.join("file.txt"), "hello\n").expect("source file");

    run_host_git(&repo, &["init", "-q"]);
    run_host_git(&repo, &["config", "user.email", "test@example.com"]);
    run_host_git(&repo, &["config", "user.name", "Test User"]);
    run_host_git(&repo, &["add", "file.txt", "tasks/_TEMPLATE.md"]);
    run_host_git(&repo, &["commit", "-qm", "init"]);

    let bin = env!("CARGO_BIN_EXE_phoenix_ide");

    let read = sandbox_run(
        bin,
        &repo,
        &tasks,
        &scratch,
        "git --no-pager log --oneline -1 && cat file.txt",
    );
    assert!(read.status.success(), "read failed: {read:?}");
    assert!(read.stderr.is_empty() || read.stderr.contains("git: error"));
    assert!(read.stdout.contains("init"), "stdout: {}", read.stdout);
    assert!(read.stdout.contains("hello"), "stdout: {}", read.stdout);

    let denied = sandbox_run(bin, &repo, &tasks, &scratch, "echo bad > file.txt");
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
        &repo,
        &tasks,
        &scratch,
        "echo ok > tasks/34001-p2-ready--sandbox-test.md && cat tasks/34001-p2-ready--sandbox-test.md",
    );
    assert!(
        task_write.status.success(),
        "task write failed: {task_write:?}"
    );
    assert!(task_write.stdout.contains("ok"));

    let scratch_write = sandbox_run(
        bin,
        &repo,
        &tasks,
        &scratch,
        "echo ok > \"$PHOENIX_SANDBOX_SCRATCH/scratch.txt\" && cat \"$PHOENIX_SANDBOX_SCRATCH/scratch.txt\"",
    );
    assert!(
        scratch_write.status.success(),
        "scratch write failed: {scratch_write:?}"
    );
    assert!(scratch_write.stdout.contains("ok"));

    let network = sandbox_run(
        bin,
        &repo,
        &tasks,
        &scratch,
        "exec 3<>/dev/tcp/example.com/443",
    );
    assert!(!network.status.success(), "network unexpectedly succeeded");
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

fn sandbox_run(
    bin: &str,
    repo: &std::path::Path,
    tasks: &std::path::Path,
    scratch: &std::path::Path,
    cmd: &str,
) -> SandboxOutput {
    let output = Command::new(bin)
        .args(["--sandbox-exec", "--", cmd])
        .env("PHOENIX_SANDBOX_REPO_ROOT", repo)
        .env("PHOENIX_SANDBOX_SCRATCH", scratch)
        .env("PHOENIX_SANDBOX_TASK_DIRS", tasks)
        .output()
        .expect("run sandbox child");
    SandboxOutput {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}
