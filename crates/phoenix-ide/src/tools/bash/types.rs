//! Request parsing and validation for the modern bash tool input schema.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub enum BashOp {
    Run,
    Peek,
    Wait,
    Kill,
}

impl BashOp {
    pub fn as_field_name(self) -> &'static str {
        match self {
            BashOp::Run => "run",
            BashOp::Peek => "peek",
            BashOp::Wait => "wait",
            BashOp::Kill => "kill",
        }
    }
}

/// Modern bash tool input shape shared by the tool parser, state machine, and UI codegen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export, export_to = "../../../ui/src/generated/")]
pub struct BashToolInput {
    pub op: BashOp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cmd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wait_seconds: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "'TERM' | 'KILL'")]
    pub signal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub lines: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub since: Option<i64>,
}
impl BashToolInput {
    pub fn run(cmd: impl Into<String>) -> Self {
        Self {
            op: BashOp::Run,
            cmd: Some(cmd.into()),
            handle: None,
            label: None,
            wait_seconds: None,
            signal: None,
            lines: None,
            since: None,
        }
    }
}
