use std::{
    ffi::CString,
    io,
    mem::{self, MaybeUninit},
    num::NonZeroU32,
    os::{
        raw::{c_int, c_void},
        unix::{ffi::OsStrExt, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus},
};

use crate::{
    BirthToken, CandidateDiscovery, CandidateVerification, ChildIdentityObservation, ControllerOs,
    CpuContentionBasisPoints, IdentityObservation, MemoryPressure, ObservationFailure,
    OsQueryError, ProcessId, ProcessIdentity, ResourceCapacity, ResourceContention,
    ResourceObservationFailure, ResourceSample, RetainedCandidateIdentity, Signal, SignalOutcome,
    StableChildEvidence,
};

unsafe extern "C" {
    fn proc_listchildpids(ppid: libc::pid_t, buffer: *mut c_void, buffersize: c_int) -> c_int;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacOsError {
    NoSuchProcess,
    PermissionDenied,
    TemporarilyUnavailable,
    MalformedObservation,
    InvalidPath,
    System(i32),
}

impl std::fmt::Display for MacOsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "macOS controller boundary error: {self:?}")
    }
}

impl std::error::Error for MacOsError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupervisionRisk {
    DescendantMayEscapeProcessGroup,
    SameSessionProcessMayJoinProcessGroup,
    ControllerExitLosesParentAuthority,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ObservedProcess {
    identity: ProcessIdentity,
    parent_pid: Option<ProcessId>,
    process_group: ProcessId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CpuTicks {
    user: u64,
    system: u64,
    idle: u64,
    nice: u64,
}

pub struct MacOsControllerOs {
    storage_path: PathBuf,
    previous_cpu: CpuTicks,
}

impl MacOsControllerOs {
    /// # Errors
    /// Returns a typed boundary error when the initial CPU counter cannot be observed.
    pub fn new(storage_path: impl Into<PathBuf>) -> Result<Self, MacOsError> {
        Ok(Self {
            storage_path: storage_path.into(),
            previous_cpu: cpu_ticks()?,
        })
    }

    /// Spawns a direct child as the leader of a controller-owned process group.
    ///
    /// The returned affine value retains the direct child without reaping it, so the
    /// kernel cannot reuse the leader PID before group signaling and final wait.
    ///
    /// # Errors
    /// Returns a typed boundary error when spawning, identity acquisition, or process-group
    /// verification fails. A partially spawned child is killed and reaped before returning.
    pub fn spawn_supervised(&self, command: &mut Command) -> Result<SupervisedChild, MacOsError> {
        // SAFETY: pre_exec runs after fork and calls only the async-signal-safe setpgid syscall.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            });
        }
        let mut child = command.spawn().map_err(io_error)?;
        let pid = process_id(child.id()).ok_or(MacOsError::MalformedObservation)?;
        let observed = match observe_process(pid) {
            Ok(observed) if observed.process_group == pid => observed,
            Ok(_) => {
                cleanup_partial_child(&mut child, pid);
                return Err(MacOsError::MalformedObservation);
            }
            Err(error) => {
                cleanup_partial_child(&mut child, pid);
                return Err(error);
            }
        };
        Ok(SupervisedChild {
            child,
            identity: observed.identity,
            process_group: pid,
            finished: false,
        })
    }

    #[must_use]
    pub const fn supervision_risks() -> [SupervisionRisk; 3] {
        [
            SupervisionRisk::DescendantMayEscapeProcessGroup,
            SupervisionRisk::SameSessionProcessMayJoinProcessGroup,
            SupervisionRisk::ControllerExitLosesParentAuthority,
        ]
    }
}

pub struct SupervisedChild {
    child: Child,
    identity: ProcessIdentity,
    process_group: ProcessId,
    finished: bool,
}

impl SupervisedChild {
    #[must_use]
    pub const fn identity(&self) -> ProcessIdentity {
        self.identity
    }

    #[must_use]
    pub const fn process_group(&self) -> ProcessId {
        self.process_group
    }

    pub fn signal_group(&mut self, signal: Signal) -> SignalOutcome {
        if self.finished {
            return SignalOutcome::ExactIdentityAlreadyAbsent;
        }
        let native_signal = match signal {
            Signal::Terminate => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };
        signal_process_group(self.process_group, native_signal)
    }

    /// # Errors
    /// Returns the native wait error while retaining drop-time cleanup authority.
    pub fn wait(mut self) -> io::Result<ExitStatus> {
        let status = self.child.wait()?;
        self.finished = true;
        Ok(status)
    }
}

impl Drop for SupervisedChild {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let _ = signal_process_group(self.process_group, libc::SIGKILL);
        let _ = self.child.wait();
        self.finished = true;
    }
}

impl ControllerOs for MacOsControllerOs {
    fn sample_resources(
        &mut self,
        sequence: u64,
    ) -> Result<ResourceSample, ResourceObservationFailure> {
        let current_cpu = cpu_ticks().map_err(resource_error)?;
        let contention = cpu_contention(self.previous_cpu, current_cpu)?;
        self.previous_cpu = current_cpu;
        let available_memory_bytes = available_memory_bytes().map_err(resource_error)?;
        let available_storage_bytes =
            available_storage_bytes(&self.storage_path).map_err(resource_error)?;
        let memory_pressure = memory_pressure().map_err(resource_error)?;
        Ok(ResourceSample {
            sequence,
            capacity: ResourceCapacity {
                available_memory_bytes,
                available_storage_bytes,
            },
            contention: ResourceContention {
                cpu: contention,
                memory_pressure,
            },
        })
    }

    fn list_children(&mut self, parent: ProcessIdentity) -> Result<Vec<ProcessId>, OsQueryError> {
        require_same_identity(parent).map_err(query_error)?;
        let children = list_child_pids(parent.pid()).map_err(query_error)?;
        require_same_identity(parent).map_err(query_error)?;
        Ok(children)
    }

    fn acquire_child_identity(
        &mut self,
        discovery: CandidateDiscovery,
    ) -> ChildIdentityObservation {
        let parent = discovery.verified_parent();
        if let Err(error) = require_same_identity(parent) {
            return ChildIdentityObservation::Unresolved(observation_error(error));
        }
        let first = match observe_process(discovery.hinted_pid()) {
            Ok(observed) => observed,
            Err(error) => {
                return ChildIdentityObservation::Unresolved(observation_error(error));
            }
        };
        if first.parent_pid != Some(parent.pid()) {
            return ChildIdentityObservation::DifferentProcess(first.identity);
        }
        if require_same_identity(parent).is_err() {
            return ChildIdentityObservation::Unresolved(
                ObservationFailure::TemporarilyUnavailable,
            );
        }
        let second = match observe_process(discovery.hinted_pid()) {
            Ok(observed) => observed,
            Err(error) => {
                return ChildIdentityObservation::Unresolved(observation_error(error));
            }
        };
        if second != first {
            return ChildIdentityObservation::DifferentProcess(second.identity);
        }
        match StableChildEvidence::new(discovery, second.identity, parent) {
            Ok(evidence) => ChildIdentityObservation::Stable(evidence),
            Err(_) => ChildIdentityObservation::Unresolved(ObservationFailure::Malformed),
        }
    }

    fn observe_identity(&mut self, identity: ProcessIdentity) -> IdentityObservation {
        let observed = match observe_process(identity.pid()) {
            Ok(observed) => observed,
            Err(MacOsError::NoSuchProcess) => return IdentityObservation::ExactIdentityAbsent,
            Err(error) => return IdentityObservation::Unresolved(observation_error(error)),
        };
        if observed.identity != identity {
            return IdentityObservation::PidReused(observed.identity);
        }
        let current_parent = observed
            .parent_pid
            .and_then(|parent_pid| observe_process(parent_pid).ok())
            .map(|parent| parent.identity);
        IdentityObservation::SameIdentity { current_parent }
    }

    fn signal_exact_identity(
        &mut self,
        _identity: ProcessIdentity,
        _signal: Signal,
    ) -> SignalOutcome {
        SignalOutcome::TemporarilyUnavailable
    }

    fn reverify_retained_candidate(
        &mut self,
        _candidate: RetainedCandidateIdentity,
    ) -> CandidateVerification {
        CandidateVerification::Unavailable
    }
}

fn observe_process(pid: ProcessId) -> Result<ObservedProcess, MacOsError> {
    let native_pid = c_int::try_from(pid.get()).map_err(|_| MacOsError::MalformedObservation)?;
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let expected = c_int::try_from(mem::size_of::<libc::proc_bsdinfo>())
        .map_err(|_| MacOsError::MalformedObservation)?;
    // SAFETY: info points to expected writable proc_bsdinfo storage for PROC_PIDTBSDINFO.
    let written = unsafe {
        libc::proc_pidinfo(
            native_pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            expected,
        )
    };
    if written == 0 {
        return Err(last_process_error());
    }
    if written != expected {
        return Err(MacOsError::MalformedObservation);
    }
    // SAFETY: proc_pidinfo returned the complete expected struct size.
    let info = unsafe { info.assume_init() };
    let observed_pid = process_id(info.pbi_pid).ok_or(MacOsError::MalformedObservation)?;
    if observed_pid != pid {
        return Err(MacOsError::MalformedObservation);
    }
    let nanos = u32::try_from(info.pbi_start_tvusec)
        .ok()
        .and_then(|micros| micros.checked_mul(1_000))
        .ok_or(MacOsError::MalformedObservation)?;
    let birth = BirthToken::new(info.pbi_start_tvsec, nanos)
        .map_err(|_| MacOsError::MalformedObservation)?;
    let parent_pid = process_id(info.pbi_ppid);
    let process_group = process_id(info.pbi_pgid).ok_or(MacOsError::MalformedObservation)?;
    Ok(ObservedProcess {
        identity: ProcessIdentity::new(pid, birth),
        parent_pid,
        process_group,
    })
}

fn require_same_identity(identity: ProcessIdentity) -> Result<(), MacOsError> {
    let observed = observe_process(identity.pid())?;
    if observed.identity == identity {
        Ok(())
    } else {
        Err(MacOsError::TemporarilyUnavailable)
    }
}

fn list_child_pids(parent: ProcessId) -> Result<Vec<ProcessId>, MacOsError> {
    let native_parent =
        c_int::try_from(parent.get()).map_err(|_| MacOsError::MalformedObservation)?;
    for extra in [8_usize, 32, 128] {
        // SAFETY: a null buffer with zero size requests the current child count.
        let count = unsafe { proc_listchildpids(native_parent, std::ptr::null_mut(), 0) };
        if count < 0 {
            return Err(last_process_error());
        }
        let capacity = usize::try_from(count)
            .map_err(|_| MacOsError::MalformedObservation)?
            .saturating_add(extra);
        let mut pids = vec![0_i32; capacity];
        let bytes = c_int::try_from(capacity.saturating_mul(mem::size_of::<libc::pid_t>()))
            .map_err(|_| MacOsError::MalformedObservation)?;
        // SAFETY: pids owns bytes writable bytes and remains alive for the call.
        let returned =
            unsafe { proc_listchildpids(native_parent, pids.as_mut_ptr().cast(), bytes) };
        if returned < 0 {
            return Err(last_process_error());
        }
        let returned = usize::try_from(returned).map_err(|_| MacOsError::MalformedObservation)?;
        if returned < capacity {
            pids.truncate(returned);
            return Ok(pids
                .into_iter()
                .filter_map(|pid| u32::try_from(pid).ok().and_then(process_id))
                .collect());
        }
    }
    Err(MacOsError::TemporarilyUnavailable)
}

fn cpu_ticks() -> Result<CpuTicks, MacOsError> {
    let mut info = MaybeUninit::<libc::host_cpu_load_info>::zeroed();
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: info is correctly sized for HOST_CPU_LOAD_INFO and count names that size.
    let status = unsafe {
        libc::host_statistics(
            libc::mach_host_self(),
            libc::HOST_CPU_LOAD_INFO,
            info.as_mut_ptr().cast(),
            &mut count,
        )
    };
    if status != libc::KERN_SUCCESS || count != libc::HOST_CPU_LOAD_INFO_COUNT {
        return Err(MacOsError::System(status));
    }
    // SAFETY: host_statistics succeeded with the complete expected count.
    let info = unsafe { info.assume_init() };
    Ok(CpuTicks {
        user: u64::from(info.cpu_ticks[libc::CPU_STATE_USER]),
        system: u64::from(info.cpu_ticks[libc::CPU_STATE_SYSTEM]),
        idle: u64::from(info.cpu_ticks[libc::CPU_STATE_IDLE]),
        nice: u64::from(info.cpu_ticks[libc::CPU_STATE_NICE]),
    })
}

fn cpu_contention(
    previous: CpuTicks,
    current: CpuTicks,
) -> Result<CpuContentionBasisPoints, ResourceObservationFailure> {
    let user = current.user.saturating_sub(previous.user);
    let system = current.system.saturating_sub(previous.system);
    let idle = current.idle.saturating_sub(previous.idle);
    let nice = current.nice.saturating_sub(previous.nice);
    let busy = user.saturating_add(system).saturating_add(nice);
    let total = busy.saturating_add(idle);
    if total == 0 {
        return Err(ResourceObservationFailure::TemporarilyUnavailable);
    }
    let basis_points = busy.saturating_mul(10_000) / total;
    let basis_points =
        u16::try_from(basis_points).map_err(|_| ResourceObservationFailure::Malformed)?;
    CpuContentionBasisPoints::new(basis_points).map_err(|_| ResourceObservationFailure::Malformed)
}

fn available_memory_bytes() -> Result<u64, MacOsError> {
    let mut stats = MaybeUninit::<libc::vm_statistics64>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: stats is correctly sized for HOST_VM_INFO64 and count names that size.
    let status = unsafe {
        libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            stats.as_mut_ptr().cast(),
            &mut count,
        )
    };
    if status != libc::KERN_SUCCESS || count != libc::HOST_VM_INFO64_COUNT {
        return Err(MacOsError::System(status));
    }
    // SAFETY: host_statistics64 succeeded with the complete expected count.
    let stats = unsafe { stats.assume_init() };
    // SAFETY: sysconf with _SC_PAGESIZE has no pointer arguments.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = u64::try_from(page_size).map_err(|_| MacOsError::MalformedObservation)?;
    u64::from(stats.free_count)
        .saturating_add(u64::from(stats.inactive_count))
        .saturating_add(u64::from(stats.speculative_count))
        .checked_mul(page_size)
        .ok_or(MacOsError::MalformedObservation)
}

fn available_storage_bytes(path: &Path) -> Result<u64, MacOsError> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| MacOsError::InvalidPath)?;
    let mut stats = MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: path is NUL-terminated and stats points to writable statvfs storage.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(io_error(io::Error::last_os_error()));
    }
    // SAFETY: statvfs succeeded and initialized the complete struct.
    let stats = unsafe { stats.assume_init() };
    stats
        .f_bavail
        .checked_mul(stats.f_frsize)
        .ok_or(MacOsError::MalformedObservation)
}

fn memory_pressure() -> Result<MemoryPressure, MacOsError> {
    let mut value = 0_i32;
    let mut size = mem::size_of_val(&value);
    let name = c"kern.memorystatus_vm_pressure_level";
    // SAFETY: name is NUL-terminated and value/size identify writable i32 storage.
    if unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&raw mut value).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io_error(io::Error::last_os_error()));
    }
    if size != mem::size_of_val(&value) {
        return Err(MacOsError::MalformedObservation);
    }
    match value {
        1 => Ok(MemoryPressure::Nominal),
        2 => Ok(MemoryPressure::Warning),
        4 => Ok(MemoryPressure::Critical),
        _ => Err(MacOsError::MalformedObservation),
    }
}

fn signal_process_group(group: ProcessId, signal: c_int) -> SignalOutcome {
    let Ok(group) = c_int::try_from(group.get()) else {
        return SignalOutcome::TemporarilyUnavailable;
    };
    // SAFETY: killpg receives a positive owned process-group id and a known signal number.
    if unsafe { libc::killpg(group, signal) } == 0 {
        return SignalOutcome::Delivered;
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => SignalOutcome::ExactIdentityAlreadyAbsent,
        Some(libc::EPERM) | Some(libc::EACCES) => SignalOutcome::PermissionDenied,
        _ => SignalOutcome::TemporarilyUnavailable,
    }
}

fn cleanup_partial_child(child: &mut Child, group: ProcessId) {
    let _ = signal_process_group(group, libc::SIGKILL);
    let _ = child.wait();
}

const fn process_id(raw: u32) -> Option<ProcessId> {
    match NonZeroU32::new(raw) {
        Some(pid) => Some(ProcessId::new(pid)),
        None => None,
    }
}

fn last_process_error() -> MacOsError {
    match io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => MacOsError::NoSuchProcess,
        Some(libc::EPERM) | Some(libc::EACCES) => MacOsError::PermissionDenied,
        Some(code) => MacOsError::System(code),
        None => MacOsError::TemporarilyUnavailable,
    }
}

fn io_error(error: io::Error) -> MacOsError {
    match error.raw_os_error() {
        Some(libc::ESRCH) => MacOsError::NoSuchProcess,
        Some(libc::EPERM) | Some(libc::EACCES) => MacOsError::PermissionDenied,
        Some(code) => MacOsError::System(code),
        None => MacOsError::TemporarilyUnavailable,
    }
}

const fn observation_error(error: MacOsError) -> ObservationFailure {
    match error {
        MacOsError::NoSuchProcess => ObservationFailure::NoSuchProcess,
        MacOsError::PermissionDenied => ObservationFailure::PermissionDenied,
        MacOsError::MalformedObservation | MacOsError::InvalidPath => ObservationFailure::Malformed,
        MacOsError::TemporarilyUnavailable | MacOsError::System(_) => {
            ObservationFailure::TemporarilyUnavailable
        }
    }
}

const fn query_error(error: MacOsError) -> OsQueryError {
    match error {
        MacOsError::PermissionDenied => OsQueryError::PermissionDenied,
        MacOsError::MalformedObservation | MacOsError::InvalidPath => OsQueryError::Malformed,
        MacOsError::NoSuchProcess | MacOsError::TemporarilyUnavailable | MacOsError::System(_) => {
            OsQueryError::TemporarilyUnavailable
        }
    }
}

const fn resource_error(error: MacOsError) -> ResourceObservationFailure {
    match error {
        MacOsError::MalformedObservation | MacOsError::InvalidPath => {
            ResourceObservationFailure::Malformed
        }
        MacOsError::NoSuchProcess
        | MacOsError::PermissionDenied
        | MacOsError::TemporarilyUnavailable
        | MacOsError::System(_) => ResourceObservationFailure::TemporarilyUnavailable,
    }
}
