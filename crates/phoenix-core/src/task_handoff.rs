//! Task-approval handoff payload.
//!
//! The pure-data parcel carried from an approved task to its successor
//! conversation: the new conversation's branch/worktree coordinates and the
//! seed plan/title used to render its opening message. Lives in the base
//! crate so both the persistence layer and the runtime can reference it
//! without a cycle.

use crate::task_source::Priority;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovedTaskSnapshot {
    pub task_id: String,
    pub task_title: String,
    pub branch_name: String,
    pub approved_commit_oid: String,
    pub base_branch: String,
    pub title: String,
    pub priority: Priority,
    pub plan: String,
    pub task_file: String,
}

impl ApprovedTaskSnapshot {
    #[must_use]
    pub fn seed_message(&self) -> String {
        format!(
            "Task approved. Execute the approved plan below.\n\n# {}\n\nPriority: {}\n\n{}",
            self.title, self.priority, self.plan
        )
    }
}

#[derive(Debug, Clone)]
pub struct TaskApprovalHandoffData {
    pub task_id: String,
    pub task_title: String,
    pub branch_name: String,
    pub approved_commit_oid: String,
    pub worktree_path: String,
    pub base_branch: String,
    pub title: String,
    pub priority: Priority,
    pub plan: String,
    pub task_file: String,
}

impl From<&TaskApprovalHandoffData> for ApprovedTaskSnapshot {
    fn from(value: &TaskApprovalHandoffData) -> Self {
        Self {
            task_id: value.task_id.clone(),
            task_title: value.task_title.clone(),
            branch_name: value.branch_name.clone(),
            approved_commit_oid: value.approved_commit_oid.clone(),
            base_branch: value.base_branch.clone(),
            title: value.title.clone(),
            priority: value.priority,
            plan: value.plan.clone(),
            task_file: value.task_file.clone(),
        }
    }
}
