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
use tokio::sync::oneshot;

use crate::db::{
    ConvMode, ConvState, Conversation, DbError, ForkProposal, ForkProposalStatus, Message,
    MessageContent, MessageType, NonEmptyString, UserContent,
};
use crate::git_ops::{
    create_worktree, ensure_local_exclude_has_phoenix, find_branch_in_worktree_list,
    materialize_branch, run_git, GitOpError, PhoenixIgnoreStrategy,
};
use crate::runtime::executor::{promote_task_status_to_in_progress, TASK_APPROVAL_MUTEX};
use crate::runtime::RuntimeManager;
use phoenix_core::task_source::TaskSource;

/// A fork-resolution operation routed to the single serialized fork-resolution
/// consumer ([`RuntimeManager::run_fork_command_consumer`]). Each variant
/// carries its inputs plus a [`oneshot::Sender`] the consumer replies on once
/// the operation has run to completion.
///
/// Mutual exclusion between fork resolution (approve / request-changes) and
/// cleanup (dismiss / retire-on-terminal / hard-delete) is STRUCTURAL: the
/// consumer processes commands one at a time, so two critical sections cannot
/// interleave and a cleanup can never tear down a just-created live fork's
/// worktree before its resolution commits. There is no lock to forget.
pub(crate) enum ForkCommand {
    /// Approve a pending proposal — spawn a Work fork. Replies with the fork
    /// conversation id.
    Approve {
        proposal_id: String,
        reply: oneshot::Sender<Result<String, ForkResolveError>>,
    },
    /// Request Changes on a pending proposal — promote to an Explore
    /// refinement. Replies with the refinement conversation id.
    RequestChanges {
        proposal_id: String,
        change_request: String,
        reply: oneshot::Sender<Result<String, ForkResolveError>>,
    },
    /// Dismiss a proposal. Replies `true` iff the row transitioned
    /// `pending -> dismissed` (the endpoint reports `false` as `no_op`).
    Dismiss {
        proposal_id: String,
        reply: oneshot::Sender<Result<bool, ForkResolveError>>,
    },
    /// Retire (dismiss) every still-`pending` proposal bound to a now-terminal
    /// origin and clean its deterministic orphans. Best-effort; reply is `()`.
    RetireForOrigin {
        origin_id: String,
        reply: oneshot::Sender<()>,
    },
    /// Before a hard-delete removes the origin row, dismiss every still-`pending`
    /// proposal bound to it and clean its deterministic orphan. Dismissing under
    /// serialization is what makes a fork-from-a-deleted-origin structurally
    /// impossible: any `Approve`/`RequestChanges` queued behind this command runs
    /// after it and finds the proposal non-`pending`, so it aborts before
    /// creating a worktree. Best-effort; reply is `()`.
    CleanupOnHardDelete {
        origin_id: String,
        reply: oneshot::Sender<()>,
    },
}

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
pub(crate) enum ResolutionKind {
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
pub(crate) fn derive_conv_id(proposal_id: &str, kind: ResolutionKind) -> String {
    let name = format!("{proposal_id}:{}", kind.as_str());
    uuid::Uuid::new_v5(&FORK_ID_NAMESPACE, name.as_bytes()).to_string()
}

/// `DeterministicForkOrphansCleaned(proposal)` (Allium): best-effort removal of
/// any orphaned worktree + branch a crashed approve/promote may have left at
/// EITHER deterministic path — `.phoenix/worktrees/{derive_conv_id(id, Spawn)}`
/// and `{derive_conv_id(id, Promote)}` — for a proposal whose resolution was
/// never recorded (so it is still `pending`).
///
/// Serialisation against approve/promote is provided by the single fork-resolution
/// consumer: every resolve and cleanup path is a [`ForkCommand`] processed one at a
/// time, so a terminal transition or hard-delete arriving mid-approve cannot delete a
/// worktree the in-flight approve is about to adopt, and the `pending` re-read by a
/// later command is authoritative. The synchronous git work additionally takes
/// [`TASK_APPROVAL_MUTEX`] to serialise branch-name races with the Explore-approval
/// path (which is NOT a fork command). Guard to
/// `pending` proposals only at the call site — for a `spawned`/`promoted`
/// proposal these paths are the LIVE decoupled fork/refinement, which must not
/// be touched.
///
/// Each git step is non-fatal and logged at WARN: `worktree remove --force`
/// with an `rm -rf` + `worktree prune` filesystem fallback, then `branch -D`.
/// A `None` `repo_root` (no project / unresolvable repo) falls back to a
/// filesystem-only directory removal.
///
/// A crashed `git worktree add -b <branch> <path> <start>` creates the BRANCH
/// ref FIRST, then checks out the dir; killed in between it leaves the
/// deterministic branch with no usable worktree. So the candidate branch name is
/// derived from the proposal (it is NOT recoverable from a missing worktree's
/// `git worktree list` entry) and passed in, letting `clean_one_orphan` delete a
/// BRANCH-ONLY orphan that would otherwise make the still-`pending` proposal
/// permanently unapprovable (`classify_branch_collision` would see the branch
/// exists but is not checked out in the deterministic worktree).
pub(crate) fn clean_deterministic_fork_orphans(repo_root: Option<&Path>, proposal: &ForkProposal) {
    for kind in [ResolutionKind::Spawn, ResolutionKind::Promote] {
        let conv_id = derive_conv_id(&proposal.id, kind);
        let branch = deterministic_fork_branch_name(proposal, &conv_id, kind);
        clean_one_orphan(repo_root, &conv_id, branch.as_deref());
    }
}

/// The deterministic branch name a crashed approve/promote of `proposal` would
/// have created at `conv_id`'s worktree, mirroring the naming in
/// `prepare_spawn_blocking` / `prepare_promote_blocking`:
///
/// - `Spawn` — `TaskSource::branch_and_id(conv_id).0` (taskmd `task-{id}-{slug}`
///   or plain `task-{stem}-{prefix}`), or `None` if the `task_file` isn't markdown.
/// - `Promote` — `task-pending-{conv_id_prefix}`.
///
/// Used to delete a BRANCH-ONLY orphan, whose branch can't be recovered from
/// `git worktree list` because the worktree dir never materialised.
fn deterministic_fork_branch_name(
    proposal: &ForkProposal,
    conv_id: &str,
    kind: ResolutionKind,
) -> Option<String> {
    match kind {
        ResolutionKind::Spawn => {
            let filename = Path::new(&proposal.task_file)
                .file_name()
                .and_then(|f| f.to_str())?;
            let source = TaskSource::detect(filename)?;
            Some(source.branch_and_id(conv_id).0)
        }
        ResolutionKind::Promote => {
            let id_prefix: String = conv_id.chars().take(8).collect();
            Some(format!("task-pending-{id_prefix}"))
        }
    }
}

/// `ForkProposalsRetiredOnOriginTerminal` (REQ-PROJ-035): dismiss every
/// still-`pending` fork proposal bound to `origin_id` and clean any deterministic
/// spawn/promote git orphan a crashed approve/promote left behind. Resolved
/// (`spawned`/`dismissed`/`promoted`) proposals are untouched — they persist as
/// audit and their deterministic path is the LIVE decoupled fork/refinement.
///
/// Runs inside the single fork-resolution consumer, so it serialises with
/// approve/promote: a terminal transition arriving mid-approve either runs after
/// the resolve commits (the proposal is no longer pending, so it is skipped and
/// its live child untouched) or before it (the proposal is dismissed + its orphan
/// cleaned, and the in-flight approve then finds it no longer pending and aborts).
/// The status re-read here is authoritative — the consumer is single-threaded, so
/// no resolve is mid-flight. Best-effort: a failure to list / clean / dismiss is
/// logged at WARN and never blocks the terminal transition.
async fn handle_retire_for_origin(db: &crate::db::Database, origin_id: &str) {
    // Re-read CURRENT status — never a pre-command stale list. Keep the full
    // proposals so orphan cleanup can derive each one's deterministic branch name.
    let pending = match db.list_fork_proposals_for_origin(origin_id).await {
        Ok(proposals) => proposals
            .into_iter()
            .filter(|p| p.status == ForkProposalStatus::Pending)
            .collect::<Vec<_>>(),
        Err(e) => {
            tracing::warn!(
                conv_id = %origin_id,
                error = %e,
                "fork retirement: failed to list proposals; pending proposals may linger"
            );
            return;
        }
    };
    if pending.is_empty() {
        return;
    }

    let repo_root = fork_origin_repo_root(db, origin_id).await;
    let _ = tokio::task::spawn_blocking(move || {
        let _guard = TASK_APPROVAL_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for proposal in &pending {
            clean_deterministic_fork_orphans(repo_root.as_deref(), proposal);
        }
    })
    .await;

    if let Err(e) = db.retire_pending_fork_proposals_for_origin(origin_id).await {
        tracing::warn!(
            conv_id = %origin_id,
            error = %e,
            "fork retirement: failed to dismiss pending proposals"
        );
    }
}

/// `ForkProposalsRemovedOnOriginDelete` (REQ-PROJ-035): before a hard-delete
/// removes the origin row, dismiss every still-`pending` proposal bound to it and
/// clean its deterministic spawn/promote git orphan. The proposal ROWS are removed
/// by the `fork_proposals.origin_conv_id` ON DELETE CASCADE when the conversation
/// row is deleted, so this does not delete rows itself — but it DOES dismiss the
/// `pending` ones so that an `Approve`/`RequestChanges` queued behind this command
/// in the consumer finds the proposal non-`pending` (via `load_resolvable_proposal`)
/// and aborts before creating a worktree. No fork can be spawned from a proposal
/// whose origin is being hard-deleted.
///
/// Guarded to still-`pending` proposals: a `spawned`/`promoted` proposal's
/// deterministic path is the LIVE decoupled fork/refinement, which survives origin
/// deletion and must NOT be touched. Best-effort: a failure to list / clean /
/// dismiss is logged at WARN and never blocks the delete.
async fn handle_cleanup_on_hard_delete(db: &crate::db::Database, origin_id: &str) {
    let pending = match db.list_fork_proposals_for_origin(origin_id).await {
        Ok(proposals) => proposals
            .into_iter()
            .filter(|p| p.status == ForkProposalStatus::Pending)
            .collect::<Vec<_>>(),
        Err(e) => {
            tracing::warn!(
                conv_id = %origin_id,
                error = %e,
                "fork orphan cleanup on delete: failed to list proposals; orphans may remain"
            );
            return;
        }
    };
    if pending.is_empty() {
        return;
    }

    let repo_root = fork_origin_repo_root(db, origin_id).await;
    let proposals_for_git = pending.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let _guard = TASK_APPROVAL_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for proposal in &proposals_for_git {
            clean_deterministic_fork_orphans(repo_root.as_deref(), proposal);
        }
    })
    .await;

    // Dismiss each cleaned pending proposal so a command queued behind this one
    // sees it non-`pending` and refuses to spawn from a deleted origin.
    if let Err(e) = db.retire_pending_fork_proposals_for_origin(origin_id).await {
        tracing::warn!(
            conv_id = %origin_id,
            error = %e,
            "fork orphan cleanup on delete: failed to dismiss pending proposals"
        );
    }
}

/// Startup reconciliation (REQ-PROJ-035): retire every still-`pending` fork
/// proposal whose origin conversation is already terminal. A crash AFTER the
/// origin was marked terminal but BEFORE `retire_fork_proposals_for_terminal_origin`
/// ran leaves the proposal `pending` across restart — so approve/request-changes
/// return 409 (origin terminal) forever while `GET /proposals` still reports it
/// `pending`, stranding a Review action that can never spawn/promote. This sweep
/// converges each such row to `dismissed` and cleans its deterministic orphans.
///
/// Runs at startup, before the fork-resolution consumer exists, so there is no
/// concurrent approve to serialise against — a direct DB dismiss + orphan clean
/// is sufficient. Best-effort: a failure to list / clean / dismiss is logged at
/// WARN and never blocks startup. Proposals whose origin is still live are left
/// pending (the live Review path). Resolved (`spawned`/`promoted`/`dismissed`)
/// proposals are excluded by the `pending`-only query, so a live decoupled
/// child's deterministic path is never touched.
pub(crate) async fn reconcile_terminal_origin_fork_proposals(db: &crate::db::Database) {
    let pending = match db.list_pending_fork_proposals().await {
        Ok(proposals) => proposals,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "fork startup reconciliation: failed to list pending proposals; stale rows may linger"
            );
            return;
        }
    };
    if pending.is_empty() {
        return;
    }

    // Group by origin so each terminal origin is retired once (clean its
    // deterministic orphans, then dismiss all its pending rows in one update).
    let mut by_origin: std::collections::HashMap<String, Vec<ForkProposal>> =
        std::collections::HashMap::new();
    for proposal in pending {
        by_origin
            .entry(proposal.origin_conversation_id.clone())
            .or_default()
            .push(proposal);
    }

    let mut retired_origins = 0usize;
    for (origin_id, proposals) in by_origin {
        // Re-read the origin's authoritative state. A missing origin row (a
        // racing/partial delete) is treated as terminal: its proposals can
        // never resolve, so retiring them is correct and matches the cascade.
        let is_terminal = match db.get_conversation(&origin_id).await {
            Ok(conv) => conv.state.is_terminal(),
            Err(e) => {
                tracing::warn!(
                    conv_id = %origin_id,
                    error = %e,
                    "fork startup reconciliation: origin conversation unreadable; treating as terminal and retiring its pending proposals"
                );
                true
            }
        };
        if !is_terminal {
            continue;
        }

        let repo_root = fork_origin_repo_root(db, &origin_id).await;
        let _ = tokio::task::spawn_blocking(move || {
            let _guard = TASK_APPROVAL_MUTEX
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for proposal in &proposals {
                clean_deterministic_fork_orphans(repo_root.as_deref(), proposal);
            }
        })
        .await;

        if let Err(e) = db
            .retire_pending_fork_proposals_for_origin(&origin_id)
            .await
        {
            tracing::warn!(
                conv_id = %origin_id,
                error = %e,
                "fork startup reconciliation: failed to dismiss pending proposals"
            );
        } else {
            retired_origins += 1;
        }
    }

    if retired_origins > 0 {
        tracing::info!(
            origins = retired_origins,
            "Retired stale pending fork proposals whose origin is terminal"
        );
    }
}

impl RuntimeManager {
    /// The single serialized fork-resolution consumer. Owns all fork-proposal
    /// resolution and cleanup: each [`ForkCommand`] is processed to completion
    /// (full git + DB + dispatch) before the next is taken, so mutual exclusion
    /// is structural — there is no lock for any entry point to forget.
    ///
    /// Wired into [`RuntimeManager::start_sub_agent_handler`]'s `select!` loop,
    /// so it shares that task's lifecycle and ends when every `fork_cmd_tx` clone
    /// drops (no standalone task, no reference cycle to leak).
    pub(crate) async fn handle_fork_command(self: &std::sync::Arc<Self>, cmd: ForkCommand) {
        match cmd {
            ForkCommand::Approve { proposal_id, reply } => {
                let _ = reply.send(self.handle_approve(&proposal_id).await);
            }
            ForkCommand::RequestChanges {
                proposal_id,
                change_request,
                reply,
            } => {
                let _ = reply.send(
                    self.handle_request_changes(&proposal_id, change_request)
                        .await,
                );
            }
            ForkCommand::Dismiss { proposal_id, reply } => {
                let _ = reply.send(self.handle_dismiss(&proposal_id).await);
            }
            ForkCommand::RetireForOrigin { origin_id, reply } => {
                handle_retire_for_origin(&self.db, &origin_id).await;
                let _ = reply.send(());
            }
            ForkCommand::CleanupOnHardDelete { origin_id, reply } => {
                handle_cleanup_on_hard_delete(&self.db, &origin_id).await;
                let _ = reply.send(());
            }
        }
    }

    /// Thin sender: enqueue an approve on the fork-resolution consumer and await
    /// its reply. Endpoint call sites are unchanged — they still call
    /// `state.runtime.approve_fork_proposal(...)`.
    pub(crate) async fn approve_fork_proposal(
        self: &std::sync::Arc<Self>,
        proposal_id: &str,
    ) -> Result<String, ForkResolveError> {
        let (reply, reply_rx) = oneshot::channel();
        self.fork_cmd_tx
            .send(ForkCommand::Approve {
                proposal_id: proposal_id.to_string(),
                reply,
            })
            .await
            .map_err(|_| fork_consumer_gone())?;
        reply_rx.await.map_err(|_| fork_consumer_gone())?
    }

    /// Thin sender for request-changes; see [`RuntimeManager::approve_fork_proposal`].
    pub(crate) async fn request_changes_on_fork_proposal(
        self: &std::sync::Arc<Self>,
        proposal_id: &str,
        change_request: String,
    ) -> Result<String, ForkResolveError> {
        let (reply, reply_rx) = oneshot::channel();
        self.fork_cmd_tx
            .send(ForkCommand::RequestChanges {
                proposal_id: proposal_id.to_string(),
                change_request,
                reply,
            })
            .await
            .map_err(|_| fork_consumer_gone())?;
        reply_rx.await.map_err(|_| fork_consumer_gone())?
    }

    /// Thin sender for dismiss; see [`RuntimeManager::approve_fork_proposal`].
    pub(crate) async fn dismiss_fork_proposal(
        self: &std::sync::Arc<Self>,
        proposal_id: &str,
    ) -> Result<bool, ForkResolveError> {
        let (reply, reply_rx) = oneshot::channel();
        self.fork_cmd_tx
            .send(ForkCommand::Dismiss {
                proposal_id: proposal_id.to_string(),
                reply,
            })
            .await
            .map_err(|_| fork_consumer_gone())?;
        reply_rx.await.map_err(|_| fork_consumer_gone())?
    }

    /// Fast-path read: does `origin_id` have any `pending` fork proposal worth
    /// engaging the resolution consumer for? A terminal / being-deleted origin
    /// creates no new proposals, so a no-`pending` read is authoritative and the
    /// common case (no proposals at all) skips the consumer round-trip entirely.
    /// On a read error, return `true` so the consumer still re-reads
    /// authoritatively rather than silently skipping cleanup.
    async fn origin_has_pending_fork_proposal(&self, origin_id: &str) -> bool {
        match self.db.list_fork_proposals_for_origin(origin_id).await {
            Ok(proposals) => proposals
                .iter()
                .any(|p| p.status == ForkProposalStatus::Pending),
            Err(e) => {
                tracing::warn!(conv_id = %origin_id, error = %e, "fork cleanup fast-path read failed; engaging consumer");
                true
            }
        }
    }

    /// Thin sender for retire-on-terminal; awaits the best-effort cleanup so the
    /// terminal hook observes completion. See [`handle_retire_for_origin`].
    #[cfg(test)]
    pub(crate) async fn retire_fork_proposals_for_terminal_origin(
        self: &std::sync::Arc<Self>,
        origin_id: &str,
    ) {
        if !self.origin_has_pending_fork_proposal(origin_id).await {
            return;
        }
        let (reply, reply_rx) = oneshot::channel();
        if self
            .fork_cmd_tx
            .send(ForkCommand::RetireForOrigin {
                origin_id: origin_id.to_string(),
                reply,
            })
            .await
            .is_err()
        {
            tracing::warn!(conv_id = %origin_id, "fork retirement: consumer gone; skipped");
            return;
        }
        let _ = reply_rx.await;
    }

    /// Thin sender for hard-delete cleanup; awaits completion so the cascade only
    /// proceeds to the row delete once pending proposals are dismissed under
    /// serialization. See [`handle_cleanup_on_hard_delete`].
    pub(crate) async fn cleanup_pending_fork_orphans_on_delete(
        self: &std::sync::Arc<Self>,
        origin_id: &str,
    ) {
        if !self.origin_has_pending_fork_proposal(origin_id).await {
            return;
        }
        let (reply, reply_rx) = oneshot::channel();
        if self
            .fork_cmd_tx
            .send(ForkCommand::CleanupOnHardDelete {
                origin_id: origin_id.to_string(),
                reply,
            })
            .await
            .is_err()
        {
            tracing::warn!(conv_id = %origin_id, "fork orphan cleanup on delete: consumer gone; skipped");
            return;
        }
        let _ = reply_rx.await;
    }
}

/// The fork-resolution consumer is unreachable (channel closed / reply sender
/// dropped). Surfaces as a clear internal error rather than a silent hang.
fn fork_consumer_gone() -> ForkResolveError {
    ForkResolveError::Internal("fork-resolution consumer is unavailable".to_string())
}

/// Resolve the repo root for a fork origin's project, for orphan cleanup.
/// `None` when the conversation is not project-scoped or the project can't be
/// loaded; otherwise the git toplevel (falling back to the project's canonical
/// path when the repo can't be detected on disk).
async fn fork_origin_repo_root(
    db: &crate::db::Database,
    origin_id: &str,
) -> Option<std::path::PathBuf> {
    let conv = db.get_conversation(origin_id).await.ok()?;
    let project_id = conv.project_id.as_deref()?;
    let project = db.get_project(project_id).await.ok()?;
    Some(
        phoenix_core::git::detect_git_repo_root(Path::new(&project.canonical_path)).map_or_else(
            || std::path::PathBuf::from(&project.canonical_path),
            std::path::PathBuf::from,
        ),
    )
}

/// The deterministic worktree path for a fork/refinement conversation id:
/// `{repo_root}/.phoenix/worktrees/{conv_id}`. The single source of truth for
/// where an approve/promote cuts its worktree, so orphan-cleanup path comparisons
/// can't drift from `create_worktree`'s layout.
fn worktree_path(repo_root: &Path, conv_id: &str) -> std::path::PathBuf {
    repo_root.join(".phoenix/worktrees").join(conv_id)
}

/// Remove one deterministic orphan worktree (`.phoenix/worktrees/{conv_id}`) and
/// its branch, if present. Best-effort — every step is non-fatal.
///
/// `candidate_branch` is the deterministic branch name the crashed attempt would
/// have created (see [`deterministic_fork_branch_name`]). It is deleted ONLY when
/// it is PROVABLY this proposal's fork orphan — i.e. a worktree admin entry for
/// the deterministic path exists/existed on it (see [`delete_branch_if_unused`]).
/// A standalone branch with the same derived name but no deterministic-worktree
/// association is indistinguishable from user work and is left in place.
fn clean_one_orphan(repo_root: Option<&Path>, conv_id: &str, candidate_branch: Option<&str>) {
    let Some(repo_root) = repo_root else {
        return;
    };
    let worktree_path = worktree_path(repo_root, conv_id);
    if !worktree_path.exists() {
        // No worktree dir. The branch is cleanable only if a stale admin entry
        // still ties it to THIS deterministic path; a standalone branch with the
        // derived name (user work) must never be force-deleted.
        if let Some(branch) = candidate_branch {
            delete_branch_if_unused(repo_root, conv_id, branch);
        }
        return;
    }
    let worktree_str = worktree_path.to_string_lossy().to_string();

    // Capture the branch BEFORE removal — once the worktree is gone `git
    // worktree list` no longer associates the branch with this path.
    let branch = orphan_branch_name(repo_root, conv_id);

    if let Err(e) = run_git(repo_root, &["worktree", "remove", &worktree_str, "--force"]) {
        tracing::warn!(
            conv_id = %conv_id,
            worktree = %worktree_str,
            error = %e,
            "fork orphan cleanup: git worktree remove failed; trying filesystem fallback"
        );
        if worktree_path.exists() {
            if let Err(rm_err) = std::fs::remove_dir_all(&worktree_path) {
                tracing::warn!(
                    conv_id = %conv_id,
                    worktree = %worktree_str,
                    error = %rm_err,
                    "fork orphan cleanup: filesystem fallback failed; orphan may remain"
                );
            }
        }
        let _ = run_git(repo_root, &["worktree", "prune"]);
    }

    // The deterministic orphan's branch is a fork task branch or an Explore
    // temp branch created by the crashed attempt — never the user's PR branch,
    // so deleting it is safe.
    if let Some(branch) = branch {
        if let Err(e) = run_git(repo_root, &["branch", "-D", &branch]) {
            tracing::warn!(
                conv_id = %conv_id,
                branch = %branch,
                error = %e,
                "fork orphan cleanup: branch delete failed (non-fatal)"
            );
        }
    }
}

/// Delete the deterministic orphan's branch ref, but ONLY when it is provably
/// THIS proposal's fork orphan: a worktree admin entry for the deterministic path
/// `worktree_path(conv_id)` is checked out on it. That entry is what a crashed
/// `git worktree add -b` leaves once it has registered the worktree (even if the
/// checkout dir was later removed), so its presence proves the branch is the
/// fork's own. In that case the (now dir-less) worktree entry is pruned and the
/// branch deleted.
///
/// A branch with this derived name that is checked out at a DIFFERENT path, or
/// has NO worktree entry at all (a standalone `git branch` ref — indistinguishable
/// from unrelated user work), is left untouched. Consequence: the ultra-rare crash
/// that created the branch ref before any worktree admin entry leaves the branch in
/// place and the proposal unapprovable until the user removes it — the correct
/// safety trade-off versus deleting user branches. Best-effort — failures are
/// logged at WARN.
fn delete_branch_if_unused(repo_root: &Path, conv_id: &str, branch: &str) {
    let branch_ref = format!("refs/heads/{branch}");
    let exists = run_git(
        repo_root,
        &["rev-parse", "--verify", "--quiet", &branch_ref],
    )
    .is_ok();
    if !exists {
        return;
    }
    let deterministic_path = worktree_path(repo_root, conv_id);
    let is_deterministic_orphan = find_branch_in_worktree_list(repo_root, branch)
        .is_some_and(|p| Path::new(&p) == deterministic_path);
    if !is_deterministic_orphan {
        // Not tied to this fork's deterministic worktree path — could be user work
        // that merely collides on the derived name. Never force-delete it.
        return;
    }
    // Prune the stale (dir-less) deterministic worktree entry so the branch is no
    // longer referenced, then delete it.
    let _ = run_git(repo_root, &["worktree", "prune"]);
    if let Err(e) = run_git(repo_root, &["branch", "-D", branch]) {
        tracing::warn!(
            conv_id = %conv_id,
            branch = %branch,
            error = %e,
            "fork orphan cleanup: deterministic-orphan branch delete failed (non-fatal)"
        );
    }
}

/// Resolve the branch name an orphaned deterministic worktree had, by matching
/// the worktree path in `git worktree list` BEFORE removal. Returns `None` when
/// the worktree is already gone or had no branch (detached).
fn orphan_branch_name(repo_root: &Path, conv_id: &str) -> Option<String> {
    let worktree_path = worktree_path(repo_root, conv_id);
    let listing = run_git(repo_root, &["worktree", "list", "--porcelain"]).ok()?;
    let mut current_path: Option<&str> = None;
    for line in listing.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            current_path = Some(p);
        } else if let Some(b) = line.strip_prefix("branch ") {
            if current_path == Some(worktree_path.to_string_lossy().as_ref()) {
                return b.strip_prefix("refs/heads/").map(String::from);
            }
        }
    }
    None
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
/// What `prepare_*_blocking` must do with the deterministic branch + worktree on
/// (re)try, given the current git state. Distinguishing `Adopt` from `Recreate`
/// is what makes a crashed-then-pruned attempt recover instead of looping: a
/// porcelain entry whose checkout directory was deleted ("prunable") still
/// reports the branch at the deterministic path, but adopting it writes files at
/// a path that is not a real worktree and `git add` fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BranchDisposition {
    /// Branch does not exist (or its only worktree was a pruned-away orphan whose
    /// branch was cleaned): create a fresh worktree with `-b` off the base.
    Create,
    /// Branch is checked out in this proposal's OWN, REAL (present, non-prunable)
    /// deterministic worktree from a crashed prior attempt: skip `create_worktree`
    /// and converge the existing checkout in place.
    Adopt,
}

/// True when `worktree_path` is an ACTUAL, present worktree — the directory
/// exists on disk AND git recognises it as a work tree (`rev-parse
/// --is-inside-work-tree` succeeds). A prunable porcelain entry (admin record
/// present, checkout dir deleted) fails this, so it is never adopted into.
fn is_real_present_worktree(worktree_path: &Path) -> bool {
    worktree_path.is_dir()
        && run_git(worktree_path, &["rev-parse", "--is-inside-work-tree"])
            .map(|out| out.trim() == "true")
            .unwrap_or(false)
}

/// Decide what to do with the deterministic branch/worktree for this proposal.
///
/// - branch absent → [`BranchDisposition::Create`].
/// - branch checked out in this proposal's own, REAL deterministic worktree →
///   [`BranchDisposition::Adopt`] (crash-recovery re-use, no duplicate id).
/// - branch reported at the deterministic path but that worktree is PRUNABLE
///   (checkout dir gone): not adoptable. Prune the stale admin entry and delete
///   the now-unreferenced deterministic branch, then [`BranchDisposition::Create`]
///   so the caller recreates a clean worktree off the base. Deleting the branch
///   is safe — it is this proposal's own deterministic branch from a crashed
///   attempt and (post-prune) is checked out nowhere.
/// - branch checked out in some OTHER worktree → `Err(Conflict)`: a real
///   collision with a distinct unit of work.
/// - branch exists but is checked out nowhere → `Err(Conflict)`: same.
fn classify_branch_collision(
    repo_root: &Path,
    worktree_path: &Path,
    branch_name: &str,
) -> Result<BranchDisposition, ForkResolveError> {
    let branch_ref = format!("refs/heads/{branch_name}");
    let exists = run_git(
        repo_root,
        &["rev-parse", "--verify", "--quiet", &branch_ref],
    )
    .is_ok();
    if !exists {
        return Ok(BranchDisposition::Create);
    }
    // Branch exists. It is adoptable only when it is checked out in THIS
    // proposal's own deterministic worktree.
    let checked_out_here = find_branch_in_worktree_list(repo_root, branch_name)
        .is_some_and(|p| Path::new(&p) == worktree_path);
    if !checked_out_here {
        return Err(ForkResolveError::Conflict(format!(
            "branch '{branch_name}' already exists and is not this fork's own worktree — \
             a fork must name a distinct unit of work"
        )));
    }
    // The porcelain entry ties the branch to the deterministic path, but that is
    // only adoptable if the checkout directory is actually a present worktree. A
    // prunable entry (dir deleted) must be pruned + recreated, not adopted.
    if is_real_present_worktree(worktree_path) {
        return Ok(BranchDisposition::Adopt);
    }
    tracing::info!(
        branch = branch_name,
        worktree = %worktree_path.display(),
        "deterministic worktree is prunable (checkout dir gone); pruning + recreating instead of adopting"
    );
    // Drop the stale admin entry, then delete the orphaned deterministic branch
    // so `create_worktree`'s `-b` can recreate it cleanly off the base. Both are
    // best-effort: a failure surfaces as the original create error downstream.
    if let Err(e) = run_git(repo_root, &["worktree", "prune"]) {
        tracing::warn!(error = %e, "git worktree prune of stale deterministic entry failed (non-fatal)");
    }
    if let Err(e) = run_git(repo_root, &["branch", "-D", branch_name]) {
        tracing::warn!(
            error = %e,
            branch = branch_name,
            "deleting orphaned deterministic branch after prune failed (non-fatal)"
        );
    }
    Ok(BranchDisposition::Create)
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
    async fn handle_approve(
        self: &std::sync::Arc<Self>,
        proposal_id: &str,
    ) -> Result<String, ForkResolveError> {
        // No lock: the fork-resolution consumer is single-threaded, so this
        // whole critical section (precondition check + git phase + DB resolve)
        // already runs to completion before any cleanup command is taken.
        let ctx = self.load_resolvable_proposal(proposal_id).await?;
        let fork_conv_id = derive_conv_id(proposal_id, ResolutionKind::Spawn);
        let proposal = ctx.proposal.clone();

        let prepared = {
            let fork_conv_id = fork_conv_id.clone();
            tokio::task::spawn_blocking(move || prepare_spawn_blocking(&ctx, &fork_conv_id))
                .await
                .map_err(|e| ForkResolveError::Internal(format!("spawn task panicked: {e}")))??
        };

        // Close the during-git terminal race: the pre-git `load_resolvable_proposal`
        // check can be made stale by the origin reaching a terminal state WHILE the
        // blocking git phase ran (the RetireForOrigin command is serialized BEHIND
        // this in-flight resolve, so it cannot moot the proposal in time). Re-read
        // the origin now; if it went terminal, abort before recording `spawned` —
        // retire the proposal + clean the orphan we just created — and 409.
        self.abort_if_origin_now_terminal(&proposal).await?;

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
    async fn handle_request_changes(
        self: &std::sync::Arc<Self>,
        proposal_id: &str,
        change_request: String,
    ) -> Result<String, ForkResolveError> {
        // No lock: serialized by the single fork-resolution consumer.
        let ctx = self.load_resolvable_proposal(proposal_id).await?;
        let refinement_conv_id = derive_conv_id(proposal_id, ResolutionKind::Promote);
        let proposal = ctx.proposal.clone();

        let prepared = {
            let refinement_conv_id = refinement_conv_id.clone();
            tokio::task::spawn_blocking(move || {
                prepare_promote_blocking(&ctx, &refinement_conv_id, &change_request)
            })
            .await
            .map_err(|e| ForkResolveError::Internal(format!("spawn task panicked: {e}")))??
        };

        // See `handle_approve`: re-check origin liveness after the git phase to
        // close the during-git terminal race before recording `promoted`.
        self.abort_if_origin_now_terminal(&proposal).await?;

        self.db
            .resolve_fork_proposal_promoted(proposal_id, &prepared.conv, &[prepared.seed])
            .await
            .map_err(map_db_resolve_error)?;

        self.get_or_create(&refinement_conv_id)
            .await
            .map_err(ForkResolveError::Internal)?;
        Ok(refinement_conv_id)
    }

    /// Dismiss a fork proposal (`ForkProposalDismissed`): clean any deterministic
    /// spawn/promote git orphan a crashed approve/promote left behind for a
    /// still-`pending` proposal (`DeterministicForkOrphansCleaned` BEFORE
    /// `status = dismissed`), then record the dismissal. Without this, an orphan
    /// left under a proposal that is then dismissed leaks forever — the
    /// terminal/hard-delete cleanup paths only consider `pending` proposals.
    ///
    /// Serialized by the single fork-resolution consumer, so the `pending` re-read
    /// is authoritative against a concurrent approve/promote. Idempotent:
    /// dismissing an already-resolved (`spawned`/`promoted`/`dismissed`) proposal
    /// cleans nothing — its deterministic path is the LIVE decoupled child — and is
    /// a no-op.
    ///
    /// Returns `true` when the row transitioned `pending -> dismissed`, `false`
    /// when it was already resolved (the endpoint reports the latter as `no_op`).
    ///
    /// # Errors
    ///
    /// [`ForkResolveError::Internal`] for a DB failure reading or updating the row.
    async fn handle_dismiss(&self, proposal_id: &str) -> Result<bool, ForkResolveError> {
        let proposal = self
            .db
            .get_fork_proposal(proposal_id)
            .await
            .map_err(|e| ForkResolveError::Internal(e.to_string()))?
            .ok_or_else(|| ForkResolveError::NotFound(format!("fork proposal {proposal_id}")))?;

        if proposal.status != ForkProposalStatus::Pending {
            return Ok(false);
        }

        let repo_root = fork_origin_repo_root(&self.db, &proposal.origin_conversation_id).await;
        let proposal_for_git = proposal.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let _guard = TASK_APPROVAL_MUTEX
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            clean_deterministic_fork_orphans(repo_root.as_deref(), &proposal_for_git);
        })
        .await;

        self.db
            .dismiss_fork_proposal(proposal_id)
            .await
            .map_err(|e| ForkResolveError::Internal(e.to_string()))
    }

    /// Retire a still-`pending` proposal whose origin has gone terminal: clean
    /// its deterministic spawn/promote orphans, then dismiss it. Called from the
    /// approve/request-changes path (already inside the single fork-resolution
    /// consumer, so this is serialized), so a user's retry converges a row left
    /// `pending` by a crash between the origin going terminal and
    /// `retire_fork_proposals_for_terminal_origin` running. Best-effort: a clean
    /// or dismiss failure is logged at WARN and never masks the returned conflict.
    /// A guard skips already-resolved proposals so a live decoupled child's
    /// deterministic path is never touched.
    async fn retire_terminal_origin_proposal(&self, proposal: &ForkProposal) {
        if proposal.status != ForkProposalStatus::Pending {
            return;
        }
        let repo_root = fork_origin_repo_root(&self.db, &proposal.origin_conversation_id).await;
        let proposal_for_git = proposal.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let _guard = TASK_APPROVAL_MUTEX
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            clean_deterministic_fork_orphans(repo_root.as_deref(), &proposal_for_git);
        })
        .await;
        if let Err(e) = self.db.dismiss_fork_proposal(&proposal.id).await {
            tracing::warn!(
                proposal_id = %proposal.id,
                error = %e,
                "terminal-origin retire: failed to dismiss stale pending proposal"
            );
        }
    }

    /// Re-read the origin AFTER the blocking git phase and, if it has reached a
    /// terminal state since `load_resolvable_proposal` checked, ABORT the
    /// spawn/promote: retire the proposal (dismiss + clean the deterministic
    /// orphan the git phase just created) and return the terminal-origin
    /// `Conflict` WITHOUT recording `spawned`/`promoted`. This closes the
    /// during-git race where the origin goes terminal while the worktree is being
    /// built and the serialized `RetireForOrigin` command cannot moot the proposal
    /// in time. `Ok(())` means the origin is still live — proceed to resolve.
    ///
    /// A DB read failure here is non-fatal to liveness: we cannot prove the origin
    /// terminal, so we proceed (the pre-git check already passed) rather than
    /// abort a valid resolve on a transient error.
    async fn abort_if_origin_now_terminal(
        &self,
        proposal: &ForkProposal,
    ) -> Result<(), ForkResolveError> {
        let still_terminal = match self
            .db
            .get_conversation(&proposal.origin_conversation_id)
            .await
        {
            Ok(origin) => origin.state.is_terminal(),
            Err(e) => {
                tracing::warn!(
                    proposal_id = %proposal.id,
                    error = %e,
                    "post-git origin re-read failed; proceeding with resolve (cannot prove terminal)"
                );
                false
            }
        };
        if !still_terminal {
            return Ok(());
        }
        // Origin went terminal during the git phase. Undo the in-flight resolve:
        // retire the proposal (dismiss + clean its deterministic orphan) under the
        // fork actor's serialization, then surface the conflict so nothing is
        // recorded as spawned/promoted and no child is started.
        self.retire_terminal_origin_proposal(proposal).await;
        Err(ForkResolveError::Conflict(
            "the originating conversation reached a terminal state during fork \
             resolution — the fork was not created"
                .to_string(),
        ))
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
            // Converge the stale row instead of only 409-ing forever: retire the
            // proposal (dismiss + clean its deterministic orphans) under the fork
            // actor's serialization, so a user's retry self-heals a pending row
            // left behind by a crash between the origin going terminal and
            // `retire_fork_proposals_for_terminal_origin` running. We still return
            // the conflict — this resolve cannot spawn/promote — but the proposal
            // is now `dismissed`, so `GET /proposals` stops offering a dead Review.
            self.retire_terminal_origin_proposal(&proposal).await;
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

        let repo_root = phoenix_core::git::detect_git_repo_root(Path::new(&project.canonical_path))
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

/// Write + promote + commit the fork's task file into `worktree_path`, driving
/// the worktree to the target state regardless of which crash point a retry hit:
/// the in-progress task file (taskmd) or plain brief is COMMITTED on the branch.
///
/// - taskmd: promote status to in-progress (rename) before committing; the file
///   is staged at its (possibly renamed) tasks-dir path.
/// - plain brief: no status segment, committed verbatim at its own path.
///
/// ADOPTED-worktree idempotency: a prior approve may have crashed at ANY point —
/// after the rename but before `git add`/`commit`, or after the commit but before
/// the DB `spawned` resolution. "Exists on disk" is NOT "committed": a retry that
/// merely saw the in-progress filename and skipped would leave a renamed-but-
/// uncommitted file, violating the "approved task is committed on the fork branch"
/// invariant. So when adopting with the file already at the in-progress name, the
/// body-WRITE and the PROMOTE are skipped (the file is already renamed on disk),
/// but `git add` of the in-progress path (and staging removal of any leftover
/// `...-ready--{slug}.md`) STILL runs, followed by a commit GUARDED by
/// `git diff --cached --quiet` so it is a no-op when nothing is staged. This
/// converges every crash point to a single committed in-progress task file:
///
/// - no task file on disk → write body, promote, add, commit;
/// - ready on disk, uncommitted → promote, add, commit;
/// - in-progress on disk, uncommitted → add, commit;
/// - in-progress committed → add (no-op), `diff --cached --quiet` true → skip commit.
fn materialize_fork_task_file(
    worktree_path: &Path,
    proposal: &ForkProposal,
    source: &TaskSource,
    filename: &str,
    task_id: &str,
    adopt: bool,
) -> Result<(), ForkResolveError> {
    let parent_rel = Path::new(&proposal.task_file).parent();
    let rebuild_rel = |fname: &str| match parent_rel {
        Some(p) if !p.as_os_str().is_empty() => format!("{}/{fname}", p.to_string_lossy()),
        _ => fname.to_string(),
    };
    let tasks_dir =
        parent_rel.map_or_else(|| worktree_path.join("tasks"), |p| worktree_path.join(p));

    // Adopt-with-already-renamed: a prior attempt renamed the taskmd file to its
    // in-progress name on disk; skip re-writing the body and re-promoting (which
    // would resurrect a duplicate ready file / collide on the id), but still stage
    // + commit below so a rename that was never committed still gets committed.
    let already_renamed_in_progress = match source {
        TaskSource::Taskmd {
            id, priority, slug, ..
        } if adopt => {
            let in_progress_filename = taskmd_core::filename::format_filename(
                id,
                (*priority).into(),
                taskmd_core::constants::Status::InProgress,
                slug,
            );
            tasks_dir
                .join(&in_progress_filename)
                .exists()
                .then(|| rebuild_rel(&in_progress_filename))
        }
        _ => None,
    };

    let committed_path = match (&already_renamed_in_progress, source) {
        (Some(in_progress_rel), _) => in_progress_rel.clone(),
        (None, TaskSource::Taskmd { id, status, .. }) => {
            write_body_to_worktree(worktree_path, &proposal.task_file, &proposal.body)?;
            let final_filename =
                promote_task_status_to_in_progress(&tasks_dir, id, *status, filename)
                    .map_err(ForkResolveError::Internal)?;
            rebuild_rel(&final_filename)
        }
        (None, TaskSource::PlainMarkdown { .. }) => {
            write_body_to_worktree(worktree_path, &proposal.task_file, &proposal.body)?;
            proposal.task_file.clone()
        }
    };

    ensure_local_exclude_has_phoenix(worktree_path).map_err(ForkResolveError::Internal)?;

    // If a taskmd rename happened, stage the old name's deletion too so the
    // commit is a rename, not a duplicate id.
    if committed_path != proposal.task_file {
        let _ = run_git(worktree_path, &["add", "--", &proposal.task_file]);
    }
    run_git(worktree_path, &["add", "--", &committed_path]).map_err(ForkResolveError::Internal)?;
    let commit_msg = format!("task {task_id}: {}", proposal.title);
    if run_git(worktree_path, &["diff", "--cached", "--quiet"]).is_err() {
        run_git(worktree_path, &["commit", "-m", &commit_msg])
            .map_err(|e| ForkResolveError::Internal(format!("failed to commit task file: {e}")))?;
    }
    Ok(())
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

    let worktree_path = worktree_path(repo_root, fork_conv_id);
    let disposition = classify_branch_collision(repo_root, &worktree_path, &branch_name)?;
    let adopt = disposition == BranchDisposition::Adopt;
    if !adopt {
        create_worktree(
            repo_root,
            fork_conv_id,
            &branch_name,
            Some(&start_point),
            PhoenixIgnoreStrategy::LocalExclude,
        )?;
    }
    let worktree_path_str = worktree_path.to_string_lossy().to_string();

    materialize_fork_task_file(&worktree_path, proposal, &source, filename, &task_id, adopt)?;

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

    let worktree_path = worktree_path(repo_root, refinement_conv_id);
    let adopt = classify_branch_collision(repo_root, &worktree_path, &temp_branch)?
        == BranchDisposition::Adopt;
    if !adopt {
        create_worktree(
            repo_root,
            refinement_conv_id,
            &temp_branch,
            Some(&start_point),
            PhoenixIgnoreStrategy::LocalExclude,
        )?;
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

    // Do NOT stage/append `.gitignore` here. The worktree was created with
    // LocalExclude, and a linked worktree shares the main repo's
    // `.git/info/exclude` (common dir), so `.phoenix/` is already ignored without
    // staging anything. The refinement must start with ONLY the drafted brief as
    // uncommitted work — a staged `.gitignore` edit would be swept into the task
    // branch by a later Explore approval (which commits everything staged).
    // Nothing is committed — the draft sits uncommitted on the temp branch for
    // the Explore agent to revise (REQ-PROJ-037).

    let conv_mode = ConvMode::Explore {
        worktree_path: Some(nes(&worktree_path_str, "worktree path")?),
        next_taskmd_id_hint: None,
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
    // Derive the slug from the FULL deterministic conv id (a globally-unique
    // UUID), not just its first 8 chars: the slug column is UNIQUE, and with the
    // `ON CONFLICT(id) DO NOTHING` insert a slug clash with a DISTINCT
    // conversation would no longer be silently swallowed but would fail the
    // resolve. Using the whole id makes such a clash unreachable while keeping a
    // same-id crash-retry idempotent (same id → same slug).
    let slug = format!("fork-{conv_id}");
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
        transcript_generation: 1,
        model: None,
        project_id: Some(project_id.to_string()),
        conv_mode,
        desired_base_branch: Some(base.to_string()),
        message_count: 0,
        seed_parent_id: None,
        seed_label: None,
        continued_in_conv_id: None,
        chain_name: None,
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
        | DbError::Serialization(_)
        | DbError::ConversationAlreadyExists(_) => ForkResolveError::Internal(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, ForkProposal, ForkProposalStatus};
    use crate::platform::PlatformCapability;
    use crate::runtime::RuntimeManager;
    use crate::tools::mcp::McpClientManager;
    use phoenix_llm::ModelRegistry;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = phoenix_core::git::command()
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
        // Canonicalize so the root matches what `git rev-parse --show-toplevel`
        // (and `git worktree list --porcelain`) report. On macOS the tempdir lives
        // under `/var/folders/...`, a symlink to `/private/var/...`; git always
        // emits the resolved path. Production feeds orphan cleanup a git-toplevel
        // root, so path-equality checks compare canonical-to-canonical — the test
        // root must too, or `delete_branch_if_unused`'s deterministic-path match
        // spuriously fails.
        let root = tmp.path().canonicalize().unwrap();
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
        let rt = Arc::new(RuntimeManager::new(
            db,
            Arc::new(ModelRegistry::new_empty()),
            PlatformCapability::None {
                details: "test".into(),
            },
            Arc::new(McpClientManager::new()),
            None,
        ));
        // Start the background handler so the single serialized fork-resolution
        // consumer is running: every public approve/request-changes/dismiss/retire
        // call resolves through the actor's channel, not inline.
        rt.start_sub_agent_handler().await;
        rt
    }

    /// Create a project + a live Direct-mode origin conversation in it. Returns
    /// (`project_id`, `origin_id`).
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
            &[
                "log",
                "-1",
                "--name-only",
                "--pretty=format:",
                "task-12345-fix-thing",
            ],
        );
        assert!(
            committed.contains("12345-p1-in-progress--fix-thing.md"),
            "committed file should be in-progress: {committed}"
        );

        // Proposal resolved as spawned with the fork id.
        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Spawned);
        assert_eq!(
            resolved.fork_conversation_id.as_deref(),
            Some(fork_id.as_str())
        );

        // Origin is UNCHANGED: no continuation, no handoff, no new transcript msg.
        let origin_conv = db.get_conversation(&origin).await.unwrap();
        assert!(origin_conv.continued_in_conv_id.is_none());
        assert_eq!(db.get_messages(&origin).await.unwrap().len(), 0);
    }

    /// Bug 3: cutting a fork's worktree must NOT dirty or stage the ORIGIN
    /// checkout's tracked `.gitignore`. In a repo whose `.gitignore` does not list
    /// `.phoenix/`, a fork spawn ignores `.phoenix/` via the repo's local untracked
    /// `.git/info/exclude` — so the origin's index/working tree is unaffected and
    /// `.phoenix/` does not show as untracked.
    #[tokio::test]
    async fn fork_spawn_does_not_stage_or_modify_origin_gitignore() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        // Precondition: the origin checkout is clean and `.gitignore` (if any) does
        // not already list `.phoenix/`.
        assert_eq!(
            git(&repo, &["status", "--porcelain"]),
            "",
            "origin must start clean"
        );
        let pid = insert_pending(
            &db,
            &origin,
            "tasks/12345-p1-ready--fix-thing.md",
            "# Fix the thing\n",
        )
        .await;
        let rt = make_runtime(db.clone()).await;

        rt.approve_fork_proposal(&pid).await.unwrap();

        // The origin checkout shows NO staged or modified `.gitignore`, and
        // `.phoenix/` is not surfaced as untracked.
        let status = git(&repo, &["status", "--porcelain"]);
        assert!(
            !status.contains(".gitignore"),
            "fork spawn must not touch the origin's .gitignore: {status:?}"
        );
        assert!(
            !status.contains(".phoenix"),
            "fork's .phoenix/ dir must be ignored in the origin, not shown untracked: {status:?}"
        );
        assert_eq!(
            git(&repo, &["diff", "--cached", "--name-only"]),
            "",
            "nothing may be staged in the origin index"
        );
        // The exclude rule is what makes `.phoenix/` ignored — confirm it landed.
        let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert!(
            exclude.lines().any(|l| l.trim() == ".phoenix/"),
            "`.phoenix/` must be added to .git/info/exclude: {exclude:?}"
        );
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
        assert_eq!(
            std::fs::read_to_string(wt.join("docs/plan.md")).unwrap(),
            body
        );
        let log = git(&wt, &["log", "-1", "--name-only", "--pretty=format:"]);
        assert!(
            log.contains("docs/plan.md"),
            "expected committed plan: {log}"
        );
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
        assert_eq!(
            resolved.fork_conversation_id.as_deref(),
            Some(first.as_str())
        );
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

    /// N7: a fork whose legacy `fork-{id_prefix}` slug already exists on ANOTHER
    /// conversation must still resolve — the fork slug is derived from the full
    /// deterministic conv id, so a distinct-conversation slug clash is unreachable
    /// and the resolve no longer silently rolls back into an unapprovable loop.
    #[tokio::test]
    async fn approve_succeeds_despite_legacy_fork_prefix_slug_collision() {
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

        // Squat the legacy `fork-{first8}` slug on an unrelated conversation.
        let fork_id = derive_conv_id(&pid, ResolutionKind::Spawn);
        let prefix: String = fork_id.chars().take(8).collect();
        db.create_conversation(
            &uuid::Uuid::new_v4().to_string(),
            &format!("fork-{prefix}"),
            &repo.to_string_lossy(),
            true,
            None,
            None,
        )
        .await
        .unwrap();

        let rt = make_runtime(db.clone()).await;
        // The fork resolves: its slug is fork-{full-id}, not fork-{prefix}.
        let got = rt.approve_fork_proposal(&pid).await.unwrap();
        assert_eq!(got, fork_id);
        let fork = db.get_conversation(&fork_id).await.unwrap();
        assert_eq!(
            fork.slug.as_deref(),
            Some(format!("fork-{fork_id}").as_str())
        );
        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Spawned);
    }

    /// N3: an approve that crashed AFTER the git commit but BEFORE the DB
    /// `spawned` resolution leaves the deterministic worktree with the taskmd file
    /// ALREADY renamed + committed as `...-in-progress--{slug}.md`. A retry adopts
    /// that worktree and must NOT re-write the original `...-ready--{slug}.md` path
    /// (which would resurrect a duplicate file / collide on the id) — it converges
    /// to the same single committed in-progress task file.
    #[tokio::test]
    async fn approve_adopts_worktree_with_already_promoted_taskmd_no_duplicate() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let body = "# Fix the thing\n\n## Plan\nDo it.\n";
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--fix-thing.md", body).await;

        // Simulate a crashed-post-commit prior approve: the deterministic worktree
        // + branch exist AND the taskmd file is already committed as in-progress
        // (the rename + commit happened before the crash), but no DB resolution
        // was recorded (the proposal is still pending).
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
        // Write the already-promoted in-progress file and commit it on the branch.
        std::fs::write(wt.join("tasks/12345-p1-in-progress--fix-thing.md"), body).unwrap();
        git(
            &wt,
            &["add", "--", "tasks/12345-p1-in-progress--fix-thing.md"],
        );
        git(&wt, &["commit", "-q", "-m", "task 12345: Fix the thing"]);

        let rt = make_runtime(db.clone()).await;
        let got = rt.approve_fork_proposal(&pid).await.unwrap();
        assert_eq!(got, fork_id, "adopted the orphan, no new id");

        // Exactly ONE in-progress task file on the branch, no duplicate ready file.
        assert!(
            wt.join("tasks/12345-p1-in-progress--fix-thing.md")
                .is_file(),
            "the committed in-progress file must remain"
        );
        assert!(
            !wt.join("tasks/12345-p1-ready--fix-thing.md").exists(),
            "the original ready file must NOT be resurrected"
        );
        let tracked = git(&wt, &["ls-files", "tasks/"]);
        let task_files: Vec<&str> = tracked.lines().filter(|l| l.contains("12345")).collect();
        assert_eq!(
            task_files,
            vec!["tasks/12345-p1-in-progress--fix-thing.md"],
            "exactly one committed task file for the id, no duplicate: {tracked}"
        );

        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Spawned);
    }

    /// Bug 1: an approve that crashed AFTER `promote_task_status_to_in_progress`
    /// renamed the taskmd file to `...-in-progress--{slug}.md` but BEFORE the
    /// `git add`/`git commit` leaves the deterministic worktree with the file
    /// renamed-on-disk but UNCOMMITTED. "Exists on disk" is not "committed": the
    /// retry must NOT early-return on the in-progress filename — it must still
    /// stage + commit so the approved task is committed on the fork branch.
    #[tokio::test]
    async fn approve_adopts_worktree_with_renamed_but_uncommitted_taskmd_commits_it() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let body = "# Fix the thing\n\n## Plan\nDo it.\n";
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--fix-thing.md", body).await;

        // Simulate a crash AFTER the rename, BEFORE the commit: the deterministic
        // worktree + branch exist and the file is renamed to its in-progress name
        // on disk, but it was NEVER committed (no `git add`/`git commit`).
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
        // Renamed-on-disk in-progress file, left UNCOMMITTED on the branch.
        std::fs::write(wt.join("tasks/12345-p1-in-progress--fix-thing.md"), body).unwrap();
        // Confirm the precondition: the file is on disk but not committed.
        let status_before = git(&wt, &["status", "--porcelain"]);
        assert!(
            status_before.contains("12345-p1-in-progress--fix-thing.md"),
            "precondition: in-progress file must be present-but-uncommitted: {status_before}"
        );

        let rt = make_runtime(db.clone()).await;
        let got = rt.approve_fork_proposal(&pid).await.unwrap();
        assert_eq!(got, fork_id, "adopted the orphan, no new id");

        // The in-progress file is now COMMITTED on the branch and the worktree is
        // clean — the renamed-but-uncommitted change was driven to committed.
        let tracked = git(&wt, &["ls-files", "tasks/"]);
        let task_files: Vec<&str> = tracked.lines().filter(|l| l.contains("12345")).collect();
        assert_eq!(
            task_files,
            vec!["tasks/12345-p1-in-progress--fix-thing.md"],
            "exactly one committed in-progress task file: {tracked}"
        );
        let log = git(
            &wt,
            &[
                "log",
                "-1",
                "--name-only",
                "--pretty=format:",
                "task-12345-fix-thing",
            ],
        );
        assert!(
            log.contains("12345-p1-in-progress--fix-thing.md"),
            "the in-progress file must appear in the latest commit: {log}"
        );
        assert_eq!(
            git(&wt, &["status", "--porcelain"]),
            "",
            "worktree must be clean after the commit"
        );

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

    /// `ForkProposalDismissed`: dismissing a still-`pending` proposal that has a
    /// deterministic worktree/branch left by a crashed approve MUST clean that
    /// orphan (`DeterministicForkOrphansCleaned` before `status = dismissed`),
    /// else it leaks forever — later terminal/delete cleanup only considers
    /// `pending` proposals, and this one is now `dismissed`.
    #[tokio::test]
    async fn dismiss_cleans_deterministic_orphan_then_marks_dismissed() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--x.md", "# x\n").await;
        // Simulate a crashed approve: the deterministic spawn worktree + branch
        // exist but no resolution was recorded (proposal still pending).
        let orphan_id =
            make_deterministic_orphan(&repo, &pid, ResolutionKind::Spawn, "task-12345-x");
        let orphan_wt = repo.join(".phoenix/worktrees").join(&orphan_id);
        assert!(orphan_wt.is_dir());
        let rt = make_runtime(db.clone()).await;

        let transitioned = rt.dismiss_fork_proposal(&pid).await.unwrap();
        assert!(
            transitioned,
            "pending -> dismissed must report transitioned"
        );

        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Dismissed);
        assert!(
            !orphan_wt.exists(),
            "dismiss must clean the crashed-approve orphan worktree"
        );
        // The orphan branch is gone too.
        let branches = git(&repo, &["branch", "--list", "task-12345-x"]);
        assert!(
            branches.is_empty(),
            "dismiss must delete the orphan branch, got: {branches}"
        );
    }

    /// Build a dir-less deterministic worktree orphan: `git worktree add -b` then
    /// `rm -rf` the checkout dir, leaving git's worktree ADMIN entry (and the
    /// branch) pointing at the deterministic path with no usable dir. This is what
    /// a crashed `git worktree add -b` leaves once the admin entry is registered —
    /// `git worktree list --porcelain` still ties the branch to the deterministic
    /// path, so the orphan is PROVABLY this fork's. Returns the orphan conv id.
    fn make_dirless_deterministic_orphan(
        repo: &Path,
        pid: &str,
        kind: ResolutionKind,
        branch: &str,
    ) -> String {
        let conv_id = make_deterministic_orphan(repo, pid, kind, branch);
        let wt = repo.join(".phoenix/worktrees").join(&conv_id);
        std::fs::remove_dir_all(&wt).unwrap();
        assert!(!wt.exists(), "checkout dir removed, admin entry retained");
        conv_id
    }

    /// N6: a crashed `git worktree add -b` can leave a deterministic worktree admin
    /// entry tied to the branch with the checkout dir gone. The deterministic-orphan
    /// cleanup must remove that PROVABLE orphan (entry pruned, branch deleted),
    /// otherwise `classify_branch_collision` rejects the branch as a real collision
    /// and the still-`pending` proposal becomes permanently unapprovable. After
    /// cleanup, a subsequent approve succeeds.
    #[tokio::test]
    async fn dirless_deterministic_orphan_is_cleaned_and_approve_then_succeeds() {
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

        let fork_id = make_dirless_deterministic_orphan(
            &repo,
            &pid,
            ResolutionKind::Spawn,
            "task-12345-fix-thing",
        );
        assert!(
            !git(&repo, &["branch", "--list", "task-12345-fix-thing"]).is_empty(),
            "the deterministic orphan branch must exist before cleanup"
        );

        // Run the deterministic-orphan cleanup directly (leaves the proposal
        // pending, unlike dismiss): it must delete the provable orphan.
        let proposal = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        clean_deterministic_fork_orphans(Some(&repo), &proposal);
        assert!(
            git(&repo, &["branch", "--list", "task-12345-fix-thing"]).is_empty(),
            "cleanup must delete the dir-less deterministic orphan branch"
        );

        // The proposal is still pending; a subsequent approve now succeeds because
        // the colliding branch is gone.
        let rt = make_runtime(db.clone()).await;
        let got = rt.approve_fork_proposal(&pid).await.unwrap();
        assert_eq!(got, fork_id);
        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Spawned);
    }

    /// N6 via dismiss: a dir-less deterministic orphan (worktree admin entry tied
    /// to the deterministic path, checkout dir gone) is also removed by the dismiss
    /// cleanup path.
    #[tokio::test]
    async fn dismiss_cleans_dirless_deterministic_orphan() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--x.md", "# x\n").await;
        make_dirless_deterministic_orphan(&repo, &pid, ResolutionKind::Spawn, "task-12345-x");
        let rt = make_runtime(db.clone()).await;

        assert!(rt.dismiss_fork_proposal(&pid).await.unwrap());
        assert!(
            git(&repo, &["branch", "--list", "task-12345-x"]).is_empty(),
            "dismiss must delete the dir-less deterministic orphan branch"
        );
    }

    /// Bug 2 safety: a STANDALONE user branch whose name happens to collide with
    /// the fork's derived branch name — but which is NOT associated with the
    /// deterministic worktree path — must NEVER be force-deleted by dismiss/retire
    /// cleanup. Deleting it would be silent user data loss. (The "branch ref but no
    /// worktree admin entry ever existed" crash is indistinguishable from this, so
    /// it is accepted as leaving the branch in place.)
    #[tokio::test]
    async fn standalone_colliding_user_branch_is_not_deleted_by_cleanup() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--fix-thing.md", "# x\n").await;

        // The user has a local branch whose name collides with the fork's derived
        // name `task-12345-fix-thing`, created independently (plain `git branch`,
        // no worktree) — real work, not a fork orphan.
        git(&repo, &["branch", "task-12345-fix-thing", "main"]);

        // Dismiss cleanup must leave it intact.
        let rt = make_runtime(db.clone()).await;
        assert!(rt.dismiss_fork_proposal(&pid).await.unwrap());
        assert!(
            !git(&repo, &["branch", "--list", "task-12345-fix-thing"]).is_empty(),
            "a standalone user branch colliding on the derived name must NOT be deleted"
        );
    }

    /// Bug 2 companion: a genuine deterministic-worktree orphan (worktree admin
    /// entry on the deterministic path) IS still cleaned, even when the checkout
    /// dir is gone — proving the safety guard does not over-correct into leaving
    /// real orphans behind.
    #[tokio::test]
    async fn genuine_deterministic_orphan_still_cleaned_by_dismiss() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--fix-thing.md", "# x\n").await;
        make_dirless_deterministic_orphan(
            &repo,
            &pid,
            ResolutionKind::Spawn,
            "task-12345-fix-thing",
        );

        let rt = make_runtime(db.clone()).await;
        assert!(rt.dismiss_fork_proposal(&pid).await.unwrap());
        assert!(
            git(&repo, &["branch", "--list", "task-12345-fix-thing"]).is_empty(),
            "a genuine deterministic-worktree orphan must still be cleaned"
        );
    }

    /// Dismissing an already-`spawned` proposal is a no-op and MUST NOT touch the
    /// live fork's worktree (its deterministic path is the live decoupled child).
    #[tokio::test]
    async fn dismiss_is_noop_on_spawned_and_leaves_live_fork() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--x.md", "# x\n").await;
        let rt = make_runtime(db.clone()).await;
        let fork_id = rt.approve_fork_proposal(&pid).await.unwrap();
        let fork_wt = repo.join(".phoenix/worktrees").join(&fork_id);
        assert!(fork_wt.is_dir());

        let transitioned = rt.dismiss_fork_proposal(&pid).await.unwrap();
        assert!(!transitioned, "dismiss of a spawned proposal is a no-op");

        let after = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(after.status, ForkProposalStatus::Spawned);
        assert!(
            fork_wt.is_dir(),
            "dismiss must NOT touch a spawned proposal's live fork worktree"
        );
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
            ConvMode::Explore { worktree_path, .. } => {
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

    /// F3: the Request-Changes refinement worktree must start with ONLY the
    /// drafted brief as uncommitted work. In a repo whose tracked `.gitignore`
    /// lacks `.phoenix/`, the promote must NOT append/stage `.gitignore` inside
    /// the refinement worktree (the worktree shares the main repo's local exclude,
    /// so `.phoenix/` is already ignored). Otherwise a later Explore approval would
    /// sweep an unrelated `.gitignore` edit onto the task branch.
    #[tokio::test]
    async fn request_changes_draft_is_only_change_gitignore_untouched() {
        let (_tmp, repo) = init_repo();
        // The origin checkout's tracked `.gitignore` exists but lacks `.phoenix/`.
        std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        git(&repo, &["add", ".gitignore"]);
        git(
            &repo,
            &["commit", "-q", "-m", "add gitignore without .phoenix"],
        );

        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "docs/plan.md", "# Plan\n\nbody\n").await;
        let rt = make_runtime(db.clone()).await;

        let refinement_id = rt
            .request_changes_on_fork_proposal(&pid, "shorten".to_string())
            .await
            .unwrap();

        let wt = repo.join(".phoenix/worktrees").join(&refinement_id);
        let prefix: String = refinement_id.chars().take(8).collect();
        let draft_rel = format!("tasks/{prefix}-plan.md");

        // The brief draft is the ONLY working-tree change.
        let status = git(&wt, &["status", "--porcelain"]);
        let changed: Vec<&str> = status.lines().collect();
        assert_eq!(
            changed.len(),
            1,
            "exactly one working-tree change (the draft) expected: {status:?}"
        );
        assert!(
            changed[0].contains(&draft_rel),
            "the single change must be the brief draft: {status:?}"
        );

        // `.gitignore` is neither modified nor staged in the refinement worktree.
        assert!(
            !status.contains(".gitignore"),
            "the refinement must not modify/stage .gitignore: {status:?}"
        );
        assert_eq!(
            git(&wt, &["diff", "--cached", "--name-only"]),
            "",
            "nothing may be staged in the refinement worktree"
        );
        // `.phoenix/` is still ignored (via the shared local exclude), so the
        // worktree's own `.phoenix` admin/dir never surfaces as untracked.
        assert!(
            !status.contains(".phoenix"),
            ".phoenix/ must remain ignored in the refinement worktree: {status:?}"
        );
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

    /// Transition-graph: `promoted` is terminal (REQ-PROJ-037), so a SECOND
    /// `request-changes` on an already-`promoted` proposal is rejected — one
    /// pending proposal promotes exactly once.
    /// `spawn_and_promote_namespaces_are_disjoint` covers the spawned↔promoted
    /// cross edges; this pins the promoted-source self edge.
    #[tokio::test]
    async fn second_request_changes_after_promote_is_rejected() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_p, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--x.md", "# x\n").await;
        let rt = make_runtime(db.clone()).await;

        let refinement = rt
            .request_changes_on_fork_proposal(&pid, "first note".to_string())
            .await
            .unwrap();
        let second = rt
            .request_changes_on_fork_proposal(&pid, "second note".to_string())
            .await;
        assert!(
            matches!(second, Err(ForkResolveError::Conflict(_))),
            "re-promote of a promoted proposal must be rejected, got {second:?}"
        );

        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Promoted);
        assert_eq!(
            resolved.refinement_conversation_id.as_deref(),
            Some(refinement.as_str())
        );
        assert!(resolved.fork_conversation_id.is_none());
    }

    /// Transition-graph: `dismissed` is terminal with no outbound edge, so
    /// `request-changes` (the promote edge) on a `dismissed` proposal is rejected.
    /// `approve_non_pending_is_rejected` covers the dismissed -> spawned edge;
    /// this covers the dismissed -> promoted edge through the runtime path.
    #[tokio::test]
    async fn request_changes_on_dismissed_is_rejected() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_p, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--x.md", "# x\n").await;
        assert!(db.dismiss_fork_proposal(&pid).await.unwrap());
        let rt = make_runtime(db.clone()).await;

        let err = rt
            .request_changes_on_fork_proposal(&pid, "note".to_string())
            .await
            .unwrap_err();
        assert!(matches!(err, ForkResolveError::Conflict(_)), "got {err:?}");

        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Dismissed);
        assert!(resolved.refinement_conversation_id.is_none());
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

    // ---- REQ-PROJ-035: retire-on-terminal + hard-delete orphan cleanup ----

    /// Simulate a crashed approve/promote: create the deterministic worktree +
    /// branch at `.phoenix/worktrees/{derive_conv_id(pid, kind)}` off main with
    /// no resolution recorded. Returns the orphan's conversation id.
    fn make_deterministic_orphan(
        repo: &Path,
        pid: &str,
        kind: ResolutionKind,
        branch: &str,
    ) -> String {
        let conv_id = derive_conv_id(pid, kind);
        let wt = repo.join(".phoenix/worktrees").join(&conv_id);
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        git(
            repo,
            &[
                "worktree",
                "add",
                "-b",
                branch,
                wt.to_str().unwrap(),
                "main",
            ],
        );
        conv_id
    }

    #[tokio::test]
    async fn retire_on_terminal_dismisses_pending_and_cleans_orphan() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--x.md", "# x\n").await;
        // A crashed approve left a deterministic spawn orphan for this pending proposal.
        let orphan_id =
            make_deterministic_orphan(&repo, &pid, ResolutionKind::Spawn, "task-12345-x");
        let orphan_wt = repo.join(".phoenix/worktrees").join(&orphan_id);
        assert!(orphan_wt.is_dir());
        let rt = make_runtime(db.clone()).await;

        rt.retire_fork_proposals_for_terminal_origin(&origin).await;

        // Pending proposal is now dismissed.
        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Dismissed);
        // The deterministic orphan worktree was cleaned up.
        assert!(
            !orphan_wt.exists(),
            "pending proposal's crashed-approve orphan must be removed on terminal"
        );
    }

    #[tokio::test]
    async fn retire_on_terminal_leaves_spawned_untouched() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let body = "# Fix the thing\n";
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--fix-thing.md", body).await;
        let rt = make_runtime(db.clone()).await;
        // Actually spawn a live fork — its worktree is the LIVE decoupled child.
        let fork_id = rt.approve_fork_proposal(&pid).await.unwrap();
        let fork_wt = repo.join(".phoenix/worktrees").join(&fork_id);
        assert!(fork_wt.is_dir());

        rt.retire_fork_proposals_for_terminal_origin(&origin).await;

        // The spawned proposal is still spawned — NOT dismissed.
        let after = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(after.status, ForkProposalStatus::Spawned);
        assert_eq!(
            after.fork_conversation_id.as_deref(),
            Some(fork_id.as_str())
        );
        // The live fork conversation + its worktree survive.
        assert!(db.get_conversation(&fork_id).await.is_ok());
        assert!(
            fork_wt.is_dir(),
            "a spawned proposal's live fork worktree must NOT be touched on terminal"
        );
    }

    #[tokio::test]
    async fn hard_delete_cleanup_removes_pending_orphan_only() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;

        // Proposal A: still pending, with a crashed-approve deterministic orphan.
        let pid_a = insert_pending(&db, &origin, "tasks/11111-p1-ready--a.md", "# a\n").await;
        let orphan_id =
            make_deterministic_orphan(&repo, &pid_a, ResolutionKind::Spawn, "task-11111-a");
        let orphan_wt = repo.join(".phoenix/worktrees").join(&orphan_id);

        // Proposal B: spawned — its deterministic path is the LIVE fork worktree.
        let pid_b = insert_pending(&db, &origin, "tasks/22222-p1-ready--b.md", "# b\n").await;
        let rt = make_runtime(db.clone()).await;
        let fork_id = rt.approve_fork_proposal(&pid_b).await.unwrap();
        let fork_wt = repo.join(".phoenix/worktrees").join(&fork_id);

        assert!(orphan_wt.is_dir());
        assert!(fork_wt.is_dir());

        // Mirror cleanup_pending_fork_orphans_on_delete: pending-only orphan cleanup.
        let repo_root = repo.clone();
        let pending: Vec<ForkProposal> = db
            .list_fork_proposals_for_origin(&origin)
            .await
            .unwrap()
            .into_iter()
            .filter(|p| p.status == ForkProposalStatus::Pending)
            .collect();
        assert_eq!(
            pending.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
            vec![pid_a.clone()],
            "only A is pending"
        );
        for proposal in &pending {
            clean_deterministic_fork_orphans(Some(&repo_root), proposal);
        }

        // Pending proposal's orphan removed; spawned proposal's live fork intact.
        assert!(
            !orphan_wt.exists(),
            "pending proposal's crashed orphan must be cleaned on hard delete"
        );
        assert!(
            fork_wt.is_dir(),
            "spawned proposal's LIVE fork worktree must NOT be touched on hard delete"
        );
    }

    /// N1: a `CleanupOnHardDelete` for an origin dismisses its still-`pending`
    /// proposals under the serialized consumer, so an `approve` queued AFTER it
    /// finds the proposal non-`pending` and is rejected — no fork can be spawned
    /// from a proposal whose origin is being hard-deleted. Because both commands
    /// run on the single consumer, the cleanup's dismiss is guaranteed visible to
    /// the later approve.
    #[tokio::test]
    async fn approve_after_hard_delete_cleanup_is_rejected() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--x.md", "# x\n").await;
        let rt = make_runtime(db.clone()).await;

        // Hard-delete cleanup runs first: it dismisses the pending proposal.
        rt.cleanup_pending_fork_orphans_on_delete(&origin).await;

        let after_cleanup = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(
            after_cleanup.status,
            ForkProposalStatus::Dismissed,
            "hard-delete cleanup must dismiss the pending proposal under serialization"
        );

        // A later approve must be rejected — the proposal is no longer pending.
        let err = rt.approve_fork_proposal(&pid).await.unwrap_err();
        assert!(
            matches!(err, ForkResolveError::Conflict(_)),
            "approve after hard-delete cleanup must be rejected, got {err:?}"
        );
        // No fork worktree was created.
        let fork_id = derive_conv_id(&pid, ResolutionKind::Spawn);
        assert!(
            !repo.join(".phoenix/worktrees").join(&fork_id).exists(),
            "no fork may be spawned from a proposal whose origin is being deleted"
        );
    }

    /// F1 (retry-converges): approving a pending proposal whose origin has gone
    /// terminal returns a 409 BUT also retires the proposal — dismiss + clean its
    /// deterministic orphan — so a user's retry self-heals the stale `pending` row
    /// instead of 409-ing forever while `GET /proposals` still shows a Review.
    #[tokio::test]
    async fn approve_on_terminal_origin_retires_proposal_and_conflicts() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--x.md", "# x\n").await;
        // A crashed approve left a deterministic spawn orphan for this proposal.
        let orphan_id =
            make_deterministic_orphan(&repo, &pid, ResolutionKind::Spawn, "task-12345-x");
        let orphan_wt = repo.join(".phoenix/worktrees").join(&orphan_id);
        assert!(orphan_wt.is_dir());
        // Origin reached terminal, but its proposals were never retired (crash).
        db.update_conversation_state(&origin, &ConvState::Terminal)
            .await
            .unwrap();
        let rt = make_runtime(db.clone()).await;

        let err = rt.approve_fork_proposal(&pid).await.unwrap_err();
        assert!(matches!(err, ForkResolveError::Conflict(_)), "got {err:?}");

        // The stale row converged to dismissed and its orphan was cleaned.
        let after = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(
            after.status,
            ForkProposalStatus::Dismissed,
            "retry against a terminal origin must converge the pending row to dismissed"
        );
        assert!(
            !orphan_wt.exists(),
            "terminal-origin retire must clean the crashed-approve orphan"
        );
    }

    /// F1 (startup self-heal): a pure crash — origin went terminal, proposals
    /// never retired, user never retries — is reconciled on restart. The startup
    /// pass dismisses each pending proposal bound to a terminal origin and cleans
    /// its deterministic orphan; a still-live origin's proposal is left pending.
    #[tokio::test]
    async fn startup_reconcile_retires_pending_against_terminal_origin_only() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();

        // Origin A is terminal with a stranded pending proposal + crashed orphan.
        let (_pa, origin_a) = seed_project_and_origin(&db, &repo).await;
        let pid_a = insert_pending(&db, &origin_a, "tasks/12345-p1-ready--a.md", "# a\n").await;
        let orphan_a =
            make_deterministic_orphan(&repo, &pid_a, ResolutionKind::Spawn, "task-12345-a");
        let orphan_a_wt = repo.join(".phoenix/worktrees").join(&orphan_a);
        assert!(orphan_a_wt.is_dir());
        db.update_conversation_state(&origin_a, &ConvState::Terminal)
            .await
            .unwrap();

        // Origin B is still live with a pending proposal — must be left pending.
        let (_pb, origin_b) = seed_project_and_origin(&db, &repo).await;
        let pid_b = insert_pending(&db, &origin_b, "tasks/22222-p1-ready--b.md", "# b\n").await;

        reconcile_terminal_origin_fork_proposals(&db).await;

        let a = db.get_fork_proposal(&pid_a).await.unwrap().unwrap();
        assert_eq!(
            a.status,
            ForkProposalStatus::Dismissed,
            "a pending proposal whose origin is terminal must be retired on startup"
        );
        assert!(
            !orphan_a_wt.exists(),
            "startup reconcile must clean the terminal origin's crashed-approve orphan"
        );
        let b = db.get_fork_proposal(&pid_b).await.unwrap().unwrap();
        assert_eq!(
            b.status,
            ForkProposalStatus::Pending,
            "a live origin's pending proposal must NOT be retired"
        );
    }

    /// Bug 3 (during-git terminal race): the origin passes the PRE-git liveness
    /// check, then goes terminal WHILE the blocking git phase builds the worktree.
    /// The post-git re-check (`abort_if_origin_now_terminal`) must abort the
    /// resolve: NO fork conversation recorded/started, the proposal converged to
    /// `dismissed`, and the deterministic worktree the git phase created cleaned.
    ///
    /// Interpose: the deterministic spawn worktree is created up-front (standing in
    /// for "the git phase already ran and produced this worktree"), the origin is
    /// driven terminal AFTER that point, and the re-check guard is invoked directly
    /// — exactly the state the real handler reaches between `prepare_spawn_blocking`
    /// and `resolve_fork_proposal_spawned`.
    #[tokio::test]
    async fn during_git_terminal_aborts_resolve_and_cleans_orphan() {
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
        // The blocking git phase has run: the deterministic spawn worktree exists.
        let fork_id =
            make_deterministic_orphan(&repo, &pid, ResolutionKind::Spawn, "task-12345-fix-thing");
        let fork_wt = repo.join(".phoenix/worktrees").join(&fork_id);
        assert!(fork_wt.is_dir(), "git phase produced the worktree");

        // The origin goes terminal DURING the git phase — after the pre-git check
        // passed, before the DB resolve.
        db.update_conversation_state(&origin, &ConvState::Terminal)
            .await
            .unwrap();

        let rt = make_runtime(db.clone()).await;
        let proposal = db.get_fork_proposal(&pid).await.unwrap().unwrap();

        // The post-git re-check guard aborts: returns a conflict, records nothing.
        let err = rt
            .abort_if_origin_now_terminal(&proposal)
            .await
            .expect_err("a now-terminal origin must abort the resolve");
        assert!(matches!(err, ForkResolveError::Conflict(_)), "got {err:?}");

        // No fork conversation was created.
        assert!(
            db.get_conversation(&fork_id).await.is_err(),
            "no fork conversation may be created when the origin went terminal mid-resolve"
        );
        // The proposal converged to dismissed (not spawned).
        let after = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(
            after.status,
            ForkProposalStatus::Dismissed,
            "the proposal must be dismissed, never spawned"
        );
        assert!(
            after.fork_conversation_id.is_none(),
            "no fork conversation id may be recorded"
        );
        // The worktree the git phase created was cleaned.
        assert!(
            !fork_wt.exists(),
            "the mid-resolve worktree orphan must be cleaned"
        );
        assert!(
            git(&repo, &["branch", "--list", "task-12345-fix-thing"]).is_empty(),
            "the orphan branch must be deleted"
        );
    }

    /// Bug 3 fast-path preserved: a still-LIVE origin passes the post-git re-check,
    /// so the resolve proceeds (the guard returns `Ok` and leaves the proposal
    /// pending for the caller to resolve).
    #[tokio::test]
    async fn during_git_recheck_is_noop_when_origin_still_live() {
        let (_tmp, repo) = init_repo();
        let db = Database::open_in_memory().await.unwrap();
        let (_pid, origin) = seed_project_and_origin(&db, &repo).await;
        let pid = insert_pending(&db, &origin, "tasks/12345-p1-ready--x.md", "# x\n").await;
        let rt = make_runtime(db.clone()).await;
        let proposal = db.get_fork_proposal(&pid).await.unwrap().unwrap();

        // Origin is live: the guard must NOT abort and must NOT touch the proposal.
        rt.abort_if_origin_now_terminal(&proposal)
            .await
            .expect("a live origin must let the resolve proceed");
        let after = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(
            after.status,
            ForkProposalStatus::Pending,
            "a live-origin re-check must leave the proposal pending"
        );
        let _ = origin;
    }

    /// Bug 4 (prunable deterministic worktree): a retry against a deterministic
    /// worktree whose checkout DIRECTORY was deleted (admin entry still present,
    /// `git worktree list --porcelain` reports it `prunable`) must NOT adopt it.
    /// Adopting skips `create_worktree` and then `git add` fails at a path that is
    /// not a real worktree, so retries loop forever. The resolve must instead prune
    /// the stale entry and recreate the worktree, then succeed.
    #[tokio::test]
    async fn approve_prunes_and_recreates_prunable_deterministic_worktree() {
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

        // Crashed prior approve: deterministic worktree + branch created, then the
        // checkout DIR deleted — git's porcelain still reports the branch at the
        // deterministic path, flagged `prunable`.
        let fork_id = make_dirless_deterministic_orphan(
            &repo,
            &pid,
            ResolutionKind::Spawn,
            "task-12345-fix-thing",
        );
        let fork_wt = repo.join(".phoenix/worktrees").join(&fork_id);
        assert!(!fork_wt.exists(), "checkout dir is gone (prunable)");
        // Sanity: the porcelain entry is prunable, the precondition for the bug.
        let porcelain = git(&repo, &["worktree", "list", "--porcelain"]);
        assert!(
            porcelain.contains("prunable"),
            "the deterministic entry must be prunable: {porcelain}"
        );

        // Approve must prune + recreate (not adopt-into-failure) and succeed.
        let rt = make_runtime(db.clone()).await;
        let got = rt
            .approve_fork_proposal(&pid)
            .await
            .expect("approve must recover from a prunable deterministic worktree");
        assert_eq!(got, fork_id, "recreated at the same deterministic id");

        // The worktree is now a REAL, present worktree with the committed task.
        assert!(fork_wt.is_dir(), "the worktree was recreated on disk");
        let committed = git(
            &fork_wt,
            &[
                "log",
                "-1",
                "--name-only",
                "--pretty=format:",
                "task-12345-fix-thing",
            ],
        );
        assert!(
            committed.contains("12345-p1-in-progress--fix-thing.md"),
            "the recreated worktree carries the committed in-progress task: {committed}"
        );
        let resolved = db.get_fork_proposal(&pid).await.unwrap().unwrap();
        assert_eq!(resolved.status, ForkProposalStatus::Spawned);
        assert_eq!(
            resolved.fork_conversation_id.as_deref(),
            Some(fork_id.as_str())
        );
    }
}
