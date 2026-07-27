//! Request parsing and validation for the modern bash tool input schema.

use crate::domain::kill_signal::KillSignal;
use crate::work_scope::WorkScopeId;
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
    #[must_use]
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
    #[ts(optional)]
    pub signal: Option<KillSignal>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BashSpawnTarget {
    Context,
    WorkScope(WorkScopeId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum BashInvocation {
    Run {
        cmd: String,
        target: BashSpawnTarget,
        label: Option<String>,
        wait_seconds: Option<i64>,
        lines: Option<i64>,
        since: Option<i64>,
    },
    Peek {
        handle: String,
        lines: Option<i64>,
        since: Option<i64>,
    },
    Wait {
        handle: String,
        wait_seconds: Option<i64>,
        lines: Option<i64>,
        since: Option<i64>,
    },
    Kill {
        handle: String,
        signal: Option<KillSignal>,
    },
}

impl BashInvocation {
    /// Converts a context-directory wire input into a durable invocation.
    ///
    /// # Errors
    /// Returns an error when the selected operation lacks a required field.
    pub fn from_context(input: BashToolInput) -> Result<Self, String> {
        Self::from_input(input, BashSpawnTarget::Context)
    }

    /// Converts a coordinator run contract whose run operation requires a persisted `WorkScope` id.
    ///
    /// # Errors
    /// Returns an error when run lacks `work_scope_id`, a non-run operation carries it,
    /// or the operation lacks another required field.
    pub fn from_with_work_scope_target(mut value: serde_json::Value) -> Result<Self, String> {
        let object = value
            .as_object_mut()
            .ok_or_else(|| "bash input must be an object".to_string())?;
        let is_run = object.get("op").and_then(serde_json::Value::as_str) == Some("run");
        let work_scope_id = object.remove("work_scope_id");
        let target = if is_run {
            let raw = work_scope_id
                .and_then(|value| value.as_str().map(str::to_string))
                .ok_or_else(|| {
                    "work-scope-target bash run requires work_scope_id string field; there is no default"
                        .to_string()
                })?;
            BashSpawnTarget::WorkScope(
                WorkScopeId::parse(&raw)
                    .map_err(|error| format!("invalid work_scope_id for bash run: {error}"))?,
            )
        } else {
            if work_scope_id.is_some() {
                return Err("`work_scope_id` is only valid for bash op=run".to_string());
            }
            BashSpawnTarget::Context
        };
        let input: BashToolInput =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        Self::from_input(input, target)
    }

    fn from_input(input: BashToolInput, target: BashSpawnTarget) -> Result<Self, String> {
        match input.op {
            BashOp::Run => Ok(Self::Run {
                cmd: input.cmd.ok_or_else(|| "run requires `cmd`".to_string())?,
                target,
                label: input.label,
                wait_seconds: input.wait_seconds,
                lines: input.lines,
                since: input.since,
            }),
            BashOp::Peek => Ok(Self::Peek {
                handle: input
                    .handle
                    .ok_or_else(|| "peek requires `handle`".to_string())?,
                lines: input.lines,
                since: input.since,
            }),
            BashOp::Wait => Ok(Self::Wait {
                handle: input
                    .handle
                    .ok_or_else(|| "wait requires `handle`".to_string())?,
                wait_seconds: input.wait_seconds,
                lines: input.lines,
                since: input.since,
            }),
            BashOp::Kill => Ok(Self::Kill {
                handle: input
                    .handle
                    .ok_or_else(|| "kill requires `handle`".to_string())?,
                signal: input.signal,
            }),
        }
    }

    #[must_use]
    pub fn work_scope_target(&self) -> Option<&WorkScopeId> {
        match self {
            Self::Run {
                target: BashSpawnTarget::WorkScope(work_scope_id),
                ..
            } => Some(work_scope_id),
            Self::Run { .. } | Self::Peek { .. } | Self::Wait { .. } | Self::Kill { .. } => None,
        }
    }

    #[must_use]
    pub fn to_context_tool_value(&self) -> serde_json::Value {
        let mut value = self.to_tool_value();
        if let Some(object) = value.as_object_mut() {
            object.remove("work_scope_id");
        }
        value
    }

    #[must_use]
    pub fn to_tool_value(&self) -> serde_json::Value {
        match self {
            Self::Run {
                cmd,
                target,
                label,
                wait_seconds,
                lines,
                since,
            } => {
                let mut value = serde_json::json!({
                    "op": "run",
                    "cmd": cmd,
                    "label": label,
                    "wait_seconds": wait_seconds,
                    "lines": lines,
                    "since": since,
                });
                if let BashSpawnTarget::WorkScope(work_scope_id) = target {
                    value["work_scope_id"] = serde_json::Value::String(work_scope_id.to_string());
                }
                value
            }
            Self::Peek {
                handle,
                lines,
                since,
            } => serde_json::json!({
                "op": "peek", "handle": handle, "lines": lines, "since": since
            }),
            Self::Wait {
                handle,
                wait_seconds,
                lines,
                since,
            } => serde_json::json!({
                "op": "wait", "handle": handle, "wait_seconds": wait_seconds,
                "lines": lines, "since": since
            }),
            Self::Kill { handle, signal } => serde_json::json!({
                "op": "kill", "handle": handle, "signal": signal
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_scope_target_run_requires_id_and_non_run_rejects_it() {
        let work_scope_id = WorkScopeId::new();
        let run = BashInvocation::from_with_work_scope_target(serde_json::json!({
            "op": "run", "cmd": "pwd", "work_scope_id": work_scope_id
        }))
        .unwrap();
        assert!(matches!(
            run,
            BashInvocation::Run {
                target: BashSpawnTarget::WorkScope(_),
                ..
            }
        ));
        assert_eq!(run.work_scope_target(), Some(&work_scope_id));
        assert!(
            BashInvocation::from_with_work_scope_target(serde_json::json!({
                "op": "run", "cmd": "pwd"
            }))
            .is_err()
        );
        assert!(
            BashInvocation::from_with_work_scope_target(serde_json::json!({
                "op": "peek", "handle": "b-1", "work_scope_id": work_scope_id
            }))
            .is_err()
        );
        assert!(matches!(
            BashInvocation::from_with_work_scope_target(serde_json::json!({
                "op": "peek", "handle": "b-1"
            }))
            .unwrap(),
            BashInvocation::Peek { .. }
        ));
    }

    #[test]
    fn signal_wire_form_is_uppercase_and_round_trips() {
        let term: BashToolInput =
            serde_json::from_value(serde_json::json!({"op": "kill", "signal": "TERM"})).unwrap();
        assert_eq!(term.signal, Some(KillSignal::Term));
        let kill: BashToolInput =
            serde_json::from_value(serde_json::json!({"op": "kill", "signal": "KILL"})).unwrap();
        assert_eq!(kill.signal, Some(KillSignal::Kill));

        let serialized = serde_json::to_value(&kill).unwrap();
        assert_eq!(serialized["signal"], "KILL");

        let absent: BashToolInput =
            serde_json::from_value(serde_json::json!({"op": "kill"})).unwrap();
        assert_eq!(absent.signal, None);
    }

    #[test]
    fn unknown_signal_value_is_rejected_at_the_type_boundary() {
        let parsed: Result<BashToolInput, _> =
            serde_json::from_value(serde_json::json!({"op": "kill", "signal": "HUP"}));
        assert!(
            parsed.is_err(),
            "an out-of-domain signal must fail to deserialize, not reach a runtime guard"
        );
    }
}
