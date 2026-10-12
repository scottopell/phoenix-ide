use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
#[serde(rename_all = "snake_case")]
pub enum PrFeedbackStatus {
    #[default]
    Open,
    InProgress,
    Approved,
}
