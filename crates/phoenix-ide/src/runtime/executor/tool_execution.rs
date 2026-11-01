use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Process-local resources for one serial tool invocation (REQ-BED-004, REQ-BED-005a).
#[derive(Default)]
pub(super) struct ToolExecutionSlot {
    generation: u64,
    cancel_token: Option<CancellationToken>,
    task: Option<JoinHandle<()>>,
}

impl ToolExecutionSlot {
    pub(super) fn begin_dispatch(&mut self, cancel_token: CancellationToken) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.cancel_token = Some(cancel_token);
        self.generation
    }

    pub(super) fn retain_task(&mut self, task: JoinHandle<()>) {
        self.task = Some(task);
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn is_stale(&self, generation: u64) -> bool {
        generation != self.generation
    }

    pub(super) fn request_cancel(&mut self) {
        if let Some(token) = self.cancel_token.take() {
            token.cancel();
        }
    }

    pub(super) fn abort_task_and_invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    pub(super) fn cancel_and_abort(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.request_cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    pub(super) fn release_task(&mut self) {
        self.task = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn cooperative_cancel_keeps_generation_and_task_alive() {
        let mut slot = ToolExecutionSlot::default();
        let token = CancellationToken::new();
        let generation = slot.begin_dispatch(token.clone());
        let (cancel_observed, observed) = oneshot::channel();
        let (finish, may_finish) = oneshot::channel();
        let (completed, completion) = oneshot::channel();
        slot.retain_task(tokio::spawn(async move {
            token.cancelled().await;
            cancel_observed.send(()).unwrap();
            may_finish.await.unwrap();
            completed.send(()).unwrap();
        }));

        slot.request_cancel();
        observed.await.unwrap();
        assert!(!slot.is_stale(generation));
        finish.send(()).unwrap();
        completion.await.unwrap();
    }

    #[tokio::test]
    async fn task_only_abort_invalidates_outcome_without_cancelling_token() {
        let mut slot = ToolExecutionSlot::default();
        let token = CancellationToken::new();
        let generation = slot.begin_dispatch(token.clone());
        let (started, start) = oneshot::channel();
        let (held, dropped) = oneshot::channel::<()>();
        slot.retain_task(tokio::spawn(async move {
            let _held = held;
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        }));
        start.await.unwrap();

        slot.abort_task_and_invalidate();
        assert!(dropped.await.is_err());
        assert!(slot.is_stale(generation));
        assert!(!token.is_cancelled());
        slot.request_cancel();
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn teardown_cancels_token_before_aborting_task() {
        struct CancelOnDrop {
            token: CancellationToken,
            observed: Option<oneshot::Sender<bool>>,
        }

        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                if let Some(observed) = self.observed.take() {
                    let _ = observed.send(self.token.is_cancelled());
                }
            }
        }

        let mut slot = ToolExecutionSlot::default();
        let token = CancellationToken::new();
        let generation = slot.begin_dispatch(token.clone());
        let (started, start) = oneshot::channel();
        let (observed, dropped) = oneshot::channel();
        slot.retain_task(tokio::spawn(async move {
            let _observe_on_drop = CancelOnDrop {
                token,
                observed: Some(observed),
            };
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        }));
        start.await.unwrap();

        slot.cancel_and_abort();
        assert!(dropped.await.unwrap());
        assert!(slot.is_stale(generation));
    }

    #[tokio::test]
    async fn releasing_task_detaches_without_aborting_or_cancelling() {
        let mut slot = ToolExecutionSlot::default();
        let token = CancellationToken::new();
        let generation = slot.begin_dispatch(token.clone());
        let (finish, may_finish) = oneshot::channel();
        let (completed, completion) = oneshot::channel();
        slot.retain_task(tokio::spawn(async move {
            may_finish.await.unwrap();
            completed.send(()).unwrap();
        }));

        slot.release_task();
        assert!(!slot.is_stale(generation));
        assert!(!token.is_cancelled());
        finish.send(()).unwrap();
        completion.await.unwrap();
    }

    #[tokio::test]
    async fn dropping_slot_detaches_without_aborting_or_cancelling() {
        let mut slot = ToolExecutionSlot::default();
        let token = CancellationToken::new();
        slot.begin_dispatch(token.clone());
        let (finish, may_finish) = oneshot::channel();
        let (completed, completion) = oneshot::channel();
        slot.retain_task(tokio::spawn(async move {
            may_finish.await.unwrap();
            completed.send(()).unwrap();
        }));

        drop(slot);
        assert!(!token.is_cancelled());
        finish.send(()).unwrap();
        completion.await.unwrap();
    }
}
