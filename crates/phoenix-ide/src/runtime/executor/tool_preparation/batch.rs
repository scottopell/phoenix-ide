use super::super::{RequiredSettlementBoundary, ToolExecOutcome, ToolExecutionSlot};
use super::{
    Arc, ConvState, ConversationRuntime, CurrentSelection, DispatchSelection, Event, LlmClient,
    PreparationDisposition, PreparationFailureBoundary, PreparationObservation, PreparationPhase,
    PreparedOrdinaryTool, Storage, ToolCall, ToolExecutor, ToolPreparation, ToolResult,
};
use futures::FutureExt as _;
use phoenix_core::domain::sm_state::AssistantMessage;
use std::task::Poll;
use std::{future::Future, pin::Pin};
use tokio::{sync::oneshot, task::JoinHandle};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(in super::super) struct SerialDispatch {
    pub(in super::super) preparation: Option<ToolPreparation>,
    pub(in super::super) execution: ToolExecutionSlot,
    pub(in super::super) publication: Option<Arc<()>>,
    pub(in super::super) running: Option<Arc<DispatchSelection>>,
    pub(super) round: Option<SerialRound>,
}

pub(super) struct SerialRound {
    origin: Arc<AssistantMessage>,
    unissued: Vec<Arc<DispatchSelection>>,
    cancelling: bool,
}

pub(in super::super) enum ForegroundDispatch {
    Serial(SerialDispatch),
    OrdinaryBatch(OrdinaryBatch),
}

impl Default for ForegroundDispatch {
    fn default() -> Self {
        Self::Serial(SerialDispatch::default())
    }
}

pub(in super::super) struct OrdinaryBatch {
    origin: Arc<AssistantMessage>,
    serial_generation: u64,
    calls: Vec<BatchCall>,
    lifecycle: BatchLifecycle,
}

enum BatchLifecycle {
    Open,
    ClosingBeforeCommit,
    ClosingAfterCommit,
}

impl BatchLifecycle {
    fn begin_closing(&mut self) {
        if matches!(self, Self::Open) {
            *self = Self::ClosingBeforeCommit;
        }
    }

    fn is_open(&self) -> bool {
        matches!(self, Self::Open)
    }
}

struct BatchCall {
    selection: Arc<DispatchSelection>,
    boundary: PreparationFailureBoundary,
    phase: CallPhase,
}

enum CallPhase {
    Unissued,
    Cleared,
    Preparing(ToolPreparation<PreparedOrdinaryTool>),
    Running(RunningCall),
    Ready {
        outcome: ToolExecOutcome,
        execution: Option<RunningCall>,
    },
    Settling(Option<RunningCall>),
    ForcedRetired {
        _execution: RunningCall,
    },
    Retired,
    NoRead,
}

struct RunningCall {
    cancel: CancellationToken,
    task: JoinHandle<()>,
    receiver: oneshot::Receiver<ToolExecOutcome>,
    publication: Option<Arc<()>>,
    cutoff: Option<tokio::time::Instant>,
    cancel_on_drop: bool,
}

impl Drop for RunningCall {
    fn drop(&mut self) {
        if self.cancel_on_drop {
            self.cancel.cancel();
        }
        self.task.abort();
    }
}

impl BatchCall {
    fn accept_outcome(&mut self, outcome: ToolExecOutcome) -> Option<Event> {
        if matches!(self.phase, CallPhase::Running(_)) {
            let old = std::mem::replace(&mut self.phase, CallPhase::Retired);
            let CallPhase::Running(mut execution) = old else {
                unreachable!()
            };
            execution.cancel_on_drop = false;
            self.phase = CallPhase::Settling(Some(execution));
        } else if !matches!(self.phase, CallPhase::Settling(_)) {
            return None;
        }
        let outcome = outcome.for_invocation(&self.selection.tool.id);
        let event = match outcome {
            ToolExecOutcome::Completed(result) => Event::ToolComplete {
                tool_use_id: self.selection.tool.id.clone(),
                result,
            },
            ToolExecOutcome::Failed { error, .. } => Event::ToolComplete {
                tool_use_id: self.selection.tool.id.clone(),
                result: ToolResult::error(self.selection.tool.id.clone(), error),
            },
            ToolExecOutcome::Aborted { .. } => Event::ToolAborted {
                tool_use_id: self.selection.tool.id.clone(),
            },
        };
        Some(event)
    }

    fn capture_ready(&mut self) {
        let CallPhase::Running(execution) = &mut self.phase else {
            return;
        };
        let outcome = match execution.receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(oneshot::error::TryRecvError::Closed) => dropped_outcome(&self.selection.tool.id),
            Err(oneshot::error::TryRecvError::Empty) => return,
        };
        let CallPhase::Running(mut execution) =
            std::mem::replace(&mut self.phase, CallPhase::Retired)
        else {
            unreachable!()
        };
        execution.cancel_on_drop = false;
        self.phase = CallPhase::Ready {
            outcome,
            execution: Some(execution),
        };
    }
}

pub(in super::super) enum ForegroundObservation {
    Serial(PreparationObservation),
    Unissued(Arc<DispatchSelection>),
    Prepared(PreparationObservation<PreparedOrdinaryTool>),
    Outcome {
        selection: Arc<DispatchSelection>,
        outcome: ToolExecOutcome,
    },
}

impl ForegroundDispatch {
    pub(in super::super) fn open_batch_origin(&self) -> Option<Arc<AssistantMessage>> {
        match self {
            Self::OrdinaryBatch(batch) if batch.lifecycle.is_open() => {
                Some(Arc::clone(&batch.origin))
            }
            Self::OrdinaryBatch(_) | Self::Serial(_) => None,
        }
    }

    pub(in super::super) fn owns_batch_origin(&self, origin: &Arc<AssistantMessage>) -> bool {
        matches!(self, Self::OrdinaryBatch(batch) if Arc::ptr_eq(&batch.origin, origin))
    }

    pub(in super::super) fn retire_expired_wait_resources(&mut self) {
        let Self::OrdinaryBatch(batch) = self else {
            return;
        };
        if !batch.lifecycle.is_open() {
            return;
        }
        let now = tokio::time::Instant::now();
        for call in &mut batch.calls {
            let expired = matches!(&call.phase, CallPhase::Running(execution)
                if execution.cutoff.is_some_and(|cutoff| wait_execution_cutoff(cutoff) <= now));
            if !expired {
                continue;
            }
            call.capture_ready();
            if !matches!(call.phase, CallPhase::Running(_)) {
                continue;
            }
            let CallPhase::Running(mut execution) =
                std::mem::replace(&mut call.phase, CallPhase::Retired)
            else {
                unreachable!()
            };
            execution.cancel_on_drop = false;
            execution.receiver.close();
            execution.publication = None;
            execution.task.abort();
            call.phase = CallPhase::Ready {
                outcome: ToolExecOutcome::Failed {
                    tool_use_id: call.selection.tool.id.clone(),
                    error: "bash wait exceeded its bounded settlement deadline".into(),
                },
                execution: Some(execution),
            };
        }
    }

    pub(in super::super) fn force_retire_batch_resources(&mut self) {
        let Self::OrdinaryBatch(batch) = self else {
            return;
        };
        for call in &mut batch.calls {
            call.capture_ready();
        }
        for call in &mut batch.calls {
            if !matches!(call.phase, CallPhase::Running(_)) {
                continue;
            }
            let CallPhase::Running(mut execution) =
                std::mem::replace(&mut call.phase, CallPhase::Retired)
            else {
                unreachable!()
            };
            execution.cancel_on_drop = false;
            execution.receiver.close();
            execution.publication = None;
            execution.task.abort();
            call.phase = CallPhase::ForcedRetired {
                _execution: execution,
            };
        }
    }

    pub(in super::super) fn has_open_batch_controls(&self) -> bool {
        matches!(self, Self::OrdinaryBatch(batch) if batch.lifecycle.is_open())
    }

    pub(in super::super) fn abort(&mut self) {
        match self {
            Self::Serial(serial) => {
                serial.preparation = None;
                serial.execution.cancel_and_abort();
                serial.publication = None;
                serial.round = None;
                serial.running = None;
            }
            Self::OrdinaryBatch(batch) => {
                batch.lifecycle.begin_closing();
                batch.calls.clear();
            }
        }
    }

    pub(in super::super) fn batch_deadline(&self) -> Option<tokio::time::Instant> {
        let Self::OrdinaryBatch(batch) = self else {
            return None;
        };
        if !batch.lifecycle.is_open() {
            return None;
        }
        batch
            .calls
            .iter()
            .filter_map(|call| match &call.phase {
                CallPhase::Running(execution) => execution.cutoff.map(wait_execution_cutoff),
                _ => None,
            })
            .min()
    }

    pub(in super::super) fn batch_settlement_remaining(&self) -> Option<std::time::Duration> {
        let Self::OrdinaryBatch(batch) = self else {
            return None;
        };
        batch
            .calls
            .iter()
            .filter_map(|call| match &call.phase {
                CallPhase::Settling(Some(execution)) => execution
                    .cutoff
                    .map(|cutoff| cutoff.saturating_duration_since(tokio::time::Instant::now())),
                _ => None,
            })
            .min()
    }

    pub(in super::super) async fn observe(&mut self) -> ForegroundObservation {
        match self {
            Self::Serial(serial) => {
                if let Some(round) = &mut serial.round {
                    if round.cancelling && !round.unissued.is_empty() {
                        return ForegroundObservation::Unissued(round.unissued.remove(0));
                    }
                }
                match &mut serial.preparation {
                    Some(preparation) => ForegroundObservation::Serial(preparation.observe().await),
                    None => std::future::pending().await,
                }
            }
            Self::OrdinaryBatch(batch) => batch.observe().await,
        }
    }
}

impl OrdinaryBatch {
    fn cancel(&mut self) {
        self.lifecycle.begin_closing();
        for call in &mut self.calls {
            call.capture_ready();
        }
        for call in &mut self.calls {
            match &mut call.phase {
                CallPhase::Running(execution) => execution.cancel.cancel(),
                CallPhase::Preparing(preparation) => {
                    preparation.phase = PreparationPhase::Interrupted;
                }
                CallPhase::Unissued | CallPhase::Cleared | CallPhase::NoRead => {
                    call.phase = CallPhase::Ready {
                        outcome: ToolExecOutcome::Completed(ToolResult::cancelled(
                            call.selection.tool.id.clone(),
                            "Skipped due to cancellation",
                        )),
                        execution: None,
                    };
                }
                CallPhase::Ready { .. }
                | CallPhase::Settling(_)
                | CallPhase::ForcedRetired { .. }
                | CallPhase::Retired => {}
            }
        }
    }

    async fn observe(&mut self) -> ForegroundObservation {
        if matches!(self.lifecycle, BatchLifecycle::ClosingBeforeCommit) {
            return std::future::pending().await;
        }
        std::future::poll_fn(|cx| {
            for call in &mut self.calls {
                let observation = match &mut call.phase {
                    CallPhase::Preparing(preparation) => {
                        let mut observation = Box::pin(preparation.observe());
                        match observation.as_mut().poll(cx) {
                            Poll::Ready(observation) => {
                                Some(ForegroundObservation::Prepared(observation))
                            }
                            Poll::Pending => None,
                        }
                    }
                    CallPhase::Running(execution) => {
                        match Pin::new(&mut execution.receiver).poll(cx) {
                            Poll::Ready(outcome) => {
                                let outcome = outcome
                                    .unwrap_or_else(|_| dropped_outcome(&call.selection.tool.id));
                                Some(ForegroundObservation::Outcome {
                                    selection: Arc::clone(&call.selection),
                                    outcome,
                                })
                            }
                            Poll::Pending => None,
                        }
                    }
                    CallPhase::Ready { .. } => {
                        let old = std::mem::replace(&mut call.phase, CallPhase::Retired);
                        let CallPhase::Ready { outcome, execution } = old else {
                            unreachable!()
                        };
                        call.phase = CallPhase::Settling(execution);
                        Some(ForegroundObservation::Outcome {
                            selection: Arc::clone(&call.selection),
                            outcome,
                        })
                    }
                    CallPhase::Unissued
                    | CallPhase::Cleared
                    | CallPhase::Settling(_)
                    | CallPhase::ForcedRetired { .. }
                    | CallPhase::Retired
                    | CallPhase::NoRead => None,
                };
                if let Some(observation) = observation {
                    return Poll::Ready(observation);
                }
            }
            Poll::Pending
        })
        .await
    }

    pub(in super::super) fn retain_execution(
        &mut self,
        selection: &Arc<DispatchSelection>,
        cancel: CancellationToken,
        task: JoinHandle<()>,
        receiver: oneshot::Receiver<ToolExecOutcome>,
        publication: Arc<()>,
    ) {
        let call = self
            .calls
            .iter_mut()
            .find(|call| Arc::ptr_eq(&call.selection, selection))
            .expect("only the gate-cleared owned call can launch");
        assert!(matches!(call.phase, CallPhase::Cleared));
        call.phase = CallPhase::Running(RunningCall {
            cancel,
            task,
            receiver,
            publication: Some(publication),
            cutoff: invocation_wait_bound(&selection.tool)
                .map(|bound| tokio::time::Instant::now() + bound),
            cancel_on_drop: true,
        });
    }
}

fn wait_execution_cutoff(settlement_cutoff: tokio::time::Instant) -> tokio::time::Instant {
    let settlement_only_grace = super::super::BASH_WAIT_SETTLEMENT_GRACE
        .checked_sub(super::super::BASH_WAIT_EXECUTION_GRACE)
        .expect("execution grace precedes settlement grace");
    settlement_cutoff
        .checked_sub(settlement_only_grace)
        .expect("launch plus settlement bound includes execution grace")
}

fn invocation_wait_bound(tool: &ToolCall) -> Option<std::time::Duration> {
    let super::super::ToolInput::Bash(phoenix_core::domain::bash_types::BashInvocation::Wait {
        wait_seconds,
        ..
    }) = &tool.input
    else {
        return None;
    };
    let seconds = wait_seconds
        .unwrap_or(
            i64::try_from(super::super::BASH_WAIT_DEFAULT_SECONDS).expect("supported default"),
        )
        .clamp(0, 900);
    Some(
        std::time::Duration::from_secs(u64::try_from(seconds).expect("clamped nonnegative wait"))
            + super::super::BASH_WAIT_SETTLEMENT_GRACE,
    )
}

fn dropped_outcome(id: &str) -> ToolExecOutcome {
    ToolExecOutcome::Failed {
        tool_use_id: id.into(),
        error: "Tool task aborted or panicked without an outcome".into(),
    }
}

impl<S: Storage + Clone + 'static, L: LlmClient + 'static, T: ToolExecutor + 'static>
    ConversationRuntime<S, L, T>
{
    pub(in super::super) fn serial_dispatch(&self) -> &SerialDispatch {
        let ForegroundDispatch::Serial(serial) = &self.foreground_dispatch else {
            panic!("serial resource access in an ordinary batch")
        };
        serial
    }

    pub(in super::super) fn serial_dispatch_mut(&mut self) -> &mut SerialDispatch {
        let ForegroundDispatch::Serial(serial) = &mut self.foreground_dispatch else {
            panic!("serial resource access in an ordinary batch")
        };
        serial
    }

    pub(super) fn begin_ordinary_batch(&mut self, selected: &ToolCall) -> Result<bool, String> {
        let ConvState::ToolExecuting {
            current_tool,
            remaining_tools,
            assistant_message,
            ..
        } = &self.state
        else {
            return Ok(false);
        };
        if current_tool != selected {
            return Err("tool effect does not own the current representative".into());
        }
        if let ForegroundDispatch::OrdinaryBatch(batch) = &self.foreground_dispatch {
            if batch.origin.as_ref() != assistant_message {
                return Err("ordinary batch origin was replaced before settlement".into());
            }
            self.fill_ordinary_batch();
            return Ok(true);
        }
        let calls: Vec<_> = std::iter::once(current_tool)
            .chain(remaining_tools)
            .cloned()
            .collect();
        let source_calls: Vec<_> = assistant_message
            .content
            .iter()
            .filter_map(|block| match block {
                super::super::ContentBlock::ToolUse { id, name, .. } => Some((id, name)),
                _ => None,
            })
            .collect();
        if self
            .serial_dispatch()
            .round
            .as_ref()
            .is_some_and(|round| round.origin.as_ref() == assistant_message)
        {
            return Ok(false);
        }
        if calls.len() < 2
            || !source_calls.iter().all(|(_, name)| {
                !matches!(
                    name.as_str(),
                    "spawn_agents"
                        | "create_goal"
                        | "enable_auto_continuation"
                        | "submit_result"
                        | "submit_error"
                        | "ask_user_question"
                        | "propose_task"
                ) && self.tool_executor.foreground_execution(name)
                    == crate::tools::ForegroundExecution::Concurrent
            })
        {
            return Ok(false);
        }
        let serial = self.serial_dispatch();
        if serial.preparation.is_some() || serial.running.is_some() {
            return Err("ordinary batch overlaps a serial preparation".into());
        }
        let origin = Arc::new(assistant_message.clone());
        let calls = calls
            .into_iter()
            .map(|tool| {
                let ordinal = source_calls
                    .iter()
                    .position(|(id, _)| *id == &tool.id)
                    .expect("call belongs to accepted assistant source");
                BatchCall {
                    selection: Arc::new(DispatchSelection {
                        runtime: self.tool_preparation_runtime,
                        assistant_message: Arc::clone(&origin),
                        tool,
                        context: self.context.clone(),
                        direct_turn: self.active_direct_turn.as_deref().cloned(),
                    }),
                    boundary: if ordinal == 0 {
                        self.required_settlement_authority
                            .as_ref()
                            .map_or(PreparationFailureBoundary::Caller, |scope| {
                                PreparationFailureBoundary::Required(scope.boundary)
                            })
                    } else {
                        PreparationFailureBoundary::Required(RequiredSettlementBoundary::Tool)
                    },
                    phase: CallPhase::Unissued,
                }
            })
            .collect();
        let serial_generation = self.serial_dispatch().execution.generation();
        self.deadline = None;
        self.foreground_dispatch = ForegroundDispatch::OrdinaryBatch(OrdinaryBatch {
            origin,
            serial_generation,
            calls,
            lifecycle: BatchLifecycle::Open,
        });
        self.fill_ordinary_batch();
        Ok(true)
    }

    pub(in super::super) fn fill_ordinary_batch(&mut self) {
        let ForegroundDispatch::OrdinaryBatch(batch) = &self.foreground_dispatch else {
            return;
        };
        if !batch.lifecycle.is_open() {
            return;
        }
        let active = batch
            .calls
            .iter()
            .filter(|call| {
                matches!(
                    call.phase,
                    CallPhase::Preparing(_)
                        | CallPhase::Running(_)
                        | CallPhase::Ready { .. }
                        | CallPhase::Settling(_)
                        | CallPhase::ForcedRetired { .. }
                )
            })
            .count();
        let selections: Vec<_> = batch
            .calls
            .iter()
            .filter(|call| {
                matches!(call.phase, CallPhase::Unissued)
                    && call
                        .selection
                        .owns(self.tool_preparation_runtime, &self.state)
            })
            .take(8_usize.saturating_sub(active))
            .map(|call| (Arc::clone(&call.selection), call.boundary))
            .collect();
        for (selection, boundary) in selections {
            let preparation = self.make_ordinary_tool_preparation(Arc::clone(&selection), boundary);
            let ForegroundDispatch::OrdinaryBatch(batch) = &mut self.foreground_dispatch else {
                unreachable!()
            };
            let call = batch
                .calls
                .iter_mut()
                .find(|call| Arc::ptr_eq(&call.selection, &selection))
                .expect("unissued capability remains owned");
            call.phase = CallPhase::Preparing(preparation);
        }
    }

    pub(super) fn own_serial_selection(
        &mut self,
        tool: &ToolCall,
    ) -> Result<Arc<DispatchSelection>, String> {
        let ConvState::ToolExecuting {
            current_tool,
            remaining_tools,
            assistant_message,
            ..
        } = &self.state
        else {
            return Err("serial selection outside executing state".into());
        };
        if self
            .serial_dispatch()
            .round
            .as_ref()
            .is_none_or(|round| round.origin.as_ref() != assistant_message)
        {
            let origin = Arc::new(assistant_message.clone());
            let unissued = std::iter::once(current_tool)
                .chain(remaining_tools)
                .map(|tool| {
                    Arc::new(DispatchSelection {
                        runtime: self.tool_preparation_runtime,
                        assistant_message: Arc::clone(&origin),
                        tool: tool.clone(),
                        context: self.context.clone(),
                        direct_turn: self.active_direct_turn.as_deref().cloned(),
                    })
                })
                .collect();
            self.serial_dispatch_mut().round = Some(SerialRound {
                origin,
                unissued,
                cancelling: false,
            });
        }
        let round = self
            .serial_dispatch_mut()
            .round
            .as_mut()
            .expect("round admitted");
        let index = round
            .unissued
            .iter()
            .position(|selection| &selection.tool == tool)
            .ok_or("serial invocation was already issued")?;
        Ok(round.unissued.remove(index))
    }

    pub(in super::super) fn cancel_ordinary_batch(&mut self) -> bool {
        if let ForegroundDispatch::OrdinaryBatch(batch) = &mut self.foreground_dispatch {
            batch.cancel();
            true
        } else {
            false
        }
    }

    pub(in super::super) fn interrupt_batch_before_cancellation_checkpoint(
        &mut self,
        event: &Event,
        next: &ConvState,
    ) {
        if matches!(event, Event::UserCancel { .. })
            && matches!(next, ConvState::CancellingTool { .. })
        {
            self.cancel_ordinary_batch();
        }
    }

    pub(in super::super) fn accept_foreground_observation(
        &mut self,
        observation: ForegroundObservation,
    ) -> Result<Option<Event>, String> {
        if self
            .fatal_local_authority_fence
            .fatal_authority_loss_is_latched()
        {
            return Err("runtime processing closed after fatal local authority loss".into());
        }
        let observation = match observation {
            ForegroundObservation::Serial(observation) => {
                return self.accept_tool_preparation(observation)
            }
            ForegroundObservation::Unissued(selection) => {
                return Ok((selection.owns(self.tool_preparation_runtime, &self.state)
                    && matches!(self.state, ConvState::CancellingTool { .. }))
                .then(|| Event::ToolComplete {
                    tool_use_id: selection.tool.id.clone(),
                    result: ToolResult::cancelled(
                        selection.tool.id.clone(),
                        "Skipped due to cancellation",
                    ),
                }));
            }
            observation => observation,
        };
        let selection = match &observation {
            ForegroundObservation::Prepared(observation) => Arc::clone(&observation.selection),
            ForegroundObservation::Outcome { selection, .. } => Arc::clone(selection),
            ForegroundObservation::Serial(_) | ForegroundObservation::Unissued(_) => unreachable!(),
        };
        let ForegroundDispatch::OrdinaryBatch(batch) = &mut self.foreground_dispatch else {
            return Ok(None);
        };
        let Some(call) = batch
            .calls
            .iter_mut()
            .find(|call| Arc::ptr_eq(&call.selection, &selection))
        else {
            return Ok(None);
        };
        if !selection.owns(self.tool_preparation_runtime, &self.state) {
            call.phase = CallPhase::Retired;
            return Ok(None);
        }
        match observation {
            ForegroundObservation::Prepared(observation) => {
                if !matches!(call.phase, CallPhase::Preparing(_)) {
                    return Ok(None);
                }
                match observation.result {
                    PreparationDisposition::Interrupted => {
                        if !matches!(self.state, ConvState::CancellingTool { .. }) {
                            call.phase = CallPhase::NoRead;
                            return Ok(None);
                        }
                        call.phase = CallPhase::Settling(None);
                        Ok(Some(Event::ToolAborted {
                            tool_use_id: selection.tool.id.clone(),
                        }))
                    }
                    PreparationDisposition::Ready(Err(error)) => {
                        if let PreparationFailureBoundary::Required(boundary) = call.boundary {
                            self.fatal_local_authority_fence
                                .close(boundary.settlement());
                        }
                        call.phase = CallPhase::NoRead;
                        Err(error)
                    }
                    PreparationDisposition::Ready(Ok(_)) if !batch.lifecycle.is_open() => {
                        call.phase = CallPhase::Settling(None);
                        Ok(Some(Event::ToolAborted {
                            tool_use_id: selection.tool.id.clone(),
                        }))
                    }
                    PreparationDisposition::Ready(Ok(PreparedOrdinaryTool::Result(result))) => {
                        call.phase = CallPhase::Settling(None);
                        Ok(Some(Event::ToolComplete {
                            tool_use_id: selection.tool.id.clone(),
                            result,
                        }))
                    }
                    PreparationDisposition::Ready(Ok(PreparedOrdinaryTool::Cleared(cleared))) => {
                        call.phase = CallPhase::Cleared;
                        let mut admitted = self.admit_authoritative_effect()?;
                        self.launch_cleared_tool(
                            CurrentSelection(selection),
                            *cleared,
                            &mut admitted,
                        );
                        Ok(None)
                    }
                }
            }
            ForegroundObservation::Outcome { outcome, .. } => Ok(call.accept_outcome(outcome)),
            ForegroundObservation::Serial(_) | ForegroundObservation::Unissued(_) => unreachable!(),
        }
    }

    pub(in super::super) async fn expire_batch_waits(&mut self) -> Result<(), String> {
        self.foreground_dispatch.retire_expired_wait_resources();
        while let Some(observation) = self.foreground_dispatch.observe().now_or_never() {
            if let Some(event) = self.accept_foreground_observation(observation)? {
                self.process_event(event).await?;
            }
            if !matches!(
                self.foreground_dispatch,
                ForegroundDispatch::OrdinaryBatch(_)
            ) {
                break;
            }
        }
        Ok(())
    }

    pub(in super::super) async fn drain_ready_before_forced_retirement(
        &mut self,
    ) -> Result<bool, String> {
        if !matches!(
            self.foreground_dispatch,
            ForegroundDispatch::OrdinaryBatch(_)
        ) {
            return Ok(false);
        }
        self.foreground_dispatch.force_retire_batch_resources();
        loop {
            let Some(observation) = self.foreground_dispatch.observe().now_or_never() else {
                break;
            };
            if let Some(event) = self.accept_foreground_observation(observation)? {
                self.process_event(event).await?;
            }
            if !matches!(
                self.foreground_dispatch,
                ForegroundDispatch::OrdinaryBatch(_)
            ) {
                return Ok(true);
            }
        }
        Ok(true)
    }

    pub(in super::super) fn commit_foreground_membership(&mut self) {
        if let ForegroundDispatch::Serial(serial) = &mut self.foreground_dispatch {
            if serial.running.as_ref().is_some_and(|selection| {
                !selection.owns(self.tool_preparation_runtime, &self.state)
            }) {
                serial.running = None;
                serial.publication = None;
                serial.execution.release_task();
            }
            if let Some(round) = &mut serial.round {
                round
                    .unissued
                    .retain(|selection| selection.owns(self.tool_preparation_runtime, &self.state));
                if matches!(&self.state, ConvState::CancellingTool { assistant_message, .. } if assistant_message == round.origin.as_ref())
                {
                    round.cancelling = true;
                } else if !matches!(&self.state, ConvState::ToolExecuting { assistant_message, .. } if assistant_message == round.origin.as_ref())
                {
                    serial.round = None;
                }
            }
        }
        if let ForegroundDispatch::OrdinaryBatch(batch) = &mut self.foreground_dispatch {
            if matches!(self.state, ConvState::CancellingTool { .. }) {
                batch.lifecycle = BatchLifecycle::ClosingAfterCommit;
            }
            for call in &mut batch.calls {
                if !call
                    .selection
                    .owns(self.tool_preparation_runtime, &self.state)
                {
                    call.phase = CallPhase::Retired;
                }
            }
            if !matches!(&self.state, ConvState::ToolExecuting { assistant_message, .. } | ConvState::CancellingTool { assistant_message, .. } if assistant_message == batch.origin.as_ref())
            {
                let execution = ToolExecutionSlot::after_generation(batch.serial_generation);
                self.cancellation_custody
                    .release_closing_origin(&batch.origin);
                self.foreground_dispatch = ForegroundDispatch::Serial(SerialDispatch {
                    execution,
                    ..SerialDispatch::default()
                });
            }
        }
    }
}

#[cfg(test)]
impl<S: Storage + Clone + 'static, L: LlmClient + 'static, T: ToolExecutor + 'static>
    ConversationRuntime<S, L, T>
{
    pub(in super::super) async fn join_batch_call_for_test(&mut self, id: &str) {
        let ForegroundDispatch::OrdinaryBatch(batch) = &mut self.foreground_dispatch else {
            panic!("batch fixture")
        };
        let call = batch
            .calls
            .iter_mut()
            .find(|call| call.selection.tool.id == id)
            .unwrap();
        let CallPhase::Running(execution) = &mut call.phase else {
            panic!("running fixture")
        };
        (&mut execution.task)
            .await
            .expect("tool task completed with its typed receipt");
    }
}

#[cfg(test)]
impl<S: Storage + Clone + 'static, L: LlmClient + 'static, T: ToolExecutor + 'static>
    ConversationRuntime<S, L, T>
{
    pub(in super::super) async fn abort_batch_call_for_test(&mut self, id: &str) {
        let ForegroundDispatch::OrdinaryBatch(batch) = &mut self.foreground_dispatch else {
            panic!("batch fixture")
        };
        let call = batch
            .calls
            .iter_mut()
            .find(|call| call.selection.tool.id == id)
            .unwrap();
        let CallPhase::Running(execution) = &mut call.phase else {
            panic!("running fixture")
        };
        execution.task.abort();
        assert!((&mut execution.task).await.unwrap_err().is_cancelled());
    }
}
