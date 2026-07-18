use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashProgressLine {
    pub offset: u64,
    pub text: String,
}

/// Ephemeral, bounded live-progress snapshot emitted by one in-flight bash
/// invocation. The runtime binds it to the invocation's `tool_use_id` at the
/// SSE boundary; the finalized tool result remains the durable authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashToolProgress {
    pub handle: String,
    pub start_offset: u64,
    pub end_offset: u64,
    pub truncated_before: bool,
    pub lines: Vec<BashProgressLine>,
    pub partial: Option<String>,
}
