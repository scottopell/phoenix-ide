use std::future::Future;
use tokio::sync::watch;

pub(crate) struct ManagedTaskShutdown {
    done: watch::Receiver<bool>,
}

pub(crate) fn spawn<F, Fut>(
    name: &'static str,
    run: F,
) -> (watch::Sender<bool>, ManagedTaskShutdown)
where
    F: FnOnce(watch::Receiver<bool>) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let (stop, stop_rx) = watch::channel(false);
    let (done, done_rx) = watch::channel(false);
    let task = tokio::spawn(run(stop_rx));
    tokio::spawn(async move {
        if let Err(error) = task.await {
            tracing::warn!(task = name, error = %error, "managed task failed");
        }
        done.send_replace(true);
    });
    (stop, ManagedTaskShutdown { done: done_rx })
}

impl ManagedTaskShutdown {
    pub(crate) async fn wait(mut self) {
        while !*self.done.borrow_and_update() {
            self.done
                .changed()
                .await
                .expect("managed task completion owner outlives shutdown waiters");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn completion_waits_for_owned_task() {
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let (stop, completion) = spawn("test", {
            let barrier = barrier.clone();
            move |mut stop| async move {
                stop.changed().await.unwrap();
                barrier.wait().await;
            }
        });
        stop.send_replace(true);

        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            completion.wait().await;
            done_tx.send(()).unwrap();
        });
        assert!(matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        barrier.wait().await;
        done_rx.await.unwrap();
    }

    #[tokio::test]
    async fn natural_completion_is_observed_before_shutdown() {
        let (stop, completion) = spawn("test", |_| async {});
        completion.wait().await;
        assert!(!*stop.borrow());
    }
}
