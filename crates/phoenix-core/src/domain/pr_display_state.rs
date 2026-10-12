//! Display state of a tracked pull request. Co-owned by the api layer
//! (serialized to the client) and the db layer (persisted), so it lives in
//! the base crate.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, ts_rs::TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
#[serde(rename_all = "snake_case")]
pub enum PrDisplayState {
    Open,
    Draft,
    Merged,
    Closed,
}
