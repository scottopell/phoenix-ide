use super::{
    console, delete_profile_if_owned_and_stopped, prepare_profile_dir_for_launch,
    profile_marker_path, user_data_dir_for_key, write_profile_marker_atomic, BrowserError,
    BrowserLaunchIdentity, BrowserProfileMarker, BrowserRetainedLaunch, BrowserRetirementCustody,
    BrowserRetirementFenceKey, BrowserSession, BrowserSessionLifecycleEvent,
    BrowserSessionLifecycleKind, BrowserSessionLifecycleSink, BrowserSessionLookup,
    BrowserSessionManager, BrowserSessionRegistryState, ProfileDeletionProof, ProfilingState,
    MAX_CONSOLE_LOGS,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::process_group::OwnedChromeGroup;
use chromiumoxide::{
    browser::Browser,
    fetcher::{BrowserFetcher, BrowserFetcherOptions, BrowserKind},
    Page,
};
use futures::StreamExt;
use phoenix_core::process_identity::current_process_identity;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;
use tokio::sync::{Notify, RwLock};
use tokio::task::JoinHandle;

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
    // Published when the original process identity is established; no session lock required.
    observed_identity: StdMutex<BrowserLaunchIdentity>,
    scenario_owner: Arc<crate::scenario::ScenarioOwner>,
    key: String,
    pub(super) retirement_gate: Arc<tokio::sync::Mutex<()>>,
    state: Arc<RwLock<LaunchState>>,
    changed: Notify,
    driver: StdMutex<Option<JoinHandle<()>>>,
    #[cfg(test)]
    fault: StdMutex<Option<Arc<tests::LaunchFault>>>,
}

pub(super) struct LaunchStateGuard(tokio::sync::OwnedRwLockWriteGuard<LaunchState>);
pub(super) struct PreparedLaunchGuards {
    pub(super) state: LaunchStateGuard,
    pub(super) ready_session: Option<tokio::sync::OwnedRwLockWriteGuard<BrowserSession>>,
}

impl LaunchStateGuard {
    pub(super) fn identity_snapshot(
        &self,
        ready_session: Option<&BrowserSession>,
    ) -> Option<BrowserLaunchIdentity> {
        match (&*self.0, ready_session) {
            (LaunchState::Ready(_), Some(session)) => Some(session.launch_identity().clone()),
            (LaunchState::Ready(_), None) => None,
            (LaunchState::Pending(pending), _) => Some(pending.identity.clone()),
            (
                LaunchState::Reserved
                | LaunchState::PreSpawnFailed(_)
                | LaunchState::UnobservableAttempt(_)
                | LaunchState::Stopped,
                _,
            ) => None,
            #[cfg(test)]
            (LaunchState::InjectedPending(pending), _) => Some(pending.identity.clone()),
        }
    }
}

impl PreparedLaunchGuards {
    pub(super) fn identity_snapshot(&self) -> Option<BrowserLaunchIdentity> {
        self.state.identity_snapshot(self.ready_session.as_deref())
    }
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
            observed_identity: StdMutex::new(identity.clone()),
            identity,
            scenario_owner: Arc::new(crate::scenario::ScenarioOwner::default()),
            key: key.to_owned(),
            retirement_gate: Arc::new(tokio::sync::Mutex::new(())),
            state: Arc::new(RwLock::new(LaunchState::Reserved)),
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
        self.observed_identity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(super) fn scenario_owner(&self) -> Arc<crate::scenario::ScenarioOwner> {
        self.scenario_owner.clone()
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

    fn prepare_existing_profile(&self) -> Result<(), BrowserError> {
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
        Ok(())
    }

    async fn initialize(&self, state: &mut LaunchState) -> Result<(), BrowserError> {
        self.prepare_existing_profile()?;
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
        let result = async {
            let page = pending
                .initialize(
                    &self.profile,
                    &self.key,
                    &self.observed_identity,
                    #[cfg(test)]
                    fault,
                )
                .await?;
            BrowserSession::inject_react_helper(&page).await;
            let reference_guard_available = BrowserSession::install_reference_guard(&page).await;
            let mut device = crate::device::DeviceState::default();
            apply_initial_device(device.configure(&page, crate::device::DeviceConfig::default()))
                .await?;
            Ok((page, reference_guard_available, device))
        }
        .await;
        let (page, reference_guard_available, device) =
            self.retain_initialization_failure(state, result).await?;
        let LaunchState::Pending(pending) = std::mem::replace(state, LaunchState::Stopped) else {
            unreachable!()
        };
        let session = Arc::new(RwLock::new((*pending).into_session(
            page,
            reference_guard_available,
            device,
            Arc::downgrade(&self.scenario_owner),
        )));
        *state = LaunchState::Ready(session.clone());
        BrowserSessionManager::setup_session_listeners(session).await;
        Ok(())
    }

    async fn retain_initialization_failure<T>(
        &self,
        state: &mut LaunchState,
        result: Result<T, BrowserError>,
    ) -> Result<T, BrowserError> {
        if result.is_err() {
            if let Err(cleanup) = self.stop_state(state).await {
                tracing::warn!(%cleanup, "partial browser cleanup failed; retaining exact launch");
            }
        }
        result
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
            LaunchState::Pending(pending) => pending.stop(&self.observed_identity).await,
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

    pub(super) async fn lock_for_teardown(
        &self,
    ) -> (
        LaunchStateGuard,
        Option<tokio::sync::OwnedRwLockWriteGuard<BrowserSession>>,
    ) {
        let state = self.state.clone().write_owned().await;
        let ready_session = match &*state {
            LaunchState::Ready(session) => Some(session.clone().write_owned().await),
            #[cfg(test)]
            LaunchState::InjectedPending(_) => None,
            LaunchState::Reserved
            | LaunchState::PreSpawnFailed(_)
            | LaunchState::UnobservableAttempt(_)
            | LaunchState::Pending(_)
            | LaunchState::Stopped => None,
        };
        (LaunchStateGuard(state), ready_session)
    }

    pub(super) async fn stop_and_delete(
        &self,
        custody: BrowserRetirementCustody,
    ) -> Result<(), BrowserError> {
        // Legacy teardown cancels scenarios before waiting for the slot lock.
        // Staged preparation remains non-destructive; its close occurs only in
        // stop_and_delete_prepared after physical entry has been authorized.
        self.scenario_owner.close();
        let (state, ready_session) = self.lock_for_teardown().await;
        self.stop_and_delete_prepared(state, ready_session, custody)
            .await
    }

    pub(super) async fn stop_and_delete_prepared(
        &self,
        state: LaunchStateGuard,
        mut ready_session: Option<tokio::sync::OwnedRwLockWriteGuard<BrowserSession>>,
        custody: BrowserRetirementCustody,
    ) -> Result<(), BrowserError> {
        let mut state = state.0;
        self.scenario_owner.close();
        if matches!(&*state, LaunchState::PreSpawnFailed(_)) {
            *state = LaunchState::Stopped;
            return Ok(());
        }
        let proof = match (&mut *state, ready_session.as_deref_mut()) {
            (LaunchState::Ready(_), Some(session)) => {
                session
                    .terminate()
                    .await
                    .map_err(|error| BrowserError::OperationFailed(error.to_string()))?;
                session.profile_deletion_proof()?
            }
            (LaunchState::Ready(_), None) => {
                return Err(BrowserError::LaunchFailed(
                    "ready browser teardown requires its prelocked session".into(),
                ));
            }
            (_, None) => self.stop_state(&mut state).await?,
            (_, Some(_)) => {
                return Err(BrowserError::LaunchFailed(
                    "prelocked session does not match launch state".into(),
                ));
            }
        };
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
            LaunchState::Ready(_) => ready_session
                .as_deref()
                .expect("ready session guard checked above")
                .launch_identity()
                .clone(),
            LaunchState::Pending(pending) => pending.identity.clone(),
            LaunchState::Reserved
            | LaunchState::PreSpawnFailed(_)
            | LaunchState::UnobservableAttempt(_)
            | LaunchState::Stopped => {
                return Err(BrowserError::LaunchFailed(
                    "no stopped profile authority".into(),
                ));
            }
        };
        let unmarked = matches!(&*state, LaunchState::Pending(pending) if !pending.marker);
        let profile = self.profile.clone();
        let key = self.key.clone();
        spawn_profile_cleanup(
            profile,
            key,
            identity,
            proof,
            unmarked,
            custody,
            #[cfg(test)]
            None,
        )
        .await
        .map_err(|error| {
            BrowserError::OperationFailed(format!("profile cleanup task failed: {error}"))
        })??;
        *state = LaunchState::Stopped;
        Ok(())
    }
}

fn spawn_profile_cleanup(
    profile: PathBuf,
    key: String,
    identity: BrowserLaunchIdentity,
    proof: ProfileDeletionProof,
    unmarked: bool,
    custody: BrowserRetirementCustody,
    #[cfg(test)] gate: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
) -> JoinHandle<Result<(), BrowserError>> {
    tokio::task::spawn_blocking(move || {
        let custody_guard = custody;
        let _ = &custody_guard.0;
        #[cfg(test)]
        if let Some((entered, release)) = gate {
            entered.send(()).expect("cleanup test waits for entry");
            release
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("cleanup test releases blocking work");
        }
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
}

async fn apply_initial_device(
    operation: impl std::future::Future<Output = Result<(), String>>,
) -> Result<(), BrowserError> {
    tokio::time::timeout(std::time::Duration::from_secs(15), operation)
        .await
        .map_err(|_| {
            BrowserError::OperationFailed(
                "default device initialization timed out after 15 seconds".into(),
            )
        })?
        .map_err(BrowserError::OperationFailed)
}

impl PendingBrowser {
    async fn initialize(
        &mut self,
        profile: &Path,
        key: &str,
        observed_identity: &StdMutex<BrowserLaunchIdentity>,
        #[cfg(test)] fault: Option<Arc<tests::LaunchFault>>,
    ) -> Result<Page, BrowserError> {
        self.establish_authority(observed_identity)?;
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

    fn establish_authority(
        &mut self,
        observed_identity: &StdMutex<BrowserLaunchIdentity>,
    ) -> Result<(), BrowserError> {
        let pid = self
            .browser
            .get_mut_child()
            .and_then(|child| child.as_mut_inner().id());
        let identity = pid.and_then(current_process_identity).ok_or_else(|| {
            BrowserError::LaunchFailed("could not capture Chrome process identity".into())
        })?;
        self.identity.chrome_process = Some(identity);
        *observed_identity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = self.identity.clone();
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

    async fn stop(
        &mut self,
        observed_identity: &StdMutex<BrowserLaunchIdentity>,
    ) -> Result<ProfileDeletionProof, BrowserError> {
        if matches!(self.authority, InitialAuthority::Unadopted) {
            self.establish_authority(observed_identity)?;
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

    fn into_session(
        self,
        page: Page,
        reference_guard_available: bool,
        device: crate::device::DeviceState,
        scenario_owner: std::sync::Weak<crate::scenario::ScenarioOwner>,
    ) -> BrowserSession {
        BrowserSession {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            owned_group: match self.authority {
                InitialAuthority::Group(group) => Some(group),
                InitialAuthority::RootOnly => None,
                InitialAuthority::Unadopted => unreachable!(),
            },
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
            device,
            touch: crate::touch::TouchState::default(),
            throttle: crate::throttle::ThrottleState::default(),
            scenario_owner,
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
    use crate::session::{session_key, BrowserRetirementOutcome, ScopedSession};
    use phoenix_core::work_scope::{EffectiveResourceAccess, ResourceAuthority, ResourceScopeKey};
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    struct CustodyDropWitness {
        dropped: Arc<AtomicBool>,
        changed: Arc<Notify>,
    }

    impl Drop for CustodyDropWitness {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
            self.changed.notify_one();
        }
    }

    fn witnessed_custody() -> (BrowserRetirementCustody, Arc<AtomicBool>, Arc<Notify>) {
        let dropped = Arc::new(AtomicBool::new(false));
        let changed = Arc::new(Notify::new());
        let custody = BrowserRetirementCustody::new(Arc::new(CustodyDropWitness {
            dropped: dropped.clone(),
            changed: changed.clone(),
        }));
        (custody, dropped, changed)
    }

    #[tokio::test]
    async fn aborted_waiter_keeps_custody_inside_actual_blocking_profile_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("owned-unmarked-profile");
        std::fs::create_dir(&profile).unwrap();
        let attempt = crate::session::KillAttempt::new();
        let (initial, initially_dropped, _) = witnessed_custody();
        attempt.retain_until_complete(initial);
        let group = attempt.custody_group();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let waiter = tokio::spawn(async move {
            spawn_profile_cleanup(
                profile,
                "test-key".into(),
                BrowserLaunchIdentity {
                    launch_uuid: "test".into(),
                    chrome_process: None,
                },
                ProfileDeletionProof::RootOnly,
                true,
                BrowserRetirementCustody::new(group),
                Some((entered_tx, release_rx)),
            )
            .await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::task::spawn_blocking(move || entered_rx.recv()),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let (late, late_dropped, changed) = witnessed_custody();
        attempt.retain_until_complete(late);
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(attempt.complete(Err("abnormal kill".into())));
        assert!(
            !initially_dropped.load(Ordering::SeqCst) && !late_dropped.load(Ordering::SeqCst),
            "blocking closure must own both caller custodies after completion and waiter abort"
        );
        let manager = Arc::new(BrowserSessionManager::default());
        let scope = scope();
        let slot = reserve_in_manager(&manager, &scope).await;
        *slot.state.write().await = LaunchState::PreSpawnFailed("safe retry".into());
        let key = session_key(
            &scope,
            &EffectiveResourceAccess::new("owner", ResourceAuthority::Work),
        );
        *manager
            .state
            .read()
            .await
            .sessions
            .get(&key)
            .unwrap()
            .current_kill
            .lock()
            .unwrap() = Some(attempt.clone());
        // Attach a genuinely new caller payload after the attempt has already
        // completed abnormally. Only the physical cleanup owns the weak group's
        // remaining lifetime; joining must not discard this late custody.
        let (after_completion, after_completion_dropped, after_completion_changed) =
            witnessed_custody();
        let held = manager
            .spawn_kill_session_by_key(key.clone(), scope.clone(), None, after_completion)
            .await;
        let crate::session::KillSessionOutcome::AlreadyRequested { attempt: joined } = held else {
            panic!("physical cleanup still holds group: no second kill task");
        };
        assert!(Arc::ptr_eq(&joined, &attempt));
        assert!(manager.wait_for_kill_completion(joined).await.is_err());
        assert!(
            !after_completion_dropped.load(Ordering::SeqCst),
            "completed result does not release custody while physical cleanup is live"
        );
        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), changed.notified())
            .await
            .expect("blocking closure releases custody after cleanup");
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            after_completion_changed.notified(),
        )
        .await
        .expect("physical cleanup releases the post-completion joiner's custody");
        assert!(after_completion_dropped.load(Ordering::SeqCst));
        assert!(initially_dropped.load(Ordering::SeqCst));
        assert!(late_dropped.load(Ordering::SeqCst));
        assert!(!root.path().join("owned-unmarked-profile").exists());
        assert!(
            attempt.join_group().is_none(),
            "completed attempt cannot keep group alive"
        );
        let retry = manager
            .spawn_kill_session_by_key(key, scope, None, BrowserRetirementCustody::no_op())
            .await;
        let crate::session::KillSessionOutcome::Started { handle, .. } = retry else {
            panic!("quiescent group permits a new kill attempt");
        };
        assert!(handle.await.unwrap().is_ok());
    }

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
        crate::process_group::ChromeExecShim::install_external_qualification_executable(&shim)
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
        pub(super) identity: BrowserLaunchIdentity,
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
                current_kill: Arc::new(StdMutex::new(None)),
                teardown_failed: Arc::new(AtomicBool::new(false)),
            },
        );
        slot
    }

    async fn assert_default_device_failure_retains_cleanup(failing_command: usize, pending: bool) {
        let manager = Arc::new(BrowserSessionManager::default());
        let scope = scope();
        let slot = reserve_in_manager(&manager, &scope).await;
        let stop_attempts = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let commands = AtomicUsize::new(0);
        let mut device = crate::device::DeviceState::default();
        let mut state = slot.state.write().await;
        *state = LaunchState::InjectedPending(InjectedPending {
            identity: slot.identity.clone(),
            failures_remaining: 1,
            stop_attempts: stop_attempts.clone(),
            dropped: dropped.clone(),
        });
        let result =
            apply_initial_device(device.apply(crate::device::DeviceConfig::default(), async {
                for command in 1..=2 {
                    commands.fetch_add(1, Ordering::SeqCst);
                    if pending && command == 1 {
                        // test-timing-allow: consuming 8s of the shared 15s deadline makes a restarted second-command budget exceed the 20s test bound.
                        tokio::time::sleep(std::time::Duration::from_secs(8)).await;
                    }
                    if command == failing_command {
                        if pending {
                            return std::future::pending::<Result<(), String>>().await;
                        }
                        return Err(format!("injected device command {command} failure"));
                    }
                }
                Ok(())
            }))
            .await;
        let error = slot
            .retain_initialization_failure(&mut state, result)
            .await
            .unwrap_err();
        assert!(error.to_string().contains(if pending {
            "default device initialization timed out"
        } else {
            "injected device command"
        }));
        assert_eq!(commands.load(Ordering::SeqCst), failing_command);
        assert!(device.known().is_err());
        assert_eq!(stop_attempts.load(Ordering::SeqCst), 1);
        assert!(!dropped.load(Ordering::SeqCst));
        assert!(matches!(&*state, LaunchState::InjectedPending(_)));
        drop(state);
        assert!(manager.get_session(&scope).await.is_err());
        assert!(slot.driver.lock().unwrap().is_none());
        let permit = manager.begin_retirement(&scope).await;
        assert_eq!(permit.instances.len(), 1);
        assert_eq!(
            manager.complete_retirement(&permit).await,
            BrowserRetirementOutcome::Retired
        );
        assert_eq!(stop_attempts.load(Ordering::SeqCst), 2);
        assert!(dropped.load(Ordering::SeqCst));
        assert!(!manager.is_active(&scope).await);
    }

    #[test]
    fn joining_an_existing_kill_retains_late_custody_until_completion() {
        let attempt = crate::session::KillAttempt::new();
        let (custody, dropped, _) = witnessed_custody();
        attempt.retain_until_complete(custody);
        assert!(!dropped.load(Ordering::SeqCst));
        assert!(attempt.complete(Ok(())));
        assert!(dropped.load(Ordering::SeqCst));
    }

    struct LockCheckDrop {
        attempt: Arc<crate::session::KillAttempt>,
        manager: Arc<BrowserSessionManager>,
        unlocked: Arc<AtomicBool>,
    }

    impl Drop for LockCheckDrop {
        fn drop(&mut self) {
            let registry = self.manager.state.try_read();
            let unlocked = registry.as_ref().is_ok_and(|registry| {
                registry
                    .sessions
                    .values()
                    .all(|entry| entry.current_kill.try_lock().is_ok())
            }) && self.attempt.progress.try_lock().is_ok();
            self.unlocked.store(unlocked, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn custody_destructors_never_run_under_kill_or_registry_locks() {
        let manager = Arc::new(BrowserSessionManager::default());
        let scope = scope();
        reserve_in_manager(&manager, &scope).await;
        let attempt = crate::session::KillAttempt::new();
        let unlocked = Arc::new(AtomicBool::new(false));
        let custody = BrowserRetirementCustody::new(Arc::new(LockCheckDrop {
            attempt: attempt.clone(),
            manager: manager.clone(),
            unlocked: unlocked.clone(),
        }));
        attempt.retain_until_complete(custody);
        assert!(attempt.complete(Ok(())));
        assert!(unlocked.load(Ordering::SeqCst));

        let unlocked = Arc::new(AtomicBool::new(false));
        let custody = BrowserRetirementCustody::new(Arc::new(LockCheckDrop {
            attempt: attempt.clone(),
            manager: manager.clone(),
            unlocked: unlocked.clone(),
        }));
        attempt.retain_until_complete(custody);
        assert!(unlocked.load(Ordering::SeqCst));
    }

    #[test]
    fn dropped_kill_task_guard_settles_abnormal_exit() {
        let attempt = crate::session::KillAttempt::new();
        let guard = crate::session::KillTaskCompletion {
            attempt: attempt.clone(),
            teardown_failed: Arc::new(AtomicBool::new(false)),
            scope: scope(),
            audience: crate::session::BrowserSessionAudience::Scope,
            sink: None,
        };
        drop(guard);
        assert!(attempt.result().unwrap().is_err());
        assert!(!attempt.complete(Ok(())));
    }

    #[tokio::test]
    async fn tampered_nonempty_permit_never_enters_teardown_or_reopens_fence() {
        let temp = tempfile::tempdir().unwrap();
        let manager = Arc::new(BrowserSessionManager {
            tmp_root: temp.path().to_path_buf(),
            ..BrowserSessionManager::default()
        });
        let scope = scope();
        let slot = reserve_in_manager(&manager, &scope).await;
        tokio::fs::create_dir_all(&slot.profile).await.unwrap();
        let sentinel = slot.profile.join("untouched");
        tokio::fs::write(&sentinel, b"original").await.unwrap();
        *slot.state.write().await = LaunchState::PreSpawnFailed("never launched".into());

        for mutation in 0..4 {
            let mut permit = manager.begin_retirement(&scope).await;
            assert_eq!(permit.instances.len(), 1);
            match mutation {
                0 => permit.actor_session_key = Some("another-actor".into()),
                1 => permit.instances.clear(),
                2 => permit.instances[0].session_key = "another-session".into(),
                3 => permit.had_entries = false,
                _ => unreachable!(),
            }
            assert!(matches!(
                manager.verify_exact_absence(&permit).await,
                BrowserRetirementOutcome::Residual { .. }
            ));
            assert!(matches!(
                manager.complete_retirement(&permit).await,
                BrowserRetirementOutcome::Residual { .. }
            ));
            manager.reopen_after_permit(permit).await;
            assert!(manager.is_retirement_fenced(&scope).await);
            let registry = manager.state.read().await;
            assert_eq!(registry.sessions.len(), 1);
            let entry = registry.sessions.values().next().unwrap();
            assert!(Arc::ptr_eq(&entry.session, &slot));
            assert!(entry.current_kill.lock().unwrap().is_none());
            assert!(!entry.teardown_failed.load(Ordering::SeqCst));
            drop(registry);
            assert!(matches!(
                *slot.state.read().await,
                LaunchState::PreSpawnFailed(_)
            ));
            assert_eq!(tokio::fs::read(&sentinel).await.unwrap(), b"original");
        }
    }

    #[tokio::test]
    async fn aborted_retirement_parent_keeps_custody_in_spawned_kill_task() {
        let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        let manager = BrowserSessionManager::with_lifecycle_sink(Some(tx));
        let scope = scope();
        let slot = reserve_in_manager(&manager, &scope).await;
        *slot.state.write().await = LaunchState::InjectedPending(InjectedPending {
            identity: slot.identity.clone(),
            failures_remaining: 0,
            stop_attempts: Arc::new(AtomicUsize::new(0)),
            dropped: Arc::new(AtomicBool::new(false)),
        });
        let permit = manager.begin_retirement(&scope).await;
        let expected = permit.instances[0].clone();
        let state_guard = slot.state.write().await;
        let (custody, dropped, changed) = witnessed_custody();
        let parent = {
            let manager = manager.clone();
            tokio::spawn(async move {
                manager
                    .complete_retirement_with_custody(&permit, custody)
                    .await
            })
        };
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.kind, BrowserSessionLifecycleKind::TeardownPending);
        let attempt = {
            let registry = manager.state.read().await;
            let attempt = registry
                .sessions
                .values()
                .next()
                .unwrap()
                .current_kill
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .clone();
            attempt
        };
        let (late, late_dropped, late_changed) = witnessed_custody();
        let key = session_key(
            &scope,
            &EffectiveResourceAccess::new("owner", ResourceAuthority::Work),
        );
        let joined = manager
            .spawn_kill_session_by_key(key, scope.clone(), Some(expected), late)
            .await;
        let crate::session::KillSessionOutcome::AlreadyRequested {
            attempt: joined_attempt,
        } = joined
        else {
            panic!("late caller must join the active kill");
        };
        assert!(Arc::ptr_eq(&attempt, &joined_attempt));
        let joiner = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.wait_for_kill_completion(joined_attempt).await })
        };
        joiner.abort();
        assert!(joiner.await.unwrap_err().is_cancelled());
        parent.abort();
        assert!(parent.await.unwrap_err().is_cancelled());
        assert!(
            !dropped.load(Ordering::SeqCst) && !late_dropped.load(Ordering::SeqCst),
            "spawned kill retains both caller custodies while blocked on slot"
        );
        drop(state_guard);
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            manager.wait_for_kill_completion(attempt),
        )
        .await
        .unwrap()
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), changed.notified())
            .await
            .expect("kill task releases initial custody after bookkeeping");
        tokio::time::timeout(std::time::Duration::from_secs(5), late_changed.notified())
            .await
            .expect("kill task releases late custody after bookkeeping");
        assert!(dropped.load(Ordering::SeqCst));
        assert!(late_dropped.load(Ordering::SeqCst));
    }

    async fn assert_aborted_kill_task(before_first_poll: bool) {
        let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        let manager = BrowserSessionManager::with_lifecycle_sink(Some(tx));
        let scope = scope();
        let slot = reserve_in_manager(&manager, &scope).await;
        let closed = if before_first_poll {
            None
        } else {
            let (closed, observed) = tokio::sync::oneshot::channel();
            slot.scenario_owner
                .start(move |mut cancelled| async move {
                    if !*cancelled.borrow() {
                        cancelled.changed().await.unwrap();
                    }
                    assert!(*cancelled.borrow());
                    let _ = closed.send(());
                })
                .unwrap();
            Some(observed)
        };
        *slot.state.write().await = LaunchState::InjectedPending(InjectedPending {
            identity: slot.identity.clone(),
            failures_remaining: 0,
            stop_attempts: Arc::new(AtomicUsize::new(0)),
            dropped: Arc::new(AtomicBool::new(false)),
        });
        let state_guard = slot.state.write().await;
        let key = session_key(
            &scope,
            &EffectiveResourceAccess::new("owner", ResourceAuthority::Work),
        );
        let started = manager
            .spawn_kill_session_by_key(key, scope.clone(), None, BrowserRetirementCustody::no_op())
            .await;
        let crate::session::KillSessionOutcome::Started {
            handle, attempt, ..
        } = started
        else {
            panic!("kill must start");
        };
        // The pending event is synchronous; consuming it must not yield before
        // the before-first-poll abort on the current-thread test runtime.
        assert_eq!(
            events.try_recv().unwrap().kind,
            BrowserSessionLifecycleKind::TeardownPending
        );
        if let Some(closed) = closed {
            tokio::time::timeout(std::time::Duration::from_secs(5), closed)
                .await
                .expect("kill enters stop_and_delete before abort")
                .unwrap();
        }
        handle.abort();
        assert!(handle.await.unwrap_err().is_cancelled());
        assert!(tokio::time::timeout(
            std::time::Duration::from_secs(5),
            manager.wait_for_kill_completion(attempt)
        )
        .await
        .unwrap()
        .is_err());
        assert_eq!(
            events.recv().await.unwrap().kind,
            BrowserSessionLifecycleKind::TeardownFailed
        );
        assert_eq!(
            manager.inventory_metadata(&scope).await.unwrap().state,
            crate::session::BrowserInventoryState::TeardownFailed
        );
        assert!(manager.get_session(&scope).await.is_err());
        assert!(manager
            .state
            .read()
            .await
            .sessions
            .values()
            .next()
            .unwrap()
            .teardown_failed
            .load(Ordering::SeqCst));
        drop(state_guard);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn aborted_kill_task_before_first_poll_marks_failure() {
        assert_aborted_kill_task(true).await;
    }

    #[tokio::test]
    async fn aborted_kill_task_while_slot_held_marks_failure() {
        assert_aborted_kill_task(false).await;
    }

    #[tokio::test]
    async fn default_device_first_command_error_attempts_cleanup_and_retains_authority() {
        assert_default_device_failure_retains_cleanup(1, false).await;
    }

    #[tokio::test]
    async fn default_device_second_command_error_attempts_cleanup_and_retains_authority() {
        assert_default_device_failure_retains_cleanup(2, false).await;
    }

    #[tokio::test]
    async fn default_device_pending_command_times_out_and_retains_cleanup_authority() {
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            assert_default_device_failure_retains_cleanup(2, true),
        )
        .await
        .expect("device deadline must settle initialization and permit exact-slot cleanup");
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
    async fn full_process_identity_capture_does_not_wait_for_launch_state() {
        let manager = Arc::new(BrowserSessionManager::default());
        let scope = scope();
        let slot = reserve_in_manager(&manager, &scope).await;
        let full = BrowserLaunchIdentity {
            launch_uuid: slot.identity.launch_uuid.clone(),
            chrome_process: Some(phoenix_core::process_identity::ProcessIdentity {
                pid: 4242,
                start_time: 9,
            }),
        };
        *slot.observed_identity.lock().unwrap() = full.clone();
        let state_guard = slot.state.write().await;
        let permit = manager.begin_retirement(&scope).await;
        assert_eq!(permit.instances[0].process_identity(), full.chrome_process);
        assert_eq!(permit.instances[0].durable, full);
        drop(state_guard);
    }

    #[tokio::test]
    async fn prepared_pre_spawn_retirement_is_inert_until_executed_and_drop_releases_gate() {
        let manager = Arc::new(BrowserSessionManager::default());
        let scope = scope();
        let slot = reserve_in_manager(&manager, &scope).await;
        let stop_attempts = Arc::new(AtomicUsize::new(0));
        *slot.state.write().await = LaunchState::InjectedPending(InjectedPending {
            identity: slot.identity.clone(),
            failures_remaining: 0,
            stop_attempts: stop_attempts.clone(),
            dropped: Arc::new(AtomicBool::new(false)),
        });
        let original_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let permit = manager
            .begin_retirement_with_deadline(&scope, original_deadline)
            .await;
        let prepared = manager
            .prepare_retirement_resource(&permit, &permit.instances[0], permit.original.deadline)
            .await
            .unwrap();
        assert_eq!(stop_attempts.load(Ordering::SeqCst), 0);
        assert!(slot.retirement_gate.try_lock().is_err());
        drop(prepared);
        assert!(slot.retirement_gate.try_lock().is_ok());
        assert_eq!(stop_attempts.load(Ordering::SeqCst), 0);
        assert!(manager.is_retirement_fenced(&scope).await);
    }

    #[tokio::test]
    async fn prepare_wait_on_slot_lock_obeys_deadline_and_releases_gate() {
        let manager = Arc::new(BrowserSessionManager::default());
        let scope = scope();
        let slot = reserve_in_manager(&manager, &scope).await;
        let permit = manager.begin_retirement(&scope).await;
        let held = slot.state.write().await;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(20);
        let result = manager
            .prepare_retirement_resource(&permit, &permit.instances[0], deadline)
            .await;
        assert!(result.unwrap_err().contains("wait timed out"));
        assert!(slot.retirement_gate.try_lock().is_ok());
        drop(held);
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
            identity: slot.identity.clone(),
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
                tokio::time::timeout(std::time::Duration::from_secs(20), release.notified())
                    .await
                    .expect("launch release event");
                *state = LaunchState::UnobservableAttempt(
                    "injected timeout after caller cancellation".into(),
                );
                slot.changed.notify_waiters();
            })
        };
        *slot.driver.lock().unwrap() = Some(driver);
        tokio::time::timeout(std::time::Duration::from_secs(20), entered.notified())
            .await
            .expect("launch entered event");
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
