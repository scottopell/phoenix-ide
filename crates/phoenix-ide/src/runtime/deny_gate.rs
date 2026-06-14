//! Permission seam: the single gate every tool call passes before execution.
//!
//! The deterministic deny layer (Layer 0) of `specs/permissions/`. A pending
//! `(tool_name, input)` is evaluated with no reference to the conversation
//! transcript; on clear the gate mints a [`CheckedToolCall`] — the only value
//! [`ToolExecutor::execute`](super::traits::ToolExecutor) accepts — and on a
//! rule match it returns a [`Denial`] the model receives as a tool result
//! (deny-and-continue).
//!
//! The token is the correct-by-construction guarantee: `CheckedToolCall` has
//! private fields and `DenyGate::check` is its sole non-test constructor, so a
//! tool call that has not passed the gate cannot reach `execute`.

use crate::tools::ToolOutput;
use serde_json::{json, Value};

/// Proof that the deny layer cleared a tool call. Carries the validated payload
/// so a proof minted for one tool cannot be replayed to run another
/// (specs/permissions REQ-PERM-001, REQ-PERM-002).
#[derive(Debug, Clone)]
pub struct CheckedToolCall {
    name: String,
    input: Value,
}

impl CheckedToolCall {
    /// The cleared tool's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Consume the proof, yielding the validated name and input for execution.
    pub fn into_parts(self) -> (String, Value) {
        (self.name, self.input)
    }

    /// Test-only mint. `#[cfg(test)]` keeps it out of production builds, so the
    /// sole-mint guarantee (`DenyGate::check`) holds wherever it matters.
    #[cfg(test)]
    pub fn cleared_for_test(name: impl Into<String>, input: Value) -> Self {
        Self {
            name: name.into(),
            input,
        }
    }
}

/// A structured rejection from the deny layer (specs/permissions REQ-PERM-004,
/// REQ-PERM-005). `error` is a stable id the model and UI branch on; `reason`
/// is human- and model-readable.
#[derive(Debug, Clone)]
pub struct Denial {
    pub error: String,
    pub reason: String,
}

impl Denial {
    /// Render to the tool-result wire shape delivered to the model. For the
    /// bash `command_safety_rejected` id this is byte-identical to
    /// `phoenix_core::domain::tool_wire::BashErrorResponse::CommandSafetyRejected`
    /// (`{error, error_message, reason}`), so existing UI parsing is unchanged.
    pub fn into_tool_output(self) -> ToolOutput {
        let Denial { error, reason } = self;
        let value = json!({
            "error": error,
            "error_message": reason.clone(),
            "reason": reason,
        });
        let serialized = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
        ToolOutput::error(serialized).with_display(value)
    }
}

/// The deterministic deny layer. Intent-agnostic: a rule reads only the tool
/// name and input, never the transcript. Rules are keyed by tool name; a tool
/// with no rule is allowed.
pub struct DenyGate;

impl DenyGate {
    /// Evaluate a pending call. On clear, returns the proof the executor
    /// requires; on a rule match, returns the denial. Sole mint of
    /// [`CheckedToolCall`] (REQ-PERM-001).
    pub fn check(name: String, input: Value) -> Result<CheckedToolCall, Denial> {
        if name == "bash" {
            bash_deny_rule(&input)?;
        }
        Ok(CheckedToolCall { name, input })
    }
}

/// Bash Layer 0 rule: vet the `run` op's command with the shared AST checker
/// (`phoenix_tools::bash_check`). Only `run` carries a command to vet; the
/// handle ops (peek/wait/kill) operate on already-spawned processes.
fn bash_deny_rule(input: &Value) -> Result<(), Denial> {
    if input.get("op").and_then(Value::as_str) != Some("run") {
        return Ok(());
    }
    let Some(cmd) = input.get("cmd").and_then(Value::as_str) else {
        return Ok(());
    };
    phoenix_tools::bash_check::check(cmd).map_err(|e| Denial {
        error: "command_safety_rejected".to_string(),
        reason: e.message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cmd: &str) -> Value {
        json!({ "op": "run", "cmd": cmd })
    }

    #[test]
    fn blocks_blind_git_add() {
        let d = DenyGate::check("bash".into(), run("git add -A")).unwrap_err();
        assert_eq!(d.error, "command_safety_rejected");
        assert!(d.reason.contains("blind git add"));
    }

    #[test]
    fn blocks_rm_rf_root() {
        let d = DenyGate::check("bash".into(), run("rm -rf /")).unwrap_err();
        assert_eq!(d.error, "command_safety_rejected");
    }

    #[test]
    fn blocks_force_push() {
        let d = DenyGate::check("bash".into(), run("git push --force")).unwrap_err();
        assert_eq!(d.error, "command_safety_rejected");
    }

    #[test]
    fn allows_force_with_lease() {
        assert!(DenyGate::check("bash".into(), run("git push --force-with-lease")).is_ok());
    }

    #[test]
    fn allows_safe_command_and_carries_payload() {
        let c = DenyGate::check("bash".into(), run("ls -la")).unwrap();
        assert_eq!(c.name(), "bash");
        let (name, input) = c.into_parts();
        assert_eq!(name, "bash");
        assert_eq!(input["cmd"], "ls -la");
    }

    #[test]
    fn handle_ops_carry_no_command_and_are_allowed() {
        assert!(DenyGate::check("bash".into(), json!({ "op": "peek", "handle": "b-1" })).is_ok());
    }

    #[test]
    fn unknown_tool_has_no_rule_and_passes() {
        assert!(DenyGate::check("think".into(), json!({ "thoughts": "x" })).is_ok());
    }

    #[test]
    fn denial_wire_shape_matches_bash_contract() {
        let out = Denial {
            error: "command_safety_rejected".to_string(),
            reason: "permission denied: x".to_string(),
        }
        .into_tool_output();
        assert!(!out.is_success());
        let v: Value = serde_json::from_str(out.output()).unwrap();
        assert_eq!(v["error"], "command_safety_rejected");
        assert_eq!(v["error_message"], "permission denied: x");
        assert_eq!(v["reason"], "permission denied: x");
    }
}
