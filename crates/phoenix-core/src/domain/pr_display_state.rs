//! Display state of a tracked pull request. Co-owned by the api layer
//! (serialized to the client) and the db layer (persisted), so it lives in
//! the base crate.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, ts_rs::TS, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum PrDisplayState {
    Open,
    Draft,
    Merged,
    Closed,
}
