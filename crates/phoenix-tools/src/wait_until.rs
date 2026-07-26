use crate::bash::handle::HandleId;
use crate::{
    work_scope_identity, RegisterWakeInput, RegisteredWake, Tool, ToolContext, ToolOutput,
    ToolOutputDisposition,
};
use async_trait::async_trait;
use phoenix_workflow::wake_profile::{BashResourceIdentity, WakeResourceIdentity};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const WAKE_DEFAULT_SECONDS: u64 = 600;
const WAKE_MAX_SECONDS: u64 = 1800;
const CONDITION_HANDLE_TERMINAL: &str = "HandleTerminal";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitUntilInput {
    handle: HandleRef,
    condition: String,
    #[serde(default)]
    max_wait_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum HandleRef {
    Bash { id: String },
    TmuxPane { id: String },
    SubAgent { id: String },
}

pub struct WaitUntilTool;

#[allow(clippy::too_many_lines)]
#[async_trait]
impl Tool for WaitUntilTool {
    fn name(&self) -> &'static str {
        "wait_until"
    }

    fn description(&self) -> String {
        format!(
            "Registers a durable wake contract instead of polling. Use when you have nothing else to do until a known handle reaches a terminal state. Input: wait_until {{ handle: {{ kind, id }}, condition, max_wait_seconds }}. V1 supports only condition=\"{CONDITION_HANDLE_TERMINAL}\". This Bash-first slice accepts only handle.kind=\"Bash\" and rejects other handle kinds explicitly. max_wait_seconds defaults to {WAKE_DEFAULT_SECONDS} and is capped at {WAKE_MAX_SECONDS}. Success returns an immediate registration receipt and parks the turn until Phoenix delivers the later terminal result; registration errors do not park."
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "description": format!("Register a wake contract: wait_until {{ handle: {{ kind, id }}, condition, max_wait_seconds }}. V1 supports only condition=\"{CONDITION_HANDLE_TERMINAL}\". This slice accepts only handle.kind=\"Bash\"."),
            "properties": {
                "handle": {
                    "type": "object",
                    "description": "Tagged handle reference. V1 accepts { kind: \"Bash\", id } in this first slice.",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "const": "Bash"
                        },
                        "id": {
                            "type": "string",
                            "description": "Handle identifier within the tagged kind."
                        }
                    },
                    "required": ["kind", "id"],
                    "additionalProperties": false
                },
                "condition": {
                    "type": "string",
                    "enum": [CONDITION_HANDLE_TERMINAL],
                    "description": "Wake condition to watch for. V1 supports only HandleTerminal."
                },
                "max_wait_seconds": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": WAKE_MAX_SECONDS,
                    "description": format!("Maximum lifetime of the wake contract in seconds. Defaults to {WAKE_DEFAULT_SECONDS}; capped at {WAKE_MAX_SECONDS}.")
                }
            },
            "required": ["handle", "condition"]
        })
    }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let parsed: WaitUntilInput = match serde_json::from_value(input) {
            Ok(parsed) => parsed,
            Err(err) => return ToolOutput::error(format!("Invalid input: {err}")),
        };
        if parsed.condition != CONDITION_HANDLE_TERMINAL {
            return ToolOutput::error(format!(
                "Unsupported condition {:?}; v1 supports only {CONDITION_HANDLE_TERMINAL}",
                parsed.condition
            ));
        }
        let Some(tool_use_id) = ctx.tool_use_id() else {
            return ToolOutput::error("wait_until requires a typed tool_use_id in ToolContext");
        };
        let Some(registrar) = ctx.wake_registrar() else {
            return ToolOutput::error("wait_until requires wake registrar support in ToolContext");
        };
        let registration_scope = match work_scope_identity(&ctx.work_scope) {
            Ok(scope) => scope,
            Err(err) => return ToolOutput::error(err),
        };

        let (resource, handle_id) = match parsed.handle {
            HandleRef::Bash { id } => {
                let handle = crate::bash::operations::lookup_handle(&ctx, &id).await.ok();
                let Some(handle) = handle else {
                    return ToolOutput::error(format!(
                        "bash handle {id:?} was not found in this work scope"
                    ));
                };
                drop(handle);
                (
                    WakeResourceIdentity::Bash(BashResourceIdentity {
                        work_scope: registration_scope.clone(),
                        handle_id: id.clone(),
                    }),
                    id,
                )
            }
            HandleRef::TmuxPane { id } => {
                return ToolOutput::error(format!(
                    "Unsupported handle kind \"TmuxPane\" for wait_until v1 Bash-first slice (id={id})"
                ));
            }
            HandleRef::SubAgent { id } => {
                return ToolOutput::error(format!(
                    "Unsupported handle kind \"SubAgent\" for wait_until v1 Bash-first slice (id={id})"
                ));
            }
        };

        let max_wait_seconds = parsed.max_wait_seconds.unwrap_or(WAKE_DEFAULT_SECONDS);
        if max_wait_seconds == 0 || max_wait_seconds > WAKE_MAX_SECONDS {
            return ToolOutput::error(format!(
                "max_wait_seconds must be between 1 and {WAKE_MAX_SECONDS}; got {max_wait_seconds}"
            ));
        }
        let Some(tool_round_id) = ctx.tool_round_id() else {
            return ToolOutput::error("wait_until requires a tool round identity");
        };

        let prepared_fingerprint = registration_fingerprint(
            &ctx.conversation_id,
            &ctx.root_conversation_id,
            tool_round_id,
            tool_use_id,
            &resource,
            max_wait_seconds,
        );
        let contract_id = format!(
            "wake-{}",
            prepared_fingerprint
                .get(..16)
                .expect("SHA-256 hex fingerprints always contain 64 ASCII bytes")
        );

        let register_input = RegisterWakeInput {
            contract_id: contract_id.clone(),
            conversation_id: ctx.conversation_id.clone(),
            root_conversation_id: ctx.root_conversation_id.clone(),
            registering_tool_use_id: tool_use_id.to_string(),
            registering_tool_round_id: tool_round_id.to_string(),
            registration_scope,
            resource: resource.clone(),
            max_wait_seconds,
            prepared_fingerprint: prepared_fingerprint.clone(),
        };

        match registrar.register(register_input).await {
            Ok(
                RegisteredWake::Registered {
                    workflow_id,
                    expires_at,
                }
                | RegisteredWake::Replayed {
                    workflow_id,
                    expires_at,
                },
            ) => {
                let receipt = json!({
                    "status": "registered",
                    "contract_id": contract_id,
                    "workflow_id": workflow_id.0,
                    "handle": { "kind": "Bash", "id": handle_id },
                    "condition": CONDITION_HANDLE_TERMINAL,
                    "max_wait_seconds": max_wait_seconds,
                    "expires_at": expires_at.0,
                    "prepared_fingerprint": prepared_fingerprint,
                });
                ToolOutput::success(receipt.to_string())
                    .with_display(receipt)
                    .with_disposition(ToolOutputDisposition::ParkAfterWakeRegistration {
                        workflow_id: workflow_id.0,
                        contract_id: contract_id.clone(),
                        resource_kind: "Bash".to_string(),
                        handle_id: handle_id.clone(),
                        expires_at: expires_at.0,
                    })
            }
            Ok(RegisteredWake::Conflict) => ToolOutput::error(
                "wait_until registration conflicted with an existing wake contract",
            ),
            Ok(other) => ToolOutput::error(format!(
                "wait_until registration failed with unexpected registrar outcome: {other:?}"
            )),
            Err(err) => ToolOutput::error(format!("wait_until registration failed: {err}")),
        }
    }
}

fn registration_fingerprint(
    conversation_id: &str,
    root_conversation_id: &str,
    tool_round_id: &str,
    tool_use_id: &str,
    resource: &WakeResourceIdentity,
    max_wait_seconds: u64,
) -> String {
    let canonical = serde_json::json!({
        "tool": "wait_until",
        "conversation_id": conversation_id,
        "root_conversation_id": root_conversation_id,
        "tool_round_id": tool_round_id,
        "tool_use_id": tool_use_id,
        "resource": resource,
        "condition": CONDITION_HANDLE_TERMINAL,
        "max_wait_seconds": max_wait_seconds,
    });
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bash::handle::Handle;
    use crate::bash::ring::RING_BUFFER_BYTES;
    use crate::{
        BashHandleRegistry, BrowserSessionManager, RegisteredWake, TmuxRegistry, WakeRegistrar,
    };
    use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
    use phoenix_workflow::wake_profile::WorkScopeIdentity;
    use phoenix_workflow::{Timestamp, WorkflowId};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;
    use tokio_util::sync::CancellationToken;

    struct MockWakeRegistrar {
        inputs: Mutex<Vec<RegisterWakeInput>>,
        outcome: Mutex<Result<RegisteredWake, String>>,
    }

    impl MockWakeRegistrar {
        fn new(outcome: Result<RegisteredWake, String>) -> Arc<Self> {
            Arc::new(Self {
                inputs: Mutex::new(Vec::new()),
                outcome: Mutex::new(outcome),
            })
        }

        fn calls(&self) -> Vec<RegisterWakeInput> {
            self.inputs.lock().expect("inputs lock").clone()
        }
    }

    #[async_trait]
    impl WakeRegistrar for MockWakeRegistrar {
        async fn register(&self, input: RegisterWakeInput) -> Result<RegisteredWake, String> {
            self.inputs.lock().expect("inputs lock").push(input);
            self.outcome.lock().expect("outcome lock").clone()
        }

        async fn cancel(&self, _input: crate::CancelWakeInput) -> Result<RegisteredWake, String> {
            Ok(RegisteredWake::CancelStale)
        }

        fn notify_activation_committed(&self) {}

        async fn rekey_work_scope(
            &self,
            _conversation_id: &str,
            _old_scope: &WorkScopeIdentity,
            _new_scope: &WorkScopeIdentity,
            _resources: crate::WakeScopeRekeyResources,
        ) -> Result<u64, String> {
            Ok(0)
        }
    }

    fn ctx(registrar: Option<Arc<dyn WakeRegistrar>>) -> ToolContext {
        ToolContext::new_with_resource_scope(
            CancellationToken::new(),
            "conv-wait".to_string(),
            PathBuf::from("/tmp"),
            BrowserSessionManager::new(),
            Arc::new(BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            Arc::new(TmuxRegistry::new()),
            Some(PathBuf::from("/tmp/worktree")),
            ResourceScopeKey::Work(WorkScopeId::parse("scope-wait").unwrap()),
            phoenix_core::work_scope::ResourceAuthority::Work,
        )
        .with_root_conversation_id("root-wait".to_string())
        .with_tool_use_id("tool-wait")
        .with_tool_round_id("round-wait")
        .with_wake_registrar(registrar)
    }

    async fn insert_live_handle(ctx: &ToolContext, id: &str) {
        let handle = Handle::new_live(
            ctx.work_scope.clone(),
            HandleId::new(id.to_string()),
            "sleep 10".into(),
            None,
            123,
            123,
            RING_BUFFER_BYTES,
        );
        ctx.bash_handle_registry()
            .register_existing_handle(&ctx.work_scope, handle)
            .await;
    }

    async fn insert_tombstoned_handle(ctx: &ToolContext, id: &str) {
        let handle = Handle::new_live(
            ctx.work_scope.clone(),
            HandleId::new(id.to_string()),
            "true".into(),
            None,
            123,
            123,
            RING_BUFFER_BYTES,
        );
        ctx.bash_handle_registry()
            .register_existing_handle(&ctx.work_scope, handle.clone())
            .await;
        handle
            .transition_to_terminal(
                crate::bash::handle::FinalCause::Exited { exit_code: Some(0) },
                std::time::Duration::from_secs(1),
                SystemTime::now(),
                crate::bash::handle::TOMBSTONE_TAIL_LINES,
            )
            .await;
    }

    fn parse(out: &ToolOutput) -> Value {
        out.display_data()
            .cloned()
            .or_else(|| serde_json::from_str(out.output()).ok())
            .expect("json output")
    }

    #[test]
    fn schema_exposes_tagged_handle_and_timeout_cap() {
        let schema = WaitUntilTool.input_schema();
        assert_eq!(schema["required"], json!(["handle", "condition"]));
        assert_eq!(
            schema["properties"]["handle"]["required"],
            json!(["kind", "id"])
        );
        assert_eq!(
            schema["properties"]["handle"]["properties"]["kind"]["const"],
            json!("Bash")
        );
        assert_eq!(
            schema["properties"]["handle"]["additionalProperties"],
            json!(false)
        );
        assert_eq!(
            schema["properties"]["condition"]["enum"],
            json!([CONDITION_HANDLE_TERMINAL])
        );
        assert_eq!(
            schema["properties"]["max_wait_seconds"]["maximum"],
            json!(WAKE_MAX_SECONDS)
        );
    }

    #[tokio::test]
    async fn missing_bash_handle_is_normative_error() {
        let registrar = MockWakeRegistrar::new(Ok(RegisteredWake::Registered {
            workflow_id: WorkflowId(7),
            expires_at: Timestamp(600),
        }));
        let out = WaitUntilTool.run(json!({"handle": {"kind": "Bash", "id": "b-missing"}, "condition": CONDITION_HANDLE_TERMINAL}), ctx(Some(registrar))).await;
        assert!(!out.is_success());
        assert_eq!(out.disposition(), ToolOutputDisposition::Continue);
        assert!(out.output().contains("was not found in this work scope"));
    }

    #[tokio::test]
    async fn foreign_bash_handle_is_not_visible_across_work_scope() {
        let registrar = MockWakeRegistrar::new(Ok(RegisteredWake::Registered {
            workflow_id: WorkflowId(7),
            expires_at: Timestamp(600),
        }));
        let foreign = ctx(None);
        insert_live_handle(&foreign, "b-foreign").await;
        let out = WaitUntilTool.run(json!({"handle": {"kind": "Bash", "id": "b-foreign"}, "condition": CONDITION_HANDLE_TERMINAL}), ctx(Some(registrar))).await;
        assert!(!out.is_success());
        assert!(out.output().contains("was not found in this work scope"));
    }

    #[tokio::test]
    async fn restricted_actor_cannot_register_wake_for_foreign_same_scope_handle() {
        let registrar = MockWakeRegistrar::new(Ok(RegisteredWake::Registered {
            workflow_id: WorkflowId(7),
            expires_at: Timestamp(600),
        }));
        let context = ctx(Some(registrar.clone()));
        let handle = Handle::new_live_for_actor(
            context.work_scope.clone(),
            HandleId::new("b-private"),
            "owner".to_string(),
            phoenix_core::work_scope::ResourceAuthority::Restricted,
            "sleep 10".into(),
            None,
            123,
            123,
            RING_BUFFER_BYTES,
        );
        context
            .bash_handle_registry()
            .register_existing_handle(&context.work_scope, handle)
            .await;
        let restricted = ToolContext::new_with_resource_scope(
            CancellationToken::new(),
            "sibling".to_string(),
            PathBuf::from("/tmp"),
            BrowserSessionManager::new(),
            context.bash_handle_registry().clone(),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            Arc::new(TmuxRegistry::new()),
            Some(PathBuf::from("/tmp/worktree")),
            context.work_scope.clone(),
            phoenix_core::work_scope::ResourceAuthority::Restricted,
        )
        .with_root_conversation_id("root-wait".to_string())
        .with_tool_use_id("tool-wait")
        .with_wake_registrar(Some(registrar.clone()));

        let out = WaitUntilTool
            .run(
                json!({"handle": {"kind": "Bash", "id": "b-private"}, "condition": CONDITION_HANDLE_TERMINAL}),
                restricted,
            )
            .await;

        assert!(!out.is_success());
        assert!(out.output().contains("was not found in this work scope"));
        assert!(registrar.calls().is_empty());
    }

    #[tokio::test]
    async fn terminal_bash_handle_registers_for_durable_delivery() {
        let registrar = MockWakeRegistrar::new(Ok(RegisteredWake::Registered {
            workflow_id: WorkflowId(7),
            expires_at: Timestamp(600),
        }));
        let context = ctx(Some(registrar.clone()));
        insert_tombstoned_handle(&context, "b-done").await;
        let out = WaitUntilTool.run(json!({"handle": {"kind": "Bash", "id": "b-done"}, "condition": CONDITION_HANDLE_TERMINAL}), context).await;
        assert!(matches!(
            out.disposition(),
            ToolOutputDisposition::ParkAfterWakeRegistration { .. }
        ));
        assert_eq!(registrar.calls().len(), 1);
    }

    #[tokio::test]
    async fn unsupported_kinds_are_rejected_explicitly() {
        let registrar = MockWakeRegistrar::new(Ok(RegisteredWake::Registered {
            workflow_id: WorkflowId(7),
            expires_at: Timestamp(600),
        }));
        let out = WaitUntilTool.run(json!({"handle": {"kind": "TmuxPane", "id": "%1"}, "condition": CONDITION_HANDLE_TERMINAL}), ctx(Some(registrar))).await;
        assert!(!out.is_success());
        assert!(out
            .output()
            .contains("Unsupported handle kind \"TmuxPane\""));
    }

    #[tokio::test]
    async fn registration_invocation_passes_deterministic_identity() {
        let registrar = MockWakeRegistrar::new(Ok(RegisteredWake::Registered {
            workflow_id: WorkflowId(11),
            expires_at: Timestamp(600),
        }));
        let context = ctx(Some(registrar.clone()));
        insert_live_handle(&context, "b-1").await;
        let out = WaitUntilTool.run(json!({"handle": {"kind": "Bash", "id": "b-1"}, "condition": CONDITION_HANDLE_TERMINAL, "max_wait_seconds": 42}), context.clone()).await;
        assert!(out.is_success(), "{}", out.output());
        assert!(matches!(
            out.disposition(),
            ToolOutputDisposition::ParkAfterWakeRegistration { .. }
        ));
        let calls = registrar.calls();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call.conversation_id, "conv-wait");
        assert_eq!(call.root_conversation_id, "root-wait");
        assert_eq!(call.registering_tool_use_id, "tool-wait");
        assert_eq!(
            call.registration_scope,
            WorkScopeIdentity("scope-wait".to_string())
        );
        assert_eq!(
            call.resource,
            WakeResourceIdentity::Bash(BashResourceIdentity {
                work_scope: call.registration_scope.clone(),
                handle_id: "b-1".to_string()
            })
        );
        assert_eq!(
            call.contract_id,
            format!("wake-{}", call.prepared_fingerprint.get(..16).unwrap())
        );
        let receipt = parse(&out);
        assert_eq!(receipt["workflow_id"], 11);
        assert_eq!(receipt["handle"]["kind"], "Bash");
        assert_eq!(receipt["max_wait_seconds"], 42);
    }

    #[test]
    fn registration_identity_uses_timeout_not_wall_clock_expiry() {
        let resource = WakeResourceIdentity::Bash(BashResourceIdentity {
            work_scope: WorkScopeIdentity("root-wait".to_string()),
            handle_id: "b-1".to_string(),
        });
        let first = registration_fingerprint(
            "conv-wait",
            "root-wait",
            "round-wait",
            "tool-wait",
            &resource,
            42,
        );
        let retried = registration_fingerprint(
            "conv-wait",
            "root-wait",
            "round-wait",
            "tool-wait",
            &resource,
            42,
        );
        let reused_provider_id_in_new_round = registration_fingerprint(
            "conv-wait",
            "root-wait",
            "round-next",
            "tool-wait",
            &resource,
            42,
        );
        let different_timeout = registration_fingerprint(
            "conv-wait",
            "root-wait",
            "round-wait",
            "tool-wait",
            &resource,
            43,
        );
        assert_eq!(first, retried);
        assert_ne!(first, reused_provider_id_in_new_round);
        assert_ne!(first, different_timeout);
    }

    #[tokio::test]
    async fn replay_and_conflict_map_to_park_or_continue() {
        let replay = MockWakeRegistrar::new(Ok(RegisteredWake::Replayed {
            workflow_id: WorkflowId(13),
            expires_at: Timestamp(600),
        }));
        let replay_ctx = ctx(Some(replay.clone()));
        insert_live_handle(&replay_ctx, "b-2").await;
        let replay_out = WaitUntilTool.run(json!({"handle": {"kind": "Bash", "id": "b-2"}, "condition": CONDITION_HANDLE_TERMINAL}), replay_ctx).await;
        assert!(replay_out.is_success());
        assert!(matches!(
            replay_out.disposition(),
            ToolOutputDisposition::ParkAfterWakeRegistration { .. }
        ));
        assert_eq!(parse(&replay_out)["expires_at"], 600);

        let conflict = MockWakeRegistrar::new(Ok(RegisteredWake::Conflict));
        let conflict_ctx = ctx(Some(conflict.clone()));
        insert_live_handle(&conflict_ctx, "b-3").await;
        let conflict_out = WaitUntilTool.run(json!({"handle": {"kind": "Bash", "id": "b-3"}, "condition": CONDITION_HANDLE_TERMINAL}), conflict_ctx).await;
        assert!(!conflict_out.is_success());
        assert_eq!(conflict_out.disposition(), ToolOutputDisposition::Continue);
        assert!(conflict_out.output().contains("conflicted"));
    }

    #[tokio::test]
    async fn registrar_error_does_not_park() {
        let registrar = MockWakeRegistrar::new(Err("db down".to_string()));
        let context = ctx(Some(registrar));
        insert_live_handle(&context, "b-4").await;
        let out = WaitUntilTool.run(json!({"handle": {"kind": "Bash", "id": "b-4"}, "condition": CONDITION_HANDLE_TERMINAL}), context).await;
        assert!(!out.is_success());
        assert_eq!(out.disposition(), ToolOutputDisposition::Continue);
        assert!(out.output().contains("db down"));
    }

    #[tokio::test]
    async fn missing_registrar_or_tool_use_id_fail_without_registration() {
        let context = ToolContext::new_with_resource_scope(
            CancellationToken::new(),
            "conv-wait".to_string(),
            PathBuf::from("/tmp"),
            BrowserSessionManager::new(),
            Arc::new(BashHandleRegistry::new()),
            Arc::new(crate::NoLlm),
            phoenix_terminal::ActiveTerminals::new(),
            Arc::new(TmuxRegistry::new()),
            Some(PathBuf::from("/tmp/worktree")),
            ResourceScopeKey::Work(WorkScopeId::parse("scope-wait").unwrap()),
            phoenix_core::work_scope::ResourceAuthority::Work,
        );
        let no_registrar = WaitUntilTool.run(json!({"handle": {"kind": "Bash", "id": "b-1"}, "condition": CONDITION_HANDLE_TERMINAL}), context.clone().with_tool_use_id("tool-only").with_tool_round_id("round-only")).await;
        assert!(!no_registrar.is_success());
        assert!(no_registrar.output().contains("wake registrar"));
        let no_tool_use = WaitUntilTool.run(json!({"handle": {"kind": "Bash", "id": "b-1"}, "condition": CONDITION_HANDLE_TERMINAL}), context.with_wake_registrar(Some(MockWakeRegistrar::new(Ok(RegisteredWake::Registered { workflow_id: WorkflowId(1), expires_at: Timestamp(600) }))))).await;
        assert!(!no_tool_use.is_success());
        assert!(no_tool_use.output().contains("tool_use_id"));
    }
}
