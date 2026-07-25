use async_trait::async_trait;
use phoenix_core::work_scope::{ResourceAuthority, ResourceScopeKey, WorkScopeId};
use phoenix_db::Database;
use phoenix_tools::{Tool, ToolContext, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_FILE_BYTES: usize = 128 * 1024;
const MAX_LOG_RESULTS: u16 = 100;
const MAX_SEARCH_RESULTS: u16 = 200;

#[derive(Debug, Deserialize)]
struct InspectInput {
    work_scope_id: String,
    #[serde(flatten)]
    operation: Operation,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    ResolveTarget,
    Status,
    Log {
        reference: String,
        limit: Option<u16>,
    },
    Diff {
        base: String,
        head: String,
        #[serde(default)]
        paths: Vec<String>,
        #[serde(default)]
        name_only: bool,
    },
    ReadFile {
        reference: String,
        path: String,
        start_line: Option<u32>,
        line_count: Option<u32>,
    },
    Search {
        reference: String,
        query: String,
        path_prefix: Option<String>,
        limit: Option<u16>,
    },
}

#[derive(Debug, Serialize)]
struct Evidence {
    work_scope_id: String,
    repository_root: String,
    operation: &'static str,
    resolved_commits: Vec<String>,
    exit_status: i32,
    truncated: bool,
    output: String,
}

#[derive(Debug, Clone)]
struct Target {
    scope: WorkScopeId,
    root: PathBuf,
}

#[derive(Clone)]
pub(crate) struct RepositoryInspectionTool {
    db: Database,
}

impl RepositoryInspectionTool {
    pub(crate) fn new(db: Database) -> Self {
        Self { db }
    }

    async fn resolve_target(&self, requested: &str, ctx: &ToolContext) -> Result<Target, String> {
        let scope =
            WorkScopeId::parse(requested).map_err(|_| "invalid work_scope_id".to_string())?;
        authorize_target(&ctx.work_scope, ctx.resource_access.authority(), &scope)?;
        self.resolve_persisted_target(scope).await
    }

    async fn resolve_persisted_target(&self, scope: WorkScopeId) -> Result<Target, String> {
        let row: Option<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT lifecycle, environment_kind, cwd, worktree_path
             FROM work_scopes WHERE id = ?1",
        )
        .bind(scope.as_str())
        .fetch_optional(self.db.pool())
        .await
        .map_err(|error| format!("target lookup failed: {error}"))?;
        let Some((lifecycle, kind, cwd, worktree)) = row else {
            return Err("work scope not found".to_string());
        };
        if lifecycle != "active" {
            return Err("work scope is not active".to_string());
        }
        let candidate = match kind.as_str() {
            "allocated_worktree" => worktree,
            "unowned_cwd" => cwd,
            _ => None,
        }
        .ok_or_else(|| "work scope has no repository target".to_string())?;
        let canonical = std::fs::canonicalize(&candidate)
            .map_err(|error| format!("repository target is unavailable: {error}"))?;
        let root = phoenix_core::git::detect_git_repo_root(&canonical)
            .ok_or_else(|| "work scope target is not a Git repository".to_string())?;
        let root = std::fs::canonicalize(root)
            .map_err(|error| format!("repository root is unavailable: {error}"))?;
        if !canonical.starts_with(&root) {
            return Err("work scope target escapes repository root".to_string());
        }
        Ok(Target { scope, root })
    }

    async fn execute(&self, input: InspectInput, ctx: &ToolContext) -> Result<Evidence, String> {
        let target = self.resolve_target(&input.work_scope_id, ctx).await?;
        match input.operation {
            Operation::ResolveTarget => inspect_resolve(&target).await,
            Operation::Status => inspect_status(&target).await,
            Operation::Log { reference, limit } => inspect_log(&target, &reference, limit).await,
            Operation::Diff {
                base,
                head,
                paths,
                name_only,
            } => inspect_diff(&target, &base, &head, paths, name_only).await,
            Operation::ReadFile {
                reference,
                path,
                start_line,
                line_count,
            } => inspect_file(&target, &reference, &path, start_line, line_count).await,
            Operation::Search {
                reference,
                query,
                path_prefix,
                limit,
            } => inspect_search(&target, &reference, query, path_prefix, limit).await,
        }
    }
}

async fn inspect_resolve(target: &Target) -> Result<Evidence, String> {
    let head = resolve_commit(target, "HEAD").await?;
    Ok(evidence(
        target,
        "resolve_target",
        vec![head.clone()],
        0,
        false,
        head,
    ))
}

async fn inspect_status(target: &Target) -> Result<Evidence, String> {
    let head = resolve_commit(target, "HEAD").await?;
    let output = run_git(
        target,
        vec![
            "status".into(),
            "--porcelain=v1".into(),
            "--untracked-files=all".into(),
        ],
        MAX_OUTPUT_BYTES,
    )
    .await?;
    Ok(evidence(
        target,
        "status",
        vec![head],
        output.status,
        output.truncated,
        output.text,
    ))
}

async fn inspect_log(
    target: &Target,
    reference: &str,
    limit: Option<u16>,
) -> Result<Evidence, String> {
    let commit = resolve_commit(target, reference).await?;
    let limit = limit.unwrap_or(20).clamp(1, MAX_LOG_RESULTS);
    let output = run_git(
        target,
        vec![
            "log".into(),
            "--no-decorate".into(),
            "--date=iso-strict".into(),
            "--format=%H%x09%aI%x09%s".into(),
            format!("-n{limit}"),
            commit.clone(),
        ],
        MAX_OUTPUT_BYTES,
    )
    .await?;
    Ok(evidence(
        target,
        "log",
        vec![commit],
        output.status,
        output.truncated,
        output.text,
    ))
}

async fn inspect_diff(
    target: &Target,
    base: &str,
    head: &str,
    paths: Vec<String>,
    name_only: bool,
) -> Result<Evidence, String> {
    let base = resolve_commit(target, base).await?;
    let head = resolve_commit(target, head).await?;
    let mut args = vec![
        "diff".into(),
        "--no-ext-diff".into(),
        "--no-textconv".into(),
    ];
    if name_only {
        args.push("--name-status".into());
    }
    args.extend([base.clone(), head.clone(), "--".into()]);
    for path in paths {
        args.push(validate_path(&path)?);
    }
    let output = run_git(target, args, MAX_OUTPUT_BYTES).await?;
    Ok(evidence(
        target,
        "diff",
        vec![base, head],
        output.status,
        output.truncated,
        output.text,
    ))
}

async fn inspect_file(
    target: &Target,
    reference: &str,
    path: &str,
    start_line: Option<u32>,
    line_count: Option<u32>,
) -> Result<Evidence, String> {
    let commit = resolve_commit(target, reference).await?;
    let path = validate_path(path)?;
    let output = run_git(
        target,
        vec!["show".into(), format!("{commit}:{path}")],
        MAX_FILE_BYTES,
    )
    .await?;
    if output.status != 0 {
        return Ok(evidence(
            target,
            "read_file",
            vec![commit],
            output.status,
            output.truncated,
            output.text,
        ));
    }
    let start = start_line.unwrap_or(1).max(1) as usize;
    let count = line_count.unwrap_or(200).clamp(1, 1000) as usize;
    let lines: Vec<&str> = output.text.lines().collect();
    let end = start
        .saturating_sub(1)
        .saturating_add(count)
        .min(lines.len());
    let mut text = String::new();
    for (index, line) in lines[start.saturating_sub(1).min(lines.len())..end]
        .iter()
        .enumerate()
    {
        let _ = writeln!(text, "{}:{path}:{}\t{line}", commit, start + index);
    }
    Ok(evidence(
        target,
        "read_file",
        vec![commit],
        0,
        output.truncated || end < lines.len(),
        text,
    ))
}

async fn inspect_search(
    target: &Target,
    reference: &str,
    query: String,
    path_prefix: Option<String>,
    limit: Option<u16>,
) -> Result<Evidence, String> {
    if query.is_empty() || query.len() > 256 || query.contains(['\0', '\n', '\r']) {
        return Err("query must be 1..256 characters on one line".to_string());
    }
    let commit = resolve_commit(target, reference).await?;
    let limit = limit.unwrap_or(50).clamp(1, MAX_SEARCH_RESULTS) as usize;
    let mut args = vec![
        "grep".into(),
        "-n".into(),
        "-I".into(),
        "-F".into(),
        "--full-name".into(),
        "-e".into(),
        query,
        commit.clone(),
        "--".into(),
    ];
    if let Some(path) = path_prefix {
        args.push(validate_path(&path)?);
    }
    let mut output = run_git(target, args, MAX_OUTPUT_BYTES).await?;
    let lines: Vec<&str> = output.text.lines().collect();
    if lines.len() > limit {
        output.text = lines[..limit].join("\n");
        output.truncated = true;
    }
    Ok(evidence(
        target,
        "search",
        vec![commit],
        output.status,
        output.truncated,
        output.text,
    ))
}

#[async_trait]
impl Tool for RepositoryInspectionTool {
    fn name(&self) -> &'static str {
        "repository_inspect"
    }
    fn description(&self) -> String {
        "Inspect an explicitly selected active repository WorkScope using bounded structured read-only operations. Supports target resolution, status, log, diff/name overlap, committed file reads, and committed-tree search. No shell, mutation, fetch, or network.".to_string()
    }
    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "work_scope_id":{"type":"string","minLength":1},
                "operation":{"type":"string","enum":["resolve_target","status","log","diff","read_file","search"]},
                "reference":{"type":"string"},"base":{"type":"string"},"head":{"type":"string"},
                "paths":{"type":"array","items":{"type":"string"},"maxItems":100},"name_only":{"type":"boolean"},
                "path":{"type":"string"},"path_prefix":{"type":"string"},"query":{"type":"string","maxLength":256},
                "limit":{"type":"integer","minimum":1,"maximum":200},"start_line":{"type":"integer","minimum":1},
                "line_count":{"type":"integer","minimum":1,"maximum":1000}
            },
            "required":["work_scope_id","operation"],"additionalProperties":false
        })
    }
    fn clearable(&self) -> bool {
        true
    }
    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input = match serde_json::from_value(input) {
            Ok(value) => value,
            Err(error) => {
                return ToolOutput::error(format!("invalid repository inspection request: {error}"))
            }
        };
        match self.execute(input, &ctx).await {
            Ok(value) => ToolOutput::success(
                serde_json::to_string_pretty(&value)
                    .unwrap_or_else(|error| format!("encoding failed: {error}")),
            ),
            Err(error) => ToolOutput::error(error),
        }
    }
}

fn authorize_target(
    caller_scope: &ResourceScopeKey,
    authority: ResourceAuthority,
    requested: &WorkScopeId,
) -> Result<(), String> {
    match caller_scope {
        ResourceScopeKey::Coordinator => Ok(()),
        ResourceScopeKey::Work(own)
            if authority == ResourceAuthority::Restricted && own == requested =>
        {
            Ok(())
        }
        ResourceScopeKey::Work(_) | ResourceScopeKey::GlobalTerminal => {
            Err("repository inspection is not authorized for this target".to_string())
        }
    }
}

fn evidence(
    target: &Target,
    operation: &'static str,
    commits: Vec<String>,
    status: i32,
    truncated: bool,
    text: String,
) -> Evidence {
    Evidence {
        work_scope_id: target.scope.to_string(),
        repository_root: target.root.display().to_string(),
        operation,
        resolved_commits: commits,
        exit_status: status,
        truncated,
        output: text,
    }
}

fn validate_ref(value: &str) -> Result<&str, String> {
    if value.is_empty()
        || value.len() > 256
        || value.starts_with('-')
        || value.contains("..")
        || value.contains("@{")
        || value
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c))
    {
        return Err("invalid Git reference".to_string());
    }
    Ok(value)
}

fn validate_path(value: &str) -> Result<String, String> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > 4096
        || path.is_absolute()
        || value
            .chars()
            .any(|character| character.is_control() || "|><`$".contains(character))
        || value
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("path must be a repository-relative path without traversal".to_string());
    }
    Ok(value.replace('\\', "/"))
}

async fn resolve_commit(target: &Target, reference: &str) -> Result<String, String> {
    let reference = validate_ref(reference)?;
    let output = run_git(
        target,
        vec![
            "rev-parse".into(),
            "--verify".into(),
            format!("{reference}^{{commit}}"),
        ],
        1024,
    )
    .await?;
    if output.status != 0 {
        return Err(format!("reference did not resolve: {}", output.text.trim()));
    }
    let oid = output.text.trim();
    if oid.len() != 40 && oid.len() != 64 {
        return Err("Git returned an invalid commit identity".to_string());
    }
    Ok(oid.to_string())
}

#[derive(Debug)]
struct CommandOutput {
    status: i32,
    truncated: bool,
    text: String,
}

async fn run_git(
    target: &Target,
    args: Vec<String>,
    max_bytes: usize,
) -> Result<CommandOutput, String> {
    run_git_with_timeout(target, args, max_bytes, TIMEOUT).await
}

async fn run_git_with_timeout(
    target: &Target,
    args: Vec<String>,
    max_bytes: usize,
    timeout: Duration,
) -> Result<CommandOutput, String> {
    let root = target.root.clone();
    let mut std_command = phoenix_core::git::command_with_config(&[
        ("core.hooksPath", "/dev/null"),
        ("protocol.allow", "never"),
        ("diff.external", ""),
        ("core.attributesFile", "/dev/null"),
    ]);
    std_command
        .current_dir(root)
        .args(args)
        .env_remove("GIT_EXTERNAL_DIFF")
        .env_remove("GIT_DIFF_OPTS")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut command = tokio::process::Command::from(std_command);
    command.kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to start Git: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Git stdout unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Git stderr unavailable".to_string())?;
    let read_limit = u64::try_from(max_bytes.saturating_add(1)).unwrap_or(u64::MAX);
    let stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout
            .take(read_limit)
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr
            .take(32 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let status = if let Ok(result) = tokio::time::timeout(timeout, child.wait()).await {
        result.map_err(|error| format!("Git execution failed: {error}"))?
    } else {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err("repository inspection timed out".to_string());
    };
    let mut stdout = stdout_task
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    let mut stderr = stderr_task
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    let mut truncated = stdout.len() > max_bytes || stderr.len() > 32 * 1024;
    stdout.truncate(max_bytes);
    stderr.truncate(32 * 1024);
    if !stderr.is_empty() {
        if !stdout.is_empty() {
            stdout.extend_from_slice(b"\n");
        }
        stdout.extend_from_slice(&stderr);
        if stdout.len() > max_bytes {
            stdout.truncate(max_bytes);
            truncated = true;
        }
    }
    Ok(CommandOutput {
        status: status.code().unwrap_or(-1),
        truncated,
        text: String::from_utf8_lossy(&stdout).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn repository() -> (TempDir, Target, String) {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-q"]);
        git(temp.path(), &["config", "user.email", "test@example.com"]);
        git(temp.path(), &["config", "user.name", "Test"]);
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/lib.rs"), "alpha\nshared\n").unwrap();
        git(temp.path(), &["add", "src/lib.rs"]);
        git(temp.path(), &["commit", "-qm", "base"]);
        let base = git(temp.path(), &["rev-parse", "HEAD"]);
        fs::write(
            temp.path().join("src/lib.rs"),
            "alpha\nshared changed\nfeature\n",
        )
        .unwrap();
        fs::write(temp.path().join("README.md"), "feature docs\n").unwrap();
        git(temp.path(), &["add", "."]);
        git(temp.path(), &["commit", "-qm", "feature"]);
        let scope = WorkScopeId::new();
        let target = Target {
            scope,
            root: fs::canonicalize(temp.path()).unwrap(),
        };
        (temp, target, base)
    }

    async fn database_with_scope(root: Option<&Path>, lifecycle: &str) -> (Database, WorkScopeId) {
        let db = Database::open_in_memory().await.unwrap();
        let scope = WorkScopeId::new();
        let (kind, cwd) = root
            .map(|path| ("unowned_cwd", Some(path.display().to_string())))
            .unwrap_or(("none", None));
        sqlx::query(
            "INSERT INTO work_scopes (
                 id, authority_kind, lifecycle, environment_kind, cwd,
                 retired_at, retired_reason, created_at, updated_at
             ) VALUES (
                 ?1, 'restricted_explore', ?2, ?3, ?4,
                 CASE WHEN ?2 = 'retired' THEN 'now' END,
                 CASE WHEN ?2 = 'retired' THEN 'test' END,
                 'now', 'now'
             )",
        )
        .bind(scope.as_str())
        .bind(lifecycle)
        .bind(kind)
        .bind(cwd)
        .execute(db.pool())
        .await
        .unwrap();
        (db, scope)
    }

    #[test]
    fn rejects_adversarial_refs_and_paths() {
        for reference in [
            "--help",
            "HEAD^{tree}",
            "HEAD@{1}",
            "a..b",
            "x y",
            "x\nstatus",
            "HEAD:src/lib.rs",
        ] {
            assert!(validate_ref(reference).is_err(), "{reference}");
        }
        for path in [
            "../secret",
            "/etc/passwd",
            "a/../../b",
            ".",
            "a/./b",
            "a|cat",
            "a>b",
        ] {
            assert!(validate_path(path).is_err(), "{path}");
        }
        assert_eq!(validate_path("src/lib.rs").unwrap(), "src/lib.rs");
    }

    #[test]
    fn authority_is_structural() {
        let own = WorkScopeId::new();
        let foreign = WorkScopeId::new();
        assert!(authorize_target(
            &ResourceScopeKey::Coordinator,
            ResourceAuthority::Work,
            &foreign
        )
        .is_ok());
        assert!(authorize_target(
            &ResourceScopeKey::Work(own.clone()),
            ResourceAuthority::Restricted,
            &own
        )
        .is_ok());
        assert!(authorize_target(
            &ResourceScopeKey::Work(own.clone()),
            ResourceAuthority::Restricted,
            &foreign
        )
        .is_err());
        assert!(authorize_target(
            &ResourceScopeKey::Work(own),
            ResourceAuthority::Work,
            &foreign
        )
        .is_err());
        assert!(authorize_target(
            &ResourceScopeKey::GlobalTerminal,
            ResourceAuthority::Restricted,
            &foreign
        )
        .is_err());
    }

    #[tokio::test]
    async fn persisted_resolution_rejects_missing_retired_and_repositoryless_scopes() {
        let empty = Database::open_in_memory().await.unwrap();
        let missing = RepositoryInspectionTool::new(empty)
            .resolve_persisted_target(WorkScopeId::new())
            .await;
        assert_eq!(missing.unwrap_err(), "work scope not found");

        let (db, retired) = database_with_scope(None, "retired").await;
        let retired = RepositoryInspectionTool::new(db)
            .resolve_persisted_target(retired)
            .await;
        assert_eq!(retired.unwrap_err(), "work scope is not active");

        let (db, none) = database_with_scope(None, "active").await;
        let none = RepositoryInspectionTool::new(db)
            .resolve_persisted_target(none)
            .await;
        assert_eq!(none.unwrap_err(), "work scope has no repository target");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn persisted_resolution_canonicalizes_symlinked_targets() {
        use std::os::unix::fs::symlink;
        let (temp, expected, _base) = repository();
        let link_parent = tempfile::tempdir().unwrap();
        let link = link_parent.path().join("repo-link");
        symlink(temp.path(), &link).unwrap();
        let (db, scope) = database_with_scope(Some(&link), "active").await;
        let target = RepositoryInspectionTool::new(db)
            .resolve_persisted_target(scope)
            .await
            .unwrap();
        assert_eq!(target.root, expected.root);
    }

    #[tokio::test]
    async fn operations_return_exact_commit_evidence() {
        let (_temp, target, base) = repository();
        let head = resolve_commit(&target, "HEAD").await.unwrap();
        assert_ne!(base, head);

        let names = run_git(
            &target,
            vec![
                "diff".into(),
                "--name-status".into(),
                base.clone(),
                head.clone(),
                "--".into(),
            ],
            MAX_OUTPUT_BYTES,
        )
        .await
        .unwrap();
        assert!(names.text.contains("README.md"));
        assert!(names.text.contains("src/lib.rs"));

        let file = run_git(
            &target,
            vec!["show".into(), format!("{head}:src/lib.rs")],
            MAX_FILE_BYTES,
        )
        .await
        .unwrap();
        assert_eq!(file.status, 0);
        assert!(file.text.contains("shared changed"));

        let search = run_git(
            &target,
            vec![
                "grep".into(),
                "-n".into(),
                "-I".into(),
                "-F".into(),
                "-e".into(),
                "feature".into(),
                head,
                "--".into(),
            ],
            MAX_OUTPUT_BYTES,
        )
        .await
        .unwrap();
        assert_eq!(search.status, 0);
        assert!(search.text.contains("src/lib.rs:3:feature"));
    }

    #[tokio::test]
    async fn timeout_is_an_outer_liveness_bound() {
        let (_temp, target, _base) = repository();
        let result = run_git_with_timeout(
            &target,
            vec!["log".into(), "--all".into()],
            MAX_OUTPUT_BYTES,
            Duration::ZERO,
        )
        .await;
        assert_eq!(result.unwrap_err(), "repository inspection timed out");
    }

    #[tokio::test]
    async fn schema_has_no_command_or_argv_escape_hatch() {
        let database = Database::open_in_memory().await.unwrap();
        let schema = RepositoryInspectionTool::new(database).input_schema();
        let properties = schema["properties"].as_object().unwrap();
        assert!(!properties.contains_key("command"));
        assert!(!properties.contains_key("argv"));
        assert!(!properties.contains_key("path_to_repo"));
        assert_eq!(schema["additionalProperties"], false);
    }

    #[tokio::test]
    async fn output_is_bounded_and_index_is_unchanged() {
        let (temp, target, _base) = repository();
        fs::write(
            temp.path().join("large.txt"),
            "x".repeat(MAX_FILE_BYTES * 2),
        )
        .unwrap();
        git(temp.path(), &["add", "large.txt"]);
        git(temp.path(), &["commit", "-qm", "large"]);
        let head = resolve_commit(&target, "HEAD").await.unwrap();
        let output = run_git(
            &target,
            vec!["show".into(), format!("{head}:large.txt")],
            MAX_FILE_BYTES,
        )
        .await
        .unwrap();
        assert!(output.truncated);
        assert!(output.text.len() <= MAX_FILE_BYTES);

        let index = temp.path().join(".git/index");
        let before = fs::metadata(&index).unwrap().modified().unwrap();
        let _ = run_git(
            &target,
            vec!["status".into(), "--porcelain=v1".into()],
            MAX_OUTPUT_BYTES,
        )
        .await
        .unwrap();
        let after = fs::metadata(&index).unwrap().modified().unwrap();
        assert_eq!(before, after);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hostile_alias_hook_and_external_diff_are_not_invoked() {
        use std::os::unix::fs::PermissionsExt;
        let (temp, target, base) = repository();
        let marker = temp.path().join("executed");
        let script = temp.path().join("malicious.sh");
        fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        git(
            temp.path(),
            &["config", "alias.diff", &format!("!{}", script.display())],
        );
        git(
            temp.path(),
            &["config", "diff.external", script.to_str().unwrap()],
        );
        fs::write(
            temp.path().join(".git/hooks/post-index-change"),
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(
            temp.path().join(".git/hooks/post-index-change"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let head = resolve_commit(&target, "HEAD").await.unwrap();
        let output = run_git(
            &target,
            vec![
                "diff".into(),
                "--no-ext-diff".into(),
                "--no-textconv".into(),
                base,
                head,
                "--".into(),
            ],
            MAX_OUTPUT_BYTES,
        )
        .await
        .unwrap();
        assert_eq!(output.status, 0);
        assert!(!marker.exists());
    }
}
