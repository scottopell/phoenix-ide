use std::collections::HashMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use phoenix_tools::bash::registry::{BashHandleRegistry, BashTeardownFence};
use phoenix_tools::browser::BrowserSessionManager;
use phoenix_tools::tmux::registry::TmuxRegistry;

use std::path::PathBuf;

use crate::terminal::session::TerminalRetirementPermit;
#[cfg(test)]
use phoenix_core::domain::close::CloseRetirementSnapshot;
use phoenix_core::domain::close::{RetiredResourceIdentity, RetiredResourceKind};
use phoenix_core::work_scope::{ResourceScopeKey, WorkScopeId};
use phoenix_tools::browser::session::BrowserRetirementPermit;
use phoenix_tools::tmux::registry::{
    PersistentTmuxDiscovery, TmuxRetirementPermit, TmuxRetirementRehydration,
};

use super::{opaque_resource, CloseResourceLease};

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
        let tmux = match self
            .registries
            .acquire_tmux(
                &self.scope,
                self.legacy_worktree_path.clone(),
                self.legacy_conversation_id.clone(),
                self.expires,
            )
            .await
        {
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
            resources.push(opaque_resource(
                RetiredResourceKind::TmuxServer,
                tmux.instance.stable_identity(),
            ));
        }
        if let Some(instance) = &self.terminal.instance {
            resources.push(opaque_resource(
                RetiredResourceKind::PtySession,
                instance.stable_identity(),
            ));
        }
        for instance in &self.browser.instances {
            resources.push(opaque_resource(
                RetiredResourceKind::BrowserSession,
                instance.stable_identity(),
            ));
        }
        Ok(CloseResourceLease {
            bash,
            tmux,
            terminal: self.terminal,
            browser: self.browser,
            resources,
            capture_evidence: None,
            native_initial_entered: Vec::new(),
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
    #[cfg(test)]
    pub(super) terminal_entry_hook: Option<super::native_initial::resource::TerminalEntryTestHook>,
    #[cfg(test)]
    pub(super) bash_stage2_hook: Option<super::native_initial::bash::BashStage2TestHook>,
    // External process-epoch supervision is never owned by an actor waiter.
    pub(super) native_initial_supervisors:
        Vec<super::native_initial::resource::NativeInitialResourceRecord>,
    pub(crate) complete: HashMap<LeaseKey, CloseResourceLease>,
    // Native initial preparation owns the original affine lease outside the global
    // store lock. Keeping the reservation here prevents reopening while it lives.
    pub(super) native_initial_leases:
        HashMap<LeaseKey, Arc<tokio::sync::Mutex<CloseResourceLease>>>,
    acquiring: HashMap<LeaseKey, AcquiringLease>,
    reopening: HashMap<String, BoxFuture<'static, Reopening>>,
    #[cfg(test)]
    pub(super) acquisition_hook: Option<AcquisitionHook>,
    #[cfg(test)]
    pub(super) reopening_hook: Option<ReopeningHook>,
    #[cfg(test)]
    pub(super) retry_hook: Option<RetryHook>,
    #[cfg(test)]
    pub(super) completion_hook: Option<CompletionHook>,
}

impl CloseResourceLeaseStore {
    pub(super) fn reserve_native_initial_lease(
        &mut self,
        key: &LeaseKey,
    ) -> Result<Arc<tokio::sync::Mutex<CloseResourceLease>>, String> {
        if let Some(lease) = self.native_initial_leases.get(key) {
            return Ok(Arc::clone(lease));
        }
        let lease = self
            .complete
            .remove(key)
            .ok_or("native initial lease is missing")?;
        let lease = Arc::new(tokio::sync::Mutex::new(lease));
        self.native_initial_leases
            .insert(key.clone(), Arc::clone(&lease));
        Ok(lease)
    }

    /// Return only definitely inert, idle reservations to the ordinary lease
    /// store. Arc uniqueness proves no preparation still borrows a permit;
    /// retained resource state separately forbids reopening transferred owners,
    /// even if SQLite release or the physical outcome remains unknown.
    fn restore_unentered_native_initial_leases(&mut self, attempt: &str) -> Result<(), String> {
        let keys = self
            .native_initial_leases
            .keys()
            .filter(|(candidate, _)| candidate == attempt)
            .cloned()
            .collect::<Vec<_>>();
        for key in &keys {
            let lease = self
                .native_initial_leases
                .get(key)
                .expect("selected reservation");
            if Arc::strong_count(lease) != 1
                || self.native_initial_supervisors.iter().any(|record| {
                    record.expected.attempt_id.as_str() == attempt
                        && record.expected.scope == key.1
                        && record.retains_entry()
                })
            {
                return Err("native initial retirement still owns the original Close lease".into());
            }
        }
        for key in keys {
            let lease = self
                .native_initial_leases
                .remove(&key)
                .expect("validated reservation");
            // No await or external reference can intervene while this store is
            // exclusively held. Do not clone the affine permits to restore them.
            match Arc::try_unwrap(lease) {
                Ok(lease) => {
                    self.complete.insert(key, lease.into_inner());
                }
                Err(lease) => {
                    self.native_initial_leases.insert(key, lease);
                    return Err("native initial lease custody changed during restoration".into());
                }
            }
        }
        Ok(())
    }

    pub(super) fn bind_capture_evidence(
        &mut self,
        attempt: &str,
        scopes: &[super::native_initial::capture_binding::ScopeCaptureEvidence],
    ) -> Result<(), String> {
        if scopes.is_empty() {
            return Err("native capture has no scopes".into());
        }
        let mut bindings = Vec::with_capacity(scopes.len());
        for (index, scope) in scopes.iter().enumerate() {
            if scopes[..index]
                .iter()
                .any(|prior| prior.scope() == scope.scope())
            {
                return Err("duplicate native capture binding scope".into());
            }
            let key = (attempt.to_string(), scope.scope().clone());
            let lease = self
                .complete
                .get(&key)
                .ok_or("native initial lease was lost before binding")?;
            let binding = scope.bind(lease.browser.deadline());
            if lease
                .capture_evidence
                .as_ref()
                .is_some_and(|bound| bound != &binding)
            {
                return Err(
                    "native initial lease was bound to different evidence or deadline".into(),
                );
            }
            bindings.push((key, binding));
        }
        for (key, binding) in bindings {
            let lease = self
                .complete
                .get_mut(&key)
                .expect("validated original native initial lease");
            lease.capture_evidence = Some(binding);
        }
        Ok(())
    }

    pub(super) fn discard_completed(&mut self, attempt: &str) {
        if self.reopening.contains_key(attempt)
            || self
                .native_initial_leases
                .keys()
                .any(|(candidate, _)| candidate == attempt)
            || self
                .acquiring
                .keys()
                .any(|(candidate, _)| candidate == attempt)
        {
            return;
        }
        self.complete
            .retain(|(candidate, _), _| candidate != attempt);
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
        if self.native_initial_leases.contains_key(&key) {
            return Err(
                "Close resource lease is reserved by native initial retirement".to_string(),
            );
        }
        if let Some(lease) = self.complete.get(&key) {
            return Ok(lease.resources.clone());
        }
        if !self.acquiring.contains_key(&key) {
            let scope = key.1.clone();
            #[cfg(test)]
            let hook = self.acquisition_hook.take();
            let future = Box::pin(async move {
                registries
                    .acquire(
                        scope,
                        legacy_worktree_path,
                        legacy_conversation_id,
                        #[cfg(test)]
                        hook,
                    )
                    .await
            });
            self.acquiring
                .insert(key.clone(), AcquiringLease::Pending(future));
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
            self.acquiring.insert(
                key.clone(),
                AcquiringLease::Pending(Box::pin(async move {
                    failure
                        .retry_tmux(
                            #[cfg(test)]
                            retry_hook,
                        )
                        .await
                        .map_err(|(failure, _)| failure)
                })),
            );
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
                *self
                    .acquiring
                    .get_mut(key)
                    .expect("pending acquisition remains retained") =
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
        self.restore_unentered_native_initial_leases(attempt)?;
        if !self.reopening.contains_key(attempt) {
            let mut acquiring = self
                .acquiring
                .keys()
                .filter(|(candidate, _)| candidate == attempt)
                .cloned()
                .collect::<Vec<_>>();
            acquiring.sort();
            for key in acquiring {
                self.finish_acquisition(&key).await?;
            }
            let mut keys = self
                .complete
                .keys()
                .filter(|(candidate, _)| candidate == attempt)
                .cloned()
                .collect::<Vec<_>>();
            keys.sort();
            let leases = keys
                .into_iter()
                .map(|key| {
                    let lease = self.complete.remove(&key).expect("selected complete lease");
                    (key, lease)
                })
                .collect();
            #[cfg(test)]
            let hook = self.reopening_hook.take();
            self.reopening.insert(
                attempt.to_string(),
                Box::pin(async move {
                    registries
                        .reopen(
                            leases,
                            #[cfg(test)]
                            hook,
                        )
                        .await
                }),
            );
        }
        let outcome = self
            .reopening
            .get_mut(attempt)
            .expect("retained reopening")
            .as_mut()
            .await;
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
        let bash = self
            .bash
            .begin_teardown(&key)
            .await
            .expect("begin_teardown installs a fenced owner table");
        let terminal = self.terminal.begin_retirement_by(&key, expires);
        let browser = self
            .browser
            .begin_retirement_with_deadline(&key, expires)
            .await;
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
                &key,
                legacy_worktree_path.clone(),
                legacy_conversation_id.clone(),
                expires,
            )
            .await
        }
        .await;
        let tmux = match tmux {
            Ok(permit) => permit,
            Err(tmux_failure) => {
                return Err(FailedAcquisition {
                    bash,
                    terminal,
                    browser,
                    tmux_failure,
                    registries: self.clone(),
                    scope: key.clone(),
                    legacy_worktree_path,
                    legacy_conversation_id,
                    expires,
                })
            }
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
            resources.push(opaque_resource(
                RetiredResourceKind::PtySession,
                instance.stable_identity(),
            ));
        }
        for instance in &browser.instances {
            resources.push(opaque_resource(
                RetiredResourceKind::BrowserSession,
                instance.stable_identity(),
            ));
        }
        Ok(CloseResourceLease {
            bash,
            tmux,
            terminal,
            browser,
            resources,
            capture_evidence: None,
            native_initial_entered: Vec::new(),
        })
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

    async fn reopen(
        self,
        leases: Vec<(LeaseKey, CloseResourceLease)>,
        #[cfg(test)] hook: Option<ReopeningHook>,
    ) -> Reopening {
        let mut cancelling = Vec::with_capacity(leases.len());
        let mut permits = Vec::with_capacity(leases.len());
        for (key, lease) in leases {
            let CloseResourceLease {
                bash,
                tmux,
                terminal,
                browser,
                resources,
                capture_evidence,
                native_initial_entered,
            } = lease;
            cancelling.push((
                key,
                bash,
                terminal,
                browser,
                resources,
                capture_evidence,
                native_initial_entered,
            ));
            permits.push(tmux);
        }
        #[cfg(test)]
        if let Some(hook) = hook.as_ref().filter(|hook| hook.before_tmux) {
            hook.pause().await;
        }
        #[cfg(test)]
        if hook.as_ref().is_some_and(|hook| hook.fail_tmux_batch) {
            let leases = cancelling
                .into_iter()
                .zip(permits)
                .map(
                    |(
                        (
                            key,
                            bash,
                            terminal,
                            browser,
                            resources,
                            capture_evidence,
                            native_initial_entered,
                        ),
                        tmux,
                    )| {
                        (
                            key,
                            CloseResourceLease {
                                bash,
                                tmux,
                                terminal,
                                browser,
                                resources,
                                capture_evidence,
                                native_initial_entered,
                            },
                        )
                    },
                )
                .collect();
            return Err((
                "injected tmux batch cancellation failure".to_string(),
                leases,
            ));
        }
        if let Err(error) = self.tmux.cancel_retirement_batch(permits).await {
            let detail = error.to_string();
            let leases = cancelling
                .into_iter()
                .zip(error.into_permits())
                .map(
                    |(
                        (
                            key,
                            bash,
                            terminal,
                            browser,
                            resources,
                            capture_evidence,
                            native_initial_entered,
                        ),
                        tmux,
                    )| {
                        (
                            key,
                            CloseResourceLease {
                                bash,
                                tmux,
                                terminal,
                                browser,
                                resources,
                                capture_evidence,
                                native_initial_entered,
                            },
                        )
                    },
                )
                .collect();
            return Err((detail, leases));
        }
        for (_, bash, terminal, browser, _, _, _) in cancelling {
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

/// Pauses the live completion wrapper after it obtains the original lease.
#[cfg(test)]
#[derive(Clone)]
pub(super) struct CompletionHook {
    pub(super) entered: Arc<tokio::sync::Notify>,
    pub(super) release: Arc<tokio::sync::Notify>,
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
        tokio::time::timeout(std::time::Duration::from_secs(30), self.release.notified())
            .await
            .expect("reopening hook was not released");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_core::work_scope::{EffectiveResourceAccess, ResourceAuthority};
    use phoenix_tools::browser::BrowserError;
    use phoenix_tools::tmux::fake_backend::FakeTmuxBackend;
    use tokio::sync::Notify;

    async fn tmux_fenced(registries: &LeaseRegistries, scope: &ResourceScopeKey) -> bool {
        match registries
            .tmux
            .ensure_live(
                scope,
                std::path::Path::new("/close-lease-custody-test"),
                None,
                None,
            )
            .await
        {
            Err(phoenix_tools::tmux::registry::TmuxError::RetirementFenced { .. }) => true,
            Err(phoenix_tools::tmux::registry::TmuxError::BinaryUnavailable) => false,
            other => panic!("unexpected fake tmux admission result: {other:?}"),
        }
    }

    fn terminal_fenced(registries: &LeaseRegistries, scope: &ResourceScopeKey) -> bool {
        match registries.terminal.reserve_spawn(scope) {
            Err(phoenix_terminal::session::ActiveTerminalInsertError::RetirementFenced) => true,
            Ok(()) => {
                registries.terminal.release_spawn(scope);
                false
            }
            other => panic!("unexpected empty terminal admission result: {other:?}"),
        }
    }

    async fn browser_fenced(registries: &LeaseRegistries, scope: &ResourceScopeKey) -> bool {
        matches!(
            registries
                .browser
                .get_existing_for_actor(
                    scope,
                    &EffectiveResourceAccess::new("custody-test", ResourceAuthority::Work),
                )
                .await,
            Err(BrowserError::RetirementFenced { .. })
        )
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
        format!(
            "{:?}/{:?}/{:?}/{:?}",
            lease.bash.generation(),
            lease.tmux.generation(),
            lease.terminal.generation(),
            lease.browser.generation()
        )
    }

    async fn assert_admission(registries: &LeaseRegistries, key: &LeaseKey, fenced: bool) {
        let scope = ResourceScopeKey::Work(key.1.clone());
        assert_eq!(tmux_fenced(registries, &scope).await, fenced);
        assert_eq!(terminal_fenced(registries, &scope), fenced);
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
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                tokio::select! {
                    _ = hook.entered.notified() => {},
                    result = &mut acquisition => panic!("acquisition escaped pause: {result:?}"),
                }
            })
            .await
            .expect("acquisition never reached its pause");
        }
        assert!(matches!(
            store.acquiring.get(&key),
            Some(AcquiringLease::Pending(_))
        ));
        assert!(store.complete.is_empty());
        assert!(terminal_fenced(
            &registries,
            &ResourceScopeKey::Work(key.1.clone())
        ));
        assert!(browser_fenced(&registries, &ResourceScopeKey::Work(key.1.clone())).await);
        hook.release.notify_one();
        let resources = store
            .acquire(key.clone(), registries.clone(), None, None)
            .await
            .unwrap();
        let original = generations(store.complete.get(&key).unwrap());
        let unused_hook = acquisition_hook(true);
        store.acquisition_hook = Some(unused_hook);
        assert_eq!(
            store
                .acquire(key.clone(), registries.clone(), None, None)
                .await
                .unwrap(),
            resources
        );
        assert_eq!(generations(store.complete.get(&key).unwrap()), original);
        assert!(
            store.acquisition_hook.is_some(),
            "reuse must not start acquisition"
        );
        assert!(store.acquiring.is_empty());
        store
            .cancel_attempt(&key.0, registries.clone())
            .await
            .unwrap();
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
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                tokio::select! {
                    _ = hook.entered.notified() => {},
                    result = &mut acquisition => panic!("acquisition escaped pause: {result:?}"),
                }
            })
            .await
            .expect("acquisition never reached its pause");
        }
        hook.release.notify_one();
        assert!(store
            .acquire(key.clone(), registries.clone(), None, None)
            .await
            .is_err());
        assert!(matches!(
            store.acquiring.get(&key),
            Some(AcquiringLease::Failed(_))
        ));
        let (bash_generation, terminal_generation, browser_generation) =
            match store.acquiring.get(&key).unwrap() {
                AcquiringLease::Failed(failure) => (
                    failure.bash.generation(),
                    failure.terminal.generation(),
                    failure.browser.generation(),
                ),
                AcquiringLease::Pending(_) => unreachable!(),
            };
        let resources = store
            .acquire(key.clone(), registries.clone(), None, None)
            .await
            .unwrap();
        let lease = store.complete.get(&key).unwrap();
        assert_ne!(
            lease.bash.generation(),
            bash_generation,
            "final retirement is a new permit generation, not the preliminary fence"
        );
        assert_eq!(lease.terminal.generation(), terminal_generation);
        assert_eq!(lease.browser.generation(), browser_generation);
        // The failed acquisition installed a real admission-fence entry. Retry
        // must obtain its permit through discovery, not pretend that entry vanished.
        assert!(lease.tmux.had_entry());
        assert_eq!(
            resources,
            vec![opaque_resource(
                RetiredResourceKind::TmuxServer,
                lease.tmux.instance.stable_identity(),
            )]
        );
        store
            .cancel_attempt(&key.0, registries.clone())
            .await
            .unwrap();
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
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                tokio::select! {
                    _ = acquisition.entered.notified() => {},
                    result = &mut future => panic!("acquisition escaped pause: {result:?}"),
                }
            })
            .await
            .expect("acquisition never reached its pause");
        }
        acquisition.release.notify_one();
        assert!(store
            .acquire(key.clone(), registries.clone(), None, None)
            .await
            .is_err());
        let original_generations = match store.acquiring.get(&key).unwrap() {
            AcquiringLease::Failed(failure) => (
                failure.bash.generation(),
                failure.terminal.generation(),
                failure.browser.generation(),
            ),
            AcquiringLease::Pending(_) => unreachable!(),
        };
        let retry = RetryHook {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        };
        store.retry_hook = Some(retry.clone());
        {
            let future = store.finish_acquisition(&key);
            tokio::pin!(future);
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                tokio::select! {
                    _ = retry.entered.notified() => {},
                    result = &mut future => panic!("retry escaped pause: {result:?}"),
                }
            })
            .await
            .expect("acquisition retry never reached its pause");
        }
        assert!(matches!(
            store.acquiring.get(&key),
            Some(AcquiringLease::Pending(_))
        ));
        assert_admission(&registries, &key, true).await;
        retry.release.notify_one();
        assert!(store.finish_acquisition(&key).await.is_ok());
        let lease = store.complete.get(&key).unwrap();
        assert_eq!(lease.terminal.generation(), original_generations.1);
        assert_eq!(lease.browser.generation(), original_generations.2);
        assert_ne!(lease.bash.generation(), original_generations.0);
        store
            .cancel_attempt(&key.0, registries.clone())
            .await
            .unwrap();
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
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                tokio::select! {
                    _ = hook.entered.notified() => {},
                    result = &mut acquisition => panic!("acquisition escaped pause: {result:?}"),
                }
            })
            .await
            .expect("acquisition never reached its pause");
        }
        hook.release.notify_one();
        assert!(store
            .acquire(key.clone(), registries.clone(), None, None)
            .await
            .is_err());
        let (bash_generation, terminal_generation, browser_generation) =
            match store.acquiring.get_mut(&key).unwrap() {
                AcquiringLease::Failed(failure) => {
                    failure.expires = tokio::time::Instant::now();
                    (
                        failure.bash.generation(),
                        failure.terminal.generation(),
                        failure.browser.generation(),
                    )
                }
                AcquiringLease::Pending(_) => unreachable!(),
            };
        assert!(store
            .cancel_attempt(&key.0, registries.clone())
            .await
            .is_err());
        match store.acquiring.get(&key).unwrap() {
            AcquiringLease::Failed(failure) => {
                assert_eq!(failure.bash.generation(), bash_generation);
                assert_eq!(failure.terminal.generation(), terminal_generation);
                assert_eq!(failure.browser.generation(), browser_generation);
                assert!(failure
                    .detail()
                    .contains("tmux reopening authority unresolved"));
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
        store
            .acquire(first.clone(), registries.clone(), None, None)
            .await
            .unwrap();
        let original = generations(store.complete.get(&first).unwrap());
        let hook = acquisition_hook(true);
        hook.release.notify_one();
        store.acquisition_hook = Some(hook);
        assert!(store
            .acquire(second.clone(), registries.clone(), None, None)
            .await
            .is_err());
        match store.acquiring.get_mut(&second).unwrap() {
            AcquiringLease::Failed(failure) => failure.expires = tokio::time::Instant::now(),
            AcquiringLease::Pending(_) => unreachable!(),
        }
        assert!(store
            .cancel_attempt(&first.0, registries.clone())
            .await
            .is_err());
        assert_eq!(generations(store.complete.get(&first).unwrap()), original);
        store.discard_completed(&first.0);
        assert_eq!(generations(store.complete.get(&first).unwrap()), original);
        assert_admission(&registries, &first, true).await;
        assert_admission(&registries, &second, true).await;
    }

    #[tokio::test]
    async fn capture_binding_batch_is_atomic_when_a_later_scope_is_missing() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let first = key("attempt", "first");
        let second = key("attempt", "second");
        store
            .acquire(first.clone(), registries.clone(), None, None)
            .await
            .unwrap();
        let snapshot = CloseRetirementSnapshot::parse("original", "fingerprint").unwrap();
        let scopes = [first.1.clone(), second.1.clone()].map(|scope| {
            super::super::native_initial::capture_binding::test_scope(scope, snapshot.clone())
        });

        let error = store.bind_capture_evidence(&first.0, &scopes).unwrap_err();

        assert!(error.contains("lease was lost before binding"));
        assert!(store
            .complete
            .get(&first)
            .unwrap()
            .capture_evidence
            .is_none());
    }

    #[tokio::test]
    async fn capture_binding_batch_is_atomic_when_a_later_binding_conflicts() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let first = key("attempt", "first");
        let second = key("attempt", "second");
        for scope_key in [&first, &second] {
            store
                .acquire(scope_key.clone(), registries.clone(), None, None)
                .await
                .unwrap();
        }
        let original = CloseRetirementSnapshot::parse("original", "fingerprint").unwrap();
        let changed = CloseRetirementSnapshot::parse("changed", "fingerprint").unwrap();
        let original_scopes = [first.1.clone(), second.1.clone()].map(|scope| {
            super::super::native_initial::capture_binding::test_scope(scope, original.clone())
        });
        let existing = [super::super::native_initial::capture_binding::test_scope(
            second.1.clone(),
            changed,
        )];
        store.bind_capture_evidence(&first.0, &existing).unwrap();
        let previous = store
            .complete
            .get(&second)
            .unwrap()
            .capture_evidence
            .clone();

        let error = store
            .bind_capture_evidence(&first.0, &original_scopes)
            .unwrap_err();

        assert!(error.contains("bound to different evidence or deadline"));
        assert!(store
            .complete
            .get(&first)
            .unwrap()
            .capture_evidence
            .is_none());
        assert_eq!(
            store.complete.get(&second).unwrap().capture_evidence,
            previous
        );
    }

    #[tokio::test]
    async fn exact_capture_binding_repeat_preserves_original_deadline_and_evidence() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let lease_key = key("attempt", "scope");
        store
            .acquire(lease_key.clone(), registries, None, None)
            .await
            .unwrap();
        let snapshot = CloseRetirementSnapshot::parse("original", "fingerprint").unwrap();
        let scopes = [super::super::native_initial::capture_binding::test_scope(
            lease_key.1.clone(),
            snapshot.clone(),
        )];
        let original_deadline = store.complete.get(&lease_key).unwrap().browser.deadline();

        store.bind_capture_evidence(&lease_key.0, &scopes).unwrap();
        let original_evidence = store
            .complete
            .get(&lease_key)
            .unwrap()
            .capture_evidence
            .clone();
        store.bind_capture_evidence(&lease_key.0, &scopes).unwrap();

        let lease = store.complete.get(&lease_key).unwrap();
        assert_eq!(lease.browser.deadline(), original_deadline);
        assert_eq!(lease.capture_evidence, original_evidence);
        assert_eq!(
            lease.capture_evidence.as_ref().unwrap().deadline(),
            original_deadline
        );
        assert!(lease
            .capture_evidence
            .as_ref()
            .unwrap()
            .verifies_request(&snapshot));
    }

    #[tokio::test]
    async fn changed_capture_binding_is_rejected_without_replacing_original_evidence() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let lease_key = key("attempt", "scope");
        store
            .acquire(lease_key.clone(), registries, None, None)
            .await
            .unwrap();
        let original = CloseRetirementSnapshot::parse("original", "fingerprint").unwrap();
        let changed = CloseRetirementSnapshot::parse("changed", "fingerprint").unwrap();
        let original_scope = [super::super::native_initial::capture_binding::test_scope(
            lease_key.1.clone(),
            original.clone(),
        )];
        let changed_scope = [super::super::native_initial::capture_binding::test_scope(
            lease_key.1.clone(),
            changed,
        )];
        store
            .bind_capture_evidence(&lease_key.0, &original_scope)
            .unwrap();
        let original_evidence = store
            .complete
            .get(&lease_key)
            .unwrap()
            .capture_evidence
            .clone();

        let error = store
            .bind_capture_evidence(&lease_key.0, &changed_scope)
            .unwrap_err();

        assert!(error.contains("bound to different evidence or deadline"));
        assert_eq!(
            store.complete.get(&lease_key).unwrap().capture_evidence,
            original_evidence
        );
        assert!(store
            .complete
            .get(&lease_key)
            .unwrap()
            .capture_evidence
            .as_ref()
            .unwrap()
            .verifies_request(&original));
    }

    #[tokio::test]
    async fn cancellation_before_native_entry_preserves_bound_capture_evidence() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let lease_key = key("attempt", "scope");
        store
            .acquire(lease_key.clone(), registries.clone(), None, None)
            .await
            .unwrap();
        let snapshot = CloseRetirementSnapshot::parse("original", "fingerprint").unwrap();
        let scopes = [super::super::native_initial::capture_binding::test_scope(
            lease_key.1.clone(),
            snapshot.clone(),
        )];
        store.bind_capture_evidence(&lease_key.0, &scopes).unwrap();
        let original_evidence = store
            .complete
            .get(&lease_key)
            .unwrap()
            .capture_evidence
            .clone();
        let reservation = store.reserve_native_initial_lease(&lease_key).unwrap();
        drop(reservation);
        let mut hook = reopening_hook(false);
        hook.fail_tmux_batch = true;
        store.reopening_hook = Some(hook);

        let error = store
            .cancel_attempt(&lease_key.0, registries)
            .await
            .unwrap_err();

        assert!(error.contains("injected tmux batch cancellation failure"));
        let restored = store
            .complete
            .get(&lease_key)
            .expect("cancel failure retains lease");
        assert!(restored.native_initial_entered.is_empty());
        assert_eq!(restored.capture_evidence, original_evidence);
        assert!(restored
            .capture_evidence
            .as_ref()
            .unwrap()
            .verifies_request(&snapshot));
        assert_eq!(
            restored.capture_evidence.as_ref().unwrap().deadline(),
            restored.browser.deadline()
        );
    }

    #[tokio::test]
    async fn multi_scope_reopen_error_returns_every_exact_permit() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let first = key("attempt", "first");
        let second = key("attempt", "second");
        for key in [&first, &second] {
            store
                .acquire(key.clone(), registries.clone(), None, None)
                .await
                .unwrap();
        }
        let snapshot = CloseRetirementSnapshot::parse("original", "fingerprint").unwrap();
        let scopes = [first.1.clone(), second.1.clone()].map(|scope| {
            super::super::native_initial::capture_binding::test_scope(scope, snapshot.clone())
        });
        store.bind_capture_evidence(&first.0, &scopes).unwrap();
        let original_binding = store.complete.get(&first).unwrap().capture_evidence.clone();
        store
            .complete
            .get_mut(&first)
            .unwrap()
            .native_initial_entered
            .push(RetiredResourceKind::TmuxServer);
        let originals = [
            generations(store.complete.get(&first).unwrap()),
            generations(store.complete.get(&second).unwrap()),
        ];
        let mut hook = reopening_hook(false);
        hook.fail_tmux_batch = true;
        store.reopening_hook = Some(hook);
        let error = store
            .cancel_attempt(&first.0, registries.clone())
            .await
            .unwrap_err();
        assert!(error.contains("injected tmux batch cancellation failure"));
        assert_eq!(store.complete.len(), 2);
        let retained = store.complete.get(&first).unwrap();
        assert_eq!(retained.capture_evidence, original_binding);
        assert!(retained
            .capture_evidence
            .as_ref()
            .unwrap()
            .verifies_request(&snapshot));
        assert_eq!(
            retained.capture_evidence.as_ref().unwrap().deadline(),
            retained.browser.deadline()
        );
        assert_eq!(
            retained.native_initial_entered,
            [RetiredResourceKind::TmuxServer]
        );
        assert_eq!(
            generations(store.complete.get(&first).unwrap()),
            originals[0]
        );
        assert_eq!(
            generations(store.complete.get(&second).unwrap()),
            originals[1]
        );
        assert_admission(&registries, &first, true).await;
        assert_admission(&registries, &second, true).await;
        store
            .cancel_attempt(&first.0, registries.clone())
            .await
            .unwrap();
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
            store
                .acquire(key.clone(), registries.clone(), None, None)
                .await
                .unwrap();
        }
        let hook = reopening_hook(true);
        store.reopening_hook = Some(hook.clone());
        {
            let reopening = store.cancel_attempt(&first.0, registries.clone());
            tokio::pin!(reopening);
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                tokio::select! {
                    _ = hook.entered.notified() => {},
                    result = &mut reopening => panic!("reopening escaped pause: {result:?}"),
                }
            })
            .await
            .expect("reopening never reached its pause");
        }
        assert!(store.is_reopening(&first.0));
        assert!(store.complete.is_empty());
        assert_admission(&registries, &first, true).await;
        assert_admission(&registries, &second, true).await;
        assert!(store
            .acquire(first.clone(), registries.clone(), None, None)
            .await
            .is_err());
        hook.release.notify_one();
        store
            .cancel_attempt(&first.0, registries.clone())
            .await
            .unwrap();
        assert!(store.reopening.is_empty());
        assert_admission(&registries, &first, false).await;
        assert_admission(&registries, &second, false).await;
    }

    #[tokio::test]
    async fn dropped_reopen_after_partial_progress_resumes_browser_without_replaying_tmux() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let key = key("attempt", "scope");
        store
            .acquire(key.clone(), registries.clone(), None, None)
            .await
            .unwrap();
        let hook = reopening_hook(false);
        store.reopening_hook = Some(hook.clone());
        {
            let reopening = store.cancel_attempt(&key.0, registries.clone());
            tokio::pin!(reopening);
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                tokio::select! {
                    _ = hook.entered.notified() => {},
                    result = &mut reopening => panic!("reopening escaped pause: {result:?}"),
                }
            })
            .await
            .expect("reopening never reached its pause");
        }
        let scope = ResourceScopeKey::Work(key.1.clone());
        assert!(!tmux_fenced(&registries, &scope).await);
        assert!(!terminal_fenced(&registries, &scope));
        assert!(browser_fenced(&registries, &scope).await);
        let newer = registries
            .tmux
            .begin_retirement(
                &scope,
                None,
                None,
                phoenix_tools::tmux::registry::close_deadline(),
            )
            .await
            .unwrap();
        hook.release.notify_one();
        store
            .cancel_attempt(&key.0, registries.clone())
            .await
            .unwrap();
        assert!(!browser_fenced(&registries, &scope).await);
        assert!(
            tmux_fenced(&registries, &scope).await,
            "newer fence must survive"
        );
        registries.tmux.cancel_retirement(newer).await.unwrap();
        assert!(store.complete.is_empty());
        assert!(store.reopening.is_empty());
    }

    #[tokio::test]
    async fn idle_unentered_reservation_restores_the_original_affine_lease() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let key = key("attempt", "scope");
        store
            .acquire(key.clone(), registries.clone(), None, None)
            .await
            .unwrap();
        let original_resources = store.complete.get(&key).unwrap().resources.clone();
        let original_generations = generations(store.complete.get(&key).unwrap());

        let reservation = store.reserve_native_initial_lease(&key).unwrap();
        assert!(!store.complete.contains_key(&key));
        drop(reservation);

        store
            .restore_unentered_native_initial_leases(&key.0)
            .unwrap();
        let restored = store.complete.get(&key).expect("original lease restored");
        assert_eq!(generations(restored), original_generations);
        assert_eq!(restored.resources, original_resources);
        assert!(!store.native_initial_leases.contains_key(&key));

        // Reserving again returns the same permit generations, not replacement permits.
        let reserved_again = store.reserve_native_initial_lease(&key).unwrap();
        let reserved_again = reserved_again.lock().await;
        assert_eq!(generations(&reserved_again), original_generations);
    }

    #[tokio::test]
    async fn restoration_refuses_an_outstanding_native_initial_lease_borrower() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let key = key("attempt", "scope");
        store
            .acquire(key.clone(), registries, None, None)
            .await
            .unwrap();
        let original_generations = generations(store.complete.get(&key).unwrap());
        let reservation = store.reserve_native_initial_lease(&key).unwrap();
        let outstanding_borrower = Arc::clone(&reservation);

        let error = store
            .restore_unentered_native_initial_leases(&key.0)
            .unwrap_err();
        assert!(error.contains("still owns the original Close lease"));
        assert!(!store.complete.contains_key(&key));
        assert!(store.native_initial_leases.contains_key(&key));
        let borrowed_lease = reservation.lock().await;
        assert_eq!(generations(&borrowed_lease), original_generations);
        drop(borrowed_lease);

        drop(outstanding_borrower);
        drop(reservation);
        store
            .restore_unentered_native_initial_leases(&key.0)
            .unwrap();
        assert_eq!(
            generations(store.complete.get(&key).unwrap()),
            original_generations
        );
    }

    #[tokio::test]
    async fn native_initial_reservation_does_not_block_unrelated_lease_acquisition() {
        let registries = registries();
        let mut store = CloseResourceLeaseStore::default();
        let reserved = key("attempt", "reserved-scope");
        let unrelated_attempt = key("other-attempt", "other-attempt-scope");
        let unrelated_scope = key("attempt", "other-scope");
        store
            .acquire(reserved.clone(), registries.clone(), None, None)
            .await
            .unwrap();
        let reservation = store.reserve_native_initial_lease(&reserved).unwrap();
        // These keys have never acquired permits: exercise new acquisition,
        // not the fast path that returns an existing complete lease.
        assert!(!store.complete.contains_key(&unrelated_attempt));
        assert!(!store.complete.contains_key(&unrelated_scope));

        assert!(store
            .acquire(reserved.clone(), registries.clone(), None, None)
            .await
            .is_err());
        assert!(store
            .acquire(unrelated_attempt.clone(), registries.clone(), None, None)
            .await
            .is_ok());
        assert!(store
            .acquire(unrelated_scope.clone(), registries, None, None)
            .await
            .is_ok());
        assert!(store.complete.contains_key(&unrelated_attempt));
        assert!(store.complete.contains_key(&unrelated_scope));
        drop(reservation);
    }
}
