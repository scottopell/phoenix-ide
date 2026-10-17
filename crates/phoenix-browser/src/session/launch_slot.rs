use super::*;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::process_group::OwnedChromeGroup;

#[derive(Clone, Copy, Default)]
pub(super) enum ProfilePolicy {
    #[default]
    StableScope,
    CliTransient,
}

impl ProfilePolicy {
    fn path(self, root: &Path, key: &str, reservation: &str) -> PathBuf {
        match self {
            Self::StableScope => user_data_dir_for_key(root, key),
            Self::CliTransient => root.join(format!("phoenix-browser-cli-{reservation}")),
        }
    }
}

pub(super) struct LaunchSlot {
    pub(super) identity: BrowserLaunchIdentity,
    pub(super) profile: PathBuf,
    key: String,
    state: RwLock<LaunchState>,
    changed: Notify,
    driver: StdMutex<Option<JoinHandle<()>>>,
    #[cfg(test)]
    fault: StdMutex<Option<Arc<tests::LaunchFault>>>,
}

enum LaunchState {
    #[cfg(test)]
    InjectedPending(tests::InjectedPending),
    Reserved,
    PreSpawnFailed(String),
    UnobservableAttempt(String),
    Pending(Box<PendingBrowser>),
    Ready(Arc<RwLock<BrowserSession>>),
    Stopped,
}

struct PendingBrowser {
    browser: Browser,
    handler: JoinHandle<()>,
    authority: InitialAuthority,
    identity: BrowserLaunchIdentity,
    marker: bool,
    failure: Option<String>,
}

enum InitialAuthority {
    Unadopted,
    RootOnly,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Group(OwnedChromeGroup),
}

impl LaunchSlot {
    pub(super) fn reserve(root: &Path, key: &str, policy: ProfilePolicy) -> Arc<Self> {
        let identity = BrowserLaunchIdentity {
            launch_uuid: uuid::Uuid::new_v4().to_string(),
            chrome_process: None,
        };
        Arc::new(Self {
            profile: policy.path(root, key, &identity.launch_uuid),
            identity,
            key: key.to_owned(),
            state: RwLock::new(LaunchState::Reserved),
            changed: Notify::new(),
            driver: StdMutex::new(None),
            #[cfg(test)]
            fault: StdMutex::new(None),
        })
    }

    #[cfg(test)]
    pub(super) fn inject(&self, fault: Option<Arc<tests::LaunchFault>>) {
        *self.fault.lock().unwrap() = fault;
    }

    pub(super) fn start(
        self: &Arc<Self>,
        registry: Arc<RwLock<BrowserSessionRegistryState>>,
        sink: Option<BrowserSessionLifecycleSink>,
    ) {
        let slot = self.clone();
        let driver = tokio::spawn(async move {
            let mut state = slot.state.write().await;
            if let Err(error) = slot.initialize(&mut state).await {
                if let LaunchState::Pending(pending) = &mut *state {
                    pending.failure = Some(error.to_string());
                }
                if matches!(&*state, LaunchState::Reserved) {
                    *state = LaunchState::PreSpawnFailed(error.to_string());
                }
                if let LaunchState::UnobservableAttempt(reason) = &mut *state {
                    *reason = error.to_string();
                }
                tracing::warn!(%error, profile = %slot.profile.display(), "browser initialization failed; retaining launch slot");
            }
            if matches!(&*state, LaunchState::Ready(_)) {
                let registry = registry.read().await;
                if let Some((key, entry)) = registry
                    .sessions
                    .iter()
                    .find(|(_, entry)| Arc::ptr_eq(&entry.session, &slot))
                {
                    if BrowserSessionManager::scope_is_fenced(&registry, &entry.scope).is_none()
                        && !registry
                            .retirements
                            .get(&BrowserRetirementFenceKey::Session(key.clone()))
                            .is_some_and(|retirement| retirement.fenced)
                        && entry
                            .current_kill
                            .lock()
                            .is_ok_and(|attempt| attempt.is_none())
                    {
                        if let Some(sink) = &sink {
                            let event = BrowserSessionLifecycleEvent {
                                work_scope: entry.scope.clone(),
                                audience: entry.audience(),
                                kind: BrowserSessionLifecycleKind::Active,
                            };
                            if let Err(error) = sink.send(event) {
                                tracing::debug!(%error, "browser lifecycle sink closed");
                            }
                        }
                    }
                }
            }
            drop(state);
            slot.changed.notify_waiters();
        });
        *self
            .driver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(driver);
    }

    pub(super) async fn lookup(&self) -> BrowserSessionLookup {
        let state = self.state.read().await;
        let reason = match &*state {
            LaunchState::Ready(session) => return BrowserSessionLookup::Ready(session.clone()),
            LaunchState::Reserved => "browser initialization is reserved".to_owned(),
            LaunchState::PreSpawnFailed(reason) | LaunchState::UnobservableAttempt(reason) => {
                reason.clone()
            }
            LaunchState::Pending(pending) => pending
                .failure
                .clone()
                .unwrap_or_else(|| "partial browser launch retained".into()),
            LaunchState::Stopped => "stopped launch awaits registry retirement".to_owned(),
            #[cfg(test)]
            LaunchState::InjectedPending(_) => "injected partial launch retained".into(),
        };
        BrowserSessionLookup::Retained(BrowserRetainedLaunch {
            reason,
            browser_lifetime: self.identity.launch_uuid.clone(),
            profile_path: self.profile.clone(),
        })
    }

    pub(super) fn observed_identity(&self) -> BrowserLaunchIdentity {
        self.state.try_read().ok().map_or_else(
            || self.identity.clone(),
            |state| match &*state {
                #[cfg(test)]
                LaunchState::InjectedPending(_) => self.identity.clone(),
                LaunchState::Pending(pending) => pending.identity.clone(),
                LaunchState::Ready(session) => session.try_read().map_or_else(
                    |_| self.identity.clone(),
                    |session| session.launch_identity.clone(),
                ),
                LaunchState::Reserved
                | LaunchState::PreSpawnFailed(_)
                | LaunchState::UnobservableAttempt(_)
                | LaunchState::Stopped => self.identity.clone(),
            },
        )
    }

    pub(super) fn ready(&self) -> Option<Arc<RwLock<BrowserSession>>> {
        self.state.try_read().ok().and_then(|state| match &*state {
            #[cfg(test)]
            LaunchState::InjectedPending(_) => None,
            LaunchState::Ready(session) => Some(session.clone()),
            LaunchState::Reserved
            | LaunchState::PreSpawnFailed(_)
            | LaunchState::Pending(_)
            | LaunchState::UnobservableAttempt(_)
            | LaunchState::Stopped => None,
        })
    }

    pub(super) async fn wait_ready(&self) -> Result<Arc<RwLock<BrowserSession>>, BrowserError> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            match &*self.state.read().await {
                #[cfg(test)]
                LaunchState::InjectedPending(_) => return Err(BrowserError::LaunchFailed("injected partial launch retained".into())),
                LaunchState::Ready(session) => return Ok(session.clone()),
                LaunchState::Reserved => {
                    if self.driver.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().is_some_and(JoinHandle::is_finished) { return Err(BrowserError::LaunchFailed("launch driver stopped before initialization".into())); }
                },
                LaunchState::PreSpawnFailed(reason) => return Err(BrowserError::LaunchFailed(reason.clone())),
                LaunchState::UnobservableAttempt(reason) => return Err(BrowserError::LaunchFailed(reason.clone())),
                LaunchState::Pending(pending) => return Err(BrowserError::LaunchFailed(pending.failure.clone().unwrap_or_else(|| "partial browser launch retained; close must stop the exact launch before reuse".into()))),
                LaunchState::Stopped => return Err(BrowserError::LaunchFailed("launch slot stopped".into())),
            }
            changed.await;
        }
    }

    async fn initialize(&self, state: &mut LaunchState) -> Result<(), BrowserError> {
        if self.profile.exists() {
            if self.profile
                != user_data_dir_for_key(
                    self.profile.parent().expect("profile has parent"),
                    &self.key,
                )
            {
                return Err(BrowserError::LaunchFailed(
                    "transient profile already exists".into(),
                ));
            }
            prepare_profile_dir_for_launch(
                self.profile.parent().expect("profile has parent"),
                &self.key,
            )?;
        }
        #[cfg(test)]
        let fault = self.fault.lock().unwrap().clone();
        #[cfg(test)]
        if fault
            .as_ref()
            .is_some_and(|fault| fault.preparation_failure.load(Ordering::SeqCst))
        {
            return Err(BrowserError::LaunchFailed(
                "injected pre-spawn preparation failure".into(),
            ));
        }
        #[cfg(test)]
        let executable = match fault.as_ref().and_then(|fault| fault.executable.clone()) {
            Some(executable) => executable,
            None => select_executable().await?,
        };
        #[cfg(not(test))]
        let executable = select_executable().await?;
        let config = BrowserSession::browser_config(&self.profile, Some(&executable))?;
        std::fs::create_dir_all(&self.profile)
            .map_err(|error| BrowserError::LaunchFailed(error.to_string()))?;
        *state = LaunchState::UnobservableAttempt(
            "browser launch has not yielded a recoverable child".into(),
        );
        #[cfg(test)]
        if let Some(fault) = self.fault.lock().unwrap().as_ref() {
            fault.attempts.fetch_add(1, Ordering::SeqCst);
            if fault.opaque {
                return Err(BrowserError::LaunchFailed(
                    "injected opaque launch error".into(),
                ));
            }
        }
        let (browser, mut handler) = BrowserSession::launch_browser(config).await?;
        let handler = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(error) = event {
                    tracing::warn!(%error, "CDP handler error");
                }
            }
        });
        *state = LaunchState::Pending(Box::new(PendingBrowser {
            browser,
            handler,
            authority: InitialAuthority::Unadopted,
            identity: self.identity.clone(),
            marker: false,
            failure: None,
        }));
        let LaunchState::Pending(pending) = state else {
            unreachable!()
        };
        let result = pending
            .initialize(
                &self.profile,
                &self.key,
                #[cfg(test)]
                fault,
            )
            .await;
        match result {
            Ok(page) => {
                BrowserSession::inject_react_helper(&page).await;
                let reference_guard_available =
                    BrowserSession::install_reference_guard(&page).await;
                let LaunchState::Pending(pending) = std::mem::replace(state, LaunchState::Stopped)
                else {
                    unreachable!()
                };
                let session = Arc::new(RwLock::new(
                    (*pending).into_session(page, reference_guard_available),
                ));
                *state = LaunchState::Ready(session.clone());
                BrowserSessionManager::setup_session_listeners(session).await;
                Ok(())
            }
            Err(error) => {
                // The aggregate stays in the slot even if cleanup fails.
                if let Err(cleanup) = self.stop_state(state).await {
                    tracing::warn!(%cleanup, "partial browser cleanup failed; retaining exact launch");
                }
                Err(error)
            }
        }
    }

    async fn stop_state(
        &self,
        state: &mut LaunchState,
    ) -> Result<ProfileDeletionProof, BrowserError> {
        match state {
            #[cfg(test)]
            LaunchState::InjectedPending(pending) => {
                pending.stop()?;
                Ok(ProfileDeletionProof::RootOnly)
            }
            LaunchState::Ready(session) => {
                let mut session = session.write().await;
                session
                    .terminate()
                    .await
                    .map_err(|error| BrowserError::OperationFailed(error.to_string()))?;
                session.profile_deletion_proof()
            }
            #[cfg(test)]
            LaunchState::Pending(_)
                if self
                    .fault
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|fault| fault.fail_cleanup()) =>
            {
                Err(BrowserError::OperationFailed(
                    "injected partial cleanup failure".into(),
                ))
            }
            LaunchState::Pending(pending) => pending.stop().await,
            LaunchState::PreSpawnFailed(_) => {
                Err(BrowserError::LaunchFailed("no process was spawned".into()))
            }
            LaunchState::UnobservableAttempt(reason) => Err(BrowserError::LaunchFailed(format!(
                "unobservable launch cannot be proven stopped: {reason}"
            ))),
            LaunchState::Reserved => Err(BrowserError::LaunchFailed(
                "launch reservation has not settled".into(),
            )),
            LaunchState::Stopped => Err(BrowserError::LaunchFailed(
                "launch authority already consumed".into(),
            )),
        }
    }

    pub(super) async fn stop_and_delete(&self) -> Result<(), BrowserError> {
        let mut state = self.state.write().await;
        if matches!(&*state, LaunchState::PreSpawnFailed(_)) {
            *state = LaunchState::Stopped;
            return Ok(());
        }
        let proof = self.stop_state(&mut state).await?;
        #[cfg(test)]
        if matches!(&*state, LaunchState::InjectedPending(_)) {
            *state = LaunchState::Stopped;
            return Ok(());
        }
        if let LaunchState::Pending(pending) = &*state {
            pending.handler.abort();
        }
        let identity = match &*state {
            #[cfg(test)]
            LaunchState::InjectedPending(_) => unreachable!("injected stop handled above"),
            LaunchState::Ready(session) => session.read().await.launch_identity().clone(),
            LaunchState::Pending(pending) => pending.identity.clone(),
            LaunchState::Reserved
            | LaunchState::PreSpawnFailed(_)
            | LaunchState::UnobservableAttempt(_)
            | LaunchState::Stopped => {
                return Err(BrowserError::LaunchFailed(
                    "no stopped profile authority".into(),
                ))
            }
        };
        let unmarked = matches!(&*state, LaunchState::Pending(pending) if !pending.marker);
        let profile = self.profile.clone();
        let key = self.key.clone();
        tokio::task::spawn_blocking(move || {
            if unmarked {
                if profile_marker_path(&profile).exists() {
                    return Err(BrowserError::OperationFailed(
                        "unmarked launch path now has a marker; preserving foreign profile".into(),
                    ));
                }
                std::fs::remove_dir_all(&profile)
                    .or_else(|error| {
                        if error.kind() == std::io::ErrorKind::NotFound {
                            Ok(())
                        } else {
                            Err(error)
                        }
                    })
                    .map_err(|error| BrowserError::OperationFailed(error.to_string()))
            } else {
                delete_profile_if_owned_and_stopped(&profile, &key, &identity, proof)
            }
        })
        .await
        .map_err(|error| {
            BrowserError::OperationFailed(format!("profile cleanup task failed: {error}"))
        })??;
        *state = LaunchState::Stopped;
        Ok(())
    }
}

impl PendingBrowser {
    async fn initialize(
        &mut self,
        profile: &Path,
        key: &str,
        #[cfg(test)] fault: Option<Arc<tests::LaunchFault>>,
    ) -> Result<Page, BrowserError> {
        self.establish_authority()?;
        let marker = BrowserProfileMarker::from_launch(key, &self.identity)
            .expect("identity contains proven process");
        write_profile_marker_atomic(profile, &marker)?;
        self.marker = true;
        #[cfg(test)]
        if fault.is_some_and(|fault| !fault.opaque) {
            return Err(BrowserError::LaunchFailed(
                "injected first-page initialization failure".into(),
            ));
        }
        BrowserSession::first_page(&mut self.browser, &self.handler).await
    }

    fn establish_authority(&mut self) -> Result<(), BrowserError> {
        let pid = self
            .browser
            .get_mut_child()
            .and_then(|child| child.as_mut_inner().id());
        let identity = pid.and_then(current_process_identity).ok_or_else(|| {
            BrowserError::LaunchFailed("could not capture Chrome process identity".into())
        })?;
        self.identity.chrome_process = Some(identity);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if crate::process_group::ChromeExecShim::registered().is_some() {
            let child = self
                .browser
                .get_mut_child()
                .ok_or_else(|| BrowserError::LaunchFailed("Chrome child missing".into()))?;
            self.authority = InitialAuthority::Group(
                OwnedChromeGroup::adopt(identity, child.as_mut_inner())
                    .map_err(|error| BrowserError::LaunchFailed(error.to_string()))?,
            );
        } else {
            self.authority = InitialAuthority::RootOnly;
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            self.authority = InitialAuthority::RootOnly;
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<ProfileDeletionProof, BrowserError> {
        if matches!(self.authority, InitialAuthority::Unadopted) {
            self.establish_authority()?;
        }
        match &mut self.authority {
            InitialAuthority::Unadopted => Err(BrowserError::LaunchFailed(
                "unadopted Chrome launch retained; group stop cannot be proven".into(),
            )),
            InitialAuthority::RootOnly => {
                BrowserSession::stop_initial_browser(
                    &mut self.browser,
                    #[cfg(any(target_os = "linux", target_os = "macos"))]
                    None,
                )
                .await
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            InitialAuthority::Group(group) => {
                BrowserSession::stop_initial_browser(&mut self.browser, Some(group)).await
            }
        }
    }

    fn into_session(self, page: Page, reference_guard_available: bool) -> BrowserSession {
        BrowserSession {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            owned_group: match self.authority {
                InitialAuthority::Group(group) => Some(group),
                InitialAuthority::RootOnly => None,
                InitialAuthority::Unadopted => unreachable!(),
            },
            chrome_pid: self.identity.chrome_process.map(|process| process.pid),
            chrome_process: self.identity.chrome_process,
            launch_identity: self.identity,
            browser: self.browser,
            handler_task: self.handler,
            console_task: None,
            profiling_tasks: Vec::new(),
            snapshot: crate::snapshot::SnapshotState::default(),
            reference_guard_available,
            reference_input_lock: Arc::new(tokio::sync::Mutex::new(())),
            page,
            profiling: Arc::new(StdMutex::new(ProfilingState::default())),
            trace_complete: Arc::new(tokio::sync::Notify::new()),
            console_logs: Arc::new(StdMutex::new(VecDeque::with_capacity(MAX_CONSOLE_LOGS))),
            console_capture: Arc::new(console::ConsoleCapture::default()),
            console_event: Arc::new(tokio::sync::Notify::new()),
            last_activity: Instant::now(),
            screencast: Arc::new(tokio::sync::Mutex::new(std::sync::Weak::new())),
            screencast_lifecycle: Arc::new(crate::screencast::ScreencastLifecycle::default()),
        }
    }
}

async fn select_executable() -> Result<PathBuf, BrowserError> {
    if let Some(explicit) = std::env::var_os("PHOENIX_CHROME_EXECUTABLE")
        .map(PathBuf::from)
        .filter(|path| path.exists())
    {
        return Ok(explicit);
    }
    if let Ok(system) = chromiumoxide::detection::default_executable(
        chromiumoxide::detection::DetectionOptions::default(),
    ) {
        return Ok(system);
    }
    let cache = BrowserSession::fetcher_cache_dir();
    std::fs::create_dir_all(&cache)
        .map_err(|error| BrowserError::LaunchFailed(error.to_string()))?;
    let options = BrowserFetcherOptions::builder()
        .with_path(&cache)
        .with_kind(BrowserKind::ChromeHeadlessShell)
        .build()
        .map_err(|error| BrowserError::LaunchFailed(error.to_string()))?;
    let revision = BrowserFetcher::new(options)
        .fetch()
        .await
        .map_err(|error| BrowserError::LaunchFailed(error.to_string()))?;
    Ok(revision.executable_path)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    pub(in crate::session) struct LaunchFault {
        pub(super) opaque: bool,
        pub(super) executable: Option<PathBuf>,
        pub(super) attempts: AtomicUsize,
        pub(super) preparation_failure: AtomicBool,
        cleanup_failures: AtomicUsize,
    }

    impl LaunchFault {
        pub(super) fn fail_cleanup(&self) -> bool {
            self.cleanup_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
        }
    }

    fn manager_with_fault(opaque: bool) -> (Arc<BrowserSessionManager>, Arc<LaunchFault>) {
        let fault = Arc::new(LaunchFault {
            opaque,
            executable: opaque.then(|| std::env::current_exe().unwrap()),
            attempts: AtomicUsize::new(0),
            preparation_failure: AtomicBool::new(false),
            cleanup_failures: AtomicUsize::new(usize::from(!opaque)),
        });
        let manager = BrowserSessionManager {
            launch_fault: Some(fault.clone()),
            profile_policy: ProfilePolicy::CliTransient,
            ..BrowserSessionManager::default()
        };
        (Arc::new(manager), fault)
    }

    #[tokio::test]
    async fn production_preparation_failure_retires_and_retries_the_same_ide_path() {
        let (mut manager, fault) = manager_with_fault(true);
        Arc::get_mut(&mut manager).unwrap().profile_policy = ProfilePolicy::StableScope;
        fault.preparation_failure.store(true, Ordering::SeqCst);
        let scope = scope();
        assert!(manager.get_session(&scope).await.is_err());
        let path = PathBuf::from(&manager.cleanup_identifiers(&scope).await[0].profile_path);
        assert!(
            !path.exists(),
            "failed preparation must not create a poison profile"
        );
        assert_eq!(fault.attempts.load(Ordering::SeqCst), 0);
        let permit = manager.begin_retirement(&scope).await;
        assert_eq!(
            manager.complete_retirement(&permit).await,
            BrowserRetirementOutcome::Retired
        );
        manager.reopen_after_permit(permit).await;
        fault.preparation_failure.store(false, Ordering::SeqCst);
        assert!(manager.get_session(&scope).await.is_err());
        assert_eq!(
            fault.attempts.load(Ordering::SeqCst),
            1,
            "repair must reach the launch boundary on the same IDE path"
        );
        assert_eq!(
            PathBuf::from(&manager.cleanup_identifiers(&scope).await[0].profile_path),
            path
        );
        assert!(matches!(
            manager.lookup_existing(&scope).await,
            BrowserSessionLookup::Retained(_)
        ));
    }

    #[tokio::test]
    async fn production_driver_opaque_launch_error_remains_a_barrier() {
        let (manager, fault) = manager_with_fault(true);
        let scope = scope();
        assert!(manager.get_session(&scope).await.is_err());
        assert_eq!(fault.attempts.load(Ordering::SeqCst), 1);
        assert!(manager.get_session(&scope).await.is_err());
        assert_eq!(
            fault.attempts.load(Ordering::SeqCst),
            1,
            "second spawning attempt forbidden"
        );
        let permit = manager.begin_retirement(&scope).await;
        assert_eq!(permit.instances.len(), 1);
        assert!(matches!(
            manager.complete_retirement(&permit).await,
            BrowserRetirementOutcome::Residual { .. }
        ));
        assert!(manager.is_active(&scope).await);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    #[ignore = "requires installed real Chrome and PHOENIX_TEST_CHROME_SHIM_EXECUTABLE pointing to the matched built CLI/server; select exact test with --ignored"]
    async fn real_chrome_partial_initialization_retains_child_and_retries_exact_close() {
        let shim = std::env::var_os("PHOENIX_TEST_CHROME_SHIM_EXECUTABLE")
            .map(PathBuf::from)
            .expect("hosted qualification must provide the actual matched native-shim binary");
        crate::process_group::ChromeExecShim::install_external_qualification_executable(shim)
            .unwrap();
        let (manager, fault) = manager_with_fault(false);
        let scope = scope();
        assert!(manager.get_session(&scope).await.is_err());
        assert_eq!(fault.attempts.load(Ordering::SeqCst), 1);
        let actor = EffectiveResourceAccess::new("system", ResourceAuthority::Work);
        let key = session_key(&scope, &actor);
        let slot = manager
            .state
            .read()
            .await
            .sessions
            .get(&key)
            .unwrap()
            .session
            .clone();
        let (pid, address) = {
            let mut state = slot.state.write().await;
            let LaunchState::Pending(pending) = &mut *state else {
                panic!("actual returned browser must remain pending")
            };
            assert!(pending.marker);
            assert!(pending.browser.get_mut_child().is_some());
            assert!(
                matches!(pending.authority, InitialAuthority::Group(_)),
                "real qualification must exercise retained group authority"
            );
            (
                pending.identity.chrome_process.unwrap().pid,
                std::ptr::from_ref(&pending.browser) as usize,
            )
        };
        assert!(manager.get_session(&scope).await.is_err());
        assert_eq!(fault.attempts.load(Ordering::SeqCst), 1);
        {
            let state = slot.state.read().await;
            let LaunchState::Pending(pending) = &*state else {
                panic!("exact pending aggregate retained")
            };
            assert_eq!(std::ptr::from_ref(&pending.browser) as usize, address);
            assert_eq!(pending.identity.chrome_process.unwrap().pid, pid);
        }
        let permit = manager.begin_retirement(&scope).await;
        assert_eq!(
            manager.complete_retirement(&permit).await,
            BrowserRetirementOutcome::Retired
        );
        assert!(!slot.profile.exists());
        assert!(!manager.is_active(&scope).await);
        manager.reopen_after_permit(permit).await;
        assert!(manager.get_session(&scope).await.is_err());
        assert_eq!(
            fault.attempts.load(Ordering::SeqCst),
            2,
            "authoritative retirement permits a fresh lifetime"
        );
        let replacement = manager
            .state
            .read()
            .await
            .sessions
            .get(&key)
            .unwrap()
            .session
            .clone();
        assert_ne!(replacement.profile, slot.profile);
        let replacement_permit = manager.begin_retirement(&scope).await;
        assert_eq!(
            manager.complete_retirement(&replacement_permit).await,
            BrowserRetirementOutcome::Retired
        );
        assert!(!replacement.profile.exists());
    }

    pub(super) struct InjectedPending {
        failures_remaining: usize,
        stop_attempts: Arc<AtomicUsize>,
        dropped: Arc<AtomicBool>,
    }

    impl InjectedPending {
        pub(super) fn stop(&mut self) -> Result<(), BrowserError> {
            self.stop_attempts.fetch_add(1, Ordering::SeqCst);
            if self.failures_remaining > 0 {
                self.failures_remaining -= 1;
                return Err(BrowserError::OperationFailed(
                    "injected owned cleanup failure".into(),
                ));
            }
            Ok(())
        }
    }

    impl Drop for InjectedPending {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    fn scope() -> ResourceScopeKey {
        ResourceScopeKey::Unattached(uuid::Uuid::new_v4().to_string())
    }

    async fn reserve_in_manager(
        manager: &BrowserSessionManager,
        scope: &ResourceScopeKey,
    ) -> Arc<LaunchSlot> {
        let actor = EffectiveResourceAccess::new("owner", ResourceAuthority::Work);
        let key = session_key(scope, &actor);
        let slot = LaunchSlot::reserve(&manager.tmp_root, &key, manager.profile_policy);
        let mut state = manager.state.write().await;
        let generation = BrowserSessionManager::destination_generation(&state, scope);
        state.sessions.insert(
            key.clone(),
            ScopedSession {
                scope: scope.clone(),
                creator_conversation_id: "owner".into(),
                authority: ResourceAuthority::Work,
                session: slot.clone(),
                user_data_key: key,
                destination_retirement_generation: generation,
                current_kill: StdMutex::new(None),
                teardown_failed: Arc::new(AtomicBool::new(false)),
            },
        );
        slot
    }

    #[test]
    fn cli_profile_policy_is_explicit_and_stable_scope_path_is_unchanged() {
        let root = Path::new("/tmp/retention-policy-test");
        let key = "same-scope";
        assert_eq!(
            ProfilePolicy::StableScope.path(root, key, "one"),
            user_data_dir_for_key(root, key)
        );
        assert_eq!(
            ProfilePolicy::StableScope.path(root, key, "one"),
            ProfilePolicy::StableScope.path(root, key, "two")
        );
        assert_ne!(
            ProfilePolicy::CliTransient.path(root, key, "one"),
            ProfilePolicy::CliTransient.path(root, key, "two")
        );
    }

    #[tokio::test]
    async fn proven_pre_spawn_failure_can_retire_without_deleting_the_profile() {
        let manager = Arc::new(BrowserSessionManager::default());
        let scope = scope();
        let slot = reserve_in_manager(&manager, &scope).await;
        std::fs::create_dir_all(&slot.profile).unwrap();
        let foreign = slot.profile.join("preserve-untrusted-profile");
        std::fs::write(&foreign, b"unchanged").unwrap();
        *slot.state.write().await =
            LaunchState::PreSpawnFailed("pre-spawn configuration failure".into());
        assert!(manager.get_session(&scope).await.is_err());
        let permit = manager.begin_retirement(&scope).await;
        assert_eq!(
            manager.complete_retirement(&permit).await,
            BrowserRetirementOutcome::Retired
        );
        assert_eq!(std::fs::read(&foreign).unwrap(), b"unchanged");
        assert!(!manager.is_active(&scope).await);
        std::fs::remove_file(foreign).unwrap();
        std::fs::remove_dir(&slot.profile).unwrap();
    }

    #[tokio::test]
    async fn unobservable_launch_error_is_retained_and_refuses_another_spawn_and_close() {
        let manager = Arc::new(BrowserSessionManager::default());
        let scope = scope();
        let slot = reserve_in_manager(&manager, &scope).await;
        *slot.state.write().await = LaunchState::UnobservableAttempt("opaque launch error".into());
        assert!(
            !manager
                .demote_work_scope_to_actor(&scope, "restricted")
                .await,
            "a namespace change cannot bypass a retained launch"
        );
        for _ in 0..2 {
            assert!(manager.get_session(&scope).await.is_err());
        }
        assert!(
            slot.driver.lock().unwrap().is_none(),
            "no new spawning driver may be installed"
        );
        let permit = manager.begin_retirement(&scope).await;
        assert_eq!(permit.instances.len(), 1);
        assert!(matches!(
            manager.complete_retirement(&permit).await,
            BrowserRetirementOutcome::Residual { .. }
        ));
        assert!(manager.is_active(&scope).await);
        assert!(matches!(
            &*slot.state.read().await,
            LaunchState::UnobservableAttempt(_)
        ));
        assert!(manager.get_session(&scope).await.is_err());
    }

    #[tokio::test]
    async fn partial_initialization_cleanup_failure_retains_authority_and_close_retries_same_slot()
    {
        let manager = Arc::new(BrowserSessionManager::default());
        let scope = scope();
        let slot = reserve_in_manager(&manager, &scope).await;
        let stop_attempts = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        *slot.state.write().await = LaunchState::InjectedPending(InjectedPending {
            failures_remaining: 1,
            stop_attempts: stop_attempts.clone(),
            dropped: dropped.clone(),
        });
        assert!(manager.get_session(&scope).await.is_err());
        let permit = manager.begin_retirement(&scope).await;
        assert_eq!(permit.instances.len(), 1);
        assert!(matches!(
            manager.complete_retirement(&permit).await,
            BrowserRetirementOutcome::Residual { .. }
        ));
        assert_eq!(stop_attempts.load(Ordering::SeqCst), 1);
        assert!(!dropped.load(Ordering::SeqCst));
        assert!(manager.get_session(&scope).await.is_err());
        assert!(slot.driver.lock().unwrap().is_none());
        assert_eq!(
            manager.complete_retirement(&permit).await,
            BrowserRetirementOutcome::Retired
        );
        assert_eq!(stop_attempts.load(Ordering::SeqCst), 2);
        assert!(dropped.load(Ordering::SeqCst));
        assert!(!manager.is_active(&scope).await);
        let next = LaunchSlot::reserve(&manager.tmp_root, "same-key", ProfilePolicy::CliTransient);
        let previous =
            LaunchSlot::reserve(&manager.tmp_root, "same-key", ProfilePolicy::CliTransient);
        assert_ne!(next.profile, previous.profile);
    }

    #[tokio::test]
    async fn canceled_launch_waiter_does_not_remove_reservation_or_hide_it_from_retirement() {
        let manager = Arc::new(BrowserSessionManager::default());
        let scope = scope();
        let slot = reserve_in_manager(&manager, &scope).await;
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let driver = {
            let slot = slot.clone();
            let entered = entered.clone();
            let release = release.clone();
            tokio::spawn(async move {
                let mut state = slot.state.write().await;
                entered.notify_one();
                release.notified().await;
                *state = LaunchState::UnobservableAttempt(
                    "injected timeout after caller cancellation".into(),
                );
                slot.changed.notify_waiters();
            })
        };
        *slot.driver.lock().unwrap() = Some(driver);
        entered.notified().await;
        let waiter = {
            let slot = slot.clone();
            tokio::spawn(async move { slot.wait_ready().await })
        };
        waiter.abort();
        let _ = waiter.await;
        let permit = manager.begin_retirement(&scope).await;
        assert_eq!(
            permit.instances.len(),
            1,
            "locked pending slots cannot become absent"
        );
        release.notify_one();
        assert!(slot.wait_ready().await.is_err());
        assert!(matches!(
            manager.complete_retirement(&permit).await,
            BrowserRetirementOutcome::Residual { .. }
        ));
        assert!(manager.is_active(&scope).await);
    }
}
