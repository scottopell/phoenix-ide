//! Broker-owned scenario task custody, reachable without the session write lock.

use std::sync::Mutex;
use tokio::sync::watch;
use tokio::task::JoinHandle;

struct Running {
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
}

#[derive(Default)]
struct State {
    closing: bool,
    running: Option<Running>,
}

#[derive(Default)]
pub struct ScenarioOwner {
    state: Mutex<State>,
}

impl ScenarioOwner {
    /// # Errors
    /// Returns an error if this owner is closing or a previous scenario task,
    /// including its cleanup, has not finished.
    pub fn start<F, Fut>(&self, operation: F) -> Result<(), String>
    where
        F: FnOnce(watch::Receiver<bool>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closing {
            return Err("Browser scenario owner is closing".into());
        }
        if state
            .running
            .as_ref()
            .is_some_and(|run| !run.task.is_finished())
        {
            return Err("Another browser scenario or its cleanup is still running".into());
        }
        let (cancel, cancelled) = watch::channel(false);
        let (registered, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            if ready.await.is_ok() {
                operation(cancelled).await;
            }
        });
        state.running = Some(Running { cancel, task });
        let _ = registered.send(());
        Ok(())
    }

    pub(crate) fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closing = true;
        if let Some(run) = &state.running {
            let _ = run.cancel.send(true);
        }
    }
}

impl Drop for ScenarioOwner {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(run) = state.running.take() {
            run.task.abort();
        }
    }
}

pub async fn cancelled(receiver: &mut watch::Receiver<bool>) {
    loop {
        if *receiver.borrow_and_update() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cleanup_keeps_exclusive_session_custody_after_caller_disappears() {
        let owner = ScenarioOwner::default();
        let session = std::sync::Arc::new(tokio::sync::RwLock::new(()));
        let worker_session = session.clone();
        let (mut reply, caller) = tokio::sync::oneshot::channel::<()>();
        let (started, active) = tokio::sync::oneshot::channel();
        let (cleaning, cleanup_started) = tokio::sync::oneshot::channel();
        let (ack, acknowledged) = tokio::sync::oneshot::channel();
        let (done, finished) = tokio::sync::oneshot::channel();
        owner
            .start(|_| async move {
                let guard = worker_session.write().await;
                started.send(()).unwrap();
                reply.closed().await;
                cleaning.send(()).unwrap();
                acknowledged.await.unwrap();
                drop(guard);
                done.send(()).unwrap();
            })
            .unwrap();
        active.await.unwrap();
        drop(caller);
        cleanup_started.await.unwrap();
        assert!(session.try_write().is_err());
        assert!(owner.start(|_| async {}).is_err());
        ack.send(()).unwrap();
        finished.await.unwrap();
        assert!(session.try_write().is_ok());
    }

    #[tokio::test]
    async fn teardown_signals_before_waiting_for_exclusive_session_custody() {
        let owner = ScenarioOwner::default();
        let session = std::sync::Arc::new(tokio::sync::RwLock::new(()));
        let worker_session = session.clone();
        let (started, active) = tokio::sync::oneshot::channel();
        let (cleaned, cleanup_done) = tokio::sync::oneshot::channel();
        owner
            .start(|mut cancellation| async move {
                let guard = worker_session.write().await;
                started.send(()).unwrap();
                cancelled(&mut cancellation).await;
                cleaned.send(()).unwrap();
                drop(guard);
            })
            .unwrap();
        active.await.unwrap();
        assert!(session.try_write().is_err());
        owner.close();
        let _guard = tokio::time::timeout(std::time::Duration::from_secs(5), session.write())
            .await
            .unwrap();
        // Acquiring the lock is downstream of cleanup, not a timing guess.
        cleanup_done.await.unwrap();
        assert!(owner.start(|_| async {}).is_err());
    }

    #[tokio::test]
    async fn close_reaches_worker_without_session_lock_and_rejects_new_work() {
        let owner = ScenarioOwner::default();
        let (done, observed) = tokio::sync::oneshot::channel();
        owner
            .start(|mut cancellation| async move {
                cancelled(&mut cancellation).await;
                let _ = done.send(());
            })
            .unwrap();
        assert!(owner.start(|_| async {}).is_err());
        owner.close();
        observed.await.unwrap();
        assert!(owner.start(|_| async {}).is_err());
    }
}
