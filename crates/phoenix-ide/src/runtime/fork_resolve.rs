//! Fork-proposal resolution: approve (spawn a Work fork), Request Changes
//! (promote to an Explore refinement), and dismiss.
//!
//! These are the executor-layer effects `Effect::SpawnFork` (REQ-PROJ-034) and
//! `Effect::PromoteForkToExplore` (REQ-PROJ-037). They are dispatched directly
//! by the `/proposals/:id/{approve,request-changes}` endpoints, NOT by the
//! originating conversation's state machine — the origin never transitions and
//! is read for immutable metadata only (`project.main_ref` and its id). The
//! decoupling is structural: the fork/refinement is created through the
//! top-level conversation path (`get_or_create`), never the sub-agent path, so
//! no `parent_event_tx` is in scope to leak a lifecycle notification back to the
//! origin (REQ-PROJ-035).

use std::path::Path;

use chrono::Utc;

use crate::db::{
    Conversation, ConvMode, ConvState, DbError, ForkProposal, ForkProposalStatus, Message,
    MessageContent, MessageType, NonEmptyString, UserContent,
};
use crate::git_ops::{
    create_worktree, ensure_gitignore_has_phoenix, find_branch_in_worktree_list,
    materialize_branch, run_git, GitOpError,
};
use crate::runtime::executor::{promote_task_status_to_in_progress, TASK_APPROVAL_MUTEX};
use crate::runtime::RuntimeManager;
use phoenix_core::task_source::TaskSource;

/// Fixed namespace for deterministic fork/refinement conversation ids. A v5
/// UUID over `{proposal_id}:{kind}` under this namespace is stable across
/// retries (so a crashed approve/promote re-derives the same id and adopts its
/// own orphaned worktree) and disjoint across the two resolution kinds (so an
/// interrupted promote's orphan can never be adopted by a later approve).
const FORK_ID_NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0x9f, 0x1c, 0x6a, 0x2e, 0x7b, 0x84, 0x4d, 0x53, 0xa1, 0x0e, 0x3c, 0x77, 0x12, 0x9b, 0x55, 0xe0,
]);

/// Resolution kind that namespaces the deterministic conversation id.
///
/// The two kinds MUST be disjoint: an interrupted `Promote`'s orphan lives at a
/// different deterministic path than a later `Spawn`, so a crash-recovery retry
/// of one never adopts the other's worktree.
#[derive(Debug, Clone, Copy)]
enum ResolutionKind {
    Spawn,
    Promote,
}

impl ResolutionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Promote => "promote",
        }
    }
}

/// Derive a deterministic conversation id from `(proposal_id, kind)`.
///
/// Anchors crash recovery: the worktree path `.phoenix/worktrees/{id}` is
/// recomputable on retry without writing the spawned/promoted-only resolution
/// field early.
fn derive_conv_id(proposal_id: &str, kind: ResolutionKind) -> String {
    let name = format!("{proposal_id}:{}", kind.as_str());
    uuid::Uuid::new_v5(&FORK_ID_NAMESPACE, name.as_bytes()).to_string()
}

/// Typed failure for the resolve paths. Maps to HTTP status at the handler.
#[derive(Debug)]
pub(crate) enum ForkResolveError {
    /// Proposal id unknown.
    NotFound(String),
    /// Proposal already resolved, origin terminal, branch collision, or other
    /// precondition violation — a 409.
    Conflict(String),
    /// Git / filesystem / DB failure during the irreversible work.
    Internal(String),
}

impl std::fmt::Display for ForkResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(m) | Self::Conflict(m) | Self::Internal(m) => write!(f, "{m}"),
        }
    }
}

impl From<GitOpError> for ForkResolveError {
    fn from(e: GitOpError) -> Self {
        Self::Internal(e.to_string())
    }
}

/// What the blocking git/FS phase produces for the async caller to persist and
/// dispatch.
struct PreparedChild {
    conv: Conversation,
    seed: Message,
}

/// Validated inputs threaded from the async precondition check into the blocking
/// git/FS phase. The origin is read for immutable metadata only (project id /
/// `main_ref` / breadcrumb); it is never mutated.
struct ResolveContext {
    proposal: ForkProposal,
    repo_root: std::path::PathBuf,
    /// The fork base — the project's `main_ref` (REQ-PROJ-034a).
    base: String,
    /// Project the fork inherits, so it is project-scoped like the origin.
    project_id: String,
}

/// Resolve `base` to the freshest non-mutating commit-ish for `worktree add -b`.
///
/// Materializes `base` (single-branch fetch + owned-environments-safe local-ref
/// resolution) then returns `origin/{base}` when that ref exists (it was just
/// fetched), else local `{base}`. Errors if neither exists.
fn resolve_fork_start_point(repo_root: &Path, base: &str) -> Result<String, ForkResolveError> {
    materialize_branch(repo_root, base)?;
    let remote = format!("origin/{base}");
    if run_git(repo_root, &["rev-parse", "--verify", &remote]).is_ok() {
        return Ok(remote);
    }
    if run_git(repo_root, &["rev-parse", "--verify", base]).is_ok() {
        return Ok(base.to_string());
    }
    Err(ForkResolveError::Internal(format!(
        "Fork base branch '{base}' not found locally or at origin"
    )))
}

/// Adopt-on-retry guard for a deterministically-named branch + worktree.
///
/// Returns `Ok(true)` when the branch must be ADOPTED (already checked out in
/// *this* proposal's deterministic worktree from a crashed prior attempt), so
/// the caller skips `create_worktree`. `Ok(false)` when the branch does not
/// exist and a fresh worktree must be created. `Err(Conflict)` when the branch
/// exists but is checked out elsewhere (or not at all) — a real collision with
/// a distinct unit of work.
fn classify_branch_collision(
    repo_root: &Path,
    worktree_path: &Path,
    branch_name: &str,
) -> Result<bool, ForkResolveError> {
    let branch_ref = format!("refs/heads/{branch_name}");
    let exists = run_git(repo_root, &["rev-parse", "--verify", "--quiet", &branch_ref]).is_ok();
    if !exists {
        return Ok(false);
    }
    // Branch exists: adopt iff it is checked out in this proposal's own worktree.
    let checked_out_here = find_branch_in_worktree_list(repo_root, branch_name)
        .is_some_and(|p| Path::new(&p) == worktree_path);
    if checked_out_here {
        Ok(true)
    } else {
        Err(ForkResolveError::Conflict(format!(
            "branch '{branch_name}' already exists and is not this fork's own worktree — \
             a fork must name a distinct unit of work"
        )))
    }
}

/// Write the snapshot body into the worktree, creating missing parent dirs.
fn write_body_to_worktree(
    worktree_path: &Path,
    rel_path: &str,
    body: &str,
) -> Result<(), ForkResolveError> {
    let dest = worktree_path.join(rel_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ForkResolveError::Internal(format!(
                "failed to create parent dirs for '{rel_path}': {e}"
            ))
        })?;
    }
    std::fs::write(&dest, body)
        .map_err(|e| ForkResolveError::Internal(format!("failed to write '{rel_path}': {e}")))?;
    Ok(())
}

impl RuntimeManager {
    /// Approve a pending fork proposal (REQ-PROJ-034 / `Effect::SpawnFork`):
    /// cut a fresh Work worktree off the project's `main_ref`, commit the brief
    /// on its task branch, persist the fork conversation + resolution atomically,
    /// and dispatch its first LLM turn. Returns the fork conversation id.
    ///
    /// Single-use on `proposal_id` (REQ-PROJ-034): a `pending` proposal spawns
    /// exactly one fork; approving an already-resolved proposal is rejected
    /// (`Conflict`). Crash-safe while still `pending`: an interrupted prior
    /// attempt's orphaned worktree at the deterministic path is adopted, not
    /// duplicated. The origin is never mutated or notified.
    ///
    /// # Errors
    ///
    /// [`ForkResolveError::NotFound`] for an unknown proposal,
    /// [`ForkResolveError::Conflict`] for a non-pending proposal / terminal
    /// origin / branch collision, [`ForkResolveError::Internal`] for git/DB
    /// failures.
    pub(crate) async fn approve_fork_proposal(
        self: &std::sync::Arc<Self>,
        proposal_id: &str,
    ) -> Result<String, ForkResolveError> {
        let ctx = self.load_resolvable_proposal(proposal_id).await?;
        let fork_conv_id = derive_conv_id(proposal_id, ResolutionKind::Spawn);

        let prepared = {
            let fork_conv_id = fork_conv_id.clone();
            tokio::task::spawn_blocking(move || prepare_spawn_blocking(&ctx, &fork_conv_id))
                .await
                .map_err(|e| ForkResolveError::Internal(format!("spawn task panicked: {e}")))??
        };

        self.db
            .resolve_fork_proposal_spawned(proposal_id, &prepared.conv, &[prepared.seed])
            .await
            .map_err(map_db_resolve_error)?;

        self.get_or_create(&fork_conv_id)
            .await
            .map_err(ForkResolveError::Internal)?;
        Ok(fork_conv_id)
    }

    /// Request Changes on a pending fork proposal (REQ-PROJ-037 /
    /// `Effect::PromoteForkToExplore`): cut a fresh Explore worktree off
    /// `main_ref`, write the brief as an uncommitted draft under the tasks dir,
    /// persist the refinement conversation + resolution atomically, and dispatch
    /// its first LLM turn. Returns the refinement conversation id.
    ///
    /// Same idempotency / adoption semantics as
    /// [`RuntimeManager::approve_fork_proposal`], under a disjoint deterministic
    /// id namespace. The origin is never mutated or notified.
    ///
    /// # Errors
    ///
    /// As [`RuntimeManager::approve_fork_proposal`].
    pub(crate) async fn request_changes_on_fork_proposal(
        self: &std::sync::Arc<Self>,
        proposal_id: &str,
        change_request: String,
    ) -> Result<String, ForkResolveError> {
        let ctx = self.load_resolvable_proposal(proposal_id).await?;
        let refinement_conv_id = derive_conv_id(proposal_id, ResolutionKind::Promote);

        let prepared = {
            let refinement_conv_id = refinement_conv_id.clone();
            tokio::task::spawn_blocking(move || {
                prepare_promote_blocking(&ctx, &refinement_conv_id, &change_request)
            })
            .await
            .map_err(|e| ForkResolveError::Internal(format!("spawn task panicked: {e}")))??
        };

        self.db
            .resolve_fork_proposal_promoted(proposal_id, &prepared.conv, &[prepared.seed])
            .await
            .map_err(map_db_resolve_error)?;

        self.get_or_create(&refinement_conv_id)
            .await
            .map_err(ForkResolveError::Internal)?;
        Ok(refinement_conv_id)
    }

    /// Load a proposal and validate the shared resolve preconditions: it exists,
    /// is `pending`, its origin is not terminal and is in a writing mode, and its
    /// project has a repo root + `main_ref`. Returns `(proposal, repo_root, base)`.
    async fn load_resolvable_proposal(
        &self,
        proposal_id: &str,
    ) -> Result<ResolveContext, ForkResolveError> {
        let proposal = self
            .db
            .get_fork_proposal(proposal_id)
            .await
            .map_err(|e| ForkResolveError::Internal(e.to_string()))?
            .ok_or_else(|| ForkResolveError::NotFound(format!("fork proposal {proposal_id}")))?;

        if proposal.status != ForkProposalStatus::Pending {
            return Err(ForkResolveError::Conflict(format!(
                "fork proposal {proposal_id} is already resolved ({})",
                proposal.status.as_str()
            )));
        }

        let origin = self
            .db
            .get_conversation(&proposal.origin_conversation_id)
            .await
            .map_err(|e| ForkResolveError::NotFound(e.to_string()))?;

        // Origin must be live (is_terminal covers terminal, context-exhausted,
        // handed-off) — a stale proposal can't spawn after the origin ended.
        if origin.state.is_terminal() {
            return Err(ForkResolveError::Conflict(
                "the originating conversation has reached a terminal state — \
                 its fork proposals can no longer be resolved"
                    .to_string(),
            ));
        }

        // Origin mode must be a writing mode (work, branch, direct). Explore is
        // the in-place gateway, not a fork proposer.
        match origin.conv_mode {
            ConvMode::Work { .. } | ConvMode::Branch { .. } | ConvMode::Direct => {}
            ConvMode::Explore { .. } => {
                return Err(ForkResolveError::Conflict(
                    "the originating conversation is in Explore mode and cannot have a fork proposal"
                        .to_string(),
                ));
            }
        }

        let project_id = origin.project_id.clone().ok_or_else(|| {
            ForkResolveError::Conflict(
                "the originating conversation is not project-scoped".to_string(),
            )
        })?;
        let project = self
            .db
            .get_project(&project_id)
            .await
            .map_err(|e| ForkResolveError::Internal(e.to_string()))?;

        let repo_root = phoenix_core::domain::db_schema::detect_git_repo_root(Path::new(
            &project.canonical_path,
        ))
        .map_or_else(
            || std::path::PathBuf::from(&project.canonical_path),
            std::path::PathBuf::from,
        );

        Ok(ResolveContext {
            proposal,
            repo_root,
            base: project.main_ref,
            project_id,
        })
    }
}

/// Build the seed `Message` for a child conversation: a single meta user
/// message carrying `text`. `sequence_id` is 1 — the child has no prior
/// transcript.
fn seed_message(conv_id: &str, text: String) -> Message {
    let now = Utc::now();
    Message {
        message_id: uuid::Uuid::new_v4().to_string(),
        conversation_id: conv_id.to_string(),
        sequence_id: 1,
        message_type: MessageType::User,
        content: MessageContent::User(UserContent::meta(text)),
        display_data: None,
        usage_data: None,
        created_at: now,
    }
}

/// The seeded initial state every fork/refinement starts in, so its first LLM
/// turn fires on `get_or_create` (mirrors the fresh-handoff successor).
fn seeded_state(seed: &Message) -> ConvState {
    ConvState::SeededLlmRequesting {
        seed_message_id: seed.message_id.clone(),
        attempt: 1,
    }
}

/// Blocking git/FS phase of the spawn (REQ-PROJ-034), under
/// `TASK_APPROVAL_MUTEX`. Produces the Work fork conversation + seed message;
/// the atomic DB resolve happens back on the async side.
fn prepare_spawn_blocking(
    ctx: &ResolveContext,
    fork_conv_id: &str,
) -> Result<PreparedChild, ForkResolveError> {
    let _guard = TASK_APPROVAL_MUTEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let ResolveContext {
        proposal,
        repo_root,
        base,
        project_id,
    } = ctx;
    let repo_root = repo_root.as_path();
    let base = base.as_str();

    let filename = Path::new(&proposal.task_file)
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| {
            ForkResolveError::Internal(format!(
                "proposal task_file has no filename: '{}'",
                proposal.task_file
            ))
        })?;
    let source = TaskSource::detect(filename).ok_or_else(|| {
        ForkResolveError::Internal(format!("proposal task_file is not markdown: '{filename}'"))
    })?;
    let (branch_name, task_id) = source.branch_and_id(fork_conv_id);

    let start_point = resolve_fork_start_point(repo_root, base)?;

    let worktree_path = repo_root.join(".phoenix/worktrees").join(fork_conv_id);
    let adopt = classify_branch_collision(repo_root, &worktree_path, &branch_name)?;
    if !adopt {
        create_worktree(repo_root, fork_conv_id, &branch_name, Some(&start_point))?;
    }
    let worktree_path_str = worktree_path.to_string_lossy().to_string();

    write_body_to_worktree(&worktree_path, &proposal.task_file, &proposal.body)?;

    // taskmd: promote status to in-progress (rename) before committing; the
    // file is staged at its (possibly renamed) tasks-dir path. plain brief: no
    // status segment, committed verbatim at its own path.
    let committed_path = match &source {
        TaskSource::Taskmd { id, status, .. } => {
            let tasks_dir = Path::new(&proposal.task_file)
                .parent()
                .map_or_else(|| repo_root.join("tasks"), |p| worktree_path.join(p));
            let final_filename = promote_task_status_to_in_progress(
                &tasks_dir,
                id,
                *status,
                filename,
            )
            .map_err(ForkResolveError::Internal)?;
            // Rebuild the repo-relative path with the (possibly new) filename.
            let parent = Path::new(&proposal.task_file).parent();
            match parent {
                Some(p) if !p.as_os_str().is_empty() => {
                    format!("{}/{final_filename}", p.to_string_lossy())
                }
                _ => final_filename,
            }
        }
        TaskSource::PlainMarkdown { .. } => proposal.task_file.clone(),
    };

    ensure_gitignore_has_phoenix(&worktree_path).map_err(ForkResolveError::Internal)?;

    // If a taskmd rename happened, stage the old name's deletion too so the
    // commit is a rename, not a duplicate id.
    if committed_path != proposal.task_file {
        let _ = run_git(&worktree_path, &["add", "--", &proposal.task_file]);
    }
    run_git(&worktree_path, &["add", "--", &committed_path])
        .map_err(ForkResolveError::Internal)?;
    let commit_msg = format!("task {task_id}: {}", proposal.title);
    if run_git(&worktree_path, &["diff", "--cached", "--quiet"]).is_err() {
        run_git(&worktree_path, &["commit", "-m", &commit_msg])
            .map_err(|e| ForkResolveError::Internal(format!("failed to commit task file: {e}")))?;
    }

    let conv_mode = ConvMode::Work {
        branch_name: nes(&branch_name, "branch name")?,
        worktree_path: nes(&worktree_path_str, "worktree path")?,
        base_branch: nes(base, "base branch")?,
        task_id: nes(&task_id, "task id")?,
        task_title: nes(&proposal.title, "task title")?,
    };

    let seed_text = format!(
        "Task approved. Execute the approved plan below. You are on branch {branch_name}.\n\n{}",
        proposal.body
    );
    let seed = seed_message(fork_conv_id, seed_text);
    let conv = build_child_conversation(
        fork_conv_id,
        &worktree_path_str,
        conv_mode,
        base,
        project_id,
        proposal,
        &seed,
    );

    Ok(PreparedChild { conv, seed })
}

/// Blocking git/FS phase of the promote (REQ-PROJ-037), under
/// `TASK_APPROVAL_MUTEX`. Produces the Explore refinement conversation + seed
/// message; the brief draft is written UNCOMMITTED under the tasks dir.
fn prepare_promote_blocking(
    ctx: &ResolveContext,
    refinement_conv_id: &str,
    change_request: &str,
) -> Result<PreparedChild, ForkResolveError> {
    let _guard = TASK_APPROVAL_MUTEX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let ResolveContext {
        proposal,
        repo_root,
        base,
        project_id,
    } = ctx;
    let repo_root = repo_root.as_path();
    let base = base.as_str();

    let id_prefix: String = refinement_conv_id.chars().take(8).collect();
    let temp_branch = format!("task-pending-{id_prefix}");

    let start_point = resolve_fork_start_point(repo_root, base)?;

    let worktree_path = repo_root.join(".phoenix/worktrees").join(refinement_conv_id);
    let adopt = classify_branch_collision(repo_root, &worktree_path, &temp_branch)?;
    if !adopt {
        create_worktree(repo_root, refinement_conv_id, &temp_branch, Some(&start_point))?;
    }
    let worktree_path_str = worktree_path.to_string_lossy().to_string();

    // Ensure the tasks dir exists in the worktree first (a plain-brief-only
    // project may have none), then write the brief as an UNCOMMITTED draft at a
    // deterministic collision-free path under it — never at the brief's own path.
    let tasks_dir_rel = taskmd_core::discover::discover_or_default(&worktree_path);
    let tasks_dir_rel = tasks_dir_rel.to_string_lossy().to_string();
    let stem = Path::new(&proposal.task_file)
        .file_stem()
        .and_then(|s| s.to_str())
        .map_or_else(|| "brief".to_string(), sanitize_stem);
    let draft_rel = format!("{tasks_dir_rel}/{id_prefix}-{stem}.md");
    write_body_to_worktree(&worktree_path, &draft_rel, &proposal.body)?;

    ensure_gitignore_has_phoenix(&worktree_path).map_err(ForkResolveError::Internal)?;
    // Nothing is committed — the draft sits uncommitted on the temp branch for
    // the Explore agent to revise (REQ-PROJ-037).

    let conv_mode = ConvMode::Explore {
        worktree_path: Some(nes(&worktree_path_str, "worktree path")?),
    };

    let seed_text = format!(
        "A fork proposal was sent here for refinement. The drafted brief is below; \
         revise it under {tasks_dir_rel}/ per the change request, then propose it.\n\n\
         ## Change request\n{change_request}\n\n## Brief\n{}",
        proposal.body
    );
    let seed = seed_message(refinement_conv_id, seed_text);
    let conv = build_child_conversation(
        refinement_conv_id,
        &worktree_path_str,
        conv_mode,
        base,
        project_id,
        proposal,
        &seed,
    );

    Ok(PreparedChild { conv, seed })
}

/// Construct the child `Conversation` row shared by spawn + promote: a fresh
/// top-level conversation seeded to start its first turn, with the provenance
/// breadcrumb pointing at the origin (REQ-PROJ-035) and `desired_base_branch`
/// recording the fork base so any later approval resolves it.
#[allow(clippy::too_many_arguments)]
fn build_child_conversation(
    conv_id: &str,
    worktree_path: &str,
    conv_mode: ConvMode,
    base: &str,
    project_id: &str,
    proposal: &ForkProposal,
    seed: &Message,
) -> Conversation {
    let now = Utc::now();
    let id_prefix: String = conv_id.chars().take(8).collect();
    let slug = format!("fork-{id_prefix}");
    Conversation {
        id: conv_id.to_string(),
        slug: Some(slug.clone()),
        title: Some(phoenix_core::domain::db_schema::title_from_slug(&slug)),
        cwd: worktree_path.to_string(),
        parent_conversation_id: None,
        user_initiated: true,
        state: seeded_state(seed),
        state_updated_at: now,
        created_at: now,
        updated_at: now,
        archived: false,
        model: None,
        project_id: Some(project_id.to_string()),
        conv_mode,
        desired_base_branch: Some(base.to_string()),
        message_count: 0,
        seed_parent_id: None,
        seed_label: None,
        continued_in_conv_id: None,
        chain_name: None,
        steering_queue: Vec::new(),
        llm_language: crate::llm_language::LlmLanguage::default(),
        spawned_from_conversation_id: Some(proposal.origin_conversation_id.clone()),
    }
}

/// Wrap a `NonEmptyString::new` with a typed error for the `ConvMode` fields.
fn nes(s: &str, what: &str) -> Result<NonEmptyString, ForkResolveError> {
    NonEmptyString::new(s).map_err(|_| ForkResolveError::Internal(format!("{what} is empty")))
}

/// Reduce a file stem to a path-safe lowercase segment for the draft filename.
fn sanitize_stem(stem: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "brief".to_string()
    } else {
        out
    }
}

/// Map the atomic DB resolve error to a typed resolve error. A divergent prior
/// resolution is a 409; everything else is internal.
fn map_db_resolve_error(e: DbError) -> ForkResolveError {
    match e {
        DbError::ForkProposalConflict(m) => ForkResolveError::Conflict(m),
        DbError::ConversationNotFound(m) => ForkResolveError::NotFound(m),
        DbError::Sqlx(_)
        | DbError::MessageNotFound(_)
        | DbError::SlugExists(_)
        | DbError::Serialization(_) => ForkResolveError::Internal(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, ForkProposal, ForkProposalStatus};
    use crate::llm::ModelRegistry;
    use crate::platform::PlatformCapability;
    use crate::runtime::RuntimeManager;
    use crate::tools::mcp::McpClientManager;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
            .env("GIT_CONFIG_VALUE_0", "false")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Init a repo on `main` with a tasks dir (so taskmd discovery resolves it)
    /// and one commit.
    fn init_repo() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        git(&root, &["config", "user.email", "t@example.com"]);
        git(&root, &["config", "user.name", "t"]);
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        std::fs::write(root.join("tasks/_TEMPLATE.md"), "# template\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        (tmp, root)
    }

    async fn make_runtime(db: Database) -> Arc<RuntimeManager> {
        Arc::new(RuntimeManager::new(
            db,
            Arc::new(ModelRegistry::new_empty()),
            PlatformCapability::None,
            Arc::new(McpClientManager::new()),
            None,
        ))
    }

    /// Create a project + a live Direct-mode origin conversation in it. Returns
    /// (project_id, origin_id).
    async fn seed_project_and_origin(db: &Database, repo: &Path) -> (String, String) {
        let project = db
            .find_or_create_project(&repo.to_string_lossy())
            .await
            .unwrap();
        let origin_id = uuid::Uuid::new_v4().to_string();
        db.create_conversation_with_project(
            &origin_id,
            "origin",
            &repo.to_string_lossy(),
            true,
            None,
            None,
            Some(&project.id),
            &ConvMode::Direct,
            None,
            None,
            None,
            crate::llm_language::LlmLanguage::default(),
        )
        .await
        .unwrap();
        (project.id, origin_id)
    }

    async fn insert_pending(db: &Database, origin_id: &str, task_file: &str, body: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let proposal = ForkProposal {
            id: id.clone(),
            origin_conversation_id: origin_id.to_string(),
            task_file: task_file.to_string(),
            title: "Fix the thing".to_string(),
            priority: "p1".to_string(),
            body: body.to_string(),
            status: ForkProposalStatus::Pending,
            fork_conversation_id: None,
            refinement_conversation_id: None,
            created_at: Utc::now(),
            resolved_at: None,
        };
        db.insert_fork_proposal(&proposal).await.unwrap();
        id
    }

    #[tokio::test]
    async fn approve_taskmd_proposal_spawns_work_fork_off_main() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let body = "# Fix the thing\n\n## Plan\nDo it.\n";
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--fix-thing.md", body).await;
        let rt = make_runtime(db.clone()).await;

        let fork_id = rt.approve_fork_proposal(&pid).await.unwrap();

        // Fork conversation exists in Work mode on the derived branch.
        let fork = db.get_conversation(&fork_id).await.unwrap();
        match &fork.conv_mode {
            ConvMode::Work {
                branch_name,
                base_branch,
                task_id,
                ..
            } => {
                assert_eq!(branch_name.as_str(), "task-12345-fix-thing");
                assert_eq!(base_branch.as_str(), "main");
                assert_eq!(task_id.as_str(), "12345");
            }
            other => panic!("expected Work mode, got {other:?}"),
        }
        assert_eq!(
            fork.spawned_from_conversation_id.as_deref(),
            Some(origin.as_str())
        );

        // Branch was cut from main and carries the committed in-progress task.
        let wt = repo.join(".phoenix/worktrees").join(&fork_id);
        assert!(wt.is_dir());
        let committed = git(
            &wt,
            &["log", "-1", "--name-only", "--pretty=format:", "task-12345-fix-thing"],
        );
        assert!(
            committed.contains("12345-p1-in-progress--fix-thing.md"),
            "committed file should be in-progress: {committed}"
        );

        // Proposal resolved as spawned with the fork id.
        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Spawned);
        assert_eq!(resolved.fork_conversation_id.as_deref(), Some(fork_id.as_str()));

        // Origin is UNCHANGED: no continuation, no handoff, no new transcript msg.
        let origin_conv = db.get_conversation(&origin).await.unwrap();
        assert!(origin_conv.continued_in_conv_id.is_none());
        assert_eq!(db.get_messages(&origin).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn approve_plain_brief_branches_with_fork_id_prefix_no_status_promotion() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let body = "# Plan\n\nplain brief body\n";
        let pid = insert_pending(&db, &origin, "docs/plan.md", body).await;
        let rt = make_runtime(db.clone()).await;

        let fork_id = rt.approve_fork_proposal(&pid).await.unwrap();

        let fork = db.get_conversation(&fork_id).await.unwrap();
        let prefix: String = fork_id.chars().take(8).collect();
        match &fork.conv_mode {
            ConvMode::Work { branch_name, .. } => {
                assert_eq!(branch_name.as_str(), format!("task-plan-{prefix}"));
            }
            other => panic!("expected Work mode, got {other:?}"),
        }
        // The plain brief is committed verbatim at its own path.
        let wt = repo.join(".phoenix/worktrees").join(&fork_id);
        assert_eq!(std::fs::read_to_string(wt.join("docs/plan.md")).unwrap(), body);
        let log = git(&wt, &["log", "-1", "--name-only", "--pretty=format:"]);
        assert!(log.contains("docs/plan.md"), "expected committed plan: {log}");
    }

    #[tokio::test]
    async fn second_approve_after_spawn_is_rejected() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(
            &db,
            &origin,
            "tasks/12345-p1-ready--fix-thing.md",
            "# Fix the thing\n",
        )
        .await;
        let rt = make_runtime(db.clone()).await;

        let first = rt.approve_fork_proposal(&pid).await.unwrap();
        // REQ-PROJ-034: a resolved proposal is single-use — re-approving an
        // already-`spawned` proposal is rejected, so it never spawns twice.
        let second = rt.approve_fork_proposal(&pid).await;
        assert!(
            matches!(second, Err(ForkResolveError::Conflict(_))),
            "re-approve of a spawned proposal must be rejected, got {second:?}"
        );

        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Spawned);
        assert_eq!(resolved.fork_conversation_id.as_deref(), Some(first.as_str()));
    }

    #[tokio::test]
    async fn approve_adopts_orphaned_worktree_at_deterministic_path() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(
            &db,
            &origin,
            "tasks/12345-p1-ready--fix-thing.md",
            "# Fix the thing\n",
        )
        .await;
        // Simulate a crashed prior approve: the deterministic worktree + branch
        // already exist (created off main) but no resolution was recorded.
        let fork_id = derive_conv_id(&pid, ResolutionKind::Spawn);
        let wt = repo.join(".phoenix/worktrees").join(&fork_id);
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "task-12345-fix-thing",
                wt.to_str().unwrap(),
                "main",
            ],
        );
        let rt = make_runtime(db.clone()).await;

        let got = rt.approve_fork_proposal(&pid).await.unwrap();
        assert_eq!(got, fork_id, "adopted the orphan, no new id");
        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Spawned);
    }

    #[tokio::test]
    async fn approve_non_pending_is_rejected() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(
            &db,
            &origin,
            "tasks/12345-p1-ready--fix-thing.md",
            "# Fix the thing\n",
        )
        .await;
        let rt = make_runtime(db.clone()).await;
        rt.approve_fork_proposal(&pid).await.unwrap();

        // Dismiss is a no-op on a spawned proposal; a fresh approve attempt on a
        // proposal we manually mark dismissed must be a conflict.
        let other = insert_pending(&db, &origin, "tasks/22222-p1-ready--two.md", "# Two\n").await;
        assert!(db.dismiss_fork_proposal(&other).await.unwrap());
        let err = rt.approve_fork_proposal(&other).await.unwrap_err();
        assert!(matches!(err, ForkResolveError::Conflict(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn dismiss_resolves_without_spawning() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--fix-thing.md", "# x\n").await;

        assert!(db.dismiss_fork_proposal(&pid).await.unwrap());
        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Dismissed);
        assert!(resolved.fork_conversation_id.is_none());
        // No worktree created.
        let fork_id = derive_conv_id(&pid, ResolutionKind::Spawn);
        assert!(!repo.join(".phoenix/worktrees").join(&fork_id).exists());
    }

    #[tokio::test]
    async fn request_changes_promotes_to_explore_with_uncommitted_draft() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let body = "# Plan\n\nbrief body\n";
        let pid = insert_pending(&db, &origin, "docs/plan.md", body).await;
        let rt = make_runtime(db.clone()).await;

        let refinement_id = rt
            .request_changes_on_fork_proposal(&pid, "make it shorter".to_string())
            .await
            .unwrap();

        let refinement = db.get_conversation(&refinement_id).await.unwrap();
        match &refinement.conv_mode {
            ConvMode::Explore { worktree_path } => {
                assert!(worktree_path.is_some());
            }
            other => panic!("expected Explore mode, got {other:?}"),
        }
        assert_eq!(
            refinement.spawned_from_conversation_id.as_deref(),
            Some(origin.as_str())
        );

        // Brief draft present UNCOMMITTED under tasks/.
        let wt = repo.join(".phoenix/worktrees").join(&refinement_id);
        let prefix: String = refinement_id.chars().take(8).collect();
        let draft = wt.join(format!("tasks/{prefix}-plan.md"));
        assert_eq!(std::fs::read_to_string(&draft).unwrap(), body);
        let status = git(&wt, &["status", "--porcelain"]);
        assert!(
            status.contains(&format!("tasks/{prefix}-plan.md")),
            "draft must be uncommitted: {status}"
        );

        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Promoted);
        assert_eq!(
            resolved.refinement_conversation_id.as_deref(),
            Some(refinement_id.as_str())
        );
        // Origin unchanged.
        assert_eq!(db.get_messages(&origin).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn spawn_and_promote_namespaces_are_disjoint() {
        let pid = "deadbeef-0000-0000-0000-000000000000";
        let spawn = derive_conv_id(pid, ResolutionKind::Spawn);
        let promote = derive_conv_id(pid, ResolutionKind::Promote);
        assert_ne!(spawn, promote);

        // promote-after-approve and approve-after-promote both rejected (the
        // proposal becomes terminal on first resolution).
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_p, origin) = seed_project_and_origin(&db, &repo).await;
        let p1 = insert_pending(&db, &origin, "tasks/12345-p1-ready--x.md", "# x\n").await;
        let rt = make_runtime(db.clone()).await;
        rt.approve_fork_proposal(&p1).await.unwrap();
        let err = rt
            .request_changes_on_fork_proposal(&p1, "note".to_string())
            .await
            .unwrap_err();
        assert!(matches!(err, ForkResolveError::Conflict(_)), "got {err:?}");

        let p2 = insert_pending(&db, &origin, "tasks/22222-p1-ready--y.md", "# y\n").await;
        rt.request_changes_on_fork_proposal(&p2, "note".to_string())
            .await
            .unwrap();
        let err = rt.approve_fork_proposal(&p2).await.unwrap_err();
        assert!(matches!(err, ForkResolveError::Conflict(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn resolve_rejected_when_origin_terminal() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_p, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--x.md", "# x\n").await;
        // Drive the origin terminal.
        db.update_conversation_state(&origin, &ConvState::Terminal)
            .await
            .unwrap();
        let rt = make_runtime(db.clone()).await;
        let err = rt.approve_fork_proposal(&pid).await.unwrap_err();
        assert!(matches!(err, ForkResolveError::Conflict(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn resolve_unknown_proposal_is_not_found() {
        let db = Database::open_in_memory().await.unwrap();
        let rt = make_runtime(db.clone()).await;
        let err = rt.approve_fork_proposal("no-such-id").await.unwrap_err();
        assert!(matches!(err, ForkResolveError::NotFound(_)), "got {err:?}");
    }
}
