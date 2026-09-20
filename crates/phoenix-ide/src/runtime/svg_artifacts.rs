use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use phoenix_db::workflow::LocalAuthorityResult;
use phoenix_svg::{SvgArtifactReference, SvgInvocationId};

use super::{traits::SvgArtifactRepository, AdmittedOperation, FatalLocalAuthorityFence};
use crate::tools::present_svg::{SvgArtifactDraft, SvgArtifactStore};

pub(super) struct RuntimeSvgArtifactStore<S> {
    repository: S,
    fence: Arc<FatalLocalAuthorityFence>,
    lifetime: Arc<SvgPublicationLifetime>,
}

enum SvgOwnerDisposition {
    Serving,
    CoordinatedShutdown,
}

pub(super) struct SvgPublicationLifetime {
    disposition: Mutex<SvgOwnerDisposition>,
}

impl SvgPublicationLifetime {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            disposition: Mutex::new(SvgOwnerDisposition::Serving),
        })
    }

    pub(super) fn coordinated_shutdown(&self) {
        *self.disposition.lock().expect("SVG lifetime poisoned") =
            SvgOwnerDisposition::CoordinatedShutdown;
    }

    fn is_serving(&self) -> bool {
        matches!(
            *self.disposition.lock().expect("SVG lifetime poisoned"),
            SvgOwnerDisposition::Serving
        )
    }
}

impl<S> RuntimeSvgArtifactStore<S> {
    pub(super) fn new(
        repository: S,
        fence: Arc<FatalLocalAuthorityFence>,
        lifetime: Arc<SvgPublicationLifetime>,
    ) -> Self {
        Self {
            repository,
            fence,
            lifetime,
        }
    }

    async fn acquire(&self) -> SvgAuthorityGuard {
        if !self.lifetime.is_serving() {
            return std::future::pending().await;
        }
        let Ok(admitted) = self.fence.try_acquire() else {
            return std::future::pending().await;
        };
        SvgAuthorityGuard {
            fence: Some(self.fence.clone()),
            lifetime: self.lifetime.clone(),
            _admitted: admitted,
        }
    }

    async fn unclassified<T>(&self, mut guard: SvgAuthorityGuard) -> T {
        self.fence.close("svg_publication_authority_unclassified");
        guard.disarm();
        drop(guard);
        std::future::pending().await
    }
}

struct SvgAuthorityGuard {
    fence: Option<Arc<FatalLocalAuthorityFence>>,
    lifetime: Arc<SvgPublicationLifetime>,
    _admitted: AdmittedOperation,
}

impl SvgAuthorityGuard {
    fn disarm(&mut self) {
        self.fence = None;
    }
}

impl Drop for SvgAuthorityGuard {
    fn drop(&mut self) {
        if let Some(fence) = self.fence.take() {
            if self.lifetime.is_serving() {
                fence.close("svg_publication_owner_disappeared");
            }
        }
    }
}

#[async_trait]
impl<S: SvgArtifactRepository> SvgArtifactStore for RuntimeSvgArtifactStore<S> {
    async fn lookup(
        &self,
        conversation_id: &str,
        invocation: &SvgInvocationId,
    ) -> Result<Option<SvgArtifactReference>, String> {
        let mut guard = self.acquire().await;
        match self.repository.lookup(conversation_id, invocation).await {
            Ok(reference) => {
                guard.disarm();
                Ok(reference)
            }
            Err(error) => {
                tracing::error!(%error, "cannot establish SVG invocation authority");
                self.unclassified(guard).await
            }
        }
    }

    async fn publish(
        &self,
        conversation_id: &str,
        invocation: &SvgInvocationId,
        draft: SvgArtifactDraft,
    ) -> Result<SvgArtifactReference, String> {
        let mut guard = self.acquire().await;
        match self
            .repository
            .publish(conversation_id, invocation, draft)
            .await
        {
            LocalAuthorityResult::DurableFactEstablished(result) => {
                guard.disarm();
                result
            }
            LocalAuthorityResult::DurableFactUnclassified => self.unclassified(guard).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    enum Outcome {
        Committed,
        NotCommitted,
        Unclassified,
        Panic,
        Pending,
        LookupFailure,
    }

    struct Repository {
        outcome: Outcome,
        started: Notify,
        calls: AtomicUsize,
    }

    fn reference() -> SvgArtifactReference {
        SvgArtifactReference {
            artifact_id: "artifact".into(),
            conversation_id: "conversation".into(),
            title: "Chart".into(),
            description: "A chart".into(),
            width: 10.0,
            height: 10.0,
            validation: phoenix_svg::SvgValidationOutcome::AcceptedStaticSvg,
        }
    }

    fn draft() -> SvgArtifactDraft {
        SvgArtifactDraft {
            metadata: phoenix_svg::SvgPresentationMetadata::new("Chart", "A chart").unwrap(),
            svg: phoenix_svg::validate(
                br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#,
            )
            .unwrap(),
        }
    }

    fn store(outcome: Outcome) -> RuntimeSvgArtifactStore<Arc<Repository>> {
        RuntimeSvgArtifactStore::new(
            Arc::new(Repository {
                outcome,
                started: Notify::new(),
                calls: AtomicUsize::new(0),
            }),
            FatalLocalAuthorityFence::new(),
            SvgPublicationLifetime::new(),
        )
    }

    #[async_trait]
    impl SvgArtifactRepository for Repository {
        async fn lookup(
            &self,
            _: &str,
            _: &SvgInvocationId,
        ) -> Result<Option<SvgArtifactReference>, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if matches!(self.outcome, Outcome::LookupFailure) {
                Err("unavailable".into())
            } else {
                Ok(None)
            }
        }

        async fn publish(
            &self,
            _: &str,
            _: &SvgInvocationId,
            _: SvgArtifactDraft,
        ) -> LocalAuthorityResult<Result<SvgArtifactReference, String>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            match self.outcome {
                Outcome::Committed => LocalAuthorityResult::DurableFactEstablished(Ok(reference())),
                Outcome::NotCommitted => {
                    LocalAuthorityResult::DurableFactEstablished(Err("not committed".into()))
                }
                Outcome::Unclassified | Outcome::LookupFailure => {
                    LocalAuthorityResult::DurableFactUnclassified
                }
                Outcome::Panic => panic!("authority owner panic"),
                Outcome::Pending => std::future::pending().await,
            }
        }
    }

    #[tokio::test]
    async fn established_results_return_without_closing_admission() {
        for outcome in [Outcome::Committed, Outcome::NotCommitted] {
            let store = store(outcome);
            let result = store
                .publish(
                    "conversation",
                    &SvgInvocationId::new("assistant", "tool"),
                    draft(),
                )
                .await;
            if matches!(store.repository.outcome, Outcome::Committed) {
                assert_eq!(result.unwrap(), reference());
            } else {
                assert_eq!(result.unwrap_err(), "not committed");
            }
            assert!(!store.fence.is_closed());
            store.fence.wait_for_owners().await;
        }
    }

    #[tokio::test]
    async fn unclassified_fact_closes_admission_without_a_tool_result() {
        for outcome in [Outcome::Unclassified, Outcome::LookupFailure] {
            let store = store(outcome);
            let fence = store.fence.clone();
            let mut receiver = fence.subscribe();
            let task = tokio::spawn(async move {
                if matches!(store.repository.outcome, Outcome::LookupFailure) {
                    store
                        .lookup("conversation", &SvgInvocationId::new("assistant", "tool"))
                        .await
                        .map(|_| ())
                } else {
                    store
                        .publish(
                            "conversation",
                            &SvgInvocationId::new("assistant", "tool"),
                            draft(),
                        )
                        .await
                        .map(|_| ())
                }
            });
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                crate::tls::wait_for_fatal_local_authority(&mut receiver),
            )
            .await
            .unwrap();
            assert!(fence.is_closed());
            assert!(!task.is_finished());
            fence.wait_for_owners().await;
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        }
    }

    #[tokio::test]
    async fn interrupted_publication_owner_closes_admission() {
        for outcome in [Outcome::Panic, Outcome::Pending] {
            let store = store(outcome);
            let fence = store.fence.clone();
            let repository = store.repository.clone();
            let task = tokio::spawn(async move {
                store
                    .publish(
                        "conversation",
                        &SvgInvocationId::new("assistant", "tool"),
                        draft(),
                    )
                    .await
            });
            if matches!(repository.outcome, Outcome::Pending) {
                tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    repository.started.notified(),
                )
                .await
                .expect("publication entered the repository");
                task.abort();
            }
            assert!(task.await.is_err());
            assert!(fence.is_closed());
            fence.wait_for_owners().await;
        }
    }

    #[tokio::test]
    async fn closed_admission_never_calls_repository() {
        let store = store(Outcome::Committed);
        store.fence.close("test");
        let repository = store.repository.clone();
        let invocation = SvgInvocationId::new("assistant", "tool");
        let publication = store.publish("conversation", &invocation, draft());
        tokio::pin!(publication);
        assert!(futures::poll!(publication.as_mut()).is_pending());
        assert_eq!(repository.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn coordinated_shutdown_retires_owner_without_fatal_authority_loss() {
        let store = store(Outcome::Pending);
        let lifetime = store.lifetime.clone();
        let fence = store.fence.clone();
        let repository = store.repository.clone();
        let task = tokio::spawn(async move {
            store
                .publish(
                    "conversation",
                    &SvgInvocationId::new("assistant", "tool"),
                    draft(),
                )
                .await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            repository.started.notified(),
        )
        .await
        .expect("publication entered the repository");
        lifetime.coordinated_shutdown();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!fence.is_closed());
        fence.wait_for_owners().await;

        let store = RuntimeSvgArtifactStore::new(repository.clone(), fence, lifetime);
        let invocation = SvgInvocationId::new("assistant", "next");
        let publication = store.publish("conversation", &invocation, draft());
        tokio::pin!(publication);
        assert!(futures::poll!(publication.as_mut()).is_pending());
        assert_eq!(repository.calls.load(Ordering::SeqCst), 1);
    }
}
