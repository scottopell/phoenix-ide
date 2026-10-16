//! Disposable same-UID Unix transport; durable diagnostic binding has no policy mutation authority.
use crate::guardian_ipc::{
    read_frame, write_frame, BuildIdentity, CodecError, GuardianServer, NativeEvidenceReadError,
    Request, Response, WireError, WireObservation, WireSlot, WireSnapshot,
};
use crate::GuardianExecutor;
use std::{
    fs::{self, DirBuilder, Metadata, Permissions},
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        io::{AsRawFd, RawFd},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Pinned read-only schema-v2 reader; it cannot commit controller transitions.
pub struct SqliteNativeEvidenceReader {
    attempt: crate::AttemptId,
    store: crate::ControllerSqliteStore,
}
impl SqliteNativeEvidenceReader {
    /// # Errors
    /// Rejects missing or incompatible stores.
    pub fn open(
        path: &Path,
        attempt: crate::AttemptId,
    ) -> Result<Self, crate::ControllerStoreError> {
        let store = crate::ControllerSqliteStore::open_read_only(path)?;
        Ok(Self { attempt, store })
    }
}
impl crate::guardian_ipc::NativeEvidenceReader for SqliteNativeEvidenceReader {
    fn read(
        &mut self,
        attempt: crate::AttemptId,
    ) -> Result<Option<(crate::LoadedAttempt, crate::NativeGuardianBinding)>, NativeEvidenceReadError>
    {
        if attempt != self.attempt {
            return Err(NativeEvidenceReadError);
        }
        self.store
            .read_native_evidence(attempt)
            .map_err(|_| NativeEvidenceReadError)
    }
}

const SOCKET_NAME: &str = "guardian.sock";
const IO_SLICE: Duration = Duration::from_millis(250);

fn permission_denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "disposable owner-only socket required",
    )
}

fn owner_only(meta: &Metadata) -> io::Result<()> {
    if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err(permission_denied());
    }
    Ok(())
}

/// Accept only a direct, non-symlinked temporary fixture directory whose
/// caller-owned parent is already private. No production path/label is accepted.
/// # Errors
/// Refuses paths outside the caller's temporary directory or unsafe ownership.
pub fn disposable_dir(path: &Path, create: bool) -> io::Result<PathBuf> {
    let parent = path.parent().ok_or_else(permission_denied)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(permission_denied)?;
    if !name.starts_with("sdc-disposable-")
        || ["prod", "phoenix", "launchd", "install", "database"]
            .iter()
            .any(|word| name.to_ascii_lowercase().contains(word))
        || name.len() <= "sdc-disposable-".len()
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(permission_denied());
    }
    let root = fs::canonicalize("/tmp")?;
    let canonical_parent = fs::canonicalize(parent)?;
    if canonical_parent == root || !canonical_parent.starts_with(&root) {
        return Err(permission_denied());
    }
    let parent_meta = fs::symlink_metadata(parent)?;
    if !parent_meta.is_dir() || parent_meta.file_type().is_symlink() {
        return Err(permission_denied());
    }
    owner_only(&parent_meta)?;
    let path = canonical_parent.join(name);
    if create {
        DirBuilder::new().mode(0o700).create(&path)?;
    }
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(permission_denied());
    }
    owner_only(&meta)?;
    Ok(path)
}

/// Bind a fresh socket under a newly created owner-only directory.
/// # Errors
/// Refuses existing socket paths and unsafe directory ownership.
pub fn bind_disposable(path: &Path) -> io::Result<UnixListener> {
    let directory = disposable_dir(path, true)?;
    let listener = UnixListener::bind(directory.join(SOCKET_NAME))?;
    fs::set_permissions(directory.join(SOCKET_NAME), Permissions::from_mode(0o600))?;
    Ok(listener)
}

fn socket_path(dir: &Path) -> io::Result<PathBuf> {
    let path = disposable_dir(dir, false)?.join(SOCKET_NAME);
    let meta = fs::symlink_metadata(&path)?;
    if !meta.file_type().is_socket() {
        return Err(permission_denied());
    }
    owner_only(&meta)?;
    Ok(path)
}

use std::os::unix::fs::FileTypeExt;

/// Verify kernel-authenticated peer identity before handling any request.
/// # Errors
/// Refuses a different UID or unsupported Unix peer credential API.
pub fn same_uid(stream: &UnixStream) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: pointers refer to initialized stack variables and fd is live.
        if unsafe { libc::getpeereid(stream.as_raw_fd(), &raw mut uid, &raw mut gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if uid != unsafe { libc::geteuid() } {
            return Err(permission_denied());
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let expected_len = libc::socklen_t::try_from(std::mem::size_of::<libc::ucred>())
            .map_err(|_| io::Error::other("peer credential size overflow"))?;
        let mut len = expected_len;
        // SAFETY: cred and len are valid writable buffers; fd is live.
        if unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&raw mut cred).cast(),
                &raw mut len,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        if len != expected_len || cred.uid != unsafe { libc::geteuid() } {
            return Err(permission_denied());
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = stream;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "peer UID unavailable",
        ))
    }
}

fn wait_ready(fd: RawFd, events: libc::c_short, deadline: Instant) -> io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        let millis = i32::try_from(remaining.as_millis().saturating_add(1)).unwrap_or(i32::MAX);
        // SAFETY: descriptor is a live stack value and fd is owned by the caller.
        let ready = unsafe { libc::poll(&raw mut descriptor, 1, millis) };
        if ready > 0 {
            if descriptor.revents & libc::POLLNVAL != 0 {
                return Err(io::Error::from_raw_os_error(libc::EBADF));
            }
            return Ok(true);
        }
        if ready == 0 {
            return Ok(false);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

struct DeadlineStream<'a> {
    stream: &'a mut UnixStream,
    deadline: Instant,
}

impl Read for DeadlineStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if !wait_ready(self.stream.as_raw_fd(), libc::POLLIN, self.deadline)? {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "guardian frame timed out",
                ));
            }
            match self.stream.read(buf) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                other => return other,
            }
        }
    }
}

impl Write for DeadlineStream<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        loop {
            if !wait_ready(self.stream.as_raw_fd(), libc::POLLOUT, self.deadline)? {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "guardian frame timed out",
                ));
            }
            match self.stream.write(buf) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                other => return other,
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

fn serve_frame<E: GuardianExecutor>(
    stream: &mut DeadlineStream<'_>,
    server: &mut GuardianServer<E>,
) -> Result<(), CodecError> {
    let result = (|| {
        let request: Request = read_frame(stream)?;
        write_frame(stream, &server.handle(request))
    })();
    if result.is_err() {
        server.fence_connection();
    }
    result
}

/// Retain the same `GuardianServer` across disconnected connections until the
/// explicit fixture lifetime ends. EOF, timeouts and malformed frames fence,
/// never initiate child cleanup or authorize another policy step.
/// # Errors
/// Returns listener errors; a broken connection is fenced and ignored.
pub fn serve_until<E: GuardianExecutor>(
    listener: &UnixListener,
    server: &mut GuardianServer<E>,
    until: Instant,
) -> io::Result<()> {
    listener.set_nonblocking(true)?;
    while Instant::now() < until {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if same_uid(&stream).is_err() {
                    continue;
                }
                stream.set_nonblocking(true)?;
                while Instant::now() < until {
                    if !wait_ready(stream.as_raw_fd(), libc::POLLIN, until)? {
                        break;
                    }
                    let mut frame = DeadlineStream {
                        stream: &mut stream,
                        deadline: (Instant::now() + IO_SLICE).min(until),
                    };
                    if serve_frame(&mut frame, server).is_err() {
                        break;
                    }
                }
                // Even an idle connection at the fixture deadline cannot retain authority.
                server.fence_connection();
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if !wait_ready(listener.as_raw_fd(), libc::POLLIN, until)? {
                    break;
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[derive(Debug)]
pub enum ClientError {
    Io(io::Error),
    Codec(CodecError),
    Refused(WireError),
    UnexpectedResponse,
    MissingBuildIdentity,
}
impl From<io::Error> for ClientError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<CodecError> for ClientError {
    fn from(value: CodecError) -> Self {
        Self::Codec(value)
    }
}

/// Typed observation client. No client method can mint a policy proof.
pub struct GuardianClient {
    stream: Option<UnixStream>,
    build: BuildIdentity,
}
impl GuardianClient {
    /// # Errors
    /// Rejects insecure socket paths or peer identities.
    pub fn connect(dir: &Path) -> Result<Self, ClientError> {
        let build = BuildIdentity::compiled().ok_or(ClientError::MissingBuildIdentity)?;
        Self::connect_with_build(dir, build)
    }

    /// Explicit identity injection for isolated transport fixtures without Git metadata.
    /// # Errors
    /// Rejects insecure socket paths or peer identities.
    pub fn connect_with_build(dir: &Path, build: BuildIdentity) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(socket_path(dir)?)?;
        same_uid(&stream)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream: Some(stream),
            build,
        })
    }
    fn exchange(&mut self, request: &Request) -> Result<Response, ClientError> {
        let result = (|| {
            let mut frame = DeadlineStream {
                stream: self
                    .stream
                    .as_mut()
                    .ok_or(ClientError::UnexpectedResponse)?,
                deadline: Instant::now() + IO_SLICE,
            };
            write_frame(&mut frame, request)?;
            let response = read_frame(&mut frame)?;
            Ok::<Response, ClientError>(response)
        })();
        if result.is_err() {
            self.stream.take();
        }
        match result? {
            Response::Refused(error) => Err(ClientError::Refused(error)),
            response @ (Response::Hello { .. }
            | Response::Snapshot(_)
            | Response::Fenced
            | Response::ReconciliationBegun { .. }
            | Response::InitialReleased { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::Acked
            | Response::SignalRoundReserved
            | Response::Admitted { .. }
            | Response::Submitted { .. }) => Ok(response),
        }
    }
    /// Release the exact committed initial session's blocked child once.
    /// # Errors
    /// Refuses missing durable evidence, a stale session, or transport failure.
    pub fn release_initial(
        &mut self,
        snapshot: &WireSnapshot,
        revision: crate::PersistenceRevision,
    ) -> Result<(), ClientError> {
        let request = crate::guardian_ipc::initial_release_request(snapshot, revision)
            .map_err(ClientError::Refused)?;
        match self.exchange(&request)? {
            Response::InitialReleased { session } if session == snapshot.session => Ok(()),
            Response::Hello { .. }
            | Response::Snapshot(_)
            | Response::Fenced
            | Response::ReconciliationBegun { .. }
            | Response::InitialReleased { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::Acked
            | Response::SignalRoundReserved
            | Response::Admitted { .. }
            | Response::Submitted { .. }
            | Response::Refused(_) => Err(ClientError::UnexpectedResponse),
        }
    }

    /// # Errors
    /// Returns transport or protocol errors rather than inferring a live binding.
    pub fn hello(&mut self) -> Result<WireSnapshot, ClientError> {
        match self.exchange(&Request::Hello {
            build: self.build.clone(),
        })? {
            Response::Hello { build, snapshot } if build == self.build => Ok(snapshot),
            Response::Hello { .. } => {
                self.stream.take();
                Err(ClientError::UnexpectedResponse)
            }
            Response::Snapshot(_)
            | Response::Fenced
            | Response::ReconciliationBegun { .. }
            | Response::InitialReleased { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::Acked
            | Response::SignalRoundReserved
            | Response::Admitted { .. }
            | Response::Submitted { .. }
            | Response::Refused(_) => Err(ClientError::UnexpectedResponse),
        }
    }
    /// # Errors
    /// Returns transport or protocol errors.
    pub fn snapshot(&mut self) -> Result<WireSnapshot, ClientError> {
        match self.exchange(&Request::Snapshot)? {
            Response::Snapshot(snapshot) => Ok(snapshot),
            Response::Hello { .. }
            | Response::Fenced
            | Response::ReconciliationBegun { .. }
            | Response::InitialReleased { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::Acked
            | Response::SignalRoundReserved
            | Response::Admitted { .. }
            | Response::Submitted { .. }
            | Response::Refused(_) => Err(ClientError::UnexpectedResponse),
        }
    }
    /// # Errors
    /// Returns transport or protocol errors.
    pub fn inventory(&mut self) -> Result<Vec<WireSlot>, ClientError> {
        match self.exchange(&Request::Inventory)? {
            Response::Inventory(slots) => Ok(slots),
            Response::Hello { .. }
            | Response::Snapshot(_)
            | Response::Fenced
            | Response::ReconciliationBegun { .. }
            | Response::InitialReleased { .. }
            | Response::ReconciliationFinished
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::Acked
            | Response::SignalRoundReserved
            | Response::Admitted { .. }
            | Response::Submitted { .. }
            | Response::Refused(_) => Err(ClientError::UnexpectedResponse),
        }
    }
    /// # Errors
    /// Returns transport or protocol errors.
    pub fn observe_slot(&mut self, slot: WireSlot) -> Result<WireObservation, ClientError> {
        match self.exchange(&Request::ObserveSlot { slot })? {
            Response::SlotObserved(observation) => Ok(observation),
            Response::Hello { .. }
            | Response::Snapshot(_)
            | Response::Fenced
            | Response::ReconciliationBegun { .. }
            | Response::InitialReleased { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::Retained(_)
            | Response::Acked
            | Response::SignalRoundReserved
            | Response::Admitted { .. }
            | Response::Submitted { .. }
            | Response::Refused(_) => Err(ClientError::UnexpectedResponse),
        }
    }
    /// # Errors
    /// Refuses stale sessions or transport failure.
    pub fn fence(&mut self, session: crate::guardian_ipc::WireSession) -> Result<(), ClientError> {
        match self.exchange(&Request::Fence { session })? {
            Response::Fenced => Ok(()),
            Response::Hello { .. }
            | Response::Snapshot(_)
            | Response::ReconciliationBegun { .. }
            | Response::InitialReleased { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::Acked
            | Response::SignalRoundReserved
            | Response::Admitted { .. }
            | Response::Submitted { .. }
            | Response::Refused(_) => Err(ClientError::UnexpectedResponse),
        }
    }

    /// # Errors
    /// Refuses missing durable evidence or transport failure.
    pub fn begin_reconciliation(
        &mut self,
        snapshot: &WireSnapshot,
        revision: crate::PersistenceRevision,
    ) -> Result<crate::guardian_ipc::WireSession, ClientError> {
        match self.exchange(&Request::BeginReconciliation {
            instance: snapshot.instance,
            inventory: snapshot.slots.clone(),
            interruption_revision: revision.get(),
        })? {
            Response::ReconciliationBegun { session } => Ok(session),
            Response::Hello { .. }
            | Response::Snapshot(_)
            | Response::Fenced
            | Response::InitialReleased { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::Acked
            | Response::SignalRoundReserved
            | Response::Admitted { .. }
            | Response::Submitted { .. }
            | Response::Refused(_) => Err(ClientError::UnexpectedResponse),
        }
    }
}

/// Verified diagnostic outcome, not a live controller session or execution permit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerifiedBindingStatus {
    InitialActive,
    Detached,
    PendingBegin,
}

/// Compare all durable continuity evidence before recording a detached session.
/// # Errors
/// Refuses missing/invalid wire identity, wrong attempt, stale binding, or store failure.
pub fn verify_native_binding(
    store: &mut crate::ControllerSqliteStore,
    attempt: crate::AttemptId,
    snapshot: &crate::guardian_ipc::WireSnapshot,
) -> Result<VerifiedBindingStatus, String> {
    use crate::guardian_ipc::{WireObservation, WireState};
    use crate::{
        GuardianLifetimeNonce, NativeGuardianIdentity, NativeGuardianSlot, NativeSessionState,
        NativeSlotLiveness,
    };
    let reject = || "native binding mismatch".to_owned();
    if snapshot.attempt != attempt.get() || snapshot.session.instance != snapshot.instance {
        return Err(reject());
    }
    let wire = snapshot.native_binding.as_ref().ok_or_else(reject)?;
    if wire.attempt != snapshot.attempt
        || wire.deadline != snapshot.deadline
        || wire.slots.len() != snapshot.slots.len()
        || snapshot.slots.iter().enumerate().any(|(index, slot)| {
            slot.instance != snapshot.instance || slot.ordinal != index as u64 + 1
        })
    {
        return Err(reject());
    }
    let identity = NativeGuardianIdentity {
        nonce: GuardianLifetimeNonce::new(wire.nonce).ok_or_else(reject)?,
        process: wire.guardian.try_into().map_err(|_| reject())?,
    };
    let slots: Vec<NativeGuardianSlot> = wire
        .slots
        .iter()
        .map(|slot| {
            Ok(NativeGuardianSlot {
                process: slot.identity.try_into().map_err(|_| reject())?,
                liveness: match slot.liveness {
                    WireObservation::Running => NativeSlotLiveness::Running,
                    WireObservation::Exited => NativeSlotLiveness::Exited,
                    WireObservation::Unavailable => NativeSlotLiveness::Unavailable,
                },
            })
        })
        .collect::<Result<_, String>>()?;
    let loaded = store
        .load_attempt(attempt)
        .map_err(|e| e.to_string())?
        .ok_or_else(reject)?;
    match store
        .load_native_binding(attempt)
        .map_err(|e| e.to_string())?
    {
        None if snapshot.state == WireState::Active && snapshot.session.generation == 1 => {
            store
                .commit_initial_binding(
                    attempt,
                    crate::MonotonicTick::new(snapshot.deadline),
                    loaded.persistence_revision(),
                    identity,
                    &slots,
                )
                .map_err(|e| e.to_string())?;
            Ok(VerifiedBindingStatus::InitialActive)
        }
        Some(binding) => {
            let stored_generation = binding.current_session().generation.get();
            let live_generation = snapshot.session.generation;
            if !(stored_generation == live_generation
                || (snapshot.state == WireState::DetachedFenced
                    && binding.current_session().state == NativeSessionState::Reconciling
                    && live_generation.checked_add(1) == Some(stored_generation)))
                || binding.identity() != identity
                || binding.deadline().get() != snapshot.deadline
                || binding.slots().len() != slots.len()
                || !binding
                    .slots()
                    .iter()
                    .zip(&slots)
                    .all(|(a, b)| a.process == b.process)
            {
                return Err(reject());
            }
            match (snapshot.state, binding.current_session().state) {
                (WireState::DetachedFenced, NativeSessionState::Reconciling)
                    if live_generation.checked_add(1) == Some(stored_generation) =>
                {
                    Ok(VerifiedBindingStatus::PendingBegin)
                }
                (
                    WireState::DetachedFenced,
                    NativeSessionState::Active | NativeSessionState::Reconciling,
                ) => {
                    store
                        .commit_detached_binding(&binding, &slots)
                        .map_err(|e| e.to_string())?;
                    Ok(VerifiedBindingStatus::Detached)
                }
                (WireState::DetachedFenced, NativeSessionState::Detached) => {
                    Ok(VerifiedBindingStatus::Detached)
                }
                _ => Err(reject()),
            }
        }
        None => Err(reject()),
    }
}

#[cfg(target_os = "macos")]
/// Spawn one harmless direct child and hold its original parent slot until the
/// explicitly bounded disposable fixture ends.
/// # Errors
/// Refuses any fixture outside the private temporary directory.
pub fn run_disposable_guardian(
    dir: &Path,
    child_seconds: u64,
    serve_seconds: u64,
    attempt: crate::AttemptId,
    deadline: crate::MonotonicTick,
    store_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::{
        Guardian, GuardianLifetimeNonce, MacOsControllerOs, MacOsGuardianExecutor,
        NativeGuardianIdentity,
    };
    if !(1..=120).contains(&child_seconds) || !(1..=120).contains(&serve_seconds) {
        return Err(Box::new(permission_denied()));
    }
    let build = BuildIdentity::compiled()
        .ok_or_else(|| io::Error::other("clean native build identity required"))?;
    let reader = SqliteNativeEvidenceReader::open(store_path, attempt)?;
    let mut nonce_bytes = [0; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut nonce_bytes)?;
    let native_identity = NativeGuardianIdentity {
        nonce: GuardianLifetimeNonce::new(nonce_bytes)
            .ok_or_else(|| io::Error::other("zero random nonce"))?,
        process: crate::macos::own_process_identity()?,
    };
    let listener = bind_disposable(dir)?;
    let os = MacOsControllerOs::new(dir)?;
    let (child, gate) = os.spawn_blocked_sleep(child_seconds)?;
    let identity = child.identity();
    let (guardian, session) = Guardian::new(
        attempt,
        deadline,
        MacOsGuardianExecutor::new(os),
        vec![child],
    )
    .map_err(|failure| {
        let (error, executor, slots) = failure.into_parts();
        drop(slots);
        drop(executor);
        error
    })?;
    let mut server = GuardianServer::new(guardian, session, build)?;
    server.stage_native_identity(native_identity);
    server.stage_native_reader(Box::new(reader));
    server.stage_initial_gate(Box::new(gate));
    println!(
        "disposable child pid={} birth={}:{}",
        identity.pid().get(),
        identity.birth().seconds(),
        identity.birth().nanos()
    );
    io::stdout().flush()?;
    serve_until(
        &listener,
        &mut server,
        Instant::now() + Duration::from_secs(serve_seconds),
    )?;
    Ok(())
}
