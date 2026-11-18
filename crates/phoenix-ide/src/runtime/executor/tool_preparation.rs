use super::LlmToolSurface;
use super::{
    ConvContext, ConvState, ConversationRuntime, Event, LlmClient, Storage, ToolCall, ToolExecutor,
    ToolResult,
};
use crate::runtime::deny_gate::{CheckedToolCall, DenyGate};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

pub(super) struct DispatchSelection {
    runtime: uuid::Uuid,
    assistant_message: Arc<phoenix_core::domain::sm_state::AssistantMessage>,
    pub(super) tool: ToolCall,
    pub(super) context: ConvContext,
    pub(super) direct_turn: Option<crate::runtime::traits::ActiveDirectTurn>,
}

impl DispatchSelection {
    pub(super) fn assistant_message_id(&self) -> &str {
        &self.assistant_message.message_id
    }

    fn owns(&self, runtime: uuid::Uuid, state: &ConvState) -> bool {
        self.runtime == runtime
            && match state {
                ConvState::ToolExecuting {
                    current_tool,
                    remaining_tools,
                    assistant_message,
                    ..
                } => {
                    (current_tool == &self.tool || remaining_tools.contains(&self.tool))
                        && assistant_message == self.assistant_message.as_ref()
                }
                ConvState::CancellingTool {
                    tool_use_id,
                    skipped_tools,
                    assistant_message,
                    ..
                } => {
                    (tool_use_id == &self.tool.id
                        || skipped_tools.iter().any(|tool| tool.id == self.tool.id))
                        && assistant_message == self.assistant_message.as_ref()
                }
                _ => false,
            }
    }
}

pub(super) struct CurrentSelection(Arc<DispatchSelection>);

impl CurrentSelection {
    pub(super) fn into_selection(self) -> Arc<DispatchSelection> {
        self.0
    }
}

pub(super) struct ClearedOrdinaryCall {
    checked: CheckedToolCall,
    schema: serde_json::Value,
    surface: LlmToolSurface,
    legacy_tmux_conversation_id: Option<String>,
}

impl ClearedOrdinaryCall {
    pub(super) fn into_parts(
        self,
    ) -> (
        CheckedToolCall,
        serde_json::Value,
        LlmToolSurface,
        Option<String>,
    ) {
        (
            self.checked,
            self.schema,
            self.surface,
            self.legacy_tmux_conversation_id,
        )
    }
}

pub(super) enum PreparedTool {
    Result(Box<Event>),
    Native(super::super::agent_execution::SpawnCatalog),
    Ordinary(Box<ClearedOrdinaryCall>),
}

type PreparationFuture = Pin<Box<dyn Future<Output = Result<PreparedTool, String>> + Send>>;

pub(super) struct ToolPreparation {
    selection: Arc<DispatchSelection>,
    failure_boundary: PreparationFailureBoundary,
    phase: PreparationPhase,
}

#[derive(Clone, Copy)]
enum PreparationFailureBoundary {
    Caller,
    Required(super::RequiredSettlementBoundary),
}

enum PreparationPhase {
    Reading(Mutex<PreparationFuture>),
    Interrupted,
    NoRead,
}

enum PreparationDisposition {
    Ready(Result<PreparedTool, String>),
    Interrupted,
}

pub(super) struct PreparationObservation {
    selection: Arc<DispatchSelection>,
    result: PreparationDisposition,
}

impl ToolPreparation {
    pub(super) async fn observe(&mut self) -> PreparationObservation {
        let result = match &mut self.phase {
            PreparationPhase::Reading(read) => PreparationDisposition::Ready(
                read.get_mut()
                    .expect("exclusive tool preparation future")
                    .as_mut()
                    .await,
            ),
            PreparationPhase::Interrupted => PreparationDisposition::Interrupted,
            PreparationPhase::NoRead => std::future::pending().await,
        };
        PreparationObservation {
            selection: Arc::clone(&self.selection),
            result,
        }
    }
}

impl<S: Storage + Clone + 'static, L: LlmClient + 'static, T: ToolExecutor + 'static>
    ConversationRuntime<S, L, T>
{
    pub(super) fn begin_tool_preparation(
        &mut self,
        tool: ToolCall,
    ) -> Result<Option<Event>, String> {
        if self.begin_ordinary_batch(&tool)? {
            return Ok(None);
        }
        let ConvState::ToolExecuting {
            current_tool,
            assistant_message,
            ..
        } = &self.state
        else {
            return Err("tool preparation outside executing state".into());
        };
        if current_tool != &tool {
            return Err("tool preparation does not own the selected call".into());
        }
        let selection = self.own_serial_selection(&tool)?;
        let failure_boundary = self
            .required_settlement_authority
            .as_ref()
            .map_or(PreparationFailureBoundary::Caller, |scope| {
                PreparationFailureBoundary::Required(scope.boundary)
            });
        let preparation = self.make_tool_preparation(selection, failure_boundary);
        self.serial_dispatch_mut().preparation = Some(preparation);
        Ok(None)
    }

    fn make_tool_preparation(
        &self,
        selection: Arc<DispatchSelection>,
        failure_boundary: PreparationFailureBoundary,
    ) -> ToolPreparation {
        let storage = self.storage.clone();
        let executor = self.tool_executor.clone();
        let surface = if selection.context.is_sub_agent && self.grace_turn_granted {
            LlmToolSurface::SubAgentTerminal
        } else {
            LlmToolSurface::Full
        };
        let catalog = super::super::agent_execution::SpawnCatalog::resolve(
            &self.agent_config,
            self.llm_registry.available_execution_routes(),
        );
        let builtin_model = self
            .llm_registry
            .is_builtin_model(&selection.context.model_id);
        let self_goal = self.prepare_self_goal(selection.tool.clone());
        let origin = Arc::clone(&selection);
        let read = prepare_selected_tool(
            storage,
            executor,
            origin,
            surface,
            (catalog, builtin_model),
            self_goal,
        );
        ToolPreparation {
            selection,
            failure_boundary,
            phase: PreparationPhase::Reading(Mutex::new(Box::pin(read))),
        }
    }

    pub(super) fn accept_tool_preparation(
        &mut self,
        observation: PreparationObservation,
    ) -> Result<Option<Event>, String> {
        if self
            .fatal_local_authority_fence
            .fatal_authority_loss_is_latched()
        {
            return Err("runtime processing closed after fatal local authority loss".into());
        }
        if !self
            .serial_dispatch_mut()
            .preparation
            .as_ref()
            .is_some_and(|pending| Arc::ptr_eq(&pending.selection, &observation.selection))
        {
            return Ok(None);
        }
        let mut pending = self
            .serial_dispatch_mut()
            .preparation
            .take()
            .expect("exact observed preparation remains owned");
        if !pending
            .selection
            .owns(self.tool_preparation_runtime, &self.state)
        {
            return Ok(None);
        }
        let prepared = match observation.result {
            PreparationDisposition::Interrupted => {
                if matches!(self.state, ConvState::CancellingTool { .. }) {
                    return Ok(Some(Event::ToolAborted {
                        tool_use_id: pending.selection.tool.id.clone(),
                    }));
                }
                pending.phase = PreparationPhase::NoRead;
                self.serial_dispatch_mut().preparation = Some(pending);
                return Ok(None);
            }
            PreparationDisposition::Ready(_)
                if !matches!(self.state, ConvState::ToolExecuting { .. }) =>
            {
                return Ok(Some(Event::ToolAborted {
                    tool_use_id: pending.selection.tool.id.clone(),
                }));
            }
            PreparationDisposition::Ready(Ok(prepared)) => prepared,
            PreparationDisposition::Ready(Err(error)) => {
                match pending.failure_boundary {
                    PreparationFailureBoundary::Caller => {
                        pending.phase = PreparationPhase::NoRead;
                        self.serial_dispatch_mut().preparation = Some(pending);
                    }
                    PreparationFailureBoundary::Required(boundary) => {
                        self.fatal_local_authority_fence
                            .close(boundary.settlement());
                    }
                }
                return Err(error);
            }
        };
        let current = CurrentSelection(pending.selection);
        match prepared {
            PreparedTool::Result(event) => Ok(Some(*event)),
            PreparedTool::Native(catalog) => {
                self.spawn_catalog = Some(catalog);
                self.start_selected_spawn_agents_tool(current)
            }
            PreparedTool::Ordinary(cleared) => {
                let mut admitted = self.admit_authoritative_effect()?;
                self.launch_cleared_tool(current, *cleared, &mut admitted);
                Ok(None)
            }
        }
    }

    pub(super) fn interrupt_read_preparation_before_cancellation_checkpoint(
        &mut self,
        event: &Event,
        next: &ConvState,
    ) {
        if !matches!(self.foreground_dispatch, ForegroundDispatch::Serial(_)) {
            return;
        }
        if matches!(event, Event::UserCancel { .. })
            && matches!(self.state, ConvState::ToolExecuting { .. })
            && matches!(next, ConvState::CancellingTool { .. })
        {
            let owns = self
                .serial_dispatch()
                .preparation
                .as_ref()
                .is_some_and(|pending| {
                    pending
                        .selection
                        .owns(self.tool_preparation_runtime, &self.state)
                        && pending.selection.owns(self.tool_preparation_runtime, next)
                });
            if owns {
                self.serial_dispatch_mut()
                    .preparation
                    .as_mut()
                    .expect("checked read owner")
                    .phase = PreparationPhase::Interrupted;
            }
        }
    }

    pub(super) fn interrupt_read_only_tool_preparation(&mut self, tool_use_id: &str) -> bool {
        if !matches!(self.foreground_dispatch, ForegroundDispatch::Serial(_)) {
            return false;
        }
        if self
            .serial_dispatch()
            .preparation
            .as_ref()
            .is_some_and(|pending| {
                pending.selection.tool.id == tool_use_id
                    && pending
                        .selection
                        .owns(self.tool_preparation_runtime, &self.state)
                    && matches!(self.state, ConvState::CancellingTool { .. })
            })
        {
            self.serial_dispatch_mut()
                .preparation
                .as_mut()
                .expect("exact read owner checked")
                .phase = PreparationPhase::Interrupted;
            return true;
        }
        false
    }
}

async fn prepare_selected_tool<S: Storage, T: ToolExecutor>(
    storage: S,
    executor: Arc<T>,
    origin: Arc<DispatchSelection>,
    surface: LlmToolSurface,
    (catalog, builtin_model): (super::super::agent_execution::SpawnCatalog, bool),
    self_goal: impl Future<Output = Result<Option<Event>, String>> + Send,
) -> Result<PreparedTool, String> {
    let tool = &origin.tool;
    let context = &origin.context;
    let policy = storage
        .load_tool_admission_policy(&context.conversation_id)
        .await?;
    let definition = policy
        .declarations()
        .iter()
        .find(|definition| definition.name == tool.name() && policy.is_callable(tool.name()));
    let Some(definition) = definition else {
        return Ok(unavailable(
            tool,
            "tool was unavailable in the originating request policy",
        ));
    };
    let schema = definition.input_schema.clone();
    if surface == LlmToolSurface::SubAgentTerminal
        && !matches!(tool.name(), "submit_result" | "submit_error")
    {
        return Ok(unavailable(
            tool,
            "tool is unavailable under the current conversation policy",
        ));
    }
    if matches!(tool.name(), "create_goal" | "enable_auto_continuation") {
        return self_goal
            .await?
            .map(prepared_result)
            .ok_or_else(|| "self-goal preparation produced no disposition".into());
    }
    if tool.name() == "spawn_agents" {
        let advertised = executor
            .definitions()
            .await
            .iter()
            .any(|definition| definition.name == "spawn_agents");
        let definition =
            catalog.tool_definition(&context.model_id, builtin_model, context.is_coordinator);
        return Ok(if advertised && definition.input_schema == schema {
            PreparedTool::Native(catalog)
        } else {
            unavailable(
                tool,
                "spawn_agents no longer matches the originating request policy",
            )
        });
    }
    let legacy_tmux_conversation_id = match &context.resource_scope {
        phoenix_core::work_scope::ResourceScopeKey::Work(scope_id) => {
            storage
                .get_work_scope_legacy_conversation_id(scope_id)
                .await?
        }
        phoenix_core::work_scope::ResourceScopeKey::Unattached(id) => Some(id.clone()),
        phoenix_core::work_scope::ResourceScopeKey::Coordinator
        | phoenix_core::work_scope::ResourceScopeKey::GlobalTerminal => None,
    };
    Ok(
        match DenyGate::check(tool.name().to_owned(), tool.input.to_value()) {
            Ok(checked) => PreparedTool::Ordinary(Box::new(ClearedOrdinaryCall {
                checked,
                schema,
                surface,
                legacy_tmux_conversation_id,
            })),
            Err(denial) => prepared_result(Event::ToolComplete {
                tool_use_id: tool.id.clone(),
                result: ToolResult {
                    tool_use_id: tool.id.clone(),
                    outcome: super::tool_output_to_outcome(denial.into_tool_output()),
                    duration_ms: Some(0),
                },
            }),
        },
    )
}

fn prepared_result(event: Event) -> PreparedTool {
    PreparedTool::Result(Box::new(event))
}

fn unavailable(tool: &ToolCall, message: &str) -> PreparedTool {
    prepared_result(Event::ToolComplete {
        tool_use_id: tool.id.clone(),
        result: ToolResult::error(tool.id.clone(), format!("EUNAVAIL: {message}")),
    })
}

#[cfg(test)]
mod tests;

mod batch;
pub(super) use batch::ForegroundDispatch;
