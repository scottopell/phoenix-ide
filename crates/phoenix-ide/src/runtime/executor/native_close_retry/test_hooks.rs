//! One-shot barriers owned by a single test actor, never by a process-wide registry.

use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Point {
    CapturedLossAssessmentReady,
    WorktreeDispatchBeforeWriter,
    WorktreePreparationReady,
    PhysicalBeforeObservation,
}

struct Checkpoint {
    point: Point,
    reached: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}

#[derive(Clone, Default)]
pub(crate) struct Hooks(Arc<Mutex<Option<Checkpoint>>>);

tokio::task_local! {
    static ACTOR_HOOKS: Hooks;
}

impl Hooks {
    pub(crate) async fn scope<F: Future>(&self, future: F) -> F::Output {
        ACTOR_HOOKS.scope(self.clone(), future).await
    }

    // Capture before spawning: task-local values do not propagate to workers.
    pub(crate) fn current() -> Option<Self> {
        ACTOR_HOOKS.try_with(Clone::clone).ok()
    }

    pub(crate) fn arm(&self, point: Point) -> Pause {
        let (reached, arrival) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        let mut slot = self.0.lock().unwrap();
        assert!(slot.is_none(), "only one barrier may be armed per actor");
        *slot = Some(Checkpoint {
            point,
            reached,
            resume,
        });
        Pause { arrival, release }
    }

    pub(crate) async fn checkpoint(&self, point: Point) {
        let checkpoint = {
            let mut slot = self.0.lock().unwrap();
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

pub(crate) struct Pause {
    arrival: oneshot::Receiver<()>,
    release: oneshot::Sender<()>,
}

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
