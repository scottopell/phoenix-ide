//! Disposable same-UID Unix transport; no durable binding or policy mutation authority.
use crate::guardian_ipc::{
    read_frame, serve_once, write_frame, CodecError, GuardianServer, Request, Response, WireError,
    WireObservation, WireSlot, WireSnapshot,
};
use crate::GuardianExecutor;
use std::{
    fs::{self, DirBuilder, Metadata, Permissions},
    io::{self, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        io::AsRawFd,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const SOCKET_NAME: &str = "guardian.sock";
const IO_SLICE: Duration = Duration::from_millis(250);
const ACCEPT_POLL: Duration = Duration::from_millis(10);

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
    let roots = [
        fs::canonicalize(std::env::temp_dir())?,
        fs::canonicalize("/tmp")?,
    ];
    let canonical_parent = fs::canonicalize(parent)?;
    if !roots
        .iter()
        .any(|root| canonical_parent != *root && canonical_parent.starts_with(root))
    {
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
        if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
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
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: cred and len are valid writable buffers; fd is live.
        if unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut _ as *mut _,
                &mut len,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        if len as usize != std::mem::size_of::<libc::ucred>()
            || cred.uid != unsafe { libc::geteuid() }
        {
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

/// Retain the same GuardianServer across disconnected connections until the
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
                stream.set_nonblocking(false)?;
                while Instant::now() < until {
                    let slice = until
                        .saturating_duration_since(Instant::now())
                        .min(IO_SLICE);
                    if slice.is_zero() {
                        break;
                    }
                    stream.set_read_timeout(Some(slice))?;
                    stream.set_write_timeout(Some(slice))?;
                    if serve_once(&mut stream, server).is_err() {
                        break;
                    }
                }
                // Even an idle connection at the fixture deadline cannot retain authority.
                server.fence_connection();
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL.min(until.saturating_duration_since(Instant::now())))
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

/// Typed observation client. No client method can mint a policy proof.
pub struct GuardianClient {
    stream: UnixStream,
}
impl GuardianClient {
    /// # Errors
    /// Rejects insecure socket paths or peer identities.
    pub fn connect(dir: &Path) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(socket_path(dir)?)?;
        same_uid(&stream)?;
        stream.set_read_timeout(Some(IO_SLICE))?;
        stream.set_write_timeout(Some(IO_SLICE))?;
        Ok(Self { stream })
    }
    fn exchange(&mut self, request: Request) -> Result<Response, ClientError> {
        write_frame(&mut self.stream, &request)?;
        match read_frame(&mut self.stream)? {
            Response::Refused(error) => Err(ClientError::Refused(error)),
            response => Ok(response),
        }
    }
    /// # Errors
    /// Returns transport or protocol errors rather than inferring a live binding.
    pub fn hello(&mut self) -> Result<WireSnapshot, ClientError> {
        match self.exchange(Request::Hello)? {
            Response::Hello { snapshot } => Ok(snapshot),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }
    /// # Errors
    /// Returns transport or protocol errors.
    pub fn snapshot(&mut self) -> Result<WireSnapshot, ClientError> {
        match self.exchange(Request::Snapshot)? {
            Response::Snapshot(snapshot) => Ok(snapshot),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }
    /// # Errors
    /// Returns transport or protocol errors.
    pub fn inventory(&mut self) -> Result<Vec<WireSlot>, ClientError> {
        match self.exchange(Request::Inventory)? {
            Response::Inventory(slots) => Ok(slots),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }
    /// # Errors
    /// Returns transport or protocol errors.
    pub fn observe_slot(&mut self, slot: WireSlot) -> Result<WireObservation, ClientError> {
        match self.exchange(Request::ObserveSlot { slot })? {
            Response::SlotObserved(observation) => Ok(observation),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }
    /// Native reconciliation needs a committed interruption and exact durable
    /// inventory/identity binding. This transport-only runner has neither.
    pub fn reconcile(&self) -> Result<(), ClientError> {
        Err(ClientError::Refused(WireError::UnsupportedDurableBinding))
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
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::{AttemptId, Guardian, MacOsControllerOs, MacOsGuardianExecutor, MonotonicTick};
    use std::process::Command;
    if !(1..=120).contains(&child_seconds) || !(1..=120).contains(&serve_seconds) {
        return Err(Box::new(permission_denied()));
    }
    let listener = bind_disposable(dir)?;
    let os = MacOsControllerOs::new(dir)?;
    let mut command = Command::new("/bin/sleep");
    command.arg(child_seconds.to_string());
    let child = os.spawn_supervised(&mut command)?;
    let identity = child.identity();
    let (guardian, session) = Guardian::new(
        AttemptId::new(1),
        MonotonicTick::new(u64::MAX),
        MacOsGuardianExecutor::new(os),
        vec![child],
    )
    .map_err(|failure| {
        let (error, executor, slots) = failure.into_parts();
        drop(slots);
        drop(executor);
        error
    })?;
    let mut server = GuardianServer::new(guardian, session)?;
    server.require_native_binding();
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
