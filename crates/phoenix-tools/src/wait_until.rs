use async_trait::async_trait;
use chrono::{DateTime, Utc};
use phoenix_core::domain::wake_contracts::{WakeBashFiredPayload, WakeTmuxFiredPayload};
use phoenix_core::work_scope::WorkScope;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;

use crate::bash::registry::BashHandleInspection;
use crate::tmux::registry::TmuxWindowInspection;
use crate::{Tool, ToolContext, ToolOutput};

const DEFAULT_MAX_WAIT_SECONDS: u64 = 600;
const MAX_WAIT_SECONDS: u64 = 1800;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WaitUntilTarget {
    Bash { handle_id: String },
    TmuxWindow { handle_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeBashInitialTerminalEvidence {
    pub observed_at: DateTime<Utc>,
    pub payload: WakeBashFiredPayload,
    pub tails: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeTmuxInitialTerminalEvidence {
    pub observed_at: DateTime<Utc>,
    pub payload: WakeTmuxFiredPayload,
    pub tails: Vec<String>,
}

/// A validated registration target. Each substrate can carry only its own
/// terminal evidence; `None` means the target was live when inspected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeRegistrationTarget {
    Bash {
        handle_id: String,
        initial_terminal_evidence: Option<WakeBashInitialTerminalEvidence>,
    },
    TmuxWindow {
        handle_id: String,
        initial_terminal_evidence: Option<WakeTmuxInitialTerminalEvidence>,
    },
}

impl WakeRegistrationTarget {
    #[must_use]
    pub fn target(&self) -> WaitUntilTarget {
        match self {
            Self::Bash { handle_id, .. } => WaitUntilTarget::Bash {
                handle_id: handle_id.clone(),
            },
            Self::TmuxWindow { handle_id, .. } => WaitUntilTarget::TmuxWindow {
                handle_id: handle_id.clone(),
            },
        }
    }
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
pub struct WakeRegistrarError(pub String);

impl fmt::Display for WakeRegistrarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[async_trait]
pub trait WakeRegistrar: Send + Sync {
    async fn register(
        &self,
        registration: WakeRegistration,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<WakeRegistrationReceipt, WakeRegistrarError>;
}

pub struct DisabledWakeRegistrar;

#[async_trait]
impl WakeRegistrar for DisabledWakeRegistrar {
    async fn register(
        &self,
        _registration: WakeRegistration,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<WakeRegistrationReceipt, WakeRegistrarError> {
        Err(WakeRegistrarError("wake registration is disabled".into()))
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
        "Register a durable wake contract for a bash handle or tmux_run window owned by this work scope. Returns a registration receipt immediately; an already-terminal target is registered and delivered through the same wake inbox protocol. Unknown or cross-scope handles are rejected.".into()
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
                            "required": ["kind", "handle_id"],
                            "additionalProperties": false,
                            "properties": {
                                "kind": { "const": "tmux_window" },
                                "handle_id": { "type": "string", "minLength": 1 }
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
            Err(parse_error) => return error("invalid_input", &parse_error.to_string()),
        };
        if parsed.max_wait_seconds == 0 || parsed.max_wait_seconds > MAX_WAIT_SECONDS {
            return error(
                "max_wait_seconds_out_of_range",
                "max_wait_seconds must be in 1..=1800",
            );
        }
        let handle_id = match &parsed.target {
            WaitUntilTarget::Bash { handle_id } | WaitUntilTarget::TmuxWindow { handle_id } => {
                handle_id
            }
        };
        if handle_id.trim().is_empty() {
            return error("invalid_handle_id", "target.handle_id must be non-empty");
        }

        let registration_target = match inspect_target(&ctx, &parsed.target).await {
            Ok(target) => target,
            Err(output) => return output,
        };

        let Some(tool_use_id) = ctx.tool_use_id.clone() else {
            return error(
                "wake_registration_unavailable",
                "current tool_use_id is unavailable",
            );
        };
        let registration = WakeRegistration {
            conversation_id: ctx.conversation_id.clone(),
            tool_use_id,
            work_scope: ctx.work_scope.clone(),
            target: registration_target,
            max_wait_seconds: parsed.max_wait_seconds,
        };
        // Inspection can await substrate state, so cancellation must be checked at
        // the last possible point before durable registration begins.
        if ctx.cancel.is_cancelled() {
            return error(
                "wake_registration_cancelled",
                "tool execution was cancelled",
            );
        }
        let receipt = match ctx
            .wake_registrar()
            .register(registration, ctx.cancel.clone())
            .await
        {
            Ok(receipt) => receipt,
            Err(registration_error) => {
                return error("wake_registration_failed", &registration_error.to_string())
            }
        };
        success(json!({
            "status": "registered",
            "contract_id": receipt.contract_id,
            "target": receipt.target,
            "expires_at": receipt.expires_at,
            "registering_tool_use_id": receipt.registering_tool_use_id,
        }))
    }
}

async fn inspect_target(
    ctx: &ToolContext,
    target: &WaitUntilTarget,
) -> Result<WakeRegistrationTarget, ToolOutput> {
    match target {
        WaitUntilTarget::Bash { handle_id } => {
            match ctx
                .bash_handle_registry()
                .inspect(&ctx.work_scope, handle_id)
                .await
            {
                BashHandleInspection::Unknown => Err(error(
                    "unknown_target",
                    "bash handle is not owned by this work scope",
                )),
                BashHandleInspection::Live => Ok(WakeRegistrationTarget::Bash {
                    handle_id: handle_id.clone(),
                    initial_terminal_evidence: None,
                }),
                BashHandleInspection::Terminal {
                    observed_at,
                    payload,
                    tails,
                } => Ok(WakeRegistrationTarget::Bash {
                    handle_id: handle_id.clone(),
                    initial_terminal_evidence: Some(WakeBashInitialTerminalEvidence {
                        observed_at,
                        payload,
                        tails,
                    }),
                }),
            }
        }
        WaitUntilTarget::TmuxWindow { handle_id } => {
            if !ctx
                .tmux_registry()
                .owns_window_run(&ctx.work_scope, handle_id)
                .await
            {
                return Err(error(
                    "unknown_target",
                    "tmux window is not owned by tmux_run in this work scope",
                ));
            }
            match ctx
                .tmux_registry()
                .inspect_window(&ctx.work_scope, handle_id)
                .await
            {
                Err(e) => {
                    tracing::error!(error = %e, %handle_id, "wait_until: tmux durable evidence unavailable");
                    Err(error("target_evidence_unavailable", &e.to_string()))
                }
                Ok(TmuxWindowInspection::Missing) => Err(error(
                    "unknown_target",
                    "tmux window is not owned by this work scope",
                )),
                Ok(TmuxWindowInspection::Live) => Ok(WakeRegistrationTarget::TmuxWindow {
                    handle_id: handle_id.clone(),
                    initial_terminal_evidence: None,
                }),
                Ok(TmuxWindowInspection::Terminal(evidence)) => {
                    let status = match evidence.status {
                        crate::tmux::registry::TmuxTerminalStatus::Exited => {
                            phoenix_core::domain::wake_contracts::WakeTmuxObservedStatus::ExitMarkerObserved
                        }
                        crate::tmux::registry::TmuxTerminalStatus::Killed => {
                            phoenix_core::domain::wake_contracts::WakeTmuxObservedStatus::WindowKilled
                        }
                    };
                    Ok(WakeRegistrationTarget::TmuxWindow {
                        handle_id: handle_id.clone(),
                        initial_terminal_evidence: Some(WakeTmuxInitialTerminalEvidence {
                            observed_at: DateTime::<Utc>::from(evidence.observed_at),
                            payload: WakeTmuxFiredPayload {
                                status,
                                exit_code: evidence.exit_code.map(i64::from),
                                duration_ms: Some(
                                    i64::try_from(evidence.duration_ms).unwrap_or(i64::MAX),
                                ),
                            },
                            tails: evidence.tail.lines().map(str::to_owned).collect(),
                        }),
                    })
                }
            }
        }
    }
}

fn success(value: Value) -> ToolOutput {
    ToolOutput::success(value.to_string()).with_display(value)
}

fn error(id: &str, message: &str) -> ToolOutput {
    let value = json!({"error": id, "message": message});
    ToolOutput::error(value.to_string()).with_display(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BashHandleRegistry, BashTool, BrowserSessionManager, TmuxRegistry};
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    #[derive(Default)]
    struct RecordingRegistrar(Mutex<Vec<WakeRegistration>>);

    impl RecordingRegistrar {
        fn expires_at() -> DateTime<Utc> {
            DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
                .unwrap()
                .with_timezone(&Utc)
        }
    }

    #[async_trait]
    impl WakeRegistrar for RecordingRegistrar {
        async fn register(
            &self,
            registration: WakeRegistration,
            _cancellation: CancellationToken,
        ) -> Result<WakeRegistrationReceipt, WakeRegistrarError> {
            let ordinal = self.0.lock().await.len() + 1;
            let receipt = WakeRegistrationReceipt {
                contract_id: format!("wake-{ordinal}"),
                target: registration.target.target(),
                expires_at: Self::expires_at(),
                registering_tool_use_id: registration.tool_use_id.clone(),
            };
            self.0.lock().await.push(registration);
            Ok(receipt)
        }
    }

    fn context(
        conversation_id: &str,
        bash: Arc<BashHandleRegistry>,
        tmux: Arc<TmuxRegistry>,
    ) -> ToolContext {
        ToolContext::new(
            CancellationToken::new(),
            conversation_id.into(),
            PathBuf::from("."),
            Arc::new(BrowserSessionManager::default()),
            bash,
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            tmux,
            None,
        )
    }

    fn json_output(output: &ToolOutput) -> Value {
        output.display_data().cloned().unwrap()
    }

    #[tokio::test]
    async fn validates_input_bounds_and_target_shape() {
        let ctx = context(
            "validation",
            Arc::new(BashHandleRegistry::new()),
            Arc::new(TmuxRegistry::new()),
        );
        for input in [
            json!({"target":{"kind":"bash","handle_id":"b-1"},"max_wait_seconds":0}),
            json!({"target":{"kind":"bash","handle_id":"b-1"},"max_wait_seconds":1801}),
            json!({"target":{"kind":"other","handle_id":"b-1"}}),
        ] {
            assert!(!WaitUntilTool.run(input, ctx.clone()).await.is_success());
        }
        assert_eq!(
            WaitUntilTool.input_schema()["properties"]["max_wait_seconds"]["default"],
            600
        );
    }

    #[tokio::test]
    async fn rejects_unknown_and_cross_scope_bash_before_registration() {
        let bash = Arc::new(BashHandleRegistry::new());
        let tmux = Arc::new(TmuxRegistry::new());
        let owner = context("owner", bash.clone(), tmux.clone());
        let spawned = BashTool
            .run(json!({"op":"run","cmd":"sleep 30","wait_seconds":0}), owner)
            .await;
        let handle = json_output(&spawned)["handle"]
            .as_str()
            .unwrap()
            .to_string();
        let registrar = Arc::new(RecordingRegistrar::default());
        let other = context("other", bash, tmux).with_wake_capability("tool-1", registrar.clone());
        let result = WaitUntilTool
            .run(json!({"target":{"kind":"bash","handle_id":handle}}), other)
            .await;
        assert_eq!(json_output(&result)["error"], "unknown_target");
        assert!(registrar.0.lock().await.is_empty());
    }

    #[tokio::test]
    async fn registers_live_bash_with_default_wait_and_tool_identity() {
        let bash = Arc::new(BashHandleRegistry::new());
        let tmux = Arc::new(TmuxRegistry::new());
        let registrar = Arc::new(RecordingRegistrar::default());
        let ctx =
            context("receipt", bash, tmux).with_wake_capability("tool-use-9", registrar.clone());
        let spawned = BashTool
            .run(
                json!({"op":"run","cmd":"sleep 30","wait_seconds":0}),
                ctx.clone(),
            )
            .await;
        let handle = json_output(&spawned)["handle"]
            .as_str()
            .unwrap()
            .to_string();
        let result = WaitUntilTool
            .run(json!({"target":{"kind":"bash","handle_id":handle}}), ctx)
            .await;
        assert_eq!(
            json_output(&result),
            json!({
                "status": "registered",
                "contract_id": "wake-1",
                "target": {"kind": "bash", "handle_id": handle},
                "expires_at": "2030-01-02T03:04:05Z",
                "registering_tool_use_id": "tool-use-9",
            })
        );
        let registrations = registrar.0.lock().await;
        assert_eq!(registrations.len(), 1);
        assert_eq!(registrations[0].tool_use_id, "tool-use-9");
        assert_eq!(registrations[0].max_wait_seconds, 600);
        assert!(matches!(
            &registrations[0].target,
            WakeRegistrationTarget::Bash {
                initial_terminal_evidence: None,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn terminal_bash_registers_with_initial_evidence_and_returns_receipt() {
        let bash = Arc::new(BashHandleRegistry::new());
        let tmux = Arc::new(TmuxRegistry::new());
        let registrar = Arc::new(RecordingRegistrar::default());
        let ctx = context("terminal", bash, tmux)
            .with_wake_capability("tool-use-terminal", registrar.clone());
        let spawned = BashTool
            .run(
                json!({"op":"run","cmd":"printf done","wait_seconds":5}),
                ctx.clone(),
            )
            .await;
        let handle = json_output(&spawned)["handle"]
            .as_str()
            .unwrap()
            .to_string();
        let result = WaitUntilTool
            .run(json!({"target":{"kind":"bash","handle_id":handle}}), ctx)
            .await;
        assert_eq!(
            json_output(&result),
            json!({
                "status": "registered",
                "contract_id": "wake-1",
                "target": {"kind": "bash", "handle_id": handle},
                "expires_at": "2030-01-02T03:04:05Z",
                "registering_tool_use_id": "tool-use-terminal",
            })
        );
        let registrations = registrar.0.lock().await;
        assert_eq!(registrations.len(), 1);
        match &registrations[0].target {
            WakeRegistrationTarget::Bash {
                initial_terminal_evidence:
                    Some(WakeBashInitialTerminalEvidence {
                        observed_at,
                        payload,
                        tails,
                    }),
                ..
            } => {
                assert!(*observed_at <= Utc::now());
                assert_eq!(
                    payload.status,
                    phoenix_core::domain::wake_contracts::WakeBashObservedStatus::Exited
                );
                assert_eq!(payload.exit_code, Some(0));
                assert_eq!(tails, &["done"]);
            }
            target @ (WakeRegistrationTarget::Bash { .. }
            | WakeRegistrationTarget::TmuxWindow { .. }) => {
                panic!("expected typed bash terminal evidence, got {target:?}")
            }
        }
    }

    #[tokio::test]
    async fn terminal_tmux_registers_with_typed_initial_evidence() {
        use crate::TmuxTerminalStatus;

        let bash = Arc::new(BashHandleRegistry::new());
        let tmux = Arc::new(TmuxRegistry::new());
        let registrar = Arc::new(RecordingRegistrar::default());
        let ctx = context("terminal-tmux", bash, tmux.clone())
            .with_wake_capability("tool-use-tmux", registrar.clone());
        tmux.install_generation_for_test(&ctx.work_scope, "test-generation")
            .await;
        let _ = tmux.register_window_start(&ctx.work_scope, "@9").await;
        let recorded = tmux
            .record_window_terminal(
                &ctx.work_scope,
                "@9",
                Some(7),
                TmuxTerminalStatus::Exited,
                "first\nsecond\n".into(),
            )
            .await
            .expect("terminal evidence persists");

        let result = WaitUntilTool
            .run(
                json!({"target":{"kind":"tmux_window","handle_id":"@9"}}),
                ctx,
            )
            .await;

        assert_eq!(json_output(&result)["contract_id"], "wake-1");
        assert_eq!(json_output(&result)["expires_at"], "2030-01-02T03:04:05Z");
        let registrations = registrar.0.lock().await;
        assert_eq!(registrations.len(), 1);
        match &registrations[0].target {
            WakeRegistrationTarget::TmuxWindow {
                initial_terminal_evidence:
                    Some(WakeTmuxInitialTerminalEvidence {
                        observed_at,
                        payload,
                        tails,
                    }),
                ..
            } => {
                assert_eq!(*observed_at, DateTime::<Utc>::from(recorded.observed_at));
                assert_eq!(
                    payload.status,
                    phoenix_core::domain::wake_contracts::WakeTmuxObservedStatus::ExitMarkerObserved
                );
                assert_eq!(payload.exit_code, Some(7));
                assert_eq!(tails, &["first", "second"]);
            }
            target @ (WakeRegistrationTarget::Bash { .. }
            | WakeRegistrationTarget::TmuxWindow { .. }) => {
                panic!("expected typed tmux terminal evidence, got {target:?}")
            }
        }
    }
}
