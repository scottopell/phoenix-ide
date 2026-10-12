use std::sync::{Arc, Mutex};

use serde::Serialize;

use super::RetrievalError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationPhase {
    Discovery,
    LocatorRepair,
    Messages,
    Pruning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReconciliationProgress {
    pub phase: ReconciliationPhase,
    pub processed_messages: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReconciliationFailure {
    Database { extended_code: Option<u32> },
    Other,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReconciliationStatus {
    Pending,
    Running {
        progress: ReconciliationProgress,
    },
    Ready,
    Failed {
        progress: ReconciliationProgress,
        reason: ReconciliationFailure,
    },
}

impl ReconciliationStatus {
    #[must_use]
    pub fn unavailable_message(self) -> Option<&'static str> {
        match self {
            Self::Ready => None,
            Self::Pending | Self::Running { .. } => Some(
                "the global message index is still warming; try again after startup reconciliation completes",
            ),
            Self::Failed { .. } => Some(
                "the global message index is unavailable because startup reconciliation failed; inspect deployment retrieval status",
            ),
        }
    }
}

pub(super) type SharedStatus = Arc<Mutex<ReconciliationStatus>>;

/// Owns one startup attempt. Dropping an unfinished attempt records interruption,
/// including when its future is cancelled before its first poll.
pub struct ReconciliationRun {
    status: SharedStatus,
}

impl ReconciliationRun {
    pub(super) fn start(status: SharedStatus) -> Result<Self, RetrievalError> {
        let progress = ReconciliationProgress {
            phase: ReconciliationPhase::Discovery,
            processed_messages: 0,
        };
        {
            let mut current = status.lock().expect("reconciliation status lock");
            if *current != ReconciliationStatus::Pending {
                return Err(RetrievalError::AlreadyStarted);
            }
            *current = ReconciliationStatus::Running { progress };
        }
        Ok(Self { status })
    }

    /// # Panics
    /// Panics if the status mutex was poisoned.
    pub fn phase(&mut self, phase: ReconciliationPhase) {
        if let ReconciliationStatus::Running { progress } =
            &mut *self.status.lock().expect("reconciliation status lock")
        {
            progress.phase = phase;
        }
    }

    /// # Panics
    /// Panics if the status mutex was poisoned.
    pub fn message_processed(&mut self) {
        if let ReconciliationStatus::Running { progress } =
            &mut *self.status.lock().expect("reconciliation status lock")
        {
            progress.processed_messages += 1;
        }
    }

    /// Publish the orchestration owner's terminal result.
    ///
    /// # Panics
    /// Panics if the status mutex was poisoned.
    pub fn complete(self, result: &Result<super::ReconcileStats, RetrievalError>) {
        let mut current = self.status.lock().expect("reconciliation status lock");
        let ReconciliationStatus::Running { progress } = *current else {
            return;
        };
        *current = match result {
            Ok(_) => ReconciliationStatus::Ready,
            Err(error) => {
                let reason = match error {
                    RetrievalError::Db(sqlx::Error::Database(error)) => {
                        ReconciliationFailure::Database {
                            extended_code: error.code().and_then(|code| code.parse().ok()),
                        }
                    }
                    RetrievalError::Db(_)
                    | RetrievalError::MessageDecode(_)
                    | RetrievalError::AlreadyStarted => ReconciliationFailure::Other,
                };
                ReconciliationStatus::Failed { progress, reason }
            }
        };
    }
}

impl Drop for ReconciliationRun {
    fn drop(&mut self) {
        let interrupted = {
            let mut current = self.status.lock().expect("reconciliation status lock");
            if let ReconciliationStatus::Running { progress } = *current {
                *current = ReconciliationStatus::Failed {
                    progress,
                    reason: ReconciliationFailure::Interrupted,
                };
                true
            } else {
                false
            }
        };
        if interrupted {
            tracing::warn!("conversation retrieval reconciliation interrupted before completion");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> SharedStatus {
        Arc::new(Mutex::new(ReconciliationStatus::Pending))
    }

    #[test]
    fn only_completed_attempt_becomes_ready_and_duplicate_start_is_rejected() {
        let status = status();
        let mut run = ReconciliationRun::start(status.clone()).unwrap();
        assert!(ReconciliationRun::start(status.clone()).is_err());
        run.phase(ReconciliationPhase::Messages);
        run.message_processed();
        assert!(matches!(
            *status.lock().unwrap(),
            ReconciliationStatus::Running {
                progress: ReconciliationProgress {
                    processed_messages: 1,
                    ..
                }
            }
        ));
        run.complete(&Ok(super::super::ReconcileStats::default()));
        assert_eq!(*status.lock().unwrap(), ReconciliationStatus::Ready);
        assert!(ReconciliationRun::start(status.clone()).is_err());
    }

    #[tokio::test]
    async fn cancellation_before_poll_and_panic_leave_failed_not_warming() {
        let cancelled = status();
        let run = ReconciliationRun::start(cancelled.clone()).unwrap();
        let future = async move {
            let _run = run;
            std::future::pending::<()>().await;
        };
        drop(future);
        assert!(matches!(
            *cancelled.lock().unwrap(),
            ReconciliationStatus::Failed {
                reason: ReconciliationFailure::Interrupted,
                ..
            }
        ));

        let panicked = status();
        let run = ReconciliationRun::start(panicked.clone()).unwrap();
        let result = tokio::spawn(async move {
            let _run = run;
            panic!("injected reconciliation panic");
        })
        .await;
        assert!(result.unwrap_err().is_panic());
        assert!(matches!(
            *panicked.lock().unwrap(),
            ReconciliationStatus::Failed {
                reason: ReconciliationFailure::Interrupted,
                ..
            }
        ));
        assert!(panicked
            .lock()
            .unwrap()
            .unavailable_message()
            .unwrap()
            .contains("failed"));
        assert!(ReconciliationRun::start(panicked).is_err());
        assert!(ReconciliationRun::start(status()).is_ok());
    }

    #[tokio::test]
    async fn abort_after_poll_releases_attempt_with_terminal_status() {
        let status = status();
        let run = ReconciliationRun::start(status.clone()).unwrap();
        let (sent, received) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _run = run;
            sent.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        received.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(matches!(
            *status.lock().unwrap(),
            ReconciliationStatus::Failed { .. }
        ));
    }
}
