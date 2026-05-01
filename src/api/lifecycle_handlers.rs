//! Conversation lifecycle HTTP handlers: task approval, abandon, mark-merged.

use super::handlers::AppError;
use super::types::{
    ConflictErrorResponse, SuccessResponse, TaskApprovalResponse, TaskFeedbackRequest,
};
use super::AppState;
use crate::db::{ConvMode, Conversation, MessageContent};
use crate::git_ops::{capture_branch_diff, run_git};
use crate::state_machine::state::TaskApprovalOutcome;
use crate::state_machine::{ConvState, Event};
use std::fmt::Write as _;

use axum::{
    extract::{Path, State},
    Json,
};
use std::path::PathBuf;

// ============================================================
// Terminal-action gate (REQ-BED-031)
// ============================================================

/// Reject terminal user actions (abandon / mark-as-merged) when the
/// conversation has an existing continuation. REQ-BED-031, enforced by
/// the Allium `TerminalActionRequiresNoContinuation` invariant and the
/// `continued_in_conv_id = absent` clause on the `ConfirmAbandon` and
/// `MarkAsMerged` rules in `specs/projects/projects.allium`.
///
/// `action` is a human-readable verb phrase (e.g. `"abandon"`,
/// `"mark as merged"`) that appears in the error message so the UI can
/// present a coherent reason.
///
/// Returns 409 Conflict with `error_type = "continuation_exists"` so
/// the frontend can dispatch on it (Phase 5) — e.g. offer to route to
/// the continuation instead of showing the raw error text.
fn reject_if_continued(conv: &Conversation, action: &str) -> Result<(), AppError> {
    if let Some(continuation_id) = conv.continued_in_conv_id.as_deref() {
        return Err(AppError::Conflict(Box::new(
            ConflictErrorResponse::new(
                format!(
                    "Cannot {action} a conversation that has been continued. \
                     The action belongs on the continuation conversation ({continuation_id})."
                ),
                "continuation_exists",
            )
            .with_continuation_id(continuation_id),
        )));
    }
    Ok(())
}

// ============================================================
// Task Approval (REQ-BED-028)
// ============================================================

pub(crate) async fn approve_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskApprovalResponse>, AppError> {
    // 1. Validate conversation exists and is in AwaitingTaskApproval state
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::AwaitingTaskApproval { .. }) {
        return Err(AppError::BadRequest(
            "Conversation is not awaiting task approval".to_string(),
        ));
    }

    // 2. Non-project conversations cannot approve tasks (propose_task is project-only)
    if conv.project_id.is_none() {
        return Err(AppError::BadRequest(
            "Task approval requires a project-scoped conversation".to_string(),
        ));
    }

    // 3. Dispatch approval event to state machine
    state
        .runtime
        .send_event(
            &id,
            Event::TaskApprovalResponse {
                outcome: TaskApprovalOutcome::Approved,
            },
        )
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(TaskApprovalResponse {
        success: true,
        first_task: None, // Set by executor via SSE if applicable
    }))
}

pub(crate) async fn reject_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    // Validate conversation exists and is in AwaitingTaskApproval state
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::AwaitingTaskApproval { .. }) {
        return Err(AppError::BadRequest(
            "Conversation is not awaiting task approval".to_string(),
        ));
    }

    state
        .runtime
        .send_event(
            &id,
            Event::TaskApprovalResponse {
                outcome: TaskApprovalOutcome::Rejected,
            },
        )
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(SuccessResponse { success: true }))
}

pub(crate) async fn task_feedback(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<TaskFeedbackRequest>,
) -> Result<Json<SuccessResponse>, AppError> {
    // Validate conversation exists and is in AwaitingTaskApproval state
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    if !matches!(conv.state, ConvState::AwaitingTaskApproval { .. }) {
        return Err(AppError::BadRequest(
            "Conversation is not awaiting task approval".to_string(),
        ));
    }

    state
        .runtime
        .send_event(
            &id,
            Event::TaskApprovalResponse {
                outcome: TaskApprovalOutcome::FeedbackProvided {
                    annotations: req.annotations,
                },
            },
        )
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(SuccessResponse { success: true }))
}

// ============================================================
// Task Abandon (REQ-PROJ-010)
// ============================================================

/// Abandon a Work or Branch conversation: delete worktree, optionally delete branch,
/// capture diff snapshot, transition to Terminal.
/// Single-phase endpoint -- the frontend confirms via a dialog before calling this.
#[allow(clippy::too_many_lines)]
pub(crate) async fn abandon_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    // 1. Validate conversation exists, is Work or Branch mode, Idle state, project-scoped
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    // REQ-BED-031: reject if the conversation has already been continued.
    // The live conversation is the continuation; terminal actions belong there.
    reject_if_continued(&conv, "abandon")?;

    // REQ-BED-031: abandon is permitted from Idle *and* ContextExhausted.
    // A context-exhausted parent with no continuation is the canonical
    // "user is done; tear it down" path — the gate above already ensured
    // no continuation exists, so the worktree/branch are still ours to
    // destroy.
    if !matches!(
        conv.state,
        ConvState::Idle | ConvState::ContextExhausted { .. },
    ) {
        return Err(AppError::BadRequest(
            "Conversation must be idle or context-exhausted to abandon a task".to_string(),
        ));
    }

    // Accept both Work and Branch mode
    let (branch_name, worktree_path, base_branch, task_id, is_work_mode) = match &conv.conv_mode {
        ConvMode::Work {
            branch_name,
            worktree_path,
            base_branch,
            task_id,
            ..
        } => (
            branch_name.to_string(),
            worktree_path.to_string(),
            base_branch.to_string(),
            task_id.to_string(),
            true,
        ),
        ConvMode::Branch {
            branch_name,
            worktree_path,
            base_branch,
            ..
        } => (
            branch_name.to_string(),
            worktree_path.to_string(),
            base_branch.to_string(),
            String::new(), // Branch mode has no task
            false,
        ),
        _ => {
            return Err(AppError::BadRequest(
                "Conversation must be in Work or Branch mode to abandon".to_string(),
            ));
        }
    };

    let project_id = conv
        .project_id
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("Conversation is not project-scoped".to_string()))?;

    let project = state
        .db
        .get_project(project_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let repo_root = PathBuf::from(&project.canonical_path);

    // 2a. Capture diff snapshot from worktree BEFORE deleting it (blocking).
    // The 100 KiB cap is enforced by capture_branch_diff via streaming
    // reads — git stdout never fully materialises in memory.
    let worktree_path_clone = worktree_path.clone();
    let base_branch_clone = base_branch.clone();
    let diff_snapshot: Option<String> = tokio::task::spawn_blocking(move || {
        const MAX_DIFF_BYTES: usize = 100 * 1024; // 100KiB

        let wt = PathBuf::from(&worktree_path_clone);
        if !wt.exists() {
            tracing::warn!(worktree = %worktree_path_clone, "Worktree gone before diff capture");
            return None;
        }

        let captured = capture_branch_diff(&wt, &base_branch_clone, MAX_DIFF_BYTES);

        if captured.committed_diff.is_empty() && captured.uncommitted_diff.is_empty() {
            return None;
        }

        let mut snapshot = String::from("## Abandoned work snapshot\n");

        let append_section =
            |out: &mut String, header: &str, body: &str, total_bytes: u64, saturated: bool| {
                out.push_str(header);
                out.push_str(body);
                let body_len_u64 = body.len() as u64;
                if body_len_u64 < total_bytes || saturated {
                    let kib = total_bytes / 1024;
                    let lower_bound = if saturated { "≥" } else { "" };
                    let _ = write!(
                        out,
                        "\n\n[truncated -- diff was {lower_bound}{kib}KiB, showing first {}KiB]",
                        body.len() / 1024
                    );
                }
                out.push_str("\n```\n");
            };

        if !captured.committed_diff.is_empty() {
            append_section(
                &mut snapshot,
                &format!(
                    "\n### Committed changes (vs `{}`)\n```diff\n",
                    captured.comparator
                ),
                &captured.committed_diff,
                captured.committed_total_bytes,
                captured.committed_saturated,
            );
        }

        if !captured.uncommitted_diff.is_empty() {
            append_section(
                &mut snapshot,
                "\n### Uncommitted changes\n```diff\n",
                &captured.uncommitted_diff,
                captured.uncommitted_total_bytes,
                captured.uncommitted_saturated,
            );
        }

        Some(snapshot)
    })
    .await
    .map_err(|e| AppError::Internal(format!("Diff capture failed: {e}")))?;

    // 2b. Write diff snapshot as a system message (before worktree deletion).
    //
    // Pre-allocate the seq from the conversation's broadcaster so this
    // message orders strictly after any ephemeral events already emitted
    // on the stream. See PersistBeforeBroadcast in
    // specs/sse_wire/sse_wire.allium. The handle obtained here is reused
    // at step 3 to actually broadcast the persisted message.
    let snapshot_msg = if let Some(ref snapshot) = diff_snapshot {
        let snap_msg_id = uuid::Uuid::new_v4().to_string();
        match state.runtime.get_or_create(&id).await {
            Ok(handle) => {
                let seq = handle.broadcast_tx.next_seq();
                match state
                    .db
                    .add_message_with_seq(
                        &snap_msg_id,
                        &id,
                        seq,
                        &MessageContent::system(snapshot),
                        None,
                        None,
                    )
                    .await
                {
                    Ok(msg) => Some((handle, msg)),
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to persist diff snapshot (non-fatal)");
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to obtain broadcaster for diff snapshot; persisting without broadcast (client will see on next init)"
                );
                // Persist with a DB-allocated seq. No broadcast follows,
                // so the seq-ordering concern doesn't apply.
                if let Err(e) = state
                    .db
                    .add_message(
                        &snap_msg_id,
                        &id,
                        &MessageContent::system(snapshot),
                        None,
                        None,
                    )
                    .await
                {
                    tracing::warn!(error = %e, "Failed to persist diff snapshot (non-fatal)");
                }
                None
            }
        }
    } else {
        None
    };

    // 2c. Delete worktree + conditionally delete branch (blocking)
    // Work mode: delete worktree AND branch (Phoenix-created), skip task file update
    //   (the branch is being deleted, so any task file committed there goes with it).
    // Branch mode: delete worktree only, keep user's branch, no task file.
    let repo_root_clone = repo_root.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        // Worktree cleanup
        let worktree_dir = PathBuf::from(&worktree_path);
        if let Err(e) = run_git(
            &repo_root_clone,
            &["worktree", "remove", &worktree_path, "--force"],
        ) {
            tracing::warn!(
                error = %e,
                worktree = %worktree_path,
                "Failed to remove worktree (non-fatal), trying filesystem fallback"
            );
            let _ = std::fs::remove_dir_all(&worktree_dir);
            let _ = run_git(&repo_root_clone, &["worktree", "prune"]);
        }

        // Work mode: delete the managed branch.
        // Branch mode: keep the user's branch.
        if is_work_mode {
            if let Err(e) = run_git(&repo_root_clone, &["branch", "-D", &branch_name]) {
                tracing::warn!(
                    error = %e,
                    branch = %branch_name,
                    "Failed to delete branch (non-fatal)"
                );
            }
        } else {
            tracing::info!(
                branch = %branch_name,
                "Branch mode abandon: keeping user's branch"
            );
        }

        // Skip task file update entirely. For Work mode the branch (and its task file)
        // is deleted above. For Branch mode there is no task file.
        if !task_id.is_empty() {
            tracing::info!(
                task_id = %task_id,
                "Skipping task file update -- branch deleted, task file goes with it"
            );
        }

        Ok(())
    })
    .await
    .map_err(|e| AppError::Internal(format!("Blocking task failed: {e}")))??;

    // 3. Broadcast diff snapshot (persisted above, before state transition).
    // Reuses the handle obtained during seq pre-allocation at step 2b.
    if let Some((handle, snap_msg)) = snapshot_msg {
        let _ = handle.broadcast_tx.send_message(snap_msg);
    }

    // 4. Route through state machine (REQ-BED-029, REQ-BED-001)
    let repo_root_str = repo_root.display().to_string();
    let mode_label = if is_work_mode { "Work" } else { "Branch" };
    let system_message = if is_work_mode {
        "Task abandoned. Worktree and branch deleted.".to_string()
    } else {
        "Abandoned. Worktree removed, branch kept.".to_string()
    };
    tracing::info!(
        conversation_id = %id,
        mode = mode_label,
        "Abandon complete"
    );
    state
        .runtime
        .send_event(
            &id,
            Event::TaskResolved {
                system_message,
                repo_root: repo_root_str,
            },
        )
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(SuccessResponse { success: true }))
}

// ============================================================
// Mark as Merged (REQ-PROJ-026)
// ============================================================

/// Mark a Work or Branch conversation as merged: delete worktree, optionally delete branch,
/// transition to Terminal. The user has already merged/PR'd the branch externally.
#[allow(clippy::too_many_lines)]
pub(crate) async fn mark_merged(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SuccessResponse>, AppError> {
    // 1. Validate conversation exists, is Work or Branch mode, Idle state
    let conv = state
        .runtime
        .db()
        .get_conversation(&id)
        .await
        .map_err(|e| AppError::NotFound(e.to_string()))?;

    // REQ-BED-031: reject if the conversation has already been continued.
    // The live conversation is the continuation; terminal actions belong there.
    reject_if_continued(&conv, "mark as merged")?;

    // REQ-BED-031: mark-as-merged is permitted from Idle *and*
    // ContextExhausted. A context-exhausted parent whose work has already
    // been merged (e.g. user committed and merged externally) needs a way
    // to dispose of the worktree without forcing a continuation first.
    if !matches!(
        conv.state,
        ConvState::Idle | ConvState::ContextExhausted { .. },
    ) {
        return Err(AppError::BadRequest(
            "Conversation must be idle or context-exhausted to mark as merged".to_string(),
        ));
    }

    let (branch_name, worktree_path, is_work_mode) = match &conv.conv_mode {
        ConvMode::Work {
            branch_name,
            worktree_path,
            ..
        } => (branch_name.to_string(), worktree_path.to_string(), true),
        ConvMode::Branch {
            branch_name,
            worktree_path,
            ..
        } => (branch_name.to_string(), worktree_path.to_string(), false),
        _ => {
            return Err(AppError::BadRequest(
                "Conversation must be in Work or Branch mode to mark as merged".to_string(),
            ));
        }
    };

    let project_id = conv
        .project_id
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("Conversation is not project-scoped".to_string()))?;

    let project = state
        .db
        .get_project(project_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let repo_root = PathBuf::from(&project.canonical_path);
    let repo_root_str = repo_root.display().to_string();

    // 2. Delete worktree + conditionally delete branch (blocking)
    let repo_root_clone = repo_root.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        // Remove worktree
        let worktree_dir = PathBuf::from(&worktree_path);
        if let Err(e) = run_git(
            &repo_root_clone,
            &["worktree", "remove", &worktree_path, "--force"],
        ) {
            tracing::warn!(
                error = %e,
                worktree = %worktree_path,
                "Failed to remove worktree (non-fatal), trying filesystem fallback"
            );
            let _ = std::fs::remove_dir_all(&worktree_dir);
            let _ = run_git(&repo_root_clone, &["worktree", "prune"]);
        }

        // Work mode (Managed): delete the task branch -- it was created by Phoenix.
        // Branch mode: keep the branch -- it's the user's PR branch.
        if is_work_mode {
            if let Err(e) = run_git(&repo_root_clone, &["branch", "-D", &branch_name]) {
                tracing::warn!(
                    error = %e,
                    branch = %branch_name,
                    "Failed to delete managed branch (non-fatal)"
                );
            }
        } else {
            tracing::info!(
                branch = %branch_name,
                "Branch mode: keeping user's branch after mark-merged"
            );
        }

        Ok(())
    })
    .await
    .map_err(|e| AppError::Internal(format!("Blocking task failed: {e}")))??;

    // 3. Route through state machine -> Terminal
    let mode_label = if is_work_mode { "Work" } else { "Branch" };
    let system_message = format!(
        "Marked as merged. Worktree removed{}.",
        if is_work_mode {
            ", task branch deleted"
        } else {
            ""
        }
    );
    tracing::info!(
        conversation_id = %id,
        mode = mode_label,
        "Mark-merged complete"
    );

    state
        .runtime
        .send_event(
            &id,
            Event::TaskResolved {
                system_message,
                repo_root: repo_root_str,
            },
        )
        .await
        .map_err(AppError::BadRequest)?;

    Ok(Json(SuccessResponse { success: true }))
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    //! REQ-BED-031 gate tests for the abandon and mark-as-merged handlers.
    //!
    //! The gate logic lives in [`reject_if_continued`], which both
    //! `abandon_task` and `mark_merged` invoke immediately after reading
    //! the conversation and before any worktree/branch destruction. The
    //! "state did NOT change on reject" property in REQ-BED-031 is
    //! therefore enforced structurally: the handler returns `Err` via
    //! `?` before reaching the `run_git` or `send_event` calls. These
    //! tests cover the gate itself; integration coverage of the full
    //! handler flow would require a full `AppState` harness that does
    //! not currently exist in the repo (Phase 2 tested `Database::
    //! continue_conversation` at the DB layer for the same reason).
    use super::*;
    use crate::db::{ConvMode, Conversation, NonEmptyString};
    use crate::state_machine::state::ConvState;
    use chrono::{TimeZone, Utc};

    fn fixture(id: &str, continued_in_conv_id: Option<String>) -> Conversation {
        let ts = Utc.with_ymd_and_hms(2026, 4, 23, 12, 0, 0).unwrap();
        Conversation {
            id: id.to_string(),
            slug: Some(format!("slug-{id}")),
            title: Some(format!("Title {id}")),
            cwd: "/tmp/work".to_string(),
            parent_conversation_id: None,
            user_initiated: true,
            state: ConvState::Idle,
            state_updated_at: ts,
            created_at: ts,
            updated_at: ts,
            archived: false,
            model: None,
            project_id: Some("proj-1".to_string()),
            conv_mode: ConvMode::Work {
                branch_name: NonEmptyString::new("task-24696-gate").unwrap(),
                worktree_path: NonEmptyString::new("/tmp/wt/gate").unwrap(),
                base_branch: NonEmptyString::new("main").unwrap(),
                task_id: NonEmptyString::new("TK24696").unwrap(),
                task_title: NonEmptyString::new("Gate test").unwrap(),
            },
            desired_base_branch: None,
            message_count: 0,
            seed_parent_id: None,
            seed_label: None,
            continued_in_conv_id,
            chain_name: None,
        }
    }

    // ---- abandon gate -------------------------------------------------

    /// Unblocked: `continued_in_conv_id = None` — gate passes, handler
    /// proceeds with existing abandon logic.
    #[test]
    fn abandon_gate_passes_when_no_continuation() {
        let conv = fixture("parent-a", None);
        assert!(reject_if_continued(&conv, "abandon").is_ok());
    }

    /// Blocked: `continued_in_conv_id = Some(...)` — gate returns 409
    /// with `error_type = "continuation_exists"` and a message naming
    /// the continuation id. Structurally prevents the handler from
    /// reaching worktree/branch destruction (REQ-BED-031).
    #[test]
    fn abandon_gate_rejects_when_continuation_exists() {
        let conv = fixture("parent-a", Some("child-conv-id".to_string()));
        let err = reject_if_continued(&conv, "abandon")
            .expect_err("gate must reject when continued_in_conv_id is set");
        match err {
            AppError::Conflict(detail) => {
                assert_eq!(detail.error_type, "continuation_exists");
                assert_eq!(
                    detail.continuation_id.as_deref(),
                    Some("child-conv-id"),
                    "typed continuation_id must be populated so FE doesn't regex-parse the message",
                );
                assert!(
                    detail.error.contains("Cannot abandon"),
                    "error must name the action: {}",
                    detail.error
                );
                assert!(
                    detail.error.contains("child-conv-id"),
                    "error must include the continuation id for FE routing: {}",
                    detail.error
                );
            }
            _ => panic!("expected AppError::Conflict, got a different variant"),
        }
    }

    // ---- mark-as-merged gate ------------------------------------------

    /// Unblocked: `continued_in_conv_id = None` — gate passes, handler
    /// proceeds with existing mark-merged logic.
    #[test]
    fn mark_merged_gate_passes_when_no_continuation() {
        let conv = fixture("parent-m", None);
        assert!(reject_if_continued(&conv, "mark as merged").is_ok());
    }

    /// Blocked: `continued_in_conv_id = Some(...)` — gate returns 409
    /// with `error_type = "continuation_exists"` and a message naming
    /// the continuation id. Structurally prevents the handler from
    /// reaching worktree/branch destruction (REQ-BED-031).
    #[test]
    fn mark_merged_gate_rejects_when_continuation_exists() {
        let conv = fixture("parent-m", Some("child-conv-id".to_string()));
        let err = reject_if_continued(&conv, "mark as merged")
            .expect_err("gate must reject when continued_in_conv_id is set");
        match err {
            AppError::Conflict(detail) => {
                assert_eq!(detail.error_type, "continuation_exists");
                assert_eq!(
                    detail.continuation_id.as_deref(),
                    Some("child-conv-id"),
                    "typed continuation_id must be populated so FE doesn't regex-parse the message",
                );
                assert!(
                    detail.error.contains("Cannot mark as merged"),
                    "error must name the action: {}",
                    detail.error
                );
                assert!(
                    detail.error.contains("child-conv-id"),
                    "error must include the continuation id for FE routing: {}",
                    detail.error
                );
            }
            _ => panic!("expected AppError::Conflict, got a different variant"),
        }
    }
}
