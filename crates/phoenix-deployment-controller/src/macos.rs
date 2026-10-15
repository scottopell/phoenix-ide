use std::{
    ffi::CString,
    io,
    mem::{self, MaybeUninit},
    num::NonZeroU32,
    os::{
        raw::c_int,
        unix::{ffi::OsStrExt, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

use crate::{
    BirthToken, CandidateDiscovery, CandidateVerification, ChildIdentityObservation, ControllerOs,
    CpuContentionBasisPoints, IdentityObservation, MemoryPressure, ObservationFailure,
    OsQueryError, ProcessId, ProcessIdentity, ResourceCapacity, ResourceContention,
    ResourceObservationFailure, ResourceSample, RetainedCandidateIdentity, Signal, SignalOutcome,
    StableChildEvidence,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacOsError {
    NoSuchProcess,
    PermissionDenied,
    TemporarilyUnavailable,
    MalformedObservation,
    InvalidPath,
    /// Identity acquisition failed after spawn; cleanup was attempted, not verified.
    PartialSpawnCleanupUnverified {
        leader_reaped: bool,
    },
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
    user: u32,
    system: u32,
    idle: u32,
    nice: u32,
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

    fn observe_supervised_slot(&self, identity: ProcessIdentity) -> crate::ChildSlotObservation {
        match observe_process(identity.pid()) {
            Ok(observed) if observed.identity == identity => crate::ChildSlotObservation::Running,
            Ok(_) | Err(MacOsError::NoSuchProcess) => crate::ChildSlotObservation::Exited,
            Err(_) => crate::ChildSlotObservation::Unavailable,
        }
    }

    fn observe_cpu_contention(
        &mut self,
        current_cpu: CpuTicks,
    ) -> Result<CpuContentionBasisPoints, ResourceObservationFailure> {
        let previous_cpu = mem::replace(&mut self.previous_cpu, current_cpu);
        cpu_contention(previous_cpu, current_cpu)
    }

    /// Spawns a direct child as the leader of a controller-owned process group.
    ///
    /// The returned affine value retains the direct child without reaping it until
    /// finalization observes its exit and no other members in its original group.
    /// Group escape and join risks still prevent a verified owned-tree cleanup claim.
    ///
    /// # Errors
    /// Returns a typed boundary error when spawning, identity acquisition, or process-group
    /// verification fails. Partial-spawn cleanup is bounded and best-effort; its error
    /// records whether the leader was reaped, never whether descendants were cleaned up.
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
        let mut child = command.spawn().map_err(|error| io_error(&error))?;
        let group = process_id(child.id());
        let observed = group
            .ok_or(MacOsError::MalformedObservation)
            .and_then(observe_process);
        match observed {
            Ok(observed) if Some(observed.process_group) == group => Ok(SupervisedChild {
                state: ChildSupervision::Retained(child),
                identity: observed.identity,
                process_group: observed.process_group,
            }),
            Ok(_) | Err(_) => Err(MacOsError::PartialSpawnCleanupUnverified {
                leader_reaped: cleanup_partial_child(&mut child, group),
            }),
        }
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

enum ChildSupervision {
    Retained(Child),
    Finalized(SupervisedFinalization),
}

/// Original-group finalization, not evidence that the entire owned tree is absent.
#[derive(Clone, Copy, Debug)]
#[must_use = "finalization retains unresolved supervision risks; it is not verified tree cleanup"]
pub struct SupervisedFinalization {
    pub leader_status: ExitStatus,
    pub unresolved_risks: [SupervisionRisk; 3],
}

pub struct SupervisedChild {
    state: ChildSupervision,
    identity: ProcessIdentity,
    process_group: ProcessId,
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
        if let ChildSupervision::Finalized(_) = self.state {
            // Reaping released the PID reservation; the old PGID is no longer authority.
            return SignalOutcome::TemporarilyUnavailable;
        }
        match observe_process(self.identity.pid()) {
            Ok(observed)
                if observed.identity == self.identity
                    && observed.process_group == self.process_group => {}
            Ok(_) => return SignalOutcome::PidReused,
            Err(MacOsError::NoSuchProcess) => {
                let ChildSupervision::Retained(child) = &mut self.state else {
                    return SignalOutcome::TemporarilyUnavailable;
                };
                match (
                    child_exited_without_reaping(child),
                    group_has_other_members(self.process_group, self.identity.pid()),
                ) {
                    (Ok(true), Ok(true)) => {}
                    (Ok(true), Ok(false)) => return SignalOutcome::ExactIdentityAlreadyAbsent,
                    _ => return SignalOutcome::TemporarilyUnavailable,
                }
            }
            Err(_) => return SignalOutcome::TemporarilyUnavailable,
        }
        let native_signal = match signal {
            Signal::Terminate => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };
        signal_process_group(self.process_group, native_signal)
    }

    /// Nonblocking finalization. A leader exit alone does not permit reaping: other
    /// original-group members must also be absent from a complete native group scan.
    /// `None` retains supervision; a report retains escape/join/controller-exit risks.
    ///
    /// # Errors
    /// Returns a typed observation/wait error without relinquishing supervision.
    pub fn try_finalize(&mut self) -> Result<Option<SupervisedFinalization>, MacOsError> {
        let child = match &mut self.state {
            ChildSupervision::Retained(child) => child,
            ChildSupervision::Finalized(report) => return Ok(Some(*report)),
        };
        if !child_exited_without_reaping(child)?
            || group_has_other_members(self.process_group, self.identity.pid())?
        {
            return Ok(None);
        }
        let Some(leader_status) = child.try_wait().map_err(|error| io_error(&error))? else {
            return Ok(None);
        };
        let report = SupervisedFinalization {
            leader_status,
            unresolved_risks: MacOsControllerOs::supervision_risks(),
        };
        self.state = ChildSupervision::Finalized(report);
        Ok(Some(report))
    }

    /// Requests forced cleanup using both original-group and retained direct-child
    /// authority, including when group signaling fails. Does not block for exit.
    ///
    /// # Errors
    /// Returns unresolved observation/wait errors; retries retain the unreaped child.
    pub fn try_cleanup(&mut self) -> Result<Option<SupervisedFinalization>, MacOsError> {
        if let ChildSupervision::Retained(child) = &mut self.state {
            let _ = signal_process_group(self.process_group, libc::SIGKILL);
            let _ = child.kill();
        }
        self.try_finalize()
    }
}

impl Drop for SupervisedChild {
    fn drop(&mut self) {
        if let ChildSupervision::Retained(child) = &mut self.state {
            // Drop forfeits authority after a bounded best-effort attempt, not cleanup proof.
            let _ = cleanup_partial_child(child, Some(self.process_group));
        }
    }
}

/// Native executor retaining the original direct-child handle in Guardian custody.
/// Only an exact match to that slot can reach the group signal operation.
pub struct MacOsGuardianExecutor {
    os: MacOsControllerOs,
}

impl MacOsGuardianExecutor {
    #[must_use]
    pub fn new(os: MacOsControllerOs) -> Self {
        Self { os }
    }
}

impl crate::GuardianExecutor for MacOsGuardianExecutor {
    type ChildSlot = SupervisedChild;

    fn execute(
        &mut self,
        ticket: crate::DispatchTicket,
        slots: &mut std::collections::BTreeMap<crate::ChildSlotId, SupervisedChild>,
    ) -> crate::ExecutedEffect {
        if let crate::OsEffect::SignalExactIdentity { identity, signal } = ticket.effect() {
            let (identity, signal) = (*identity, *signal);
            let outcome = match slots.values_mut().find(|slot| slot.identity() == identity) {
                Some(slot) => slot.signal_group(signal),
                None => SignalOutcome::TemporarilyUnavailable,
            };
            crate::ExecutedEffect::from_parts(
                ticket.key(),
                crate::EffectCompletion::SignalExactIdentity {
                    identity,
                    signal,
                    outcome,
                },
            )
        } else {
            crate::EffectExecutor::new(&mut self.os).execute(ticket)
        }
    }

    fn poll_observation_only(&self, slot: &SupervisedChild) -> crate::ChildSlotObservation {
        self.os.observe_supervised_slot(slot.identity())
    }
}

impl ControllerOs for MacOsControllerOs {
    fn sample_resources(
        &mut self,
        sequence: u64,
    ) -> Result<ResourceSample, ResourceObservationFailure> {
        let current_cpu = cpu_ticks().map_err(resource_error)?;
        let contention = self.observe_cpu_contention(current_cpu)?;
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
        let count = unsafe { libc::proc_listchildpids(native_parent, std::ptr::null_mut(), 0) };
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
            unsafe { libc::proc_listchildpids(native_parent, pids.as_mut_ptr().cast(), bytes) };
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

#[allow(deprecated)]
fn current_host_port() -> libc::host_t {
    // SAFETY: mach_host_self takes no arguments and returns a send right for this host.
    unsafe { libc::mach_host_self() }
}

fn cpu_ticks() -> Result<CpuTicks, MacOsError> {
    let mut info = MaybeUninit::<libc::host_cpu_load_info>::zeroed();
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: info is correctly sized for HOST_CPU_LOAD_INFO and count names that size.
    let kern_result = unsafe {
        libc::host_statistics(
            current_host_port(),
            libc::HOST_CPU_LOAD_INFO,
            info.as_mut_ptr().cast(),
            &raw mut count,
        )
    };
    if kern_result != libc::KERN_SUCCESS || count != libc::HOST_CPU_LOAD_INFO_COUNT {
        return Err(MacOsError::System(kern_result));
    }
    // SAFETY: host_statistics succeeded with the complete expected count.
    let info = unsafe { info.assume_init() };
    Ok(CpuTicks {
        user: cpu_tick(&info, libc::CPU_STATE_USER)?,
        system: cpu_tick(&info, libc::CPU_STATE_SYSTEM)?,
        idle: cpu_tick(&info, libc::CPU_STATE_IDLE)?,
        nice: cpu_tick(&info, libc::CPU_STATE_NICE)?,
    })
}

fn cpu_tick(info: &libc::host_cpu_load_info, state: c_int) -> Result<u32, MacOsError> {
    let index = usize::try_from(state).map_err(|_| MacOsError::MalformedObservation)?;
    info.cpu_ticks
        .get(index)
        .copied()
        .ok_or(MacOsError::MalformedObservation)
}

fn cpu_contention(
    previous: CpuTicks,
    current: CpuTicks,
) -> Result<CpuContentionBasisPoints, ResourceObservationFailure> {
    let delta = |current: u32, previous: u32| {
        current
            .checked_sub(previous)
            .map(u64::from)
            .ok_or(ResourceObservationFailure::TemporarilyUnavailable)
    };
    let user = delta(current.user, previous.user)?;
    let system = delta(current.system, previous.system)?;
    let idle = delta(current.idle, previous.idle)?;
    let nice = delta(current.nice, previous.nice)?;
    let busy = user + system + nice;
    let total = busy + idle;
    if total == 0 {
        return Err(ResourceObservationFailure::TemporarilyUnavailable);
    }
    let basis_points = busy * 10_000 / total;
    let basis_points =
        u16::try_from(basis_points).map_err(|_| ResourceObservationFailure::Malformed)?;
    CpuContentionBasisPoints::new(basis_points).map_err(|_| ResourceObservationFailure::Malformed)
}

fn available_memory_bytes() -> Result<u64, MacOsError> {
    let mut stats = MaybeUninit::<libc::vm_statistics64>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: stats is correctly sized for HOST_VM_INFO64 and count names that size.
    let kern_result = unsafe {
        libc::host_statistics64(
            current_host_port(),
            libc::HOST_VM_INFO64,
            stats.as_mut_ptr().cast(),
            &raw mut count,
        )
    };
    if kern_result != libc::KERN_SUCCESS || count != libc::HOST_VM_INFO64_COUNT {
        return Err(MacOsError::System(kern_result));
    }
    // SAFETY: host_statistics64 succeeded with the complete expected count.
    let stats = unsafe { stats.assume_init() };
    // SAFETY: sysconf with _SC_PAGESIZE has no pointer arguments.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = u64::try_from(page_size).map_err(|_| MacOsError::MalformedObservation)?;
    reclaimable_memory_bytes(&stats, page_size)
}

fn reclaimable_memory_bytes(
    stats: &libc::vm_statistics64,
    page_size: u64,
) -> Result<u64, MacOsError> {
    if page_size == 0 {
        return Err(MacOsError::MalformedObservation);
    }
    // Darwin free_count already includes speculative_count.
    (u64::from(stats.free_count) + u64::from(stats.inactive_count))
        .checked_mul(page_size)
        .ok_or(MacOsError::MalformedObservation)
}

fn available_storage_bytes(path: &Path) -> Result<u64, MacOsError> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| MacOsError::InvalidPath)?;
    let mut stats = MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: path is NUL-terminated and stats points to writable statvfs storage.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(io_error(&io::Error::last_os_error()));
    }
    // SAFETY: statvfs succeeded and initialized the complete struct.
    let stats = unsafe { stats.assume_init() };
    u64::from(stats.f_bavail)
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
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io_error(&io::Error::last_os_error()));
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
        Some(libc::EPERM | libc::EACCES) => SignalOutcome::PermissionDenied,
        _ => SignalOutcome::TemporarilyUnavailable,
    }
}

fn child_exited_without_reaping(child: &Child) -> Result<bool, MacOsError> {
    let mut info = MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: the retained Child reserves its PID; WNOWAIT preserves that reservation,
    // WNOHANG never waits for exit, and info is writable siginfo_t storage.
    let status = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id(),
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
        )
    };
    if status != 0 {
        return Err(io_error(&io::Error::last_os_error()));
    }
    // SAFETY: the zero-initialized siginfo_t was filled by successful waitid.
    let info = unsafe { info.assume_init() };
    if info.si_pid == 0 {
        return Ok(false);
    }
    let pid = u32::try_from(info.si_pid).map_err(|_| MacOsError::MalformedObservation)?;
    if pid != child.id() {
        return Err(MacOsError::MalformedObservation);
    }
    Ok(true)
}

fn group_has_other_members(group: ProcessId, leader: ProcessId) -> Result<bool, MacOsError> {
    let group = c_int::try_from(group.get()).map_err(|_| MacOsError::MalformedObservation)?;
    let mut pids = [0_i32; 256];
    let bytes =
        c_int::try_from(mem::size_of_val(&pids)).map_err(|_| MacOsError::MalformedObservation)?;
    // SAFETY: pids is writable for bytes bytes. libproc returns a PID count, not bytes.
    let count = unsafe { libc::proc_listpgrppids(group, pids.as_mut_ptr().cast(), bytes) };
    if count < 0 {
        return Err(last_process_error());
    }
    let count = usize::try_from(count).map_err(|_| MacOsError::MalformedObservation)?;
    if count >= pids.len() {
        return Err(MacOsError::TemporarilyUnavailable);
    }
    for &pid in &pids[..count] {
        let pid = u32::try_from(pid)
            .ok()
            .and_then(process_id)
            .ok_or(MacOsError::MalformedObservation)?;
        if pid != leader {
            return Ok(true);
        }
    }
    Ok(false)
}

const BEST_EFFORT_CLEANUP_BUDGET: Duration = Duration::from_millis(100);
const CLEANUP_POLL_INTERVAL: Duration = Duration::from_millis(5);

fn cleanup_partial_child(child: &mut Child, group: Option<ProcessId>) -> bool {
    if let Some(group) = group {
        let _ = signal_process_group(group, libc::SIGKILL);
    }
    // Child::kill retains direct-child authority when group signaling is unavailable.
    let _ = child.kill();
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {}
            Err(_) => return false,
        }
        let remaining = BEST_EFFORT_CLEANUP_BUDGET.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return false;
        }
        thread::sleep(CLEANUP_POLL_INTERVAL.min(remaining));
    }
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
        Some(libc::EPERM | libc::EACCES) => MacOsError::PermissionDenied,
        Some(code) => MacOsError::System(code),
        None => MacOsError::TemporarilyUnavailable,
    }
}

fn io_error(error: &io::Error) -> MacOsError {
    match error.raw_os_error() {
        Some(libc::ESRCH) => MacOsError::NoSuchProcess,
        Some(libc::EPERM | libc::EACCES) => MacOsError::PermissionDenied,
        Some(code) => MacOsError::System(code),
        None => MacOsError::TemporarilyUnavailable,
    }
}

const fn observation_error(error: MacOsError) -> ObservationFailure {
    match error {
        MacOsError::NoSuchProcess => ObservationFailure::NoSuchProcess,
        MacOsError::PermissionDenied => ObservationFailure::PermissionDenied,
        MacOsError::MalformedObservation | MacOsError::InvalidPath => ObservationFailure::Malformed,
        MacOsError::TemporarilyUnavailable
        | MacOsError::PartialSpawnCleanupUnverified { .. }
        | MacOsError::System(_) => ObservationFailure::TemporarilyUnavailable,
    }
}

const fn query_error(error: MacOsError) -> OsQueryError {
    match error {
        MacOsError::PermissionDenied => OsQueryError::PermissionDenied,
        MacOsError::MalformedObservation | MacOsError::InvalidPath => OsQueryError::Malformed,
        MacOsError::NoSuchProcess
        | MacOsError::TemporarilyUnavailable
        | MacOsError::PartialSpawnCleanupUnverified { .. }
        | MacOsError::System(_) => OsQueryError::TemporarilyUnavailable,
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
        | MacOsError::PartialSpawnCleanupUnverified { .. }
        | MacOsError::System(_) => ResourceObservationFailure::TemporarilyUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_state_indices_are_checked_c_int_values() {
        let info = libc::host_cpu_load_info {
            cpu_ticks: [11, 22, 33, 44],
        };
        for (state, expected) in [
            (libc::CPU_STATE_USER, 11),
            (libc::CPU_STATE_SYSTEM, 22),
            (libc::CPU_STATE_IDLE, 33),
            (libc::CPU_STATE_NICE, 44),
        ] {
            assert_eq!(cpu_tick(&info, state), Ok(expected));
        }
        assert_eq!(cpu_tick(&info, -1), Err(MacOsError::MalformedObservation));
        assert_eq!(
            cpu_tick(&info, libc::CPU_STATE_MAX),
            Err(MacOsError::MalformedObservation)
        );
    }

    #[test]
    fn every_regressing_or_wrapping_cpu_counter_is_unavailable() {
        let unchanged = CpuTicks {
            user: 100,
            system: 100,
            idle: 100,
            nice: 100,
        };
        assert_eq!(
            cpu_contention(unchanged, unchanged),
            Err(ResourceObservationFailure::TemporarilyUnavailable)
        );
        let ticks = |[user, system, idle, nice]: [u32; 4]| CpuTicks {
            user,
            system,
            idle,
            nice,
        };
        for state in [0_usize, 1, 2, 3] {
            for old in [100, u32::MAX] {
                let mut previous = [100; 4];
                previous[state] = old;
                let mut current = [110, 110, 210, 110];
                current[state] = if old == u32::MAX { 0 } else { 99 };
                assert_eq!(
                    cpu_contention(ticks(previous), ticks(current)),
                    Err(ResourceObservationFailure::TemporarilyUnavailable)
                );
            }
        }
        assert_eq!(
            cpu_contention(
                unchanged,
                CpuTicks {
                    user: 200,
                    idle: 200,
                    ..unchanged
                }
            )
            .expect("half of measured ticks are busy")
            .get(),
            5_000
        );
    }

    #[test]
    fn sample_rebaselines_after_counter_regression() {
        let mut os = MacOsControllerOs {
            storage_path: PathBuf::from("/tmp"),
            previous_cpu: CpuTicks {
                user: 100,
                system: 100,
                idle: 100,
                nice: 100,
            },
        };
        let reset = CpuTicks {
            user: 0,
            system: 101,
            idle: 200,
            nice: 101,
        };
        assert_eq!(
            os.observe_cpu_contention(reset),
            Err(ResourceObservationFailure::TemporarilyUnavailable)
        );
        assert_eq!(os.previous_cpu, reset);
        assert_eq!(
            os.observe_cpu_contention(CpuTicks {
                user: 100,
                idle: 300,
                ..reset
            })
            .expect("subsequent sample uses the new baseline")
            .get(),
            5_000
        );
    }

    #[test]
    fn speculative_pages_are_not_added_twice() {
        // SAFETY: vm_statistics64 contains only integer counters; all-zero is valid.
        let mut stats = unsafe { MaybeUninit::<libc::vm_statistics64>::zeroed().assume_init() };
        stats.free_count = 100;
        stats.inactive_count = 20;
        stats.speculative_count = 40;
        assert_eq!(reclaimable_memory_bytes(&stats, 4096), Ok(120 * 4096));
        stats.speculative_count = 0;
        assert_eq!(reclaimable_memory_bytes(&stats, 4096), Ok(120 * 4096));
        assert_eq!(
            reclaimable_memory_bytes(&stats, 0),
            Err(MacOsError::MalformedObservation)
        );
        assert_eq!(
            reclaimable_memory_bytes(&stats, u64::MAX),
            Err(MacOsError::MalformedObservation)
        );
    }

    #[test]
    fn partial_cleanup_without_a_group_still_targets_the_retained_child() {
        let mut child = Command::new("/bin/sleep")
            .arg("15")
            .spawn()
            .expect("harmless child starts");
        let started = Instant::now();
        let _leader_reaped = cleanup_partial_child(&mut child, None);
        assert!(started.elapsed() < Duration::from_secs(2));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().expect("nonblocking reap succeeds") {
                assert!(!status.success());
                break;
            }
            assert!(
                Instant::now() < deadline,
                "direct-child kill must not depend on a PGID"
            );
            // test-timing-allow: each retry observes retained-child exit; the sleep only bounds polling load.
            thread::sleep(Duration::from_millis(10));
        }
    }
}
