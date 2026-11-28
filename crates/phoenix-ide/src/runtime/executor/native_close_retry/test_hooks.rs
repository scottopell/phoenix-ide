//! One-shot barriers owned by a single test actor, never by a process-wide registry.
//! Physical-worktree test drivers are Linux-only; passive checkpoints remain portable.

#[cfg(target_os = "linux")]
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Point {
    CapturedLossAssessmentReady,
    WorktreeDispatchBeforeWriter,
    WorktreePreparationReady,
    PhysicalBeforeMutation,
    PhysicalBeforeCheckoutRemoval,
    PhysicalBeforeObservation,
}

struct Checkpoint {
    point: Point,
    reached: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}

#[cfg(target_os = "linux")]
type FinalWriterGap = (
    phoenix_db::CloseWriterObservation,
    oneshot::Sender<std::path::PathBuf>,
);

#[derive(Clone, Default)]
pub(crate) struct Hooks {
    checkpoint: Arc<Mutex<Option<Checkpoint>>>,
    #[cfg(target_os = "linux")]
    final_writer_gap: Arc<Mutex<Option<FinalWriterGap>>>,
    revalidations: Arc<std::sync::atomic::AtomicUsize>,
    pending_ticks: Arc<std::sync::atomic::AtomicUsize>,
    pending_tick: Arc<tokio::sync::Notify>,
}

tokio::task_local! {
    static ACTOR_HOOKS: Hooks;
}

impl Hooks {
    #[cfg(target_os = "linux")]
    pub(crate) async fn scope<F: Future>(&self, future: F) -> F::Output {
        ACTOR_HOOKS.scope(self.clone(), future).await
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn inject_final_writer_gap(
        &self,
        observation: phoenix_db::CloseWriterObservation,
    ) -> oneshot::Receiver<std::path::PathBuf> {
        let (scanned_path, observed) = oneshot::channel();
        let mut slot = self.final_writer_gap.lock().unwrap();
        assert!(slot.is_none());
        *slot = Some((observation, scanned_path));
        observed
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn take_final_writer_gap(&self) -> Option<FinalWriterGap> {
        self.final_writer_gap.lock().unwrap().take()
    }

    // Capture before spawning: task-local values do not propagate to workers.
    pub(crate) fn current() -> Option<Self> {
        ACTOR_HOOKS.try_with(Clone::clone).ok()
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn arm(&self, point: Point) -> Pause {
        let (reached, arrival) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        let mut slot = self.checkpoint.lock().unwrap();
        assert!(slot.is_none(), "only one barrier may be armed per actor");
        *slot = Some(Checkpoint {
            point,
            reached,
            resume,
        });
        Pause { arrival, release }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn revalidation_count(&self) -> usize {
        self.revalidations.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn record_revalidation(&self) {
        self.revalidations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn record_pending_tick(&self) {
        self.pending_ticks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.pending_tick.notify_one();
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn pending_tick_count(&self) -> usize {
        self.pending_ticks.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(target_os = "linux")]
    pub(crate) async fn wait_for_pending_ticks(&self, target: usize) {
        loop {
            let notified = self.pending_tick.notified();
            if self.pending_ticks.load(std::sync::atomic::Ordering::SeqCst) >= target {
                return;
            }
            notified.await;
        }
    }

    pub(crate) async fn checkpoint(&self, point: Point) {
        let checkpoint = {
            let mut slot = self.checkpoint.lock().unwrap();
            if slot.as_ref().is_some_and(|hook| hook.point == point) {
                slot.take()
            } else {
                None
            }
        };
        if let Some(checkpoint) = checkpoint {
            let _ = checkpoint.reached.send(());
            // Dropping a failed test's release sender must not strand a worker.
            let _ = checkpoint.resume.await;
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) struct Pause {
    arrival: oneshot::Receiver<()>,
    release: oneshot::Sender<()>,
}

#[cfg(target_os = "linux")]
impl Pause {
    pub(crate) async fn reached(&mut self) -> Result<(), &'static str> {
        tokio::time::timeout(std::time::Duration::from_secs(10), &mut self.arrival)
            .await
            .map_err(|_| "retry worker did not reach the causal barrier before its deadline")?
            .map_err(|_| "retry worker dropped the causal barrier")
    }

    pub(crate) fn release(self) {
        self.release
            .send(())
            .expect("held retry worker disappeared");
    }
}

pub(crate) async fn checkpoint(hooks: Option<&Hooks>, point: Point) {
    if let Some(hooks) = hooks {
        hooks.checkpoint(point).await;
    }
}
