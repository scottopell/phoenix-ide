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
///
/// ```compile_fail
/// use phoenix_deployment_controller::guardian_ipc::NativeEvidenceReader;
/// ```
///
/// ```compile_fail
/// use phoenix_deployment_controller::{guardian_ipc::GuardianServer, native_guardian::SqliteNativeEvidenceReader, GuardianExecutor};
/// fn inject<E: GuardianExecutor>(server: &mut GuardianServer<E>, reader: SqliteNativeEvidenceReader) {
///     server.stage_native_reader(Box::new(reader));
/// }
/// ```
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

    /// Reads exact prepared evidence for diagnostics, never a live permit.
    /// # Errors
    /// Refuses another attempt or invalid durable evidence.
    pub fn reconciliation_evidence(
        &mut self,
        attempt: crate::AttemptId,
        generation: std::num::NonZeroU64,
    ) -> Result<
        Option<(
            crate::LoadedAttempt,
            crate::NativeGuardianBinding,
            crate::NativeReconciliationCompletion,
        )>,
        NativeEvidenceReadError,
    > {
        if attempt != self.attempt {
            return Err(NativeEvidenceReadError);
        }
        self.store
            .read_native_reconciliation_evidence(attempt, generation)
            .map_err(|_| NativeEvidenceReadError)
    }

    /// Reads exact durable activation diagnostics, never a live capability.
    /// # Errors
    /// Refuses another attempt or stale/contradictory evidence.
    pub fn activation_evidence(
        &mut self,
        attempt: crate::AttemptId,
        generation: std::num::NonZeroU64,
    ) -> Result<
        Option<(
            crate::LoadedAttempt,
            crate::NativeGuardianBinding,
            crate::NativeReconciliationActivation,
        )>,
        NativeEvidenceReadError,
    > {
        if attempt != self.attempt {
            return Err(NativeEvidenceReadError);
        }
        self.store
            .read_native_reconciliation_activation_evidence(attempt, generation)
            .map_err(|_| NativeEvidenceReadError)
    }

    /// Reads checkpoint and binding diagnostics in one snapshot.
    /// # Errors
    /// Refuses another attempt or invalid durable evidence.
    pub fn read(
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
    /// Reads sampling diagnostics without constructing executable authority.
    /// # Errors
    /// Refuses another attempt or invalid durable evidence.
    pub fn sampling_evidence(
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

    /// Reads exact durable native identity-observation evidence without granting authority.
    /// # Errors
    /// Refuses another attempt or malformed issue/application evidence.
    pub fn identity_observation_evidence(
        &mut self,
        key: crate::DispatchKey,
    ) -> Result<
        Option<(
            crate::LoadedAttempt,
            crate::NativeGuardianBinding,
            crate::NativeIdentityObservationStatus,
        )>,
        NativeEvidenceReadError,
    > {
        if key.attempt_id() != self.attempt {
            return Err(NativeEvidenceReadError);
        }
        self.store
            .read_native_identity_observation_evidence(key)
            .map_err(|_| NativeEvidenceReadError)
    }

    /// Reads exact root child-list evidence without constructing executable authority.
    /// # Errors
    /// Refuses another attempt or invalid durable evidence.
    pub fn child_list_evidence(
        &mut self,
        key: crate::DispatchKey,
    ) -> Result<
        Option<(
            crate::LoadedAttempt,
            crate::NativeGuardianBinding,
            crate::NativeChildListStatus,
        )>,
        NativeEvidenceReadError,
    > {
        if key.attempt_id() != self.attempt {
            return Err(NativeEvidenceReadError);
        }
        self.store
            .read_native_child_list_evidence(key)
            .map_err(|_| NativeEvidenceReadError)
    }
}

impl crate::guardian_ipc::NativeEvidenceReader for SqliteNativeEvidenceReader {
    fn activation_evidence(
        &mut self,
        attempt: crate::AttemptId,
        generation: std::num::NonZeroU64,
    ) -> Result<
        Option<(
            crate::LoadedAttempt,
            crate::NativeGuardianBinding,
            crate::NativeReconciliationActivation,
        )>,
        NativeEvidenceReadError,
    > {
        Self::activation_evidence(self, attempt, generation)
    }
    fn read(
        &mut self,
        attempt: crate::AttemptId,
    ) -> Result<Option<(crate::LoadedAttempt, crate::NativeGuardianBinding)>, NativeEvidenceReadError>
    {
        Self::read(self, attempt)
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
        Self::sampling_evidence(self, key)
    }
    fn identity_observation_evidence(
        &mut self,
        key: crate::DispatchKey,
    ) -> Result<
        Option<(
            crate::LoadedAttempt,
            crate::NativeGuardianBinding,
            crate::NativeIdentityObservationStatus,
        )>,
        NativeEvidenceReadError,
    > {
        Self::identity_observation_evidence(self, key)
    }

    fn child_list_evidence(
        &mut self,
        key: crate::DispatchKey,
    ) -> Result<
        Option<(
            crate::LoadedAttempt,
            crate::NativeGuardianBinding,
            crate::NativeChildListStatus,
        )>,
        NativeEvidenceReadError,
    > {
        Self::child_list_evidence(self, key)
    }
    fn reconciliation_evidence(
        &mut self,
        attempt: crate::AttemptId,
        generation: std::num::NonZeroU64,
    ) -> Result<
        Option<(
            crate::LoadedAttempt,
            crate::NativeGuardianBinding,
            crate::NativeReconciliationCompletion,
        )>,
        NativeEvidenceReadError,
    > {
        Self::reconciliation_evidence(self, attempt, generation)
    }
}

const SOCKET_NAME: &str = "guardian.sock";
const IO_SLICE: Duration = Duration::from_millis(250);
const EFFECT_RESPONSE_SLICE: Duration = Duration::from_secs(2);

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
        if matches!(
            request,
            Request::DispatchSubmit { .. }
                | Request::NativeChildListSubmit { .. }
                | Request::NativeIdentityObservationSubmit { .. }
        ) {
            stream.deadline = (Instant::now() + EFFECT_RESPONSE_SLICE).min(until);
        }
        let ack_request = matches!(
            request,
            Request::AckAfterAppliedRevision { .. }
                | Request::AckNativeChildListAfterApplied { .. }
                | Request::AckNativeIdentityObservationAfterApplied { .. }
        );
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

/// Affine proof that this exact ticket was never submitted for execution.
/// Only the authenticated client's dispatch path can create it.
///
/// ```compile_fail
/// use phoenix_deployment_controller::native_guardian::UnsubmittedNativeSampling;
/// fn duplicate(value: UnsubmittedNativeSampling) { let _ = value.clone(); }
/// ```
/// ```compile_fail
/// use phoenix_deployment_controller::{DispatchTicket, NativeSamplingIssue};
/// use phoenix_deployment_controller::native_guardian::{ClientError, UnsubmittedNativeSampling};
/// fn fabricate(ticket: DispatchTicket, issue: NativeSamplingIssue, error: ClientError)
///     -> UnsubmittedNativeSampling {
///     UnsubmittedNativeSampling { ticket, issue }
/// }
/// ```
/// ```compile_fail
/// use phoenix_deployment_controller::{NativeSamplingIssue, guardian_ipc::WireError};
/// use phoenix_deployment_controller::native_guardian::UnsubmittedNativeSampling;
/// fn fabricate(issue: NativeSamplingIssue, refused: WireError) -> UnsubmittedNativeSampling {
///     (issue, refused).into()
/// }
/// ```
/// ```compile_fail
/// use phoenix_deployment_controller::DispatchTicket;
/// use phoenix_deployment_controller::native_guardian::UnsubmittedNativeSampling;
/// fn reconstruct(value: UnsubmittedNativeSampling) -> DispatchTicket { value.into() }
/// ```
/// ```compile_fail
/// use phoenix_deployment_controller::ControllerSqliteStore;
/// use phoenix_deployment_controller::native_guardian::UnsubmittedNativeSampling;
/// fn replay(store: &mut ControllerSqliteStore, value: UnsubmittedNativeSampling) {
///     let _ = store.cancel_unsubmitted_native_sampling(value);
///     let _ = store.cancel_unsubmitted_native_sampling(value);
/// }
/// ```
#[derive(Debug)]
pub struct UnsubmittedNativeSampling {
    ticket: crate::DispatchTicket,
    issue: crate::NativeSamplingIssue,
}

impl UnsubmittedNativeSampling {
    pub(crate) fn into_parts(self) -> (crate::DispatchTicket, crate::NativeSamplingIssue) {
        (self.ticket, self.issue)
    }
}

#[derive(Debug)]
pub enum NativeSamplingDispatchError {
    KnownUnsubmitted {
        error: ClientError,
        unsubmitted: Box<UnsubmittedNativeSampling>,
    },
    SubmissionUnknown(ClientError),
}

// These refusals originate before executor invocation in native_dispatch_submit /
// submit_bound. InvalidValue and ExecutorCorrelationMismatch can follow execution.
fn sampling_submit_refusal_is_unsubmitted(error: &ClientError) -> bool {
    use crate::guardian_ipc::WireGuardianError as G;
    matches!(
        error,
        ClientError::Refused(
            WireError::UnknownAdmission
                | WireError::AdmissionMismatch
                | WireError::MissingProof
                | WireError::UnsupportedDurableBinding
                | WireError::Guardian(
                    G::StaleSession
                        | G::StaleDispatchRevision
                        | G::NotActive
                        | G::SessionFenced
                        | G::AttemptMismatch
                        | G::InstanceMismatch
                        | G::DeadlineExpired
                        | G::AuthorityLost
                        | G::RetainedResultCapacityExceeded
                )
        )
    )
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

fn recovered_sampling_slots_match(
    binding: &crate::NativeGuardianBinding,
    snapshot: &WireSnapshot,
    native: &crate::guardian_ipc::WireNativeBinding,
) -> bool {
    let slots = binding.slots();
    slots.len() == native.slots.len()
        && slots.len() == snapshot.slots.len()
        && slots
            .iter()
            .zip(&native.slots)
            .zip(&snapshot.slots)
            .enumerate()
            .all(|(index, ((stored, wire), slot))| {
                let liveness = match stored.liveness {
                    crate::NativeSlotLiveness::Running => WireObservation::Running,
                    crate::NativeSlotLiveness::Exited => WireObservation::Exited,
                    crate::NativeSlotLiveness::Unavailable => WireObservation::Unavailable,
                };
                wire.identity == stored.process.into()
                    && wire.liveness == liveness
                    && slot.instance == snapshot.instance
                    && slot.ordinal == (index + 1) as u64
            })
}

/// Typed observation client. No client method can mint a policy proof.
pub struct GuardianClient {
    stream: Option<UnixStream>,
    build: BuildIdentity,
    verified_snapshot: Option<WireSnapshot>,
}
/// Affine proof that this exact child-list ticket was never submitted.
#[derive(Debug)]
pub struct UnsubmittedNativeChildList {
    ticket: crate::DispatchTicket,
    issue: crate::NativeChildListIssue,
}

impl UnsubmittedNativeChildList {
    /// Recovers the original affine values for an explicit retry after a proven pre-execution refusal.
    #[must_use]
    pub fn into_retry_parts(self) -> (crate::DispatchTicket, crate::NativeChildListIssue) {
        (self.ticket, self.issue)
    }
}

#[derive(Debug)]
pub enum NativeChildListDispatchError {
    KnownUnsubmitted {
        error: ClientError,
        unsubmitted: Box<UnsubmittedNativeChildList>,
    },
    SubmissionUnknown(ClientError),
}

fn child_list_submit_refusal_is_unsubmitted(error: &ClientError) -> bool {
    use crate::guardian_ipc::WireGuardianError as G;
    matches!(
        error,
        ClientError::Refused(
            WireError::UnknownAdmission
                | WireError::AdmissionMismatch
                | WireError::MissingProof
                | WireError::UnsupportedDurableBinding
                | WireError::Guardian(
                    G::StaleSession
                        | G::StaleDispatchRevision
                        | G::NotActive
                        | G::SessionFenced
                        | G::AttemptMismatch
                        | G::InstanceMismatch
                        | G::DeadlineExpired
                        | G::AuthorityLost
                        | G::RetainedResultCapacityExceeded
                )
        )
    )
}

#[derive(Debug)]
pub struct UnsubmittedNativeIdentityObservation {
    ticket: crate::DispatchTicket,
    issue: crate::NativeIdentityObservationIssue,
}
impl UnsubmittedNativeIdentityObservation {
    #[must_use]
    pub fn into_retry_parts(
        self,
    ) -> (crate::DispatchTicket, crate::NativeIdentityObservationIssue) {
        (self.ticket, self.issue)
    }
}

#[derive(Debug)]
pub enum NativeIdentityObservationDispatchError {
    KnownUnsubmitted {
        error: ClientError,
        unsubmitted: Box<UnsubmittedNativeIdentityObservation>,
    },
    SubmissionUnknown(ClientError),
}

fn identity_observation_snapshot_matches(
    snapshot: &WireSnapshot,
    issue: crate::NativeIdentityObservationIssue,
) -> bool {
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
        && native.slots.len() == 1
        && native.slots[0].identity == issue.target().into()
}

fn identity_observation_ack_snapshot_matches(
    snapshot: &WireSnapshot,
    issue: crate::NativeIdentityObservationIssue,
) -> bool {
    let Some(native) = snapshot.native_binding.as_ref() else {
        return false;
    };
    let guardian = issue.guardian_identity();
    matches!(
        snapshot.state,
        crate::guardian_ipc::WireState::Active | crate::guardian_ipc::WireState::Reconciling
    ) && snapshot.attempt == issue.key().attempt_id().get()
        && snapshot.session.instance == snapshot.instance
        && snapshot.session.generation >= issue.session_generation().get()
        && native.attempt == snapshot.attempt
        && native.deadline == snapshot.deadline
        && native.nonce == guardian.nonce.bytes()
        && native.guardian == guardian.process.into()
        && native.slots.len() == 1
        && native.slots[0].identity == issue.target().into()
}

fn identity_observation_completion(
    completion: crate::guardian_ipc::WireCompletion,
    target: crate::ProcessIdentity,
) -> Result<crate::IdentityObservation, ClientError> {
    use crate::guardian_ipc::{
        WireCompletion, WireIdentityObservation as W, WireObservationFailure as F,
    };
    let WireCompletion::ObserveIdentity {
        identity,
        observation,
    } = completion
    else {
        return Err(ClientError::UnexpectedResponse);
    };
    if identity != target.into() {
        return Err(ClientError::UnexpectedResponse);
    }
    let decoded = match observation {
        W::SameIdentity { current_parent } => crate::IdentityObservation::SameIdentity {
            current_parent: current_parent
                .map(std::convert::TryInto::try_into)
                .transpose()
                .map_err(|_| ClientError::UnexpectedResponse)?,
        },
        W::ExactIdentityAbsent => crate::IdentityObservation::ExactIdentityAbsent,
        W::PidReused(replacement) => {
            let replacement: crate::ProcessIdentity = replacement
                .try_into()
                .map_err(|_| ClientError::UnexpectedResponse)?;
            if replacement.pid() != target.pid() || replacement == target {
                return Err(ClientError::UnexpectedResponse);
            }
            crate::IdentityObservation::PidReused(replacement)
        }
        W::Unresolved(failure) => crate::IdentityObservation::Unresolved(match failure {
            F::NoSuchProcess => crate::ObservationFailure::NoSuchProcess,
            F::PermissionDenied => crate::ObservationFailure::PermissionDenied,
            F::TemporarilyUnavailable => crate::ObservationFailure::TemporarilyUnavailable,
            F::Malformed => crate::ObservationFailure::Malformed,
        }),
    };
    Ok(decoded)
}

fn child_list_snapshot_matches(
    snapshot: &WireSnapshot,
    issue: crate::NativeChildListIssue,
) -> bool {
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
        && native.slots.len() == 1
        && native.slots[0].identity == issue.parent().into()
}

fn child_list_ack_snapshot_matches(
    snapshot: &WireSnapshot,
    issue: crate::NativeChildListIssue,
) -> bool {
    let Some(native) = snapshot.native_binding.as_ref() else {
        return false;
    };
    let guardian = issue.guardian_identity();
    matches!(
        snapshot.state,
        crate::guardian_ipc::WireState::Active | crate::guardian_ipc::WireState::Reconciling
    ) && snapshot.attempt == issue.key().attempt_id().get()
        && snapshot.session.instance == snapshot.instance
        && native.attempt == snapshot.attempt
        && native.deadline == snapshot.deadline
        && native.nonce == guardian.nonce.bytes()
        && native.guardian == guardian.process.into()
        && native.slots.len() == 1
        && native.slots[0].identity == issue.parent().into()
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
        self.exchange_with_slice(request, IO_SLICE)
    }

    fn exchange_with_slice(
        &mut self,
        request: &Request,
        slice: Duration,
    ) -> Result<Response, ClientError> {
        let result = (|| {
            let mut frame = DeadlineStream {
                stream: self
                    .stream
                    .as_mut()
                    .ok_or(ClientError::UnexpectedResponse)?,
                deadline: Instant::now() + slice,
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
            | Response::ReconciliationPrepared { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::RecoveredNativeSampling { .. }
            | Response::RecoveredNativeChildList { .. }
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
            | Response::ReconciliationPrepared { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::RecoveredNativeSampling { .. }
            | Response::RecoveredNativeChildList { .. }
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
            | Response::ReconciliationPrepared { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::RecoveredNativeSampling { .. }
            | Response::RecoveredNativeChildList { .. }
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
            | Response::ReconciliationPrepared { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::RecoveredNativeSampling { .. }
            | Response::RecoveredNativeChildList { .. }
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
            | Response::ReconciliationPrepared { .. }
            | Response::ReconciliationFinished
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::RecoveredNativeSampling { .. }
            | Response::RecoveredNativeChildList { .. }
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
            | Response::ReconciliationPrepared { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::Retained(_)
            | Response::RecoveredNativeSampling { .. }
            | Response::RecoveredNativeChildList { .. }
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
    /// Returns affine cancellation custody only for known nonexecution. Any ambiguous
    /// submission consumes the ticket and retains the durable issue for recovery.
    pub fn dispatch_native_sampling(
        &mut self,
        ticket: crate::DispatchTicket,
        issue: crate::NativeSamplingIssue,
    ) -> Result<crate::NativeSamplingDispatch, NativeSamplingDispatchError> {
        let prepared = (|| {
            if ticket.key() != issue.key()
                || !matches!(ticket.effect(), crate::OsEffect::SampleResources { sequence }
                    if *sequence == issue.sequence())
                || self.verified_snapshot.is_none()
            {
                return Err(ClientError::UnexpectedResponse);
            }
            let snapshot = self.snapshot()?;
            if !sampling_snapshot_matches(&snapshot, issue)
                || !crate::MonotonicTick::native_now()
                    .is_ok_and(|now| now.get() < snapshot.deadline)
            {
                return Err(ClientError::UnexpectedResponse);
            }
            let key = crate::guardian_ipc::WireKey::try_from(issue.key())
                .map_err(ClientError::Refused)?;
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
            let now = crate::MonotonicTick::native_now().map_err(ClientError::Io)?;
            if now.get() >= snapshot.deadline {
                return Err(ClientError::UnexpectedResponse);
            }
            Ok((snapshot.session, key, admission, now))
        })();
        let (session, key, admission, now) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                // EOF fences the connection and clears any harmless pre-submit admission.
                self.stream.take();
                self.verified_snapshot = None;
                return Err(NativeSamplingDispatchError::KnownUnsubmitted {
                    error,
                    unsubmitted: Box::new(UnsubmittedNativeSampling { ticket, issue }),
                });
            }
        };
        let result = self.exchange_with_slice(
            &Request::DispatchSubmit {
                session,
                admission,
                now: now.get(),
            },
            EFFECT_RESPONSE_SLICE,
        );
        match result {
            Ok(Response::Submitted { key: returned }) if returned == key => {
                Ok(crate::NativeSamplingDispatch::new(ticket, issue))
            }
            result => {
                let error = result.err().unwrap_or(ClientError::UnexpectedResponse);
                self.stream.take();
                self.verified_snapshot = None;
                if sampling_submit_refusal_is_unsubmitted(&error) {
                    Err(NativeSamplingDispatchError::KnownUnsubmitted {
                        error,
                        unsubmitted: Box::new(UnsubmittedNativeSampling { ticket, issue }),
                    })
                } else {
                    Err(NativeSamplingDispatchError::SubmissionUnknown(error))
                }
            }
        }
    }

    /// Submit a policy-issued exact-root identity observation, retaining the ticket only on known nonexecution.
    /// # Errors
    /// Ambiguous post-submit outcomes consume the dispatch and require reconciliation.
    pub fn dispatch_native_identity_observation(
        &mut self,
        ticket: crate::DispatchTicket,
        issue: crate::NativeIdentityObservationIssue,
    ) -> Result<crate::NativeIdentityObservationDispatch, NativeIdentityObservationDispatchError>
    {
        let prepared = (|| {
            if ticket.key() != issue.key()
                || !matches!(ticket.effect(), crate::OsEffect::ObserveIdentity { identity } if *identity == issue.target())
                || self.verified_snapshot.is_none()
            {
                return Err(ClientError::UnexpectedResponse);
            }
            let snapshot = self.snapshot()?;
            if !identity_observation_snapshot_matches(&snapshot, issue)
                || !crate::MonotonicTick::native_now()
                    .is_ok_and(|now| now.get() < snapshot.deadline)
            {
                return Err(ClientError::UnexpectedResponse);
            }
            let key = crate::guardian_ipc::WireKey::try_from(issue.key())
                .map_err(ClientError::Refused)?;
            let Response::Admitted { admission } =
                self.exchange(&Request::NativeIdentityObservationAdmit {
                    session: snapshot.session,
                    key,
                })?
            else {
                return Err(ClientError::UnexpectedResponse);
            };
            let now = crate::MonotonicTick::native_now().map_err(ClientError::Io)?;
            if now.get() >= snapshot.deadline {
                return Err(ClientError::UnexpectedResponse);
            }
            Ok((snapshot.session, key, admission, now))
        })();
        let (session, key, admission, now) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.stream.take();
                self.verified_snapshot = None;
                return Err(NativeIdentityObservationDispatchError::KnownUnsubmitted {
                    error,
                    unsubmitted: Box::new(UnsubmittedNativeIdentityObservation { ticket, issue }),
                });
            }
        };
        let result = self.exchange_with_slice(
            &Request::NativeIdentityObservationSubmit {
                session,
                admission,
                now: now.get(),
            },
            EFFECT_RESPONSE_SLICE,
        );
        match result {
            Ok(Response::Submitted { key: returned }) if returned == key => {
                Ok(crate::NativeIdentityObservationDispatch::new(ticket, issue))
            }
            result => {
                let error = result.err().unwrap_or(ClientError::UnexpectedResponse);
                self.stream.take();
                self.verified_snapshot = None;
                if child_list_submit_refusal_is_unsubmitted(&error) {
                    Err(NativeIdentityObservationDispatchError::KnownUnsubmitted {
                        error,
                        unsubmitted: Box::new(UnsubmittedNativeIdentityObservation {
                            ticket,
                            issue,
                        }),
                    })
                } else {
                    Err(NativeIdentityObservationDispatchError::SubmissionUnknown(
                        error,
                    ))
                }
            }
        }
    }

    /// Verify the retained completion against exact key, target identity and outcome shape.
    /// # Errors
    /// Refuses stale or mismatched guardian evidence or an unrelated retained completion.
    pub fn observe_native_identity_observation(
        &mut self,
        dispatch: crate::NativeIdentityObservationDispatch,
    ) -> Result<crate::VerifiedNativeIdentityObservationCompletion, ClientError> {
        let issue = dispatch.issue();
        if self.verified_snapshot.is_none() {
            return Err(ClientError::UnexpectedResponse);
        }
        let snapshot = self.snapshot()?;
        if !identity_observation_snapshot_matches(&snapshot, issue) {
            return Err(ClientError::UnexpectedResponse);
        }
        let key =
            crate::guardian_ipc::WireKey::try_from(issue.key()).map_err(ClientError::Refused)?;
        let Response::Retained(Some(retained)) =
            self.exchange(&Request::ObserveNativeIdentityObservation {
                session: snapshot.session,
                key,
            })?
        else {
            return Err(ClientError::UnexpectedResponse);
        };
        if retained.key != key {
            return Err(ClientError::UnexpectedResponse);
        }
        let observation = identity_observation_completion(retained.completion, issue.target())?;
        Ok(crate::VerifiedNativeIdentityObservationCompletion::new(
            dispatch,
            observation,
        ))
    }

    /// Confirm an ACK only after the exact durable application and correlated guardian receipt.
    /// # Errors
    /// Rejects stale evidence, an unmatched ACK receipt, or transport failure.
    pub fn ack_native_identity_observation(
        &mut self,
        status: &crate::NativeIdentityObservationStatus,
    ) -> Result<crate::VerifiedNativeIdentityObservationAckConfirmation, ClientError> {
        let &crate::NativeIdentityObservationStatus::AppliedPendingAck {
            issue,
            observation,
            applying_revision,
        } = status
        else {
            return Err(ClientError::UnexpectedResponse);
        };
        if self.verified_snapshot.is_none() {
            return Err(ClientError::UnexpectedResponse);
        }
        let snapshot = self.snapshot()?;
        if !identity_observation_ack_snapshot_matches(&snapshot, issue)
            || !crate::MonotonicTick::native_now().is_ok_and(|now| now.get() < snapshot.deadline)
        {
            return Err(ClientError::UnexpectedResponse);
        }
        let key =
            crate::guardian_ipc::WireKey::try_from(issue.key()).map_err(ClientError::Refused)?;
        let Response::Acked {
            key: returned,
            applied_revision: returned_revision,
        } = self.exchange(&Request::AckNativeIdentityObservationAfterApplied {
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
        Ok(
            crate::VerifiedNativeIdentityObservationAckConfirmation::new(
                issue,
                observation,
                applying_revision,
                generation,
            ),
        )
    }

    /// Consumes and submits an exact durable root child-list issue.
    /// # Errors
    /// Returns the original authority only when execution is proven not to have started; ambiguous
    /// post-submit outcomes remain `SubmissionUnknown` and return no retry authority.
    pub fn dispatch_native_child_list(
        &mut self,
        ticket: crate::DispatchTicket,
        issue: crate::NativeChildListIssue,
    ) -> Result<crate::NativeChildListDispatch, NativeChildListDispatchError> {
        let prepared = (|| {
            if ticket.key() != issue.key()
                || !matches!(ticket.effect(), crate::OsEffect::ListChildren { parent } if *parent == issue.parent())
                || self.verified_snapshot.is_none()
            {
                return Err(ClientError::UnexpectedResponse);
            }
            let snapshot = self.snapshot()?;
            if !child_list_snapshot_matches(&snapshot, issue)
                || !crate::MonotonicTick::native_now()
                    .is_ok_and(|now| now.get() < snapshot.deadline)
            {
                return Err(ClientError::UnexpectedResponse);
            }
            let key = crate::guardian_ipc::WireKey::try_from(issue.key())
                .map_err(ClientError::Refused)?;
            let Response::Admitted { admission } =
                self.exchange(&Request::NativeChildListAdmit {
                    session: snapshot.session,
                    key,
                    parent: issue.parent().into(),
                })?
            else {
                return Err(ClientError::UnexpectedResponse);
            };
            let now = crate::MonotonicTick::native_now().map_err(ClientError::Io)?;
            if now.get() >= snapshot.deadline {
                return Err(ClientError::UnexpectedResponse);
            }
            Ok((snapshot.session, key, admission, now))
        })();
        let (session, key, admission, now) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.stream.take();
                self.verified_snapshot = None;
                return Err(NativeChildListDispatchError::KnownUnsubmitted {
                    error,
                    unsubmitted: Box::new(UnsubmittedNativeChildList { ticket, issue }),
                });
            }
        };
        let result = self.exchange_with_slice(
            &Request::NativeChildListSubmit {
                session,
                admission,
                now: now.get(),
            },
            EFFECT_RESPONSE_SLICE,
        );
        match result {
            Ok(Response::Submitted { key: returned }) if returned == key => {
                Ok(crate::NativeChildListDispatch::new(ticket, issue))
            }
            result => {
                let error = result.err().unwrap_or(ClientError::UnexpectedResponse);
                self.stream.take();
                self.verified_snapshot = None;
                if child_list_submit_refusal_is_unsubmitted(&error) {
                    Err(NativeChildListDispatchError::KnownUnsubmitted {
                        error,
                        unsubmitted: Box::new(UnsubmittedNativeChildList { ticket, issue }),
                    })
                } else {
                    Err(NativeChildListDispatchError::SubmissionUnknown(error))
                }
            }
        }
    }

    /// Read and verify the exact retained root child-list completion.
    /// # Errors
    /// Refuses stale snapshots, mismatched custody, malformed child vectors, or wrong parents.
    pub fn observe_native_child_list(
        &mut self,
        dispatch: crate::NativeChildListDispatch,
    ) -> Result<crate::VerifiedNativeChildListCompletion, ClientError> {
        let issue = dispatch.issue();
        if self.verified_snapshot.is_none() {
            return Err(ClientError::UnexpectedResponse);
        }
        let snapshot = self.snapshot()?;
        if !child_list_snapshot_matches(&snapshot, issue) {
            return Err(ClientError::UnexpectedResponse);
        }
        let key =
            crate::guardian_ipc::WireKey::try_from(issue.key()).map_err(ClientError::Refused)?;
        let retained = match self.exchange(&Request::ObserveNativeChildList {
            session: snapshot.session,
            key,
        })? {
            Response::Retained(Some(retained)) if retained.key == key => retained,
            Response::Hello { .. }
            | Response::Snapshot(_)
            | Response::Fenced
            | Response::ReconciliationBegun { .. }
            | Response::InitialReleased { .. }
            | Response::ReconciliationPrepared { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::RecoveredNativeSampling { .. }
            | Response::RecoveredNativeChildList { .. }
            | Response::Acked { .. }
            | Response::SignalRoundReserved
            | Response::Admitted { .. }
            | Response::Submitted { .. }
            | Response::Refused(_) => return Err(ClientError::UnexpectedResponse),
        };
        let crate::guardian_ipc::WireCompletion::ListChildren { parent, result } =
            retained.completion
        else {
            return Err(ClientError::UnexpectedResponse);
        };
        let parent: crate::ProcessIdentity = parent
            .try_into()
            .map_err(|_| ClientError::UnexpectedResponse)?;
        if parent != issue.parent() {
            return Err(ClientError::UnexpectedResponse);
        }
        let completion = match result {
            Ok(children) => {
                if children.len() > crate::guardian_ipc::MAX_NATIVE_CHILDREN {
                    return Err(ClientError::UnexpectedResponse);
                }
                let children = children
                    .into_iter()
                    .map(|pid| std::num::NonZeroU32::new(pid).map(crate::ProcessId::new))
                    .collect::<Option<Vec<_>>>()
                    .ok_or(ClientError::UnexpectedResponse)?;
                crate::NativeChildListCompletion::Children(children)
            }
            Err(error) => crate::NativeChildListCompletion::Failure(match error {
                crate::guardian_ipc::WireQueryError::PermissionDenied => {
                    crate::OsQueryError::PermissionDenied
                }
                crate::guardian_ipc::WireQueryError::TemporarilyUnavailable => {
                    crate::OsQueryError::TemporarilyUnavailable
                }
                crate::guardian_ipc::WireQueryError::Malformed => crate::OsQueryError::Malformed,
            }),
        };
        Ok(crate::VerifiedNativeChildListCompletion::new(
            dispatch, completion,
        ))
    }

    /// ACK only the exact successful durable application after its result was persisted.
    /// # Errors
    /// Refuses absent application evidence, mismatched retained custody, stale sessions, or an
    /// uncorrelated ACK response.
    pub fn ack_native_child_list(
        &mut self,
        status: crate::NativeChildListStatus,
    ) -> Result<crate::VerifiedNativeChildListAckConfirmation, ClientError> {
        let crate::NativeChildListStatus::AppliedPendingAck {
            issue,
            children,
            applying_revision,
        } = status
        else {
            return Err(ClientError::UnexpectedResponse);
        };
        if self.verified_snapshot.is_none() {
            return Err(ClientError::UnexpectedResponse);
        }
        let snapshot = self.snapshot()?;
        if !child_list_ack_snapshot_matches(&snapshot, issue)
            || !crate::MonotonicTick::native_now().is_ok_and(|now| now.get() < snapshot.deadline)
        {
            return Err(ClientError::UnexpectedResponse);
        }
        let key =
            crate::guardian_ipc::WireKey::try_from(issue.key()).map_err(ClientError::Refused)?;
        let Response::Acked {
            key: returned,
            applied_revision: returned_revision,
        } = self.exchange(&Request::AckNativeChildListAfterApplied {
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
        Ok(crate::VerifiedNativeChildListAckConfirmation::new(
            issue,
            children,
            applying_revision,
            generation,
        ))
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
            | Response::ReconciliationPrepared { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::RecoveredNativeSampling { .. }
            | Response::RecoveredNativeChildList { .. }
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
    /// Verify application-only recovery against fresh live and pinned durable evidence.
    /// No dispatch, ACK, or transition authority is returned.
    /// # Errors
    /// Refuses mismatched identity, session, custody or response; transport uncertainty fences.
    pub fn verify_recovered_native_sampling(
        &mut self,
        candidate: crate::NativeSamplingRecoveryCandidate,
    ) -> Result<crate::VerifiedRecoveredNativeSampling, ClientError> {
        let result = (|| {
            if self.verified_snapshot.is_none() {
                return Err(ClientError::UnexpectedResponse);
            }
            let snapshot = self.snapshot()?;
            let binding = candidate.binding();
            let issue = candidate.issue();
            let native = snapshot
                .native_binding
                .as_ref()
                .ok_or(ClientError::UnexpectedResponse)?;
            let current = binding.current_session();
            if snapshot.state != crate::guardian_ipc::WireState::Reconciling
                || snapshot.session.instance != snapshot.instance
                || snapshot.session.generation != current.generation.get()
                || current.state != crate::NativeSessionState::Reconciling
                || current.generation <= issue.session_generation()
                || binding.dispatch_floor() != current.interruption_revision
                || !binding
                    .sessions()
                    .iter()
                    .any(|s| s.generation == issue.session_generation())
                || binding.identity() != issue.guardian_identity()
                || binding.attempt_id() != issue.key().attempt_id()
                || snapshot.attempt != binding.attempt_id().get()
                || snapshot.deadline != binding.deadline().get()
                || native.attempt != snapshot.attempt
                || native.deadline != snapshot.deadline
                || native.nonce != binding.identity().nonce.bytes()
                || native.guardian != binding.identity().process.into()
                || !recovered_sampling_slots_match(binding, &snapshot, native)
                || !crate::MonotonicTick::native_now()
                    .is_ok_and(|now| now.get() < snapshot.deadline)
            {
                return Err(ClientError::UnexpectedResponse);
            }
            let key = crate::guardian_ipc::WireKey::try_from(issue.key())
                .map_err(ClientError::Refused)?;
            let response = self.exchange(&Request::VerifyRecoveredNativeSampling {
                session: snapshot.session,
                key,
                sequence: issue.sequence(),
                issuing_session_generation: issue.session_generation().get(),
            })?;
            let Response::RecoveredNativeSampling {
                key: returned,
                sequence,
                sample,
            } = response
            else {
                return Err(ClientError::UnexpectedResponse);
            };
            if returned != key || sequence != issue.sequence() || sample.sequence != sequence {
                return Err(ClientError::UnexpectedResponse);
            }
            let cpu = crate::CpuContentionBasisPoints::new(sample.cpu_basis_points)
                .map_err(|_| ClientError::UnexpectedResponse)?;
            let pressure = match sample.memory_pressure {
                crate::guardian_ipc::WireMemoryPressure::Normal => crate::MemoryPressure::Nominal,
                crate::guardian_ipc::WireMemoryPressure::Elevated => crate::MemoryPressure::Warning,
                crate::guardian_ipc::WireMemoryPressure::Critical => {
                    crate::MemoryPressure::Critical
                }
            };
            Ok(
                crate::VerifiedRecoveredNativeSampling::after_live_verification(
                    candidate,
                    crate::ResourceSample {
                        sequence,
                        capacity: crate::ResourceCapacity {
                            available_memory_bytes: sample.memory_bytes,
                            available_storage_bytes: sample.storage_bytes,
                        },
                        contention: crate::ResourceContention {
                            cpu,
                            memory_pressure: pressure,
                        },
                    },
                ),
            )
        })();
        if result.is_err() {
            self.stream.take();
            self.verified_snapshot = None;
        }
        result
    }

    /// Verify a retained successful child list in the fresh same-guardian reconciling session.
    /// # Errors
    /// Refuses stale custody or mismatched responses and fences this connection.
    pub fn verify_recovered_native_child_list(
        &mut self,
        candidate: crate::NativeChildListRecoveryCandidate,
    ) -> Result<crate::VerifiedRecoveredNativeChildList, ClientError> {
        let result = (|| {
            if self.verified_snapshot.is_none() {
                return Err(ClientError::UnexpectedResponse);
            }
            let snapshot = self.snapshot()?;
            let binding = candidate.binding();
            let issue = candidate.issue();
            let native = snapshot
                .native_binding
                .as_ref()
                .ok_or(ClientError::UnexpectedResponse)?;
            let current = binding.current_session();
            if snapshot.state != crate::guardian_ipc::WireState::Reconciling
                || snapshot.session.instance != snapshot.instance
                || snapshot.session.generation != current.generation.get()
                || current.state != crate::NativeSessionState::Reconciling
                || current.generation <= issue.session_generation()
                || !binding
                    .sessions()
                    .iter()
                    .any(|s| s.generation == issue.session_generation())
                || binding.dispatch_floor() != current.interruption_revision
                || issue.key().revision() >= current.interruption_revision
                || binding.identity() != issue.guardian_identity()
                || binding.attempt_id() != issue.key().attempt_id()
                || snapshot.attempt != binding.attempt_id().get()
                || snapshot.deadline != binding.deadline().get()
                || native.attempt != snapshot.attempt
                || native.deadline != snapshot.deadline
                || native.nonce != binding.identity().nonce.bytes()
                || native.guardian != binding.identity().process.into()
                || binding.slots().len() != 1
                || binding.slots()[0].process != issue.parent()
                || !recovered_sampling_slots_match(binding, &snapshot, native)
                || !crate::MonotonicTick::native_now()
                    .is_ok_and(|now| now.get() < snapshot.deadline)
            {
                return Err(ClientError::UnexpectedResponse);
            }
            let key = crate::guardian_ipc::WireKey::try_from(issue.key())
                .map_err(ClientError::Refused)?;
            let response = self.exchange(&Request::VerifyRecoveredNativeChildList {
                session: snapshot.session,
                key,
                parent: issue.parent().into(),
                issuing_session_generation: issue.session_generation().get(),
            })?;
            let Response::RecoveredNativeChildList {
                key: returned,
                parent,
                children,
            } = response
            else {
                return Err(ClientError::UnexpectedResponse);
            };
            if returned != key
                || parent != issue.parent().into()
                || children.len() > crate::guardian_ipc::MAX_NATIVE_CHILDREN
            {
                return Err(ClientError::UnexpectedResponse);
            }
            let children = children
                .into_iter()
                .map(|pid| std::num::NonZeroU32::new(pid).map(crate::ProcessId::new))
                .collect::<Option<Vec<_>>>()
                .ok_or(ClientError::UnexpectedResponse)?;
            Ok(
                crate::VerifiedRecoveredNativeChildList::after_live_verification(
                    candidate, children,
                ),
            )
        })();
        if result.is_err() {
            self.stream.take();
            self.verified_snapshot = None;
        }
        result
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
            | Response::ReconciliationPrepared { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::RecoveredNativeSampling { .. }
            | Response::RecoveredNativeChildList { .. }
            | Response::Acked { .. }
            | Response::SignalRoundReserved
            | Response::Admitted { .. }
            | Response::Submitted { .. }
            | Response::Refused(_) => Err(ClientError::UnexpectedResponse),
        }
    }

    /// Prepares live activation using locators, without making the guardian Active.
    /// # Errors
    /// Refuses stale/missing evidence, wrong response, deadline expiry, or transport failure.
    pub fn finish_reconciliation(
        &mut self,
        session: crate::guardian_ipc::WireSession,
        revision: crate::PersistenceRevision,
    ) -> Result<crate::PreparedNativeReconciliation, ClientError> {
        let snapshot = self.snapshot()?;
        let native = snapshot
            .native_binding
            .as_ref()
            .ok_or(ClientError::UnexpectedResponse)?;
        if snapshot.session != session
            || snapshot.state != crate::guardian_ipc::WireState::Reconciling
        {
            return Err(ClientError::UnexpectedResponse);
        }
        let identity = crate::NativeGuardianIdentity {
            nonce: crate::GuardianLifetimeNonce::new(native.nonce)
                .ok_or(ClientError::UnexpectedResponse)?,
            process: native
                .guardian
                .try_into()
                .map_err(|_| ClientError::UnexpectedResponse)?,
        };
        let now = crate::MonotonicTick::native_now().map_err(ClientError::Io)?;
        let response = self.exchange(&Request::FinishReconciliation {
            session,
            applied_revision: revision.get(),
            now: now.get(),
        })?;
        if response
            != (Response::ReconciliationPrepared {
                session,
                attempt: snapshot.attempt,
                recovery_revision: revision.get(),
            })
        {
            return Err(ClientError::UnexpectedResponse);
        }
        self.verified_snapshot = None;
        Ok(crate::PreparedNativeReconciliation::after_live_prepare(
            session,
            crate::AttemptId::new(snapshot.attempt),
            identity,
            crate::MonotonicTick::new(snapshot.deadline),
            revision,
        ))
    }

    /// Consumes proof of durable activation; only locator fields cross the wire.
    /// # Errors
    /// Refuses a missing/mismatched staged capability, expired deadline, fence, or replay.
    pub fn activate_reconciliation(
        &mut self,
        committed: crate::CommittedNativeReconciliationActivation,
    ) -> Result<(), ClientError> {
        let (session, attempt, recovery_revision, binding_revision) = committed.into_locator();
        let response = self.exchange(&Request::ActivateReconciliation {
            session,
            attempt: attempt.get(),
            recovery_revision: recovery_revision.get(),
            binding_revision: binding_revision.get(),
        })?;
        if response != Response::ReconciliationFinished {
            return Err(ClientError::UnexpectedResponse);
        }
        self.verified_snapshot = None;
        Ok(())
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
            | Response::ReconciliationPrepared { .. }
            | Response::ReconciliationFinished
            | Response::Inventory(_)
            | Response::SlotObserved(_)
            | Response::Retained(_)
            | Response::RecoveredNativeSampling { .. }
            | Response::RecoveredNativeChildList { .. }
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

/// Applied native identity-observation ACK recovery; no dispatch authority is recreated.
#[derive(Debug)]
pub enum NativeIdentityObservationAckRecoveryError {
    Store(crate::ControllerStoreError),
    Guardian(ClientError),
    NoPendingApplication,
}

/// # Errors
/// Refuses absent or unapplied evidence, guardian mismatch or store failure.
pub fn recover_native_identity_observation_ack(
    store: &mut crate::ControllerSqliteStore,
    client: &mut GuardianClient,
    key: crate::DispatchKey,
) -> Result<crate::NativeIdentityObservationStatus, NativeIdentityObservationAckRecoveryError> {
    let (_, _, status) = store
        .read_native_identity_observation_evidence(key)
        .map_err(NativeIdentityObservationAckRecoveryError::Store)?
        .ok_or(NativeIdentityObservationAckRecoveryError::NoPendingApplication)?;
    if matches!(
        status,
        crate::NativeIdentityObservationStatus::AckConfirmed { .. }
    ) {
        return Ok(status);
    }
    if !matches!(
        status,
        crate::NativeIdentityObservationStatus::AppliedPendingAck { .. }
    ) {
        return Err(NativeIdentityObservationAckRecoveryError::NoPendingApplication);
    }
    let proof = client
        .ack_native_identity_observation(&status)
        .map_err(NativeIdentityObservationAckRecoveryError::Guardian)?;
    store
        .confirm_native_identity_observation_ack(proof)
        .map_err(NativeIdentityObservationAckRecoveryError::Store)
}

/// Diagnostic child-list recovery ACK; it never recreates a ticket or policy loop.
#[derive(Debug)]
pub enum NativeChildListAckRecoveryError {
    Store(crate::ControllerStoreError),
    Guardian(ClientError),
    NoPendingApplication,
}

/// # Errors
/// Refuses absent/failed application, guardian mismatch, or store failure.
pub fn recover_native_child_list_ack(
    store: &mut crate::ControllerSqliteStore,
    client: &mut GuardianClient,
    key: crate::DispatchKey,
) -> Result<crate::NativeChildListStatus, NativeChildListAckRecoveryError> {
    let (_, _, status) = store
        .read_native_child_list_evidence(key)
        .map_err(NativeChildListAckRecoveryError::Store)?
        .ok_or(NativeChildListAckRecoveryError::NoPendingApplication)?;
    if matches!(
        status,
        crate::NativeChildListStatus::ChildrenAckConfirmed { .. }
    ) {
        return Ok(status);
    }
    if !matches!(
        status,
        crate::NativeChildListStatus::AppliedPendingAck { .. }
    ) {
        return Err(NativeChildListAckRecoveryError::NoPendingApplication);
    }
    let proof = client
        .ack_native_child_list(status)
        .map_err(NativeChildListAckRecoveryError::Guardian)?;
    store
        .confirm_native_child_list_ack(proof)
        .map_err(NativeChildListAckRecoveryError::Store)
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

    #[test]
    fn identity_completion_rejects_wrong_target_and_reused_birth() {
        use crate::guardian_ipc::{WireCompletion, WireIdentityObservation};
        let target = process(80);
        let wrong = process(81);
        assert!(matches!(
            identity_observation_completion(
                WireCompletion::ObserveIdentity {
                    identity: wrong.into(),
                    observation: WireIdentityObservation::ExactIdentityAbsent,
                },
                target,
            ),
            Err(ClientError::UnexpectedResponse)
        ));
        assert!(matches!(
            identity_observation_completion(
                WireCompletion::ObserveIdentity {
                    identity: target.into(),
                    observation: WireIdentityObservation::PidReused(target.into()),
                },
                target,
            ),
            Err(ClientError::UnexpectedResponse)
        ));
        assert!(matches!(
            identity_observation_completion(
                WireCompletion::ListChildren {
                    parent: target.into(),
                    result: Ok(vec![])
                },
                target,
            ),
            Err(ClientError::UnexpectedResponse)
        ));
        assert_eq!(
            identity_observation_completion(
                WireCompletion::ObserveIdentity {
                    identity: target.into(),
                    observation: WireIdentityObservation::ExactIdentityAbsent,
                },
                target,
            )
            .unwrap(),
            crate::IdentityObservation::ExactIdentityAbsent
        );
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

    fn dispatch_fixture(
        path: &Path,
        deadline: MonotonicTick,
    ) -> (
        ControllerSqliteStore,
        PolicyLoop,
        DispatchTicket,
        crate::NativeSamplingIssue,
        WireSnapshot,
    ) {
        let mut store = ControllerSqliteStore::open(path).unwrap();
        let attempt = crate::AttemptId::new(91);
        let committed = DeploymentAttempt::start(
            attempt,
            deadline,
            ResourcePolicy::new(
                NonZeroUsize::new(2).unwrap(),
                0,
                0,
                CpuContentionBasisPoints::new(100).unwrap(),
                MemoryPressure::Critical,
            ),
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
                attempt,
                deadline,
                PersistenceRevision::first(),
                identity,
                &[],
            )
            .unwrap();
        let mut policy = PolicyLoop::from_committed(committed);
        let ticket = policy.issue_next().unwrap();
        let issue = store
            .commit_native_sampling_issue(&ticket, &binding)
            .unwrap();
        let snapshot = WireSnapshot {
            instance: 7,
            attempt: attempt.get(),
            deadline: deadline.get(),
            state: crate::guardian_ipc::WireState::Active,
            session: crate::guardian_ipc::WireSession {
                instance: 7,
                generation: 1,
            },
            slots: vec![],
            retained: vec![],
            native_binding: Some(crate::guardian_ipc::WireNativeBinding {
                attempt: attempt.get(),
                deadline: deadline.get(),
                nonce: identity.nonce.bytes(),
                guardian: identity.process.into(),
                slots: vec![],
            }),
        };
        (store, policy, ticket, issue, snapshot)
    }

    fn unsubmitted_locally(
        ticket: DispatchTicket,
        issue: crate::NativeSamplingIssue,
    ) -> UnsubmittedNativeSampling {
        let mut client = GuardianClient {
            stream: None,
            build: BuildIdentity::compiled(),
            verified_snapshot: None,
        };
        let NativeSamplingDispatchError::KnownUnsubmitted { unsubmitted, .. } =
            client.dispatch_native_sampling(ticket, issue).unwrap_err()
        else {
            panic!("local validation must preserve cancellation custody")
        };
        *unsubmitted
    }

    fn durable_rows(path: &Path) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
        let db = rusqlite::Connection::open(path).unwrap();
        let tables = db
            .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        tables
            .into_iter()
            .map(|table| {
                let mut query = db
                    .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                    .unwrap();
                let columns = query.column_count();
                let rows = query
                    .query_map([], |row| {
                        (0..columns)
                            .map(|i| row.get(i))
                            .collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                (table, rows)
            })
            .collect()
    }

    #[test]
    fn sqlite_cancel_unsubmitted_is_exact_atomic_and_rejects_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel.sqlite");
        let deadline = MonotonicTick::new(100);
        let (mut store, _, ticket, issue, _) = dispatch_fixture(&path, deadline);
        let cap = unsubmitted_locally(ticket, issue);
        let mut expected = durable_rows(&path);
        expected.get_mut("native_sampling_issues").unwrap().clear();
        store.cancel_unsubmitted_native_sampling(cap).unwrap();
        assert_eq!(durable_rows(&path), expected);
        assert_eq!(
            store.load_native_sampling_status(issue.key()).unwrap(),
            None
        );
        drop(store);
        let mut reopened = ControllerSqliteStore::open_existing(&path).unwrap();
        assert_eq!(
            reopened.load_native_sampling_status(issue.key()).unwrap(),
            None
        );
        assert_eq!(
            reopened
                .load_attempt(issue.key().attempt_id())
                .unwrap()
                .unwrap()
                .phase(),
            crate::Phase::ResourceSampling
        );

        // Independent affine custody with the same identity cannot cancel an absent row.
        let (_, _, ticket, same_issue, _) =
            dispatch_fixture(&dir.path().join("other.sqlite"), deadline);
        assert_eq!(same_issue, issue);
        let replay = unsubmitted_locally(ticket, same_issue);
        assert!(reopened.cancel_unsubmitted_native_sampling(replay).is_err());
        assert_eq!(durable_rows(&path), expected);
        assert!(matches!(
            reopened.load_native_sampling_status(issue.key()),
            Err(crate::ControllerStoreError::Poisoned)
        ));
    }

    #[test]
    fn sqlite_cancel_unsubmitted_rolls_back_delete_failure_and_poisons() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("atomic.sqlite");
        let (mut store, _, ticket, issue, _) = dispatch_fixture(&path, MonotonicTick::new(100));
        let cap = unsubmitted_locally(ticket, issue);
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_cancel AFTER DELETE ON native_sampling_issues BEGIN
             UPDATE sampling_obligations SET pending_sequence=x'0000000000000009';
             SELECT RAISE(ABORT, 'cancel fixture'); END;",
            )
            .unwrap();
        let before = durable_rows(&path);
        assert!(store.cancel_unsubmitted_native_sampling(cap).is_err());
        assert_eq!(durable_rows(&path), before);
        assert!(matches!(
            store.load_native_sampling_status(issue.key()),
            Err(crate::ControllerStoreError::Poisoned)
        ));
        drop(store);
        assert_eq!(
            ControllerSqliteStore::open_existing(&path)
                .unwrap()
                .load_native_sampling_status(issue.key())
                .unwrap(),
            Some(crate::NativeSamplingStatus::IssuedPendingApplication { issue })
        );
    }

    #[test]
    fn sqlite_cancel_unsubmitted_refuses_row_key_sequence_and_identity_mismatches() {
        for mutation in [
            "UPDATE native_sampling_issues SET sequence=x'0000000000000007'",
            "UPDATE guardian_sessions SET generation=x'0000000000000002'; UPDATE native_sampling_issues SET session_generation=x'0000000000000002'",
            "UPDATE native_sampling_issues SET dispatch_revision=x'0000000000000002'",
            "UPDATE guardian_bindings SET nonce=x'06060606060606060606060606060606'",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("mismatch.sqlite");
            let (mut store, _, ticket, issue, _) = dispatch_fixture(&path, MonotonicTick::new(100));
            let cap = unsubmitted_locally(ticket, issue);
            let raw = rusqlite::Connection::open(&path).unwrap();
            raw.pragma_update(None, "foreign_keys", false).unwrap();
            raw.execute_batch(mutation).unwrap();
            let before = durable_rows(&path);
            assert!(store.cancel_unsubmitted_native_sampling(cap).is_err());
            assert_eq!(durable_rows(&path), before, "{mutation}");
            assert!(matches!(store.load_native_sampling_status(issue.key()), Err(crate::ControllerStoreError::Poisoned)));
        }
    }

    #[test]
    fn sqlite_cancel_unsubmitted_refuses_applied_and_confirmed_issues() {
        for confirmed in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settled.sqlite");
            let (mut store, mut policy, ticket, issue, _) =
                dispatch_fixture(&path, MonotonicTick::new(100));
            // Two independently issued affine tickets model a write winning before cancellation.
            let (_, _, other_ticket, other_issue, _) =
                dispatch_fixture(&dir.path().join("other.sqlite"), MonotonicTick::new(100));
            assert_eq!(issue, other_issue);
            let cap = unsubmitted_locally(other_ticket, other_issue);
            let sample = ResourceSample {
                sequence: issue.sequence(),
                capacity: ResourceCapacity {
                    available_memory_bytes: 100,
                    available_storage_bytes: 100,
                },
                contention: ResourceContention {
                    cpu: CpuContentionBasisPoints::new(0).unwrap(),
                    memory_pressure: MemoryPressure::Nominal,
                },
            };
            policy
                .apply_native_sampling(
                    crate::VerifiedNativeSamplingCompletion::new(
                        crate::NativeSamplingDispatch::new(ticket, issue),
                        crate::NativeSamplingCompletion::Sample(sample),
                    ),
                    &mut store,
                )
                .unwrap();
            if confirmed {
                let crate::NativeSamplingStatus::AppliedPendingAck {
                    applying_revision, ..
                } = store
                    .load_native_sampling_status(issue.key())
                    .unwrap()
                    .unwrap()
                else {
                    panic!("applied")
                };
                store
                    .confirm_native_sampling_ack(crate::VerifiedNativeSamplingAckConfirmation::new(
                        issue,
                        sample,
                        applying_revision,
                        issue.session_generation(),
                    ))
                    .unwrap();
            }
            let before = durable_rows(&path);
            assert!(store.cancel_unsubmitted_native_sampling(cap).is_err());
            assert_eq!(durable_rows(&path), before);
            assert!(matches!(
                store.load_native_sampling_status(issue.key()),
                Err(crate::ControllerStoreError::Poisoned)
            ));
        }
    }

    fn recovered_candidate_fixture() -> (
        tempfile::TempDir,
        ControllerSqliteStore,
        crate::NativeSamplingRecoveryCandidate,
        WireSnapshot,
        crate::NativeSamplingIssue,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let deadline = MonotonicTick::native_now()
            .unwrap()
            .checked_add(Duration::from_secs(120))
            .unwrap();
        let path = dir.path().join("recovery.sqlite");
        let (mut store, _, _ticket, issue, mut snapshot) = dispatch_fixture(&path, deadline);
        let binding = store
            .load_native_binding(issue.key().attempt_id())
            .unwrap()
            .unwrap();
        let detached = store
            .commit_detached_binding(&binding, binding.slots())
            .unwrap();
        let loaded = store
            .load_attempt(issue.key().attempt_id())
            .unwrap()
            .unwrap();
        let crate::RecoveryDisposition::Checkpoint(interruption) =
            loaded.prepare_recovery().unwrap()
        else {
            panic!("interruption")
        };
        let revision = interruption
            .persist(&mut store)
            .unwrap()
            .checkpoint()
            .persistence_revision()
            .unwrap();
        let binding = store
            .commit_fresh_interruption_binding(&detached, revision)
            .unwrap();
        let loaded = store
            .load_attempt(issue.key().attempt_id())
            .unwrap()
            .unwrap();
        let candidate = store.prepare_native_sampling_recovery(loaded).unwrap();
        snapshot.state = crate::guardian_ipc::WireState::Reconciling;
        snapshot.session.generation = binding.current_session().generation.get();
        (dir, store, candidate, snapshot, issue)
    }

    #[test]
    fn client_recovery_verifies_correlated_sample_only_and_fences_uncertainty() {
        for case in 0..7 {
            let (_dir, mut store, candidate, snapshot, issue) = recovered_candidate_fixture();
            let (stream, mut peer) = UnixStream::pair().unwrap();
            stream.set_nonblocking(true).unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let prior = snapshot.clone();
            let response = crate::guardian_ipc::Response::RecoveredNativeSampling {
                key: crate::guardian_ipc::WireKey::try_from(issue.key()).unwrap(),
                sequence: issue.sequence(),
                sample: crate::guardian_ipc::WireResourceSample {
                    sequence: issue.sequence(),
                    memory_bytes: 10,
                    storage_bytes: 20,
                    cpu_basis_points: 25,
                    memory_pressure: crate::guardian_ipc::WireMemoryPressure::Normal,
                },
            };
            let mut response = response;
            match case {
                1 => {
                    if let Response::RecoveredNativeSampling { key, .. } = &mut response {
                        key.ordinal += 1;
                    }
                }
                2 => {
                    if let Response::RecoveredNativeSampling { sequence, .. } = &mut response {
                        *sequence += 1;
                    }
                }
                3 => {
                    if let Response::RecoveredNativeSampling { sample, .. } = &mut response {
                        sample.sequence += 1;
                    }
                }
                4 => {
                    if let Response::RecoveredNativeSampling { sample, .. } = &mut response {
                        sample.cpu_basis_points = u16::MAX;
                    }
                }
                5 => response = Response::Retained(None),
                6 => response = Response::Refused(WireError::MissingProof),
                _ => (),
            }
            std::thread::scope(|scope| {
                scope.spawn(move || {
                    assert_eq!(read_frame::<Request>(&mut peer).unwrap(), Request::Snapshot);
                    write_frame(&mut peer, &Response::Snapshot(snapshot.clone())).unwrap();
                    assert_eq!(
                        read_frame::<Request>(&mut peer).unwrap(),
                        Request::VerifyRecoveredNativeSampling {
                            session: snapshot.session,
                            key: crate::guardian_ipc::WireKey::try_from(issue.key()).unwrap(),
                            sequence: issue.sequence(),
                            issuing_session_generation: issue.session_generation().get(),
                        }
                    );
                    write_frame(&mut peer, &response).unwrap();
                });
                let mut client = GuardianClient {
                    stream: Some(stream),
                    build: BuildIdentity::compiled(),
                    verified_snapshot: Some(prior),
                };
                let outcome = client.verify_recovered_native_sampling(candidate);
                if case == 0 {
                    let verified = outcome.unwrap();
                    assert_eq!(
                        store.apply_recovered_native_sampling(verified).unwrap(),
                        store
                            .load_native_sampling_status(issue.key())
                            .unwrap()
                            .unwrap()
                    );
                    assert!(client.stream.is_some());
                } else {
                    assert!(outcome.is_err(), "case {case}");
                    assert!(client.stream.is_none());
                    assert!(client.verified_snapshot.is_none());
                }
            });
        }
    }

    #[test]
    fn client_pre_admit_refusal_returns_cancellable_capability_and_closes_connection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refused.sqlite");
        let deadline = MonotonicTick::native_now()
            .unwrap()
            .checked_add(Duration::from_secs(120))
            .unwrap();
        let (mut store, _, ticket, issue, snapshot) = dispatch_fixture(&path, deadline);
        let (stream, mut peer) = UnixStream::pair().unwrap();
        stream.set_nonblocking(true).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        std::thread::scope(|scope| {
            let original = snapshot.clone();
            scope.spawn(move || {
                assert_eq!(read_frame::<Request>(&mut peer).unwrap(), Request::Snapshot);
                write_frame(&mut peer, &Response::Snapshot(snapshot)).unwrap();
                assert!(matches!(
                    read_frame::<Request>(&mut peer).unwrap(),
                    Request::DispatchAdmit { .. }
                ));
                write_frame(&mut peer, &Response::Refused(WireError::MissingProof)).unwrap();
                let mut byte = [0];
                assert_eq!(
                    io::Read::read(&mut peer, &mut byte).unwrap(),
                    0,
                    "no submit; connection fenced by EOF"
                );
            });
            let mut client = GuardianClient {
                stream: Some(stream),
                build: BuildIdentity::compiled(),
                verified_snapshot: Some(original),
            };
            let NativeSamplingDispatchError::KnownUnsubmitted {
                error: ClientError::Refused(WireError::MissingProof),
                unsubmitted,
            } = client.dispatch_native_sampling(ticket, issue).unwrap_err()
            else {
                panic!("pre-submit refusal")
            };
            assert!(client.stream.is_none());
            store
                .cancel_unsubmitted_native_sampling(*unsubmitted)
                .unwrap();
            assert_eq!(
                store.load_native_sampling_status(issue.key()).unwrap(),
                None
            );
        });
    }

    #[test]
    fn client_explicit_preexecution_submit_refusal_returns_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("submit-refused.sqlite");
        let deadline = MonotonicTick::native_now()
            .unwrap()
            .checked_add(Duration::from_secs(120))
            .unwrap();
        let (mut store, _, ticket, issue, snapshot) = dispatch_fixture(&path, deadline);
        let (stream, mut peer) = UnixStream::pair().unwrap();
        stream.set_nonblocking(true).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        std::thread::scope(|scope| {
            let original = snapshot.clone();
            scope.spawn(move || {
                assert_eq!(read_frame::<Request>(&mut peer).unwrap(), Request::Snapshot);
                write_frame(&mut peer, &Response::Snapshot(snapshot)).unwrap();
                assert!(matches!(
                    read_frame::<Request>(&mut peer).unwrap(),
                    Request::DispatchAdmit { .. }
                ));
                write_frame(&mut peer, &Response::Admitted { admission: 1 }).unwrap();
                assert!(matches!(
                    read_frame::<Request>(&mut peer).unwrap(),
                    Request::DispatchSubmit { admission: 1, .. }
                ));
                write_frame(
                    &mut peer,
                    &Response::Refused(WireError::Guardian(
                        crate::guardian_ipc::WireGuardianError::DeadlineExpired,
                    )),
                )
                .unwrap();
                let mut byte = [0];
                assert_eq!(io::Read::read(&mut peer, &mut byte).unwrap(), 0);
            });
            let mut client = GuardianClient {
                stream: Some(stream),
                build: BuildIdentity::compiled(),
                verified_snapshot: Some(original),
            };
            let NativeSamplingDispatchError::KnownUnsubmitted { unsubmitted, .. } =
                client.dispatch_native_sampling(ticket, issue).unwrap_err()
            else {
                panic!("explicit pre-execution refusal")
            };
            assert!(client.stream.is_none());
            store
                .cancel_unsubmitted_native_sampling(*unsubmitted)
                .unwrap();
            assert_eq!(
                store.load_native_sampling_status(issue.key()).unwrap(),
                None
            );
        });
    }

    #[test]
    fn client_post_submit_transport_or_correlation_ambiguity_never_returns_cancellation() {
        use crate::guardian_ipc::WireGuardianError;
        for response in [
            None,
            Some(Response::Fenced),
            Some(Response::Refused(WireError::Guardian(
                WireGuardianError::ExecutorCorrelationMismatch,
            ))),
            Some(Response::Refused(WireError::InvalidValue)),
            Some(Response::Submitted {
                key: crate::guardian_ipc::WireKey {
                    attempt: 92,
                    revision: 1,
                    ordinal: 0,
                },
            }),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("ambiguous.sqlite");
            let deadline = MonotonicTick::native_now()
                .unwrap()
                .checked_add(Duration::from_secs(120))
                .unwrap();
            let (mut store, _, ticket, issue, snapshot) = dispatch_fixture(&path, deadline);
            let before = durable_rows(&path);
            let (stream, mut peer) = UnixStream::pair().unwrap();
            stream.set_nonblocking(true).unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            std::thread::scope(|scope| {
                let original = snapshot.clone();
                scope.spawn(move || {
                    assert_eq!(read_frame::<Request>(&mut peer).unwrap(), Request::Snapshot);
                    write_frame(&mut peer, &Response::Snapshot(snapshot)).unwrap();
                    assert!(matches!(
                        read_frame::<Request>(&mut peer).unwrap(),
                        Request::DispatchAdmit { .. }
                    ));
                    write_frame(&mut peer, &Response::Admitted { admission: 1 }).unwrap();
                    assert!(matches!(
                        read_frame::<Request>(&mut peer).unwrap(),
                        Request::DispatchSubmit { admission: 1, .. }
                    ));
                    if let Some(response) = response {
                        write_frame(&mut peer, &response).unwrap();
                    }
                });
                let mut client = GuardianClient {
                    stream: Some(stream),
                    build: BuildIdentity::compiled(),
                    verified_snapshot: Some(original),
                };
                assert!(matches!(
                    client.dispatch_native_sampling(ticket, issue),
                    Err(NativeSamplingDispatchError::SubmissionUnknown(_))
                ));
                assert!(client.stream.is_none());
            });
            assert_eq!(durable_rows(&path), before);
            assert_eq!(
                store.load_native_sampling_status(issue.key()).unwrap(),
                Some(crate::NativeSamplingStatus::IssuedPendingApplication { issue })
            );
        }
    }

    #[test]
    fn child_list_submit_refusal_classification_keeps_only_preexecution_errors_retryable() {
        use crate::guardian_ipc::WireGuardianError as G;

        for error in [
            ClientError::Refused(WireError::UnknownAdmission),
            ClientError::Refused(WireError::AdmissionMismatch),
            ClientError::Refused(WireError::MissingProof),
            ClientError::Refused(WireError::UnsupportedDurableBinding),
            ClientError::Refused(WireError::Guardian(G::StaleSession)),
            ClientError::Refused(WireError::Guardian(G::DeadlineExpired)),
            ClientError::Refused(WireError::Guardian(G::AuthorityLost)),
        ] {
            assert!(child_list_submit_refusal_is_unsubmitted(&error));
        }
        for error in [
            ClientError::UnexpectedResponse,
            ClientError::Refused(WireError::InvalidValue),
            ClientError::Refused(WireError::Guardian(G::ExecutorCorrelationMismatch)),
        ] {
            assert!(!child_list_submit_refusal_is_unsubmitted(&error));
        }
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
    fn client_accepts_only_exact_prepared_response_not_finished_or_mismatched_dtos() {
        use crate::guardian_ipc::{WireNativeBinding, WireSession, WireState};
        let session = WireSession {
            instance: 1,
            generation: 2,
        };
        let revision = PersistenceRevision::first().checked_next().unwrap();
        for response in [
            Response::ReconciliationPrepared {
                session,
                attempt: 10,
                recovery_revision: revision.get(),
            },
            Response::ReconciliationFinished,
            Response::ReconciliationPrepared {
                session: WireSession {
                    generation: 3,
                    ..session
                },
                attempt: 10,
                recovery_revision: revision.get(),
            },
            Response::ReconciliationPrepared {
                session,
                attempt: 11,
                recovery_revision: revision.get(),
            },
            Response::ReconciliationPrepared {
                session,
                attempt: 10,
                recovery_revision: revision.get() + 1,
            },
        ] {
            let exact = response
                == (Response::ReconciliationPrepared {
                    session,
                    attempt: 10,
                    recovery_revision: revision.get(),
                });
            let deadline = MonotonicTick::native_now()
                .unwrap()
                .checked_add(Duration::from_secs(120))
                .unwrap();
            let snapshot = WireSnapshot {
                instance: 1,
                attempt: 10,
                deadline: deadline.get(),
                state: WireState::Reconciling,
                session,
                slots: vec![],
                retained: vec![],
                native_binding: Some(WireNativeBinding {
                    attempt: 10,
                    deadline: deadline.get(),
                    nonce: [7; 16],
                    guardian: process(44).into(),
                    slots: vec![],
                }),
            };
            let (stream, mut peer) = UnixStream::pair().unwrap();
            stream.set_nonblocking(true).unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            std::thread::scope(|scope| {
                scope.spawn(move || {
                    assert_eq!(read_frame::<Request>(&mut peer).unwrap(), Request::Snapshot);
                    write_frame(&mut peer, &Response::Snapshot(snapshot)).unwrap();
                    assert!(matches!(read_frame::<Request>(&mut peer).unwrap(), Request::FinishReconciliation { session: requested, applied_revision, .. } if requested == session && applied_revision == revision.get()));
                    write_frame(&mut peer, &response).unwrap();
                });
                let mut client = GuardianClient {
                    stream: Some(stream),
                    build: BuildIdentity::compiled(),
                    verified_snapshot: None,
                };
                assert_eq!(
                    client.finish_reconciliation(session, revision).is_ok(),
                    exact
                );
            });
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
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
                for _ in 0..17 {
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
            client.fence(snapshot.session).unwrap();
            let detached_binding = store
                .commit_detached_binding(&binding, binding.slots())
                .unwrap();
            let loaded = store.load_attempt(attempt).unwrap().unwrap();
            let crate::RecoveryDisposition::Checkpoint(interrupted) =
                loaded.prepare_recovery().unwrap()
            else {
                panic!("interruption")
            };
            let interrupted = interrupted.persist(&mut store).unwrap();
            let revision = interrupted.checkpoint().persistence_revision().unwrap();
            let binding = store
                .commit_fresh_interruption_binding(&detached_binding, revision)
                .unwrap();
            let detached = client.snapshot().unwrap();
            let session = client.begin_reconciliation(&detached, revision).unwrap();
            let loaded = store.load_attempt(attempt).unwrap().unwrap();
            let retained_plan = store
                .commit_native_sampling_recovery_at(
                    loaded,
                    &binding,
                    MonotonicTick::native_now().unwrap(),
                )
                .unwrap();
            let revision = retained_plan.checkpoint().persistence_revision().unwrap();
            let (_, _, completion) = store
                .read_native_reconciliation_evidence(attempt, binding.current_session().generation)
                .unwrap()
                .unwrap();
            let prepared = client.finish_reconciliation(session, revision).unwrap();
            assert_eq!(
                client.snapshot().unwrap().state,
                crate::guardian_ipc::WireState::Reconciling
            );
            let (_, _, committed) = store
                .commit_native_reconciliation_activation_at(
                    &binding,
                    completion,
                    prepared,
                    MonotonicTick::native_now().unwrap(),
                )
                .unwrap();
            assert_eq!(
                client.snapshot().unwrap().state,
                crate::guardian_ipc::WireState::Reconciling
            );
            client.activate_reconciliation(committed).unwrap();
            let active = client.snapshot().unwrap();
            assert_eq!(active.state, crate::guardian_ipc::WireState::Active);
            assert_eq!(active.instance, snapshot.instance);
            assert_eq!(active.deadline, snapshot.deadline);
            assert_eq!(active.slots, snapshot.slots);
            drop(retained_plan);
        });
        assert!(matches!(
            store.load_native_sampling_status(issue.key()).unwrap(),
            Some(crate::NativeSamplingStatus::SampleAckConfirmed { issue: stored, .. })
                if stored == issue
        ));
        assert_confirmed_application_is_immutable(&database);
    }
}
