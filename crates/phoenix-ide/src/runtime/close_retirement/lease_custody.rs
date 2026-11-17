use std::collections::HashMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use phoenix_tools::bash::registry::{BashHandleRegistry, BashTeardownFence};
use phoenix_tools::browser::BrowserSessionManager;
use phoenix_tools::tmux::registry::TmuxRegistry;

use std::path::PathBuf;

use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
use phoenix_core::domain::close::{RetiredResourceIdentity, RetiredResourceKind};
use phoenix_tools::browser::session::BrowserRetirementPermit;
use phoenix_tools::tmux::registry::{
    PersistentTmuxDiscovery, TmuxRetirementPermit, TmuxRetirementRehydration,
};
use crate::terminal::session::TerminalRetirementPermit;

use super::{CloseResourceLease, opaque_resource};

type LeaseKey = (String, WorkScopeId);
type Acquisition = Result<CloseResourceLease, FailedAcquisition>;
type Reopening = Result<(), (String, Vec<(LeaseKey, CloseResourceLease)>)>;

#[derive(Clone)]
pub(super) struct LeaseRegistries {
    pub(super) bash: Arc<BashHandleRegistry>,
    pub(super) tmux: Arc<TmuxRegistry>,
    pub(super) terminal: crate::terminal::ActiveTerminals,
    pub(super) browser: Arc<BrowserSessionManager>,
}

pub(super) struct FailedAcquisition {
    bash: BashTeardownFence,
    terminal: TerminalRetirementPermit,
    browser: BrowserRetirementPermit,
    tmux_failure: String,
    registries: LeaseRegistries,
    scope: ResourceScopeKey,
    legacy_worktree_path: Option<PathBuf>,
    legacy_conversation_id: Option<String>,
    expires: tokio::time::Instant,
}

impl FailedAcquisition {
    async fn retry_tmux(
        mut self,
        #[cfg(test)] retry_hook: Option<RetryHook>,
    ) -> Result<CloseResourceLease, (Self, String)> {
        #[cfg(test)]
        if let Some(hook) = retry_hook {
            hook.entered.notify_one();
            hook.release.notified().await;
        }
        if tokio::time::Instant::now() >= self.expires {
            self.tmux_failure = "tmux Close deadline elapsed before identity discovery".to_string();
            let detail = self.detail();
            return Err((self, detail));
        }
        let tmux = match self.registries.acquire_tmux(
            &self.scope,
            self.legacy_worktree_path.clone(),
            self.legacy_conversation_id.clone(),
            self.expires,
        ).await {
            Ok(tmux) => tmux,
            Err(error) => {
                self.tmux_failure = error;
                let detail = self.detail();
                return Err((self, detail));
            }
        };
        let bash = self.registries.bash.begin_retirement(&self.scope).await;
        let mut resources = Vec::new();
        for target in &bash.exact_process_groups {
            resources.push(opaque_resource(
                RetiredResourceKind::BashProcessGroup,
                target.stable_resource_identity(),
            ));
        }
        if tmux.had_entry() {
            resources.push(opaque_resource(RetiredResourceKind::TmuxServer, tmux.instance.stable_identity()));
        }
        if let Some(instance) = &self.terminal.instance {
            resources.push(opaque_resource(RetiredResourceKind::PtySession, instance.stable_identity()));
        }
        for instance in &self.browser.instances {
            resources.push(opaque_resource(RetiredResourceKind::BrowserSession, instance.stable_identity()));
        }
        Ok(CloseResourceLease {
            bash, tmux, terminal: self.terminal, browser: self.browser, resources,
        })
    }

    fn detail(&self) -> String {
        format!(
            "{}; retained partial Close custody (bash {:?}, terminal {:?}, browser {:?}); tmux reopening authority unresolved",
            self.tmux_failure,
            self.bash.generation(),
            self.terminal.generation(),
            self.browser.generation(),
        )
    }
}

enum AcquiringLease {
    Pending(BoxFuture<'static, Acquisition>),
    Failed(FailedAcquisition),
}

#[derive(Default)]
pub(crate) struct CloseResourceLeaseStore {
    pub(crate) complete: HashMap<LeaseKey, CloseResourceLease>,
    acquiring: HashMap<LeaseKey, AcquiringLease>,
    reopening: HashMap<String, BoxFuture<'static, Reopening>>,
    #[cfg(test)]
    pub(super) acquisition_hook: Option<AcquisitionHook>,
    #[cfg(test)]
    pub(super) reopening_hook: Option<ReopeningHook>,
    #[cfg(test)]
    pub(super) retry_hook: Option<RetryHook>,
}

impl CloseResourceLeaseStore {
    pub(super) fn discard_completed(&mut self, attempt: &str) {
        if self.reopening.contains_key(attempt)
            || self.acquiring.keys().any(|(candidate, _)| candidate == attempt)
        {
            return;
        }
        self.complete.retain(|(candidate, _), _| candidate != attempt);
    }

    pub(super) fn is_reopening(&self, attempt: &str) -> bool {
        self.reopening.contains_key(attempt)
    }

    pub(super) fn has_acquisition(&self, key: &LeaseKey) -> bool {
        self.acquiring.contains_key(key)
    }

    pub(super) async fn acquire(
        &mut self,
        key: LeaseKey,
        registries: LeaseRegistries,
        legacy_worktree_path: Option<PathBuf>,
        legacy_conversation_id: Option<String>,
    ) -> Result<Vec<RetiredResourceIdentity>, String> {
        if self.is_reopening(&key.0) {
            return Err("Close lease reopening is incomplete".to_string());
        }
        if let Some(lease) = self.complete.get(&key) {
            return Ok(lease.resources.clone());
        }
        if !self.acquiring.contains_key(&key) {
            let scope = key.1.clone();
            #[cfg(test)]
            let hook = self.acquisition_hook.take();
            let future = Box::pin(async move {
                registries.acquire(
                    scope,
                    legacy_worktree_path,
                    legacy_conversation_id,
                    #[cfg(test)]
                    hook,
                ).await
            });
            self.acquiring.insert(key.clone(), AcquiringLease::Pending(future));
        }
        self.finish_acquisition(&key).await
    }

    pub(super) async fn finish_acquisition(
        &mut self,
        key: &LeaseKey,
    ) -> Result<Vec<RetiredResourceIdentity>, String> {
        if matches!(self.acquiring.get(key), Some(AcquiringLease::Failed(_))) {
            let Some(AcquiringLease::Failed(failure)) = self.acquiring.remove(key) else {
                unreachable!("failed acquisition was selected")
            };
            // Transfer the original partial permits into retained custody before
            // polling any retry work. Dropping this caller must not drop them.
            #[cfg(test)]
            let retry_hook = self.retry_hook.take();
            self.acquiring.insert(key.clone(), AcquiringLease::Pending(Box::pin(async move {
                failure.retry_tmux(#[cfg(test)] retry_hook).await.map_err(|(failure, _)| failure)
            })));
        }
        let outcome = match self.acquiring.get_mut(key).expect("retained acquisition") {
            AcquiringLease::Pending(future) => future.as_mut().await,
            AcquiringLease::Failed(_) => unreachable!("failed acquisition handled above"),
        };
        match outcome {
            Ok(lease) => {
                let resources = lease.resources.clone();
                self.acquiring.remove(key);
                self.complete.insert(key.clone(), lease);
                Ok(resources)
            }
            Err(failure) => {
                let detail = failure.detail();
                *self.acquiring.get_mut(key).expect("pending acquisition remains retained") =
                    AcquiringLease::Failed(failure);
                Err(detail)
            }
        }
    }

    pub(super) async fn cancel_attempt(
        &mut self,
        attempt: &str,
        registries: LeaseRegistries,
    ) -> Result<(), String> {
        if !self.reopening.contains_key(attempt) {
            let mut acquiring = self.acquiring.keys()
                .filter(|(candidate, _)| candidate == attempt)
                .cloned().collect::<Vec<_>>();
            acquiring.sort();
            for key in acquiring {
                self.finish_acquisition(&key).await?;
            }
            let mut keys = self.complete.keys()
                .filter(|(candidate, _)| candidate == attempt)
                .cloned().collect::<Vec<_>>();
            keys.sort();
            let leases = keys.into_iter()
                .map(|key| {
                    let lease = self.complete.remove(&key).expect("selected complete lease");
                    (key, lease)
                }).collect();
            #[cfg(test)]
            let hook = self.reopening_hook.take();
            self.reopening.insert(attempt.to_string(), Box::pin(async move {
                registries.reopen(leases, #[cfg(test)] hook).await
            }));
        }
        let outcome = self.reopening.get_mut(attempt)
            .expect("retained reopening").as_mut().await;
        self.reopening.remove(attempt);
        match outcome {
            Ok(()) => Ok(()),
            Err((detail, leases)) => {
                self.complete.extend(leases);
                Err(detail)
            }
        }
    }
}

impl LeaseRegistries {
    pub(super) async fn acquire(
        self,
        scope: WorkScopeId,
        legacy_worktree_path: Option<PathBuf>,
        legacy_conversation_id: Option<String>,
        #[cfg(test)] hook: Option<AcquisitionHook>,
    ) -> Acquisition {
        let expires = phoenix_tools::tmux::registry::close_deadline();
        let key = ResourceScopeKey::Work(scope);
        let bash = self.bash.begin_teardown(&key).await
            .expect("begin_teardown installs a fenced owner table");
        let terminal = self.terminal.begin_retirement(&key);
        let browser = self.browser.begin_retirement(&key).await;
        let tmux = async {
            #[cfg(test)]
            if let Some(hook) = hook {
                hook.entered.notify_one();
                hook.release.notified().await;
                if hook.fail {
                    self.tmux.fence_retirement_admission(&key).await;
                    return Err("injected tmux acquisition failure".to_string());
                }
            }
            self.acquire_tmux(
                &key, legacy_worktree_path.clone(), legacy_conversation_id.clone(), expires,
            ).await
        }.await;
        let tmux = match tmux {
            Ok(permit) => permit,
            Err(tmux_failure) => return Err(FailedAcquisition {
                bash, terminal, browser, tmux_failure,
                registries: self.clone(), scope: key.clone(),
                legacy_worktree_path, legacy_conversation_id, expires,
            }),
        };
        let bash = self.bash.begin_retirement(&key).await;
        let mut resources = Vec::new();
        for target in &bash.exact_process_groups {
            resources.push(opaque_resource(
                RetiredResourceKind::BashProcessGroup,
                target.stable_resource_identity(),
            ));
        }
        if tmux.had_entry() {
            resources.push(opaque_resource(
                RetiredResourceKind::TmuxServer,
                tmux.instance.stable_identity(),
            ));
        }
        if let Some(instance) = &terminal.instance {
            resources.push(opaque_resource(RetiredResourceKind::PtySession, instance.stable_identity()));
        }
        for instance in &browser.instances {
            resources.push(opaque_resource(RetiredResourceKind::BrowserSession, instance.stable_identity()));
        }
        Ok(CloseResourceLease { bash, tmux, terminal, browser, resources })
    }

    async fn acquire_tmux(
        &self,
        key: &ResourceScopeKey,
        legacy_worktree_path: Option<PathBuf>,
        legacy_conversation_id: Option<String>,
        tmux_expires: tokio::time::Instant,
    ) -> Result<TmuxRetirementPermit, String> {
        if tokio::time::Instant::now() >= tmux_expires {
            self.tmux.fence_retirement_admission(key).await;
            return Err("tmux Close deadline elapsed before identity discovery".to_string());
        }
        let tmux_discovery = match self
            .tmux
            .discover_persistent_identity(
                key,
                legacy_worktree_path.as_deref(),
                legacy_conversation_id.as_deref(),
                tmux_expires,
            )
            .await
        {
            Ok(discovery) => discovery,
            Err(error) => {
                self.tmux.fence_retirement_admission(key).await;
                return Err(format!("tmux identity discovery failed: {error}"));
            }
        };
        let tmux = match tmux_discovery {
            discovery @ (PersistentTmuxDiscovery::EndpointAbsent
            | PersistentTmuxDiscovery::ServerAbsent) => match self
                .tmux
                .begin_retirement_after_discovery(key, &discovery, tmux_expires)
                .await
            {
                Ok(permit) => permit,
                Err(outcome) => {
                    self.tmux.fence_retirement_admission(key).await;
                    return Err(format!("tmux retirement fencing failed: {outcome:?}"));
                }
            },
            PersistentTmuxDiscovery::Exact(identity) => {
                match self
                    .tmux
                    .rehydrate_retirement(key, &identity, tmux_expires)
                    .await
                {
                    Ok(TmuxRetirementRehydration::Permit(permit)) => permit,
                    Ok(TmuxRetirementRehydration::AbsenceVerified) => match self
                        .tmux
                        .begin_retirement(key, None, None, tmux_expires)
                        .await
                    {
                        Ok(permit) => permit,
                        Err(outcome) => {
                            self.tmux.fence_retirement_admission(key).await;
                            return Err(format!("tmux retirement fencing failed: {outcome:?}"));
                        }
                    },
                    Ok(TmuxRetirementRehydration::Residual { reason }) => {
                        self.tmux.fence_retirement_admission(key).await;
                        return Err(format!("tmux identity is ambiguous: {reason}"));
                    }
                    Err(error) => {
                        self.tmux.fence_retirement_admission(key).await;
                        return Err(format!("tmux rehydration failed: {error}"));
                    }
                }
            }
            PersistentTmuxDiscovery::Ambiguous { reason } => {
                self.tmux.fence_retirement_admission(key).await;
                return Err(format!("tmux identity is ambiguous: {reason}"));
            }
        };
        Ok(tmux)
    }

    async fn reopen(self, leases: Vec<(LeaseKey, CloseResourceLease)>,
        #[cfg(test)] hook: Option<ReopeningHook>,
    ) -> Reopening {
        let mut cancelling = Vec::with_capacity(leases.len());
        let mut permits = Vec::with_capacity(leases.len());
        for (key, lease) in leases {
            let CloseResourceLease { bash, tmux, terminal, browser, resources } = lease;
            cancelling.push((key, bash, terminal, browser, resources));
            permits.push(tmux);
        }
        #[cfg(test)]
        if let Some(hook) = hook.as_ref().filter(|hook| hook.before_tmux) {
            hook.pause().await;
        }
        #[cfg(test)]
        if hook.as_ref().is_some_and(|hook| hook.fail_tmux_batch) {
            let leases = cancelling.into_iter().zip(permits)
                .map(|((key, bash, terminal, browser, resources), tmux)| {
                    (key, CloseResourceLease { bash, tmux, terminal, browser, resources })
                }).collect();
            return Err(("injected tmux batch cancellation failure".to_string(), leases));
        }
        if let Err(error) = self.tmux.cancel_retirement_batch(permits).await {
            let detail = error.to_string();
            let leases = cancelling.into_iter().zip(error.into_permits())
                .map(|((key, bash, terminal, browser, resources), tmux)| {
                    (key, CloseResourceLease { bash, tmux, terminal, browser, resources })
                }).collect();
            return Err((detail, leases));
        }
        for (_, bash, terminal, browser, _) in cancelling {
            self.bash.cancel_retirement(bash).await;
            self.terminal.cancel_retirement(terminal);
            #[cfg(test)]
            if let Some(hook) = hook.as_ref().filter(|hook| !hook.before_tmux) {
                hook.pause().await;
            }
            self.browser.reopen_after_permit(browser).await;
        }
        Ok(())
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(super) struct AcquisitionHook {
    pub(super) entered: Arc<tokio::sync::Notify>,
    pub(super) release: Arc<tokio::sync::Notify>,
    pub(super) fail: bool,
}

#[cfg(test)]
#[derive(Clone)]
pub(super) struct RetryHook {
    pub(super) entered: Arc<tokio::sync::Notify>,
    pub(super) release: Arc<tokio::sync::Notify>,
}

#[cfg(test)]
#[derive(Clone)]
pub(super) struct ReopeningHook {
    pub(super) entered: Arc<tokio::sync::Notify>,
    pub(super) release: Arc<tokio::sync::Notify>,
    pub(super) before_tmux: bool,
    pub(super) fail_tmux_batch: bool,
}

#[cfg(test)]
impl ReopeningHook {
    async fn pause(&self) {
        self.entered.notify_one();
        self.release.notified().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_tools::tmux::fake_backend::FakeTmuxBackend;
    use tokio::sync::Notify;
    use phoenix_core::work_scope::{EffectiveResourceAccess, ResourceAuthority};
    use phoenix_tools::browser::BrowserError;

    async fn browser_fenced(registries: &LeaseRegistries, scope: &ResourceScopeKey) -> bool {
        matches!(registries.browser.get_existing_for_actor(
            scope, &EffectiveResourceAccess::new("custody-test", ResourceAuthority::Work),
        ).await, Err(BrowserError::RetirementFenced { .. }))
    }

    fn registries() -> LeaseRegistries {
        LeaseRegistries {
            bash: Arc::new(BashHandleRegistry::new()),
            tmux: Arc::new(TmuxRegistry::with_backend(
                PathBuf::from("/close-lease-custody-test"),
                FakeTmuxBackend::unavailable(),
                None,
            )),
            terminal: crate::terminal::ActiveTerminals::new(),
            browser: BrowserSessionManager::new(),
        }
    }

    fn key(attempt: &str, scope: &str) -> LeaseKey {
        (attempt.to_string(), WorkScopeId::parse(scope).unwrap())
    }

    fn acquisition_hook(fail: bool) -> AcquisitionHook {
        AcquisitionHook {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            fail,
        }
    }

    fn reopening_hook(before_tmux: bool) -> ReopeningHook {
        ReopeningHook {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            before_tmux,
            fail_tmux_batch: false,
        }
    }

    fn generations(lease: &CloseResourceLease) -> String {
        format!("{:?}/{:?}/{:?}/{:?}", lease.bash.generation(),
            lease.tmux.generation(), lease.terminal.generation(), lease.browser.generation())
    }

    async fn assert_admission(registries: &LeaseRegistries, key: &LeaseKey, fenced: bool) {
        let scope = ResourceScopeKey::Work(key.1.clone());
        assert_eq!(registries.tmux.is_retirement_fenced(&scope).await, fenced);
        assert_eq!(registries.terminal.is_retirement_fenced(&scope), fenced);
        assert_eq!(browser_fenced(registries, &scope).await, fenced);
    }

    #[tokio::test]
    async fn dropped_acquisition_is_retained_and_completed_lease_is_reused() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let key = key("attempt", "scope");
        let hook = acquisition_hook(false);
        store.acquisition_hook = Some(hook.clone());
        {
            let acquisition = store.acquire(key.clone(), registries.clone(), None, None);
            tokio::pin!(acquisition);
            tokio::select! {
                _ = hook.entered.notified() => {},
                result = &mut acquisition => panic!("acquisition escaped pause: {result:?}"),
            }
        }
        assert!(matches!(store.acquiring.get(&key), Some(AcquiringLease::Pending(_))));
        assert!(store.complete.is_empty());
        assert!(registries.terminal.is_retirement_fenced(&ResourceScopeKey::Work(key.1.clone())));
        assert!(browser_fenced(&registries, &ResourceScopeKey::Work(key.1.clone())).await);
        hook.release.notify_one();
        let resources = store.acquire(key.clone(), registries.clone(), None, None).await.unwrap();
        let original = generations(store.complete.get(&key).unwrap());
        let unused_hook = acquisition_hook(true);
        store.acquisition_hook = Some(unused_hook);
        assert_eq!(store.acquire(key.clone(), registries.clone(), None, None).await.unwrap(), resources);
        assert_eq!(generations(store.complete.get(&key).unwrap()), original);
        assert!(store.acquisition_hook.is_some(), "reuse must not start acquisition");
        assert!(store.acquiring.is_empty());
        store.cancel_attempt(&key.0, registries.clone()).await.unwrap();
        assert_admission(&registries, &key, false).await;
    }

    #[tokio::test]
    async fn failed_acquisition_retains_partial_permits_without_inventing_tmux_authority() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let key = key("attempt", "scope");
        let hook = acquisition_hook(true);
        store.acquisition_hook = Some(hook.clone());
        {
            let acquisition = store.acquire(key.clone(), registries.clone(), None, None);
            tokio::pin!(acquisition);
            tokio::select! {
                _ = hook.entered.notified() => {},
                result = &mut acquisition => panic!("acquisition escaped pause: {result:?}"),
            }
        }
        hook.release.notify_one();
        assert!(store.acquire(key.clone(), registries.clone(), None, None).await.is_err());
        assert!(matches!(store.acquiring.get(&key), Some(AcquiringLease::Failed(_))));
        let (bash_generation, terminal_generation, browser_generation) = match store.acquiring.get(&key).unwrap() {
            AcquiringLease::Failed(failure) => (
                failure.bash.generation(), failure.terminal.generation(), failure.browser.generation(),
            ),
            AcquiringLease::Pending(_) => unreachable!(),
        };
        let resources = store.acquire(key.clone(), registries.clone(), None, None).await.unwrap();
        let lease = store.complete.get(&key).unwrap();
        assert_ne!(lease.bash.generation(), bash_generation,
            "final retirement is a new permit generation, not the preliminary fence");
        assert_eq!(lease.terminal.generation(), terminal_generation);
        assert_eq!(lease.browser.generation(), browser_generation);
        assert!(!lease.tmux.had_entry());
        assert!(resources.is_empty());
        store.cancel_attempt(&key.0, registries.clone()).await.unwrap();
        assert_admission(&registries, &key, false).await;
        assert!(store.reopening.is_empty());
    }

    #[tokio::test]
    async fn cancelled_tmux_retry_keeps_original_partial_custody() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let key = key("attempt", "scope");
        let acquisition = acquisition_hook(true);
        store.acquisition_hook = Some(acquisition.clone());
        {
            let future = store.acquire(key.clone(), registries.clone(), None, None);
            tokio::pin!(future);
            tokio::select! {
                _ = acquisition.entered.notified() => {},
                result = &mut future => panic!("acquisition escaped pause: {result:?}"),
            }
        }
        acquisition.release.notify_one();
        assert!(store.acquire(key.clone(), registries.clone(), None, None).await.is_err());
        let original_generations = match store.acquiring.get(&key).unwrap() {
            AcquiringLease::Failed(failure) => (
                failure.bash.generation(), failure.terminal.generation(), failure.browser.generation(),
            ),
            AcquiringLease::Pending(_) => unreachable!(),
        };
        let retry = RetryHook { entered: Arc::new(Notify::new()), release: Arc::new(Notify::new()) };
        store.retry_hook = Some(retry.clone());
        {
            let future = store.finish_acquisition(&key);
            tokio::pin!(future);
            tokio::select! {
                _ = retry.entered.notified() => {},
                result = &mut future => panic!("retry escaped pause: {result:?}"),
            }
        }
        assert!(matches!(store.acquiring.get(&key), Some(AcquiringLease::Pending(_))));
        assert_admission(&registries, &key, true).await;
        retry.release.notify_one();
        assert!(store.finish_acquisition(&key).await.is_ok());
        let lease = store.complete.get(&key).unwrap();
        assert_eq!(lease.terminal.generation(), original_generations.1);
        assert_eq!(lease.browser.generation(), original_generations.2);
        assert_ne!(lease.bash.generation(), original_generations.0);
        store.cancel_attempt(&key.0, registries.clone()).await.unwrap();
        assert_admission(&registries, &key, false).await;
    }

    #[tokio::test]
    async fn unresolved_tmux_retry_keeps_original_partial_custody_and_deadline() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let key = key("attempt", "scope");
        let hook = acquisition_hook(true);
        store.acquisition_hook = Some(hook.clone());
        {
            let acquisition = store.acquire(key.clone(), registries.clone(), None, None);
            tokio::pin!(acquisition);
            tokio::select! {
                _ = hook.entered.notified() => {},
                result = &mut acquisition => panic!("acquisition escaped pause: {result:?}"),
            }
        }
        hook.release.notify_one();
        assert!(store.acquire(key.clone(), registries.clone(), None, None).await.is_err());
        let (bash_generation, terminal_generation, browser_generation) = match store.acquiring.get_mut(&key).unwrap() {
            AcquiringLease::Failed(failure) => {
                failure.expires = tokio::time::Instant::now();
                (failure.bash.generation(), failure.terminal.generation(), failure.browser.generation())
            }
            AcquiringLease::Pending(_) => unreachable!(),
        };
        assert!(store.cancel_attempt(&key.0, registries.clone()).await.is_err());
        match store.acquiring.get(&key).unwrap() {
            AcquiringLease::Failed(failure) => {
                assert_eq!(failure.bash.generation(), bash_generation);
                assert_eq!(failure.terminal.generation(), terminal_generation);
                assert_eq!(failure.browser.generation(), browser_generation);
                assert!(failure.detail().contains("tmux reopening authority unresolved"));
            }
            AcquiringLease::Pending(_) => unreachable!(),
        }
        assert!(store.complete.is_empty());
        assert_admission(&registries, &key, true).await;
        assert!(store.reopening.is_empty());
    }

    #[tokio::test]
    async fn multi_scope_acquisition_failure_keeps_prior_lease_and_blocks_batch_reopening() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let first = key("attempt", "first");
        let second = key("attempt", "second");
        store.acquire(first.clone(), registries.clone(), None, None).await.unwrap();
        let original = generations(store.complete.get(&first).unwrap());
        let hook = acquisition_hook(true);
        hook.release.notify_one();
        store.acquisition_hook = Some(hook);
        assert!(store.acquire(second.clone(), registries.clone(), None, None).await.is_err());
        match store.acquiring.get_mut(&second).unwrap() {
            AcquiringLease::Failed(failure) => failure.expires = tokio::time::Instant::now(),
            AcquiringLease::Pending(_) => unreachable!(),
        }
        assert!(store.cancel_attempt(&first.0, registries.clone()).await.is_err());
        assert_eq!(generations(store.complete.get(&first).unwrap()), original);
        store.discard_completed(&first.0);
        assert_eq!(generations(store.complete.get(&first).unwrap()), original);
        assert_admission(&registries, &first, true).await;
        assert_admission(&registries, &second, true).await;
    }

    #[tokio::test]
    async fn multi_scope_reopen_error_returns_every_exact_permit() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let first = key("attempt", "first");
        let second = key("attempt", "second");
        for key in [&first, &second] {
            store.acquire(key.clone(), registries.clone(), None, None).await.unwrap();
        }
        let originals = [generations(store.complete.get(&first).unwrap()),
            generations(store.complete.get(&second).unwrap())];
        let mut hook = reopening_hook(false);
        hook.fail_tmux_batch = true;
        store.reopening_hook = Some(hook);
        let error = store.cancel_attempt(&first.0, registries.clone()).await.unwrap_err();
        assert!(error.contains("injected tmux batch cancellation failure"));
        assert_eq!(store.complete.len(), 2);
        assert_eq!(generations(store.complete.get(&first).unwrap()), originals[0]);
        assert_eq!(generations(store.complete.get(&second).unwrap()), originals[1]);
        assert_admission(&registries, &first, true).await;
        assert_admission(&registries, &second, true).await;
        store.cancel_attempt(&first.0, registries.clone()).await.unwrap();
        assert!(store.complete.is_empty());
        assert_admission(&registries, &first, false).await;
        assert_admission(&registries, &second, false).await;
    }

    #[tokio::test]
    async fn dropped_reopen_before_tmux_keeps_entire_batch_and_resumes_it() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let first = key("attempt", "first");
        let second = key("attempt", "second");
        for key in [&first, &second] {
            store.acquire(key.clone(), registries.clone(), None, None).await.unwrap();
        }
        let hook = reopening_hook(true);
        store.reopening_hook = Some(hook.clone());
        {
            let reopening = store.cancel_attempt(&first.0, registries.clone());
            tokio::pin!(reopening);
            tokio::select! {
                _ = hook.entered.notified() => {},
                result = &mut reopening => panic!("reopening escaped pause: {result:?}"),
            }
        }
        assert!(store.is_reopening(&first.0));
        assert!(store.complete.is_empty());
        assert_admission(&registries, &first, true).await;
        assert_admission(&registries, &second, true).await;
        assert!(store.acquire(first.clone(), registries.clone(), None, None).await.is_err());
        hook.release.notify_one();
        store.cancel_attempt(&first.0, registries.clone()).await.unwrap();
        assert!(store.reopening.is_empty());
        assert_admission(&registries, &first, false).await;
        assert_admission(&registries, &second, false).await;
    }

    #[tokio::test]
    async fn dropped_reopen_after_partial_progress_resumes_browser_without_replaying_tmux() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let key = key("attempt", "scope");
        store.acquire(key.clone(), registries.clone(), None, None).await.unwrap();
        let hook = reopening_hook(false);
        store.reopening_hook = Some(hook.clone());
        {
            let reopening = store.cancel_attempt(&key.0, registries.clone());
            tokio::pin!(reopening);
            tokio::select! {
                _ = hook.entered.notified() => {},
                result = &mut reopening => panic!("reopening escaped pause: {result:?}"),
            }
        }
        let scope = ResourceScopeKey::Work(key.1.clone());
        assert!(!registries.tmux.is_retirement_fenced(&scope).await);
        assert!(!registries.terminal.is_retirement_fenced(&scope));
        assert!(browser_fenced(&registries, &scope).await);
        let newer = registries.tmux.begin_retirement(
            &scope, None, None, phoenix_tools::tmux::registry::close_deadline(),
        ).await.unwrap();
        hook.release.notify_one();
        store.cancel_attempt(&key.0, registries.clone()).await.unwrap();
        assert!(!browser_fenced(&registries, &scope).await);
        assert!(registries.tmux.is_retirement_fenced(&scope).await, "newer fence must survive");
        registries.tmux.cancel_retirement(newer).await.unwrap();
        assert!(store.complete.is_empty());
        assert!(store.reopening.is_empty());
    }
}
