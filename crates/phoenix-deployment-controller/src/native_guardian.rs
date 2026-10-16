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

/// Pinned read-only exact-schema reader; it cannot commit controller transitions.
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
    fn sampling_evidence(
        &mut self,
        key: crate::DispatchKey,
    ) -> Result<
        Option<(
            crate::LoadedAttempt,
            crate::NativeGuardianBinding,
            crate::NativeSamplingStatus,
        )>,
        NativeEvidenceReadError,
    > {
        if key.attempt_id() != self.attempt {
            return Err(NativeEvidenceReadError);
        }
        self.store
            .read_native_sampling_evidence(key)
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

fn pass_ack_response_barrier(listener: &UnixListener, until: Instant) -> io::Result<()> {
    listener.set_nonblocking(true)?;
    if !wait_ready(listener.as_raw_fd(), libc::POLLIN, until)? {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "ACK response barrier accept deadline expired",
        ));
    }
    let (mut control, _) = listener.accept()?;
    same_uid(&control)?;
    control.set_nonblocking(true)?;
    let mut control = DeadlineStream {
        stream: &mut control,
        deadline: until,
    };
    control.write_all(b"A")?;
    let mut release = [0];
    control.read_exact(&mut release)?;
    if release != [b'R'] {
        return Err(io::Error::other("invalid ACK barrier release"));
    }
    Ok(())
}

fn serve_frame<E: GuardianExecutor>(
    stream: &mut DeadlineStream<'_>,
    server: &mut GuardianServer<E>,
    ack_response_barrier: &mut Option<&UnixListener>,
    until: Instant,
) -> Result<(), CodecError> {
    let result = (|| {
        let request: Request = read_frame(stream)?;
        let ack_request = matches!(request, Request::AckAfterAppliedRevision { .. });
        let response = server.handle(request);
        if ack_request && matches!(response, Response::Acked { .. }) {
            if let Some(listener) = ack_response_barrier.take() {
                pass_ack_response_barrier(listener, until).map_err(CodecError::Io)?;
            }
        }
        write_frame(stream, &response)
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
    serve_until_with_ack_response_barrier(listener, server, until, None)
}

fn serve_until_with_ack_response_barrier<E: GuardianExecutor>(
    listener: &UnixListener,
    server: &mut GuardianServer<E>,
    until: Instant,
    ack_response_barrier: Option<&UnixListener>,
) -> io::Result<()> {
    listener.set_nonblocking(true)?;
    let mut ack_response_barrier = ack_response_barrier;
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
                    if serve_frame(&mut frame, server, &mut ack_response_barrier, until).is_err() {
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

fn sampling_snapshot_matches(snapshot: &WireSnapshot, issue: crate::NativeSamplingIssue) -> bool {
    let Some(native) = snapshot.native_binding.as_ref() else {
        return false;
    };
    let guardian = issue.guardian_identity();
    snapshot.state == crate::guardian_ipc::WireState::Active
        && snapshot.attempt == issue.key().attempt_id().get()
        && snapshot.session.instance == snapshot.instance
        && snapshot.session.generation == issue.session_generation().get()
        && native.attempt == snapshot.attempt
        && native.deadline == snapshot.deadline
        && native.nonce == guardian.nonce.bytes()
        && native.guardian == guardian.process.into()
}

/// Typed observation client. No client method can mint a policy proof.
pub struct GuardianClient {
    stream: Option<UnixStream>,
    build: BuildIdentity,
    verified_snapshot: Option<WireSnapshot>,
}
impl GuardianClient {
    /// # Errors
    /// Rejects insecure socket paths or peer identities.
    pub fn connect(dir: &Path) -> Result<Self, ClientError> {
        Self::connect_with_build(dir, BuildIdentity::compiled())
    }

    /// Explicit identity injection for isolated transport fixtures.
    /// # Errors
    /// Rejects insecure socket paths or peer identities.
    pub fn connect_with_build(dir: &Path, build: BuildIdentity) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(socket_path(dir)?)?;
        same_uid(&stream)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream: Some(stream),
            build,
            verified_snapshot: None,
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
            | Response::Acked { .. }
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
            | Response::Acked { .. }
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
            Response::Hello { build, snapshot } if build == self.build => {
                self.verified_snapshot = Some(snapshot.clone());
                Ok(snapshot)
            }
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
            | Response::Acked { .. }
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
            Response::Snapshot(snapshot) => {
                self.verified_snapshot = Some(snapshot.clone());
                Ok(snapshot)
            }
            Response::Hello { .. }
            | Response::Fenced
            | Response::ReconciliationBegun { .. }
            | Response::InitialReleased { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::Acked { .. }
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
            | Response::Acked { .. }
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
            | Response::Acked { .. }
            | Response::SignalRoundReserved
            | Response::Admitted { .. }
            | Response::Submitted { .. }
            | Response::Refused(_) => Err(ClientError::UnexpectedResponse),
        }
    }

    /// Consumes the exact affine ticket for the durable issue and derives the
    /// snapshot, wire effect, and guardian admission internally.
    /// # Errors
    /// Refuses stale identity/session/deadline or any guardian/transport mismatch.
    pub fn dispatch_native_sampling(
        &mut self,
        ticket: crate::DispatchTicket,
        issue: crate::NativeSamplingIssue,
    ) -> Result<crate::NativeSamplingDispatch, ClientError> {
        if ticket.key() != issue.key()
            || !matches!(
                ticket.effect(),
                crate::OsEffect::SampleResources { sequence } if *sequence == issue.sequence()
            )
            || self.verified_snapshot.is_none()
        {
            return Err(ClientError::UnexpectedResponse);
        }
        let snapshot = self.snapshot()?;
        if !sampling_snapshot_matches(&snapshot, issue)
            || !crate::MonotonicTick::native_now().is_ok_and(|now| now.get() < snapshot.deadline)
        {
            return Err(ClientError::UnexpectedResponse);
        }
        let key =
            crate::guardian_ipc::WireKey::try_from(issue.key()).map_err(ClientError::Refused)?;
        let Response::Admitted { admission } = self.exchange(&Request::DispatchAdmit {
            session: snapshot.session,
            key,
            effect: crate::guardian_ipc::WireEffect::SampleResources {
                sequence: issue.sequence(),
            },
        })?
        else {
            return Err(ClientError::UnexpectedResponse);
        };
        let now =
            crate::MonotonicTick::native_now().map_err(|_| ClientError::UnexpectedResponse)?;
        let Response::Submitted { key: returned } = self.exchange(&Request::DispatchSubmit {
            session: snapshot.session,
            admission,
            now: now.get(),
        })?
        else {
            return Err(ClientError::UnexpectedResponse);
        };
        if returned == key {
            Ok(crate::NativeSamplingDispatch::new(ticket, issue))
        } else {
            Err(ClientError::UnexpectedResponse)
        }
    }

    /// Read and verify the exact retained sampling completion from this live guardian.
    /// # Errors
    /// Refuses a stale snapshot, wrong session/key/effect, absent custody, or malformed sample.
    pub fn observe_native_sampling(
        &mut self,
        dispatch: crate::NativeSamplingDispatch,
    ) -> Result<crate::VerifiedNativeSamplingCompletion, ClientError> {
        let issue = dispatch.issue();
        if self.verified_snapshot.is_none() {
            return Err(ClientError::UnexpectedResponse);
        }
        let snapshot = self.snapshot()?;
        if !sampling_snapshot_matches(&snapshot, issue) {
            return Err(ClientError::UnexpectedResponse);
        }
        let key =
            crate::guardian_ipc::WireKey::try_from(issue.key()).map_err(ClientError::Refused)?;
        let retained = match self.exchange(&Request::ObserveRetained { key })? {
            Response::Retained(Some(retained)) if retained.key == key => retained,
            Response::Hello { .. }
            | Response::Snapshot(_)
            | Response::Fenced
            | Response::ReconciliationBegun { .. }
            | Response::InitialReleased { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::Acked { .. }
            | Response::SignalRoundReserved
            | Response::Admitted { .. }
            | Response::Submitted { .. }
            | Response::Refused(_) => return Err(ClientError::UnexpectedResponse),
        };
        let completion = match retained.completion {
            crate::guardian_ipc::WireCompletion::SampleResources { sequence, result }
                if sequence == issue.sequence() =>
            {
                match result {
                    Ok(sample) if sample.sequence == sequence => {
                        let cpu = crate::CpuContentionBasisPoints::new(sample.cpu_basis_points)
                            .map_err(|_| ClientError::UnexpectedResponse)?;
                        let pressure = match sample.memory_pressure {
                            crate::guardian_ipc::WireMemoryPressure::Normal => {
                                crate::MemoryPressure::Nominal
                            }
                            crate::guardian_ipc::WireMemoryPressure::Elevated => {
                                crate::MemoryPressure::Warning
                            }
                            crate::guardian_ipc::WireMemoryPressure::Critical => {
                                crate::MemoryPressure::Critical
                            }
                        };
                        crate::NativeSamplingCompletion::Sample(crate::ResourceSample {
                            sequence,
                            capacity: crate::ResourceCapacity {
                                available_memory_bytes: sample.memory_bytes,
                                available_storage_bytes: sample.storage_bytes,
                            },
                            contention: crate::ResourceContention {
                                cpu,
                                memory_pressure: pressure,
                            },
                        })
                    }
                    Ok(_) => return Err(ClientError::UnexpectedResponse),
                    Err(error) => crate::NativeSamplingCompletion::Failure(match error {
                        crate::guardian_ipc::WireResourceError::TemporarilyUnavailable => {
                            crate::ResourceObservationFailure::TemporarilyUnavailable
                        }
                        crate::guardian_ipc::WireResourceError::Unsupported => {
                            crate::ResourceObservationFailure::Unsupported
                        }
                        crate::guardian_ipc::WireResourceError::Malformed => {
                            crate::ResourceObservationFailure::Malformed
                        }
                    }),
                }
            }
            crate::guardian_ipc::WireCompletion::SampleResources { .. }
            | crate::guardian_ipc::WireCompletion::ListChildren { .. }
            | crate::guardian_ipc::WireCompletion::AcquireChildIdentity { .. }
            | crate::guardian_ipc::WireCompletion::ObserveIdentity { .. }
            | crate::guardian_ipc::WireCompletion::SignalExactIdentity { .. }
            | crate::guardian_ipc::WireCompletion::ReverifyRetainedCandidate { .. } => {
                return Err(ClientError::UnexpectedResponse)
            }
        };
        Ok(crate::VerifiedNativeSamplingCompletion::new(
            dispatch, completion,
        ))
    }
    /// ACK an exact successful durable application, including one loaded for recovery.
    /// The authenticated live snapshot is refreshed; no dispatch or policy authority is minted.
    /// # Errors
    /// Refuses stale identity/session, failed outcomes, or uncorrelated guardian responses.
    pub fn ack_native_sampling(
        &mut self,
        status: crate::NativeSamplingStatus,
    ) -> Result<crate::VerifiedNativeSamplingAckConfirmation, ClientError> {
        let crate::NativeSamplingStatus::AppliedPendingAck {
            issue,
            sample,
            applying_revision,
        } = status
        else {
            return Err(ClientError::UnexpectedResponse);
        };
        if self.verified_snapshot.is_none() {
            return Err(ClientError::UnexpectedResponse);
        }
        let snapshot = self.snapshot()?;
        let native = snapshot
            .native_binding
            .as_ref()
            .ok_or(ClientError::UnexpectedResponse)?;
        let identity = issue.guardian_identity();
        if !matches!(
            snapshot.state,
            crate::guardian_ipc::WireState::Active | crate::guardian_ipc::WireState::Reconciling
        ) || snapshot.attempt != issue.key().attempt_id().get()
            || snapshot.session.instance != snapshot.instance
            || snapshot.session.generation == 0
            || native.attempt != snapshot.attempt
            || native.deadline != snapshot.deadline
            || native.nonce != identity.nonce.bytes()
            || native.guardian != identity.process.into()
            || sample.sequence != issue.sequence()
            || !crate::MonotonicTick::native_now().is_ok_and(|now| now.get() < snapshot.deadline)
        {
            return Err(ClientError::UnexpectedResponse);
        }
        let key =
            crate::guardian_ipc::WireKey::try_from(issue.key()).map_err(ClientError::Refused)?;
        let Response::Acked {
            key: returned,
            applied_revision: returned_revision,
        } = self.exchange(&Request::AckAfterAppliedRevision {
            session: snapshot.session,
            key,
            applied_revision: applying_revision.get(),
        })?
        else {
            return Err(ClientError::UnexpectedResponse);
        };
        if returned != key || returned_revision != applying_revision.get() {
            return Err(ClientError::UnexpectedResponse);
        }
        let generation = std::num::NonZeroU64::new(snapshot.session.generation)
            .ok_or(ClientError::UnexpectedResponse)?;
        Ok(crate::VerifiedNativeSamplingAckConfirmation::new(
            issue,
            sample,
            applying_revision,
            generation,
        ))
    }

    /// # Errors
    /// Refuses stale sessions or transport failure.
    pub fn fence(&mut self, session: crate::guardian_ipc::WireSession) -> Result<(), ClientError> {
        match self.exchange(&Request::Fence { session })? {
            Response::Fenced => {
                self.verified_snapshot = None;
                Ok(())
            }
            Response::Hello { .. }
            | Response::Snapshot(_)
            | Response::ReconciliationBegun { .. }
            | Response::InitialReleased { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::Acked { .. }
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
            | Response::Acked { .. }
            | Response::SignalRoundReserved
            | Response::Admitted { .. }
            | Response::Submitted { .. }
            | Response::Refused(_) => Err(ClientError::UnexpectedResponse),
        }
    }
}

/// Compose diagnostic recovery evidence, authenticated ACK and atomic confirmation.
/// No dispatch ticket, policy effect, or application is recreated.
#[derive(Debug)]
pub enum NativeSamplingAckRecoveryError {
    Store(crate::ControllerStoreError),
    Guardian(ClientError),
    NoPendingApplication,
}

/// # Errors
/// Refuses absent/failed application, guardian mismatch, or store failure.
pub fn recover_native_sampling_ack(
    store: &mut crate::ControllerSqliteStore,
    client: &mut GuardianClient,
    key: crate::DispatchKey,
) -> Result<crate::NativeSamplingStatus, NativeSamplingAckRecoveryError> {
    let (_, _, status) = store
        .read_native_sampling_evidence(key)
        .map_err(NativeSamplingAckRecoveryError::Store)?
        .ok_or(NativeSamplingAckRecoveryError::NoPendingApplication)?;
    if matches!(
        status,
        crate::NativeSamplingStatus::SampleAckConfirmed { .. }
    ) {
        return Ok(status);
    }
    if !matches!(
        status,
        crate::NativeSamplingStatus::AppliedPendingAck { .. }
    ) {
        return Err(NativeSamplingAckRecoveryError::NoPendingApplication);
    }
    let proof = client
        .ack_native_sampling(status)
        .map_err(NativeSamplingAckRecoveryError::Guardian)?;
    store
        .confirm_native_sampling_ack(proof)
        .map_err(NativeSamplingAckRecoveryError::Store)
}

/// Verified diagnostic outcome, not a live controller session or execution permit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerifiedBindingStatus {
    InitialActive,
    Detached,
    PendingBegin {
        observed_slots: Vec<crate::NativeGuardianSlot>,
    },
}

fn persist_detached_observation(
    store: &mut crate::ControllerSqliteStore,
    binding: &crate::NativeGuardianBinding,
    slots: &[crate::NativeGuardianSlot],
) -> Result<VerifiedBindingStatus, String> {
    if binding.slots() != slots {
        store
            .commit_detached_binding(binding, slots)
            .map_err(|error| error.to_string())?;
    }
    Ok(VerifiedBindingStatus::Detached)
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
                    Ok(VerifiedBindingStatus::PendingBegin {
                        observed_slots: slots,
                    })
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
                    persist_detached_observation(store, &binding, &slots)
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
    ack_response_barrier: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::{
        Guardian, GuardianLifetimeNonce, MacOsControllerOs, MacOsGuardianExecutor,
        NativeGuardianIdentity,
    };
    if !(1..=120).contains(&child_seconds) || !(1..=120).contains(&serve_seconds) {
        return Err(Box::new(permission_denied()));
    }
    let build = BuildIdentity::compiled();
    let reader = SqliteNativeEvidenceReader::open(store_path, attempt)?;
    let mut nonce_bytes = [0; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut nonce_bytes)?;
    let native_identity = NativeGuardianIdentity {
        nonce: GuardianLifetimeNonce::new(nonce_bytes)
            .ok_or_else(|| io::Error::other("zero random nonce"))?,
        process: crate::macos::own_process_identity()?,
    };
    let listener = bind_disposable(dir)?;
    let barrier = if ack_response_barrier {
        let path = dir.join("ack-response-barrier.sock");
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(path, Permissions::from_mode(0o600))?;
        Some(listener)
    } else {
        None
    };
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
    serve_until_with_ack_response_barrier(
        &listener,
        &mut server,
        Instant::now() + Duration::from_secs(serve_seconds),
        barrier.as_ref(),
    )?;
    Ok(())
}

#[cfg(test)]
mod native_sampling_snapshot_tests {
    use super::*;
    use crate::{
        AckDisposition, BirthToken, ChildSlotObservation, ControllerSqliteStore,
        CpuContentionBasisPoints, DeploymentAttempt, DispatchTicket, EffectCompletion,
        ExecutedEffect, Guardian, GuardianExecutor, GuardianLifetimeNonce, LoopProgress,
        MemoryPressure, MonotonicTick, NativeGuardianIdentity, OsEffect, PersistenceRevision,
        PolicyLoop, ProcessId, ProcessIdentity, ResourceCapacity, ResourceContention,
        ResourcePolicy, ResourceSample,
    };
    use std::{
        collections::BTreeMap,
        fs,
        num::{NonZeroU32, NonZeroUsize},
        os::unix::fs::PermissionsExt,
        time::Duration,
    };

    fn process(pid: u32) -> ProcessIdentity {
        ProcessIdentity::new(
            ProcessId::new(NonZeroU32::new(pid).unwrap()),
            BirthToken::new(12, 3).unwrap(),
        )
    }

    fn issue_and_snapshot() -> (crate::NativeSamplingIssue, WireSnapshot) {
        let mut store = crate::ControllerSqliteStore::in_memory().unwrap();
        let policy = ResourcePolicy::new(
            NonZeroUsize::new(2).unwrap(),
            0,
            0,
            CpuContentionBasisPoints::new(100).unwrap(),
            MemoryPressure::Critical,
        );
        let committed = DeploymentAttempt::start(
            crate::AttemptId::new(9),
            MonotonicTick::new(100),
            policy,
            process(42),
        )
        .persist(&mut store)
        .unwrap();
        let identity = NativeGuardianIdentity {
            nonce: GuardianLifetimeNonce::new([5; 16]).unwrap(),
            process: process(100),
        };
        let binding = store
            .commit_initial_binding(
                crate::AttemptId::new(9),
                MonotonicTick::new(100),
                PersistenceRevision::first(),
                identity,
                &[],
            )
            .unwrap();
        let mut loop_ = PolicyLoop::from_committed(committed);
        let ticket = loop_.issue_next().unwrap();
        let issue = store
            .commit_native_sampling_issue(&ticket, &binding)
            .unwrap();
        let snapshot = WireSnapshot {
            instance: 7,
            attempt: 9,
            deadline: 100,
            state: crate::guardian_ipc::WireState::Active,
            session: crate::guardian_ipc::WireSession {
                instance: 7,
                generation: 1,
            },
            slots: vec![],
            retained: vec![],
            native_binding: Some(crate::guardian_ipc::WireNativeBinding {
                attempt: 9,
                deadline: 100,
                nonce: identity.nonce.bytes(),
                guardian: identity.process.into(),
                slots: vec![],
            }),
        };
        (issue, snapshot)
    }

    #[test]
    fn sampling_snapshot_requires_fresh_active_exact_guardian_identity() {
        let (issue, snapshot) = issue_and_snapshot();
        assert!(sampling_snapshot_matches(&snapshot, issue));

        let mut detached = snapshot.clone();
        detached.state = crate::guardian_ipc::WireState::DetachedFenced;
        assert!(!sampling_snapshot_matches(&detached, issue));

        let mut replacement = snapshot.clone();
        replacement.native_binding.as_mut().unwrap().guardian = process(101).into();
        assert!(!sampling_snapshot_matches(&replacement, issue));

        let mut wrong_generation = snapshot.clone();
        wrong_generation.session.generation += 1;
        assert!(!sampling_snapshot_matches(&wrong_generation, issue));

        let mut wrong_nonce = snapshot.clone();
        wrong_nonce.native_binding.as_mut().unwrap().nonce = [9; 16];
        assert!(!sampling_snapshot_matches(&wrong_nonce, issue));

        let mut wrong_deadline = snapshot.clone();
        wrong_deadline.native_binding.as_mut().unwrap().deadline += 1;
        assert!(!sampling_snapshot_matches(&wrong_deadline, issue));

        let mut wrong_attempt = snapshot.clone();
        wrong_attempt.attempt += 1;
        assert!(!sampling_snapshot_matches(&wrong_attempt, issue));

        let mut wrong_instance = snapshot;
        wrong_instance.session.instance += 1;
        assert!(!sampling_snapshot_matches(&wrong_instance, issue));
    }

    struct SamplingExecutor;

    impl GuardianExecutor for SamplingExecutor {
        type ChildSlot = ProcessIdentity;

        fn execute(
            &mut self,
            ticket: DispatchTicket,
            _: &mut BTreeMap<crate::ChildSlotId, Self::ChildSlot>,
        ) -> ExecutedEffect {
            let OsEffect::SampleResources { sequence } = *ticket.effect() else {
                panic!("sampling test received another effect")
            };
            ExecutedEffect::from_parts(
                ticket.key(),
                EffectCompletion::SampleResources {
                    sequence,
                    result: Ok(ResourceSample {
                        sequence,
                        capacity: ResourceCapacity {
                            available_memory_bytes: 10,
                            available_storage_bytes: 20,
                        },
                        contention: ResourceContention {
                            cpu: CpuContentionBasisPoints::new(25).unwrap(),
                            memory_pressure: MemoryPressure::Nominal,
                        },
                    }),
                },
            )
        }

        fn poll_observation_only(&self, _: &Self::ChildSlot) -> ChildSlotObservation {
            ChildSlotObservation::Running
        }

        fn diagnostic_slot_identity(&self, slot: &Self::ChildSlot) -> Option<ProcessIdentity> {
            Some(*slot)
        }
    }

    fn assert_confirmed_application_is_immutable(database: &std::path::Path) {
        assert!(rusqlite::Connection::open(database)
            .unwrap()
            .execute(
                "UPDATE native_sampling_applications SET available_memory=available_memory+1",
                [],
            )
            .is_err());
    }

    #[test]
    fn native_client_dispatch_observation_and_policy_application_are_exact() {
        let parent = tempfile::Builder::new()
            .prefix("sdc-sampling-")
            .tempdir_in("/tmp")
            .unwrap();
        fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let directory = parent.path().join("sdc-disposable-sampling");
        let listener = bind_disposable(&directory).unwrap();
        let database = parent.path().join("controller.sqlite");
        let mut store = ControllerSqliteStore::open(&database).unwrap();
        let attempt = crate::AttemptId::new(10);
        let deadline = MonotonicTick::native_now()
            .unwrap()
            .checked_add(Duration::from_secs(120))
            .unwrap();
        let policy = ResourcePolicy::new(
            NonZeroUsize::new(2).unwrap(),
            0,
            0,
            CpuContentionBasisPoints::new(100).unwrap(),
            MemoryPressure::Critical,
        );
        let committed = DeploymentAttempt::start(attempt, deadline, policy, process(42))
            .persist(&mut store)
            .unwrap();
        let (guardian, session) =
            Guardian::new(attempt, deadline, SamplingExecutor, vec![process(43)]).unwrap();
        let build = crate::guardian_ipc::BuildIdentity::parse(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap();
        let mut server =
            crate::guardian_ipc::GuardianServer::new(guardian, session, build.clone()).unwrap();
        server.stage_native_identity(NativeGuardianIdentity {
            nonce: GuardianLifetimeNonce::new([7; 16]).unwrap(),
            process: process(44),
        });
        let crate::guardian_ipc::Response::Hello { snapshot, .. } =
            server.handle(crate::guardian_ipc::Request::Hello {
                build: build.clone(),
            })
        else {
            panic!("guardian hello failed")
        };
        verify_native_binding(&mut store, attempt, &snapshot).unwrap();
        let binding = store.load_native_binding(attempt).unwrap().unwrap();
        let mut policy_loop = PolicyLoop::from_committed(committed);
        let ticket = policy_loop.issue_next().unwrap();
        let issue = store
            .commit_native_sampling_issue(&ticket, &binding)
            .unwrap();
        server.stage_native_reader(Box::new(
            SqliteNativeEvidenceReader::open(&database, attempt).unwrap(),
        ));

        std::thread::scope(|scope| {
            scope.spawn(|| {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                for _ in 0..8 {
                    crate::guardian_ipc::serve_once(&mut stream, &mut server).unwrap();
                }
            });
            let mut client = GuardianClient::connect_with_build(&directory, build).unwrap();
            client.hello().unwrap();
            let dispatched = client.dispatch_native_sampling(ticket, issue).unwrap();
            let verified = client.observe_native_sampling(dispatched).unwrap();
            assert!(matches!(
                policy_loop
                    .apply_native_sampling(verified, &mut store)
                    .unwrap(),
                LoopProgress::Applied {
                    key,
                    ack: AckDisposition::AcknowledgeNow,
                    ..
                } if key == issue.key()
            ));
            assert!(matches!(
                recover_native_sampling_ack(&mut store, &mut client, issue.key()).unwrap(),
                crate::NativeSamplingStatus::SampleAckConfirmed {
                    issue: stored,
                    confirming_session_generation,
                    ..
                } if stored == issue && confirming_session_generation == binding.current_session().generation
            ));
            assert!(matches!(
                recover_native_sampling_ack(&mut store, &mut client, issue.key()).unwrap(),
                crate::NativeSamplingStatus::SampleAckConfirmed { issue: stored, .. }
                    if stored == issue
            ));
        });
        assert!(matches!(
            store.load_native_sampling_status(issue.key()).unwrap(),
            Some(crate::NativeSamplingStatus::SampleAckConfirmed { issue: stored, .. })
                if stored == issue
        ));
        assert_confirmed_application_is_immutable(&database);
    }
}
