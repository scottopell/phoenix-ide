use super::{
    AcknowledgedEventOutcome, ConvState, ConversationRuntime, CreationSettlementDisposition,
    Effect, LlmClient, LocalTerminalAuthority, RuntimeExitDisposition, StepResult, Storage,
    ToolExecutor, CANCELLATION_DEADLINE,
};
use crate::runtime::cancellation_inbox::{CancellationInput, CancellationReceipt};
use std::future::Future;
use tokio::sync::mpsc;

pub(super) struct ObservedCancellation {
    input: CancellationInput,
    cutoff: Option<CancellationCutoff>,
}

pub(super) struct CancellationCutoff {
    at: tokio::time::Instant,
    phase: CutoffPhase,
}

enum CutoffPhase {
    Pending,
    Serviced,
}

#[derive(Default)]
pub(super) enum CancellationCustody {
    #[default]
    Idle,
    Pending(ObservedCancellation),
    Applying(Option<CancellationCutoff>),
}

impl CancellationCustody {
    fn pending_cutoff(&self) -> Option<tokio::time::Instant> {
        let cutoff = match self {
            Self::Pending(observed) => observed.cutoff.as_ref(),
            Self::Applying(cutoff) => cutoff.as_ref(),
            Self::Idle => None,
        }?;
        matches!(cutoff.phase, CutoffPhase::Pending).then_some(cutoff.at)
    }

    fn mark_cutoff_serviced(&mut self) {
        let cutoff = match self {
            Self::Pending(observed) => observed.cutoff.as_mut(),
            Self::Applying(cutoff) => cutoff.as_mut(),
            Self::Idle => None,
        };
        if let Some(cutoff) = cutoff {
            cutoff.phase = CutoffPhase::Serviced;
        }
    }

    pub(super) fn take_pending(&mut self) -> Option<ObservedCancellation> {
        if !matches!(self, Self::Pending(_)) {
            return None;
        }
        let Self::Pending(observed) = std::mem::take(self) else {
            unreachable!()
        };
        Some(observed)
    }
}

impl<S: Storage + Clone + 'static, L: LlmClient + 'static, T: ToolExecutor + 'static>
    ConversationRuntime<S, L, T>
{
    pub(crate) fn with_cancellation_input_receiver(
        mut self,
        receiver: mpsc::Receiver<CancellationInput>,
    ) -> Self {
        self.cancellation_input_rx = Some(receiver);
        self
    }

    pub(super) fn observe_cancellation_input(
        &mut self,
        input: CancellationInput,
    ) -> ObservedCancellation {
        let cutoff = if self.foreground_dispatch.has_open_batch_controls() {
            let cutoff = tokio::time::Instant::now() + CANCELLATION_DEADLINE;
            self.cancel_ordinary_batch();
            Some(CancellationCutoff {
                at: cutoff,
                phase: CutoffPhase::Pending,
            })
        } else {
            None
        };
        ObservedCancellation { input, cutoff }
    }

    pub(super) async fn await_ordinary_batch_commit(
        &mut self,
        write: impl Future<Output = Result<(), String>> + Send,
    ) -> Result<(), String> {
        tokio::pin!(write);
        loop {
            let observe = self.foreground_dispatch.has_open_batch_controls()
                && matches!(self.cancellation_custody, CancellationCustody::Idle)
                && self.cancellation_input_rx.is_some();
            let cancellation_cutoff = self.cancellation_custody.pending_cutoff();
            let resource_deadline =
                cancellation_cutoff.or_else(|| self.foreground_dispatch.batch_deadline());
            tokio::select! {
                biased;
                input = async { self.cancellation_input_rx.as_mut().expect("open cancellation inbox").recv().await }, if observe => {
                    match input {
                        Some(input) => self.cancellation_custody = CancellationCustody::Pending(self.observe_cancellation_input(input)),
                        None => self.cancellation_input_rx = None,
                    }
                }
                () = async {
                    tokio::time::sleep_until(resource_deadline.expect("owned resource deadline")).await;
                }, if resource_deadline.is_some() => {
                    if cancellation_cutoff.is_some() {
                        self.foreground_dispatch.force_retire_batch_resources();
                        self.cancellation_custody.mark_cutoff_serviced();
                    } else {
                        self.foreground_dispatch.retire_expired_wait_resources();
                    }
                }
                result = &mut write => return result,
            }
        }
    }

    fn retain_original_cancellation_cutoff(&mut self, cutoff: Option<tokio::time::Instant>) {
        if matches!(self.state, ConvState::CancellingTool { .. }) {
            if let Some(cutoff) = cutoff {
                self.deadline = Some(self.deadline.map_or(cutoff, |current| current.min(cutoff)));
            }
        }
    }

    pub(super) async fn settle_cancellation_input(
        &mut self,
        observed: ObservedCancellation,
    ) -> Option<RuntimeExitDisposition> {
        let ObservedCancellation { input, cutoff } = observed;
        let (event, receipt) = input.into_parts();
        self.cancellation_custody = CancellationCustody::Applying(cutoff);
        let mut result = if matches!(&receipt, CancellationReceipt::Acknowledged { .. }) {
            self.process_acknowledged_event(event).await
        } else {
            self.process_event(event)
                .await
                .map(|()| AcknowledgedEventOutcome::Settled)
        };
        let CancellationCustody::Applying(cutoff) = std::mem::take(&mut self.cancellation_custody)
        else {
            unreachable!("one semantic cancellation owns its custody")
        };
        let cutoff = cutoff.map(|cutoff| cutoff.at);
        self.retain_original_cancellation_cutoff(cutoff);
        if cutoff.is_some_and(|cutoff| cutoff <= tokio::time::Instant::now())
            && matches!(self.state, ConvState::CancellingTool { .. })
            && !self
                .fatal_local_authority_fence
                .fatal_authority_loss_is_latched()
        {
            self.handle_cancelling_tool_timeout().await;
        }
        let fatal = self
            .fatal_local_authority_fence
            .fatal_authority_loss_is_latched()
            || self.local_terminal_authority == LocalTerminalAuthority::Fatal;
        if fatal && result.is_ok() {
            result = Err("runtime processing closed after fatal local authority loss".into());
        }
        let terminal = matches!(self.state.step_result(), StepResult::Terminal(_));
        if terminal && !fatal {
            self.emit_terminal_lifecycle_event().await;
        }
        match receipt {
            CancellationReceipt::Addressed => {
                if let Err(error) = result {
                    tracing::error!(%error, "failed to settle addressed cancellation");
                }
            }
            CancellationReceipt::Acknowledged {
                acknowledgement,
                retirement,
            } => {
                let _ = acknowledgement.send(result);
                if self.creation_settlement_disposition
                    == CreationSettlementDisposition::StaleAuthority
                {
                    let _ = retirement.await;
                    return Some(RuntimeExitDisposition::Interrupted);
                }
            }
        }
        if fatal {
            return Some(self.fatal_local_authority_exit());
        }
        terminal.then_some(RuntimeExitDisposition::Terminal)
    }

    pub(super) fn cancellation_defers_dispatch(&self, effect: &Effect) -> bool {
        self.has_pending_cancellation()
            && matches!(effect, Effect::ExecuteTool { .. } | Effect::RequestLlm)
    }

    pub(super) fn has_pending_cancellation(&self) -> bool {
        matches!(self.cancellation_custody, CancellationCustody::Pending(_))
    }
}
