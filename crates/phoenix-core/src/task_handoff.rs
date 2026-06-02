//! Task-approval handoff payload.
//!
//! The pure-data parcel carried from an approved task to its successor
//! conversation: the new conversation's branch/worktree coordinates and the
//! seed plan/title used to render its opening message. Lives in the base
//! crate so both the persistence layer and the runtime can reference it
//! without a cycle.

use crate::task_source::Priority;

#[derive(Debug, Clone)]
pub struct TaskApprovalHandoffData {
    pub task_id: String,
    pub task_title: String,
    pub branch_name: String,
    pub worktree_path: String,
    pub base_branch: String,
    pub title: String,
    pub priority: Priority,
    pub plan: String,
    pub task_file: String,
}
