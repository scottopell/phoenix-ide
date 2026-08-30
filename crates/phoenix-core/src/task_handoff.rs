//! Task-approval handoff payload.
//!
//! The pure-data parcel carried from an approved task to its successor
//! conversation: the exact reviewed artifact and seed plan/title used to
//! render its opening message. Lives in the base
//! crate so both the persistence layer and the runtime can reference it
//! without a cycle.

use crate::task_source::Priority;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovedTaskSnapshot {
    pub task_id: String,
    pub task_title: String,
    pub title: String,
    pub priority: Priority,
    pub plan: String,
    pub task_file: String,
    pub artifact_body: String,
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
    pub title: String,
    pub priority: Priority,
    pub plan: String,
    pub task_file: String,
    pub artifact_body: String,
}

impl From<&TaskApprovalHandoffData> for ApprovedTaskSnapshot {
    fn from(value: &TaskApprovalHandoffData) -> Self {
        Self {
            task_id: value.task_id.clone(),
            task_title: value.task_title.clone(),
            title: value.title.clone(),
            priority: value.priority,
            plan: value.plan.clone(),
            task_file: value.task_file.clone(),
            artifact_body: value.artifact_body.clone(),
        }
    }
}
