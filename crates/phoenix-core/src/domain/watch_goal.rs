use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum WatchGoalState {
    Underway,
    Blocked { reason: String },
    Complete { reason: String },
    Exhausted { reason: String },
    Suspended { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum WatchGoalUsage {
    UnavailableNotCaptured,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum WatchGoalCapture {
    LegacyNotCaptured,
    NotCapturedDisabled,
    CapturedNoGoal {
        owner_id: String,
        owner_version: i64,
        usage: WatchGoalUsage,
    },
    CapturedSavedGoal {
        owner_id: String,
        goal_id: String,
        objective: String,
        owner_version: i64,
        state: WatchGoalState,
        usage: WatchGoalUsage,
    },
}
