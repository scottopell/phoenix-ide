use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize, ts_rs::TS, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum PrFeedbackStatus {
    #[default]
    Open,
    InProgress,
    Approved,
}
