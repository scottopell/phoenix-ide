//! Admission fence for request-handler futures. Streaming response bodies and
//! upgraded sessions remain owned by the HTTP server's graceful connection drain.

use std::sync::{Arc, Mutex};
use tokio::sync::watch;

#[derive(Clone, Debug)]
pub(crate) struct RequestDrain {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    state: Mutex<State>,
    active: watch::Sender<usize>,
}

#[derive(Debug)]
struct State {
    accepting: bool,
    active: usize,
}

#[derive(Debug)]
pub(crate) struct RequestAdmission {
    inner: Arc<Inner>,
}

#[derive(Debug)]
pub(crate) struct RequestDrainStarted {
    active: watch::Receiver<usize>,
}

impl RequestDrain {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    accepting: true,
                    active: 0,
                }),
                active: watch::channel(0).0,
            }),
        }
    }

    pub(crate) fn admit(&self) -> Option<RequestAdmission> {
        let mut state = self.inner.state.lock().unwrap();
        if !state.accepting {
            return None;
        }
        state.active = state
            .active
            .checked_add(1)
            .expect("request admission overflow");
        self.inner.active.send_replace(state.active);
        Some(RequestAdmission {
            inner: self.inner.clone(),
        })
    }

    pub(crate) fn begin(&self) -> RequestDrainStarted {
        let mut state = self.inner.state.lock().unwrap();
        state.accepting = false;
        RequestDrainStarted {
            active: self.inner.active.subscribe(),
        }
    }
}

impl RequestDrainStarted {
    pub(crate) async fn wait(mut self) {
        while *self.active.borrow_and_update() != 0 {
            self.active
                .changed()
                .await
                .expect("request drain sender outlives every waiter");
        }
    }
}

impl Drop for RequestAdmission {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap();
        state.active = state
            .active
            .checked_sub(1)
            .expect("request admission dropped exactly once");
        self.inner.active.send_replace(state.active);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn begin_rejects_new_admission_and_waits_for_owned_work() {
        let drain = RequestDrain::new();
        let admission = drain.admit().unwrap();
        let started = drain.begin();
        assert!(drain.admit().is_none());

        let (completed_tx, mut completed_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            started.wait().await;
            completed_tx.send(()).unwrap();
        });
        assert!(matches!(
            completed_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        drop(admission);
        completed_rx.await.unwrap();
    }

    #[tokio::test]
    async fn repeated_begin_is_closed_and_completes_after_the_same_admissions() {
        let drain = RequestDrain::new();
        let admission = drain.admit().unwrap();
        let first = drain.begin();
        let second = drain.begin();
        assert!(drain.admit().is_none());

        drop(admission);
        tokio::join!(first.wait(), second.wait());
    }
}
