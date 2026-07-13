use std::fmt;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use phoenix_core::work_scope::WorkScope;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{Tool, ToolContext, ToolOutput};

const DEFAULT_MAX_WAIT_SECONDS: u64 = 600;
const MAX_WAIT_SECONDS: u64 = 1800;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WaitUntilTarget {
    Bash { handle_id: String },
    TmuxWindow { window_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeRegistrationTarget {
    Bash {
        handle_id: String,
    },
    TmuxWindow {
        server_generation: String,
        window_id: String,
    },
}

#[derive(Debug, Clone)]
pub struct WakeRegistration {
    pub conversation_id: String,
    pub tool_use_id: String,
    pub work_scope: WorkScope,
    pub target: WakeRegistrationTarget,
    pub max_wait_seconds: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WakeRegistrationReceipt {
    pub contract_id: String,
    pub target: WaitUntilTarget,
    pub expires_at: DateTime<Utc>,
    pub registering_tool_use_id: String,
}

#[derive(Debug, Clone)]
pub enum WakeRegistrarError {
    Conflict,
    Retryable,
    NotAccepting,
    Persistence(String),
    Unavailable,
}

impl fmt::Display for WakeRegistrarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict => f.write_str("wake registration conflicts with an earlier intent"),
            Self::Retryable => f.write_str("wake registration must be retried"),
            Self::NotAccepting => f.write_str("wake registration is not accepting new work"),
            Self::Persistence(message) => {
                write!(f, "wake registration persistence failed: {message}")
            }
            Self::Unavailable => f.write_str("wake registration is unavailable"),
        }
    }
}

#[async_trait]
pub trait WakeRegistrar: Send + Sync {
    async fn register(
        &self,
        registration: WakeRegistration,
    ) -> Result<WakeRegistrationReceipt, WakeRegistrarError>;
}

pub struct DisabledWakeRegistrar;

#[async_trait]
impl WakeRegistrar for DisabledWakeRegistrar {
    async fn register(
        &self,
        _registration: WakeRegistration,
    ) -> Result<WakeRegistrationReceipt, WakeRegistrarError> {
        Err(WakeRegistrarError::Unavailable)
    }
}

pub struct WaitUntilTool;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitUntilInput {
    target: WaitUntilTarget,
    #[serde(default = "default_max_wait_seconds")]
    max_wait_seconds: u64,
}

const fn default_max_wait_seconds() -> u64 {
    DEFAULT_MAX_WAIT_SECONDS
}

#[async_trait]
impl Tool for WaitUntilTool {
    fn name(&self) -> &'static str {
        "wait_until"
    }

    fn description(&self) -> String {
        "Register a bounded durable wake for an existing bash handle or tmux_run window owned by this work scope. Use it instead of polling when no useful work remains until the resource terminates. Returns an immediate registration receipt; the later terminal outcome is a separate runtime observation."
            .to_owned()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["target"],
            "additionalProperties": false,
            "properties": {
                "target": {
                    "oneOf": [
                        {
                            "type": "object",
                            "required": ["kind", "handle_id"],
                            "additionalProperties": false,
                            "properties": {
                                "kind": { "const": "bash" },
                                "handle_id": { "type": "string", "minLength": 1 }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["kind", "window_id"],
                            "additionalProperties": false,
                            "properties": {
                                "kind": { "const": "tmux_window" },
                                "window_id": { "type": "string", "minLength": 1 }
                            }
                        }
                    ]
                },
                "max_wait_seconds": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_WAIT_SECONDS,
                    "default": DEFAULT_MAX_WAIT_SECONDS
                }
            }
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let parsed: WaitUntilInput = match serde_json::from_value(input) {
            Ok(parsed) => parsed,
            Err(error) => return error_output("invalid_input", &error.to_string()),
        };
        if !(1..=MAX_WAIT_SECONDS).contains(&parsed.max_wait_seconds) {
            return error_output(
                "max_wait_seconds_out_of_range",
                "max_wait_seconds must be in 1..=1800",
            );
        }
        let target = match validate_target(&ctx, &parsed.target).await {
            Ok(target) => target,
            Err(output) => return output,
        };
        let Some(tool_use_id) = ctx.tool_use_id.clone() else {
            return error_output(
                "wake_registration_unavailable",
                "current tool identity is unavailable",
            );
        };
        if ctx.cancel.is_cancelled() {
            return error_output(
                "wake_registration_cancelled",
                "tool execution was cancelled",
            );
        }
        let receipt = match ctx
            .wake_registrar()
            .register(WakeRegistration {
                conversation_id: ctx.conversation_id.clone(),
                tool_use_id,
                work_scope: ctx.work_scope.clone(),
                target,
                max_wait_seconds: parsed.max_wait_seconds,
            })
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => {
                return error_output(registration_error_id(&error), &error.to_string());
            }
        };
        success_output(json!({
            "status": "registered",
            "contract_id": receipt.contract_id,
            "target": receipt.target,
            "expires_at": receipt.expires_at,
            "registering_tool_use_id": receipt.registering_tool_use_id,
        }))
    }
}

async fn validate_target(
    ctx: &ToolContext,
    target: &WaitUntilTarget,
) -> Result<WakeRegistrationTarget, ToolOutput> {
    match target {
        WaitUntilTarget::Bash { handle_id } => {
            if handle_id.trim().is_empty() {
                return Err(error_output(
                    "invalid_handle_id",
                    "target.handle_id must be non-empty",
                ));
            }
            match ctx
                .bash_handle_registry()
                .inspect_terminal(&ctx.work_scope, handle_id)
                .await
            {
                crate::BashTerminalInspection::Unknown => Err(error_output(
                    "wake_unauthorized_handle",
                    "bash handle is not owned by this work scope",
                )),
                crate::BashTerminalInspection::Live
                | crate::BashTerminalInspection::KillPendingKernel { .. }
                | crate::BashTerminalInspection::Terminal { .. } => {
                    Ok(WakeRegistrationTarget::Bash {
                        handle_id: handle_id.clone(),
                    })
                }
            }
        }
        WaitUntilTarget::TmuxWindow { window_id } => {
            if window_id.trim().is_empty() {
                return Err(error_output(
                    "invalid_window_id",
                    "target.window_id must be non-empty",
                ));
            }
            let Some(server) = ctx.tmux_registry().get_existing(&ctx.work_scope).await else {
                return Err(error_output(
                    "wake_unauthorized_handle",
                    "tmux window is not owned by this work scope",
                ));
            };
            let Some(server_generation) = server.read().await.server_generation.clone() else {
                return Err(error_output(
                    "wake_unauthorized_handle",
                    "tmux server has no stable generation identity",
                ));
            };
            let identity = crate::TmuxWindowIdentity {
                work_scope: ctx.work_scope.clone(),
                server_generation: server_generation.clone(),
                window_id: window_id.clone(),
            };
            if !ctx
                .tmux_registry()
                .is_wait_targetable_window(&identity)
                .await
            {
                return Err(error_output(
                    "wake_unauthorized_handle",
                    "tmux window is not a Phoenix-managed tmux_run resource in this work scope",
                ));
            }
            Ok(WakeRegistrationTarget::TmuxWindow {
                server_generation,
                window_id: window_id.clone(),
            })
        }
    }
}

fn registration_error_id(error: &WakeRegistrarError) -> &'static str {
    match error {
        WakeRegistrarError::Conflict => "wake_registration_conflict",
        WakeRegistrarError::Retryable => "wake_registration_retryable",
        WakeRegistrarError::NotAccepting => "wake_registration_not_accepting",
        WakeRegistrarError::Persistence(_) => "wake_registration_failed",
        WakeRegistrarError::Unavailable => "wake_registration_unavailable",
    }
}

fn success_output(value: Value) -> ToolOutput {
    ToolOutput::success(value.to_string()).with_display(value)
}

fn error_output(id: &str, message: &str) -> ToolOutput {
    let value = json!({"error": id, "message": message});
    ToolOutput::error(value.to_string()).with_display(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BashHandleRegistry, BashTool, BrowserSessionManager, TmuxRegistry};
    use std::{path::PathBuf, sync::Arc};
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    #[derive(Default)]
    struct RecordingRegistrar(Mutex<Vec<WakeRegistration>>);

    #[async_trait]
    impl WakeRegistrar for RecordingRegistrar {
        async fn register(
            &self,
            registration: WakeRegistration,
        ) -> Result<WakeRegistrationReceipt, WakeRegistrarError> {
            let receipt = WakeRegistrationReceipt {
                contract_id: "contract-1".to_owned(),
                target: match &registration.target {
                    WakeRegistrationTarget::Bash { handle_id } => WaitUntilTarget::Bash {
                        handle_id: handle_id.clone(),
                    },
                    WakeRegistrationTarget::TmuxWindow { window_id, .. } => {
                        WaitUntilTarget::TmuxWindow {
                            window_id: window_id.clone(),
                        }
                    }
                },
                expires_at: DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
                    .unwrap()
                    .with_timezone(&Utc),
                registering_tool_use_id: registration.tool_use_id.clone(),
            };
            self.0.lock().await.push(registration);
            Ok(receipt)
        }
    }

    fn context(conversation_id: &str, bash: Arc<BashHandleRegistry>) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            conversation_id.to_owned(),
            PathBuf::from("."),
            Arc::new(BrowserSessionManager::default()),
            bash,
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            Arc::new(TmuxRegistry::new()),
            None,
        )
    }

    #[tokio::test]
    async fn validates_owned_bash_before_registering() {
        let bash = Arc::new(BashHandleRegistry::new());
        let registrar = Arc::new(RecordingRegistrar::default());
        let ctx = context("owner", bash).with_wake_capability("tool-9", registrar.clone());
        let spawned = BashTool
            .run(
                json!({"op":"run","cmd":"sleep 30","wait_seconds":0}),
                ctx.clone(),
            )
            .await;
        let handle = spawned.display_data().unwrap()["handle"]
            .as_str()
            .unwrap()
            .to_owned();
        let result = WaitUntilTool
            .run(json!({"target":{"kind":"bash","handle_id":handle}}), ctx)
            .await;
        assert!(result.is_success(), "{}", result.output());
        assert_eq!(registrar.0.lock().await[0].tool_use_id, "tool-9");
    }

    #[tokio::test]
    async fn registration_failure_is_a_typed_tool_error() {
        let ctx = context("owner", Arc::new(BashHandleRegistry::new()))
            .with_wake_capability("tool-9", Arc::new(DisabledWakeRegistrar));
        let result = WaitUntilTool
            .run(json!({"target":{"kind":"bash","handle_id":"missing"}}), ctx)
            .await;
        assert!(!result.is_success());
        assert_eq!(
            result.display_data().unwrap()["error"],
            "wake_unauthorized_handle"
        );
    }
}
