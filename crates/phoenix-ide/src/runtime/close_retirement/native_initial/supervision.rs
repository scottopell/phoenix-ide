//! Per-resource custody gate for native initial retirement.
//!
//! Transfer and release are deliberately separate: the synchronous transfer is
//! suitable for a SQLite callback, while the work future is not even constructed
//! until the owner explicitly confirms that the writer released its transaction.
//! Keep the external supervisor (including its task handle and result receiver)
//! for as long as this process epoch must retain custody.

use std::future::Future;
use tokio::{sync::oneshot, task::JoinHandle};

/// Result of the synchronous ownership handoff. Rejected transfers return the
/// still-owned value, so a failed send cannot silently dispose of custody.
pub(crate) enum EntryDecision<T> {
    Transferred,
    NotTransferred(T),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObserveError {
    WorkerEndedWithoutResult,
}

/// Callback-local entry/release channels. For Terminal, the transferred owner
/// shell contains only handles to the independently retained custody slot and
/// browser custody; the entered native owner remains in that slot until confirmed
/// release permits the retained worker to take it.
pub(crate) struct EntryGate<T> {
    entry: Option<oneshot::Sender<T>>,
    release: Option<oneshot::Sender<()>>,
    transferred: bool,
}

impl<T> EntryGate<T> {
    /// Synchronous handoff for the database callback; this method takes no lock,
    /// awaits nothing, and does not construct or poll the work future.
    pub(crate) fn transfer(&mut self, owner: T) -> EntryDecision<T> {
        let Some(entry) = self.entry.take() else {
            return EntryDecision::NotTransferred(owner);
        };
        match entry.send(owner) {
            Ok(()) => {
                self.transferred = true;
                EntryDecision::Transferred
            }
            Err(owner) => EntryDecision::NotTransferred(owner),
        }
    }

    /// Confirms writer release. Calling before a successful transfer is inert.
    pub(crate) fn confirm_release(&mut self) -> bool {
        if !self.transferred {
            return false;
        }
        self.release
            .take()
            .is_some_and(|release| release.send(()).is_ok())
    }

    /// Cancels an entry that was never transferred. It does not abort the task;
    /// the task observes the closed entry channel and exits without running work.
    pub(crate) fn cancel_before_entry(&mut self) {
        if !self.transferred {
            self.entry.take();
            self.release.take();
        }
    }
}

/// External-owner record for one resource. It owns observation state and the
/// worker handle, but not the callback-local entry/release channels.
pub(crate) struct ResourceSupervisor<R> {
    result: Option<oneshot::Receiver<R>>,
    observed: Option<Result<R, ObserveError>>,
    task: JoinHandle<()>,
    // Lets tests synchronize on the actor having received its owner, without
    // scheduler-yield assumptions. Production code has no extra channel.
    #[cfg(test)]
    entered: Option<oneshot::Receiver<()>>,
}

impl<R> ResourceSupervisor<R>
where
    R: Clone + Send + 'static,
{
    /// Starts the gate actor and returns its callback-local gate separately from
    /// the external record. `work` is constructed only after explicit release.
    pub(crate) fn spawn<T, F, Fut>(work: F) -> (EntryGate<T>, Self)
    where
        T: Send + 'static,
        F: FnOnce(T) -> Fut + Send + 'static,
        Fut: Future<Output = R> + Send + 'static,
    {
        let (entry_tx, entry_rx) = oneshot::channel::<T>();
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let (result_tx, result_rx) = oneshot::channel::<R>();
        #[cfg(test)]
        let (entered_tx, entered_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let Ok(owner) = entry_rx.await else {
                return;
            };
            #[cfg(test)]
            let _ = entered_tx.send(());

            // A closed release channel is not evidence that the writer released
            // its transaction. Keep the affine owner live across permanent wait.
            if release_rx.await.is_err() {
                std::future::pending::<()>().await;
                drop(owner);
                return;
            }

            let result = work(owner).await;
            let _ = result_tx.send(result);
        });

        (
            EntryGate {
                entry: Some(entry_tx),
                release: Some(release_tx),
                transferred: false,
            },
            Self {
                result: Some(result_rx),
                observed: None,
                task,
                #[cfg(test)]
                entered: Some(entered_rx),
            },
        )
    }

    /// Retain the physical outcome before returning a copy to an actor waiter.
    /// Cancelling a pending receive leaves it here; losing a response after a
    /// completed receive leaves the same cached result available to a new waiter.
    /// Neither case is permission to run the effect again.
    pub(crate) async fn observe(&mut self) -> Result<R, ObserveError> {
        if let Some(result) = &self.observed {
            return result.clone();
        }
        let receiver = self
            .result
            .as_mut()
            .expect("unobserved result retains receiver");
        let result = receiver
            .await
            .map_err(|_| ObserveError::WorkerEndedWithoutResult);
        // No await between consuming the channel and retaining the result.
        self.observed = Some(result.clone());
        self.result.take();
        result
    }

    /// The external owner retains this handle; callers must not abort it merely
    /// because an initiating caller or response receiver was dropped.
    pub(crate) fn task(&self) -> &JoinHandle<()> {
        &self.task
    }

    #[cfg(test)]
    async fn wait_until_entered(&mut self) {
        self.entered
            .take()
            .expect("entry signal available")
            .await
            .expect("actor sends entry signal");
    }
}

#[cfg(test)]
mod tests {
    use super::{EntryDecision, ResourceSupervisor};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::oneshot;

    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn cancel_before_entry_is_inert() {
        let calls = Arc::new(AtomicUsize::new(0));
        let work_calls = Arc::clone(&calls);
        let (mut gate, supervisor) = ResourceSupervisor::<()>::spawn(move |_: ()| async move {
            work_calls.fetch_add(1, Ordering::SeqCst);
        });

        gate.cancel_before_entry();
        supervisor
            .task
            .await
            .expect("gate task should exit cleanly");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn transferred_owner_is_quarantined_if_release_channel_closes() {
        let drops = Arc::new(AtomicUsize::new(0));
        let owner_drops = Arc::clone(&drops);
        let calls = Arc::new(AtomicUsize::new(0));
        let work_calls = Arc::clone(&calls);
        let (mut gate, mut supervisor) =
            ResourceSupervisor::<()>::spawn(move |owner: DropProbe| async move {
                work_calls.fetch_add(1, Ordering::SeqCst);
                drop(owner);
            });

        assert!(matches!(
            gate.transfer(DropProbe(owner_drops)),
            EntryDecision::Transferred
        ));
        supervisor.wait_until_entered().await;
        // Close the release sender without confirmation. Actor has definitely
        // taken the owner and is now waiting on release.
        drop(gate);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        // Deliberately detach this quarantined task: it models process-epoch custody.
        drop(supervisor);
    }

    #[tokio::test]
    async fn confirmed_release_runs_once_and_result_survives_lost_response() {
        let calls = Arc::new(AtomicUsize::new(0));
        let work_calls = Arc::clone(&calls);
        let (response_tx, response_rx) = oneshot::channel::<usize>();
        drop(response_rx); // initiating caller disappears before completion
        let (mut gate, mut supervisor) =
            ResourceSupervisor::<usize>::spawn(move |value| async move {
                work_calls.fetch_add(1, Ordering::SeqCst);
                let _ = response_tx.send(value);
                value + 1
            });

        assert!(matches!(gate.transfer(41), EntryDecision::Transferred));
        assert!(gate.confirm_release());
        assert!(!gate.confirm_release());
        assert_eq!(supervisor.observe().await, Ok(42));
        assert_eq!(supervisor.observe().await, Ok(42));
        supervisor.task.await.expect("worker should exit cleanly");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_waiter_after_consuming_result_can_reobserve_without_reentry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let work_calls = Arc::clone(&calls);
        let (mut gate, supervisor) = ResourceSupervisor::<usize>::spawn(move |value| async move {
            work_calls.fetch_add(1, Ordering::SeqCst);
            value
        });
        assert!(matches!(gate.transfer(19), EntryDecision::Transferred));
        assert!(gate.confirm_release());
        let supervisor = Arc::new(tokio::sync::Mutex::new(supervisor));
        let waiter_supervisor = Arc::clone(&supervisor);
        let (consumed_tx, consumed_rx) = oneshot::channel();
        let waiter = tokio::spawn(async move {
            let result = waiter_supervisor.lock().await.observe().await;
            consumed_tx
                .send(result)
                .expect("test retains completion signal");
            // Model an actor worker whose returned value is lost before its
            // JoinHandle is consumed. The external record must retain custody
            // of the outcome independently of that response.
            std::future::pending::<()>().await;
        });
        assert_eq!(
            consumed_rx.await.expect("waiter consumed physical result"),
            Ok(19)
        );
        waiter.abort();
        assert!(waiter.await.expect_err("waiter cancelled").is_cancelled());
        assert_eq!(supervisor.lock().await.observe().await, Ok(19));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unconfirmed_release_does_not_construct_work() {
        let calls = Arc::new(AtomicUsize::new(0));
        let work_calls = Arc::clone(&calls);
        let (mut gate, mut supervisor) = ResourceSupervisor::<()>::spawn(move |_| {
            work_calls.fetch_add(1, Ordering::SeqCst);
            async {}
        });

        assert!(matches!(gate.transfer(()), EntryDecision::Transferred));
        supervisor.wait_until_entered().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        gate.cancel_before_entry(); // cannot cancel after transfer
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(gate);
        drop(supervisor);
    }

    #[tokio::test]
    async fn cancelled_observe_keeps_receiver_for_later_observation() {
        let (mut gate, mut supervisor) =
            ResourceSupervisor::<usize>::spawn(|value| async move { value });
        assert!(matches!(gate.transfer(7), EntryDecision::Transferred));

        // No result can arrive before release, so timing out this observation
        // cancels a genuinely pending receive while preserving its receiver.
        tokio::select! {
            biased;
            result = supervisor.observe() => panic!("result arrived before release: {result:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(1)) => {}
        }
        assert!(gate.confirm_release());
        assert_eq!(supervisor.observe().await, Ok(7));
    }
}
