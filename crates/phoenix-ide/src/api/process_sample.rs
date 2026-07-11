//! Process-group resource sampler for the Process Inspector
//! (`specs/process-inspector/` REQ-PINSP-004).
//!
//! Samples the core trio — `cpu_pct`, `memory_bytes` (proportional /
//! shared-aware, NOT RSS), `process_count` — over a bash handle's process
//! *group* (the `pgid`), at request time only. There is no background
//! sampler: the group is read exactly when an operator is watching it.
//!
//! Two platform concerns, each with a `#[cfg(target_os = …)]` arm and a
//! null-returning fallback so every metric degrades to `null` (with a
//! `debug` log) rather than a misleading `0` on an unsupported host — the
//! same null-not-zero convention `api::deployment::sample_resources` uses:
//!
//! 1. **Group membership** — which pids share the `pgid`:
//!    - Linux: scan `/proc/<pid>/stat` for processes whose process-group id
//!      (field 5) equals `pgid`.
//!    - macOS: enumerate all pids with `libc::proc_listallpids` and keep those
//!      whose `proc_pidinfo(PROC_PIDTBSDINFO)` `pbi_pgid` equals `pgid`.
//!      (`proc_listpgrppids` is unreliable here — it over-sizes on the
//!      null-buffer query and returns a bogus written count on the fill, so
//!      the all-pids-plus-per-pid-pgid scan is the dependable equivalent.)
//! 2. **Proportional memory** — summed over the group:
//!    - Linux: `/proc/<pid>/smaps_rollup` `Pss:`.
//!    - macOS: `libc::proc_pid_rusage(pid, RUSAGE_INFO_V2, …)` `ri_phys_footprint`.
//!
//! `cpu_pct` and `process_count` come from `sysinfo`: it enumerates the
//! group's pids, sums per-process CPU%, and counts live members. CPU% needs
//! two samples separated by `MINIMUM_CPU_UPDATE_INTERVAL` — the same
//! two-refresh pattern `sample_resources` uses, so the first read is
//! meaningful rather than `0`.

use phoenix_core::domain::process_inspection::ResourceSample;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq)]
pub struct SampledProcesses {
    pub cpu_pct: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub process_count: Option<u32>,
}

impl SampledProcesses {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            cpu_pct: Some(0.0),
            memory_bytes: Some(0),
            process_count: Some(0),
        }
    }
}

pub async fn sample_processes(pids: &BTreeSet<u32>) -> SampledProcesses {
    if pids.is_empty() {
        return SampledProcesses::empty();
    }
    let pid_vec: Vec<u32> = pids.iter().copied().collect();
    let process_count = u32::try_from(pid_vec.len()).ok();
    let memory_bytes = group_pss_bytes(&pid_vec);
    let cpu_pct = if pid_vec.is_empty() {
        Some(0.0)
    } else {
        group_cpu_percent(&pid_vec).await
    };
    SampledProcesses {
        cpu_pct,
        memory_bytes,
        process_count,
    }
}

pub fn process_pss_bytes(pid: u32) -> Option<u64> {
    process_pss_bytes_impl(pid)
}

/// Sample the resource trio over the process group identified by `pgid`.
///
/// Called per inspection request for a live handle (the ~1s inspector poll).
/// Latency is dominated by the CPU two-sample interval
/// (`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`, ~200ms); the `/proc` or
/// `proc_listpgrppids` membership reads and the memory reads are cheap.
///
/// Each field is `None` when that metric cannot be read on the host (an old
/// kernel without `smaps_rollup`, a `proc_pid_rusage` failure, a pid that
/// exited between enumeration and read); the gap is logged at `debug`.
pub async fn sample_process_group(pgid: i32) -> ResourceSample {
    let members = group_member_pids(pgid);

    let process_count = match &members {
        Some(pids) => u32::try_from(pids.len()).ok(),
        None => None,
    };

    let memory_bytes = members.as_ref().and_then(|pids| group_pss_bytes(pids));

    let cpu_pct = match &members {
        Some(pids) if !pids.is_empty() => group_cpu_percent(pids).await,
        Some(_) => Some(0.0),
        None => None,
    };

    ResourceSample {
        cpu_pct,
        memory_bytes,
        process_count,
    }
}

/// Sum per-process CPU% over the group's pids via `sysinfo`. Two refreshes
/// separated by the minimum CPU update interval make the percentage
/// meaningful on the first read (mirrors `deployment::sample_resources`).
/// `None` when no group member could be enumerated by `sysinfo`.
async fn group_cpu_percent(pids: &[u32]) -> Option<f32> {
    use sysinfo::{Pid, ProcessesToUpdate, System};

    let sys_pids: Vec<Pid> = pids.iter().map(|&p| Pid::from_u32(p)).collect();

    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&sys_pids), true);
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    sys.refresh_processes(ProcessesToUpdate::Some(&sys_pids), true);

    let mut total = 0.0_f32;
    let mut seen = 0_usize;
    for p in &sys_pids {
        if let Some(proc_) = sys.process(*p) {
            total += proc_.cpu_usage();
            seen += 1;
        }
    }
    if seen == 0 {
        tracing::debug!(
            n = pids.len(),
            "process-inspector: no group members visible to sysinfo for cpu_pct — reporting null"
        );
        return None;
    }
    Some(total)
}

// ===========================================================================
// Linux: /proc-based group membership + smaps_rollup Pss
// ===========================================================================

#[cfg(target_os = "linux")]
fn group_member_pids(pgid: i32) -> Option<Vec<u32>> {
    let entries = match std::fs::read_dir("/proc") {
        Ok(e) => e,
        Err(e) => {
            tracing::debug!(error = %e, "process-inspector: /proc unreadable — group membership null");
            return None;
        }
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(member_pid) = name.parse::<u32>() else {
            continue;
        };
        if proc_pgrp(member_pid) == Some(pgid) {
            out.push(member_pid);
        }
    }
    Some(out)
}

/// Read the process-group id (field 5, 1-indexed) from `/proc/<pid>/stat`.
///
/// The `comm` field (2) is parenthesised and may itself contain spaces or
/// `)`, so we split on the LAST `)` and index into the remaining
/// space-separated fields: after `comm`, field 3 is `state`, field 4 is
/// `ppid`, field 5 is `pgrp`. That places `pgrp` at index 2 of the tail.
#[cfg(target_os = "linux")]
fn proc_pgrp(pid: u32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    let tail = stat.get(close + 1..)?;
    let pgrp = tail.split_whitespace().nth(2)?;
    pgrp.parse::<i32>().ok()
}

/// Sum `/proc/<pid>/smaps_rollup` `Pss:` (in kB, converted to bytes) over the
/// group. `None` when no member exposed a `Pss` line (e.g. an old kernel
/// without `smaps_rollup`, or every member exited mid-read).
#[cfg(target_os = "linux")]
fn group_pss_bytes(pids: &[u32]) -> Option<u64> {
    let mut total: u64 = 0;
    let mut any = false;
    for &pid in pids {
        if let Some(pss) = process_pss_bytes_impl(pid) {
            total = total.saturating_add(pss);
            any = true;
        }
    }
    if any {
        Some(total)
    } else {
        tracing::debug!(
            n = pids.len(),
            "process-inspector: no smaps_rollup Pss readable for group (old kernel or all exited) \
             — memory_bytes null"
        );
        None
    }
}

#[cfg(target_os = "linux")]
fn process_pss_bytes_impl(pid: u32) -> Option<u64> {
    let rollup = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
    for line in rollup.lines() {
        if let Some(rest) = line.strip_prefix("Pss:") {
            // Format: "Pss:                 1234 kB"
            let kb = rest.split_whitespace().next()?.parse::<u64>().ok()?;
            return Some(kb.saturating_mul(1024));
        }
    }
    None
}

// ===========================================================================
// macOS: proc_listpgrppids membership + proc_pid_rusage phys_footprint
// ===========================================================================

#[cfg(target_os = "macos")]
fn group_member_pids(pgid: i32) -> Option<Vec<u32>> {
    // Size the all-pids buffer. A null buffer returns the *count* of pids the
    // kernel would write — not a byte size — so we allocate that many `c_int`
    // slots (plus headroom to absorb pids that appear between the two calls).
    // SAFETY: proc_listallpids with a null buffer only queries the count.
    let needed = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if needed <= 0 {
        tracing::debug!(
            "process-inspector: proc_listallpids sizing failed — group membership null"
        );
        return None;
    }
    let slots = usize::try_from(needed).unwrap_or(0).saturating_add(32);
    let mut buf = vec![0_i32; slots];
    let buf_bytes = (buf.len() * std::mem::size_of::<libc::c_int>())
        .try_into()
        .unwrap_or(libc::c_int::MAX);
    // SAFETY: buf holds `slots` c_int entries; buf_bytes is its size in bytes,
    // matching proc_listallpids' byte-sized buffersize parameter.
    let written =
        unsafe { libc::proc_listallpids(buf.as_mut_ptr().cast::<libc::c_void>(), buf_bytes) };
    if written <= 0 {
        tracing::debug!("process-inspector: proc_listallpids fill failed — group membership null");
        return None;
    }
    let written_count = usize::try_from(written).unwrap_or(0).min(buf.len());
    buf.truncate(written_count);
    Some(
        buf.into_iter()
            .filter(|&p| p > 0 && proc_pgid(p) == Some(pgid))
            .map(u32::try_from)
            .filter_map(Result::ok)
            .collect(),
    )
}

/// Read a process's group id via `proc_pidinfo(PROC_PIDTBSDINFO)` `pbi_pgid`.
/// `None` when the pid is gone or the call returns a short read (the pid
/// exited between enumeration and this read).
#[cfg(target_os = "macos")]
fn proc_pgid(pid: i32) -> Option<i32> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: proc_pidinfo writes a `proc_bsdinfo` for the PROC_PIDTBSDINFO
    // flavor into the provided, correctly-sized buffer. A full write returns
    // exactly `size`.
    let rc = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::addr_of_mut!(info).cast::<libc::c_void>(),
            size,
        )
    };
    if rc == size {
        i32::try_from(info.pbi_pgid).ok()
    } else {
        None
    }
}

/// Sum `proc_pid_rusage(pid, RUSAGE_INFO_V2)`'s `ri_phys_footprint` over the
/// group — the proportional, shared-aware footprint macOS attributes to each
/// process. `None` when no member's rusage could be read.
#[cfg(target_os = "macos")]
fn group_pss_bytes(pids: &[u32]) -> Option<u64> {
    let mut total: u64 = 0;
    let mut any = false;
    for &pid in pids {
        if let Some(bytes) = proc_phys_footprint(pid) {
            total = total.saturating_add(bytes);
            any = true;
        }
    }
    if any {
        Some(total)
    } else {
        tracing::debug!(
            n = pids.len(),
            "process-inspector: no proc_pid_rusage phys_footprint readable for group \
             — memory_bytes null"
        );
        None
    }
}

#[cfg(target_os = "macos")]
fn proc_phys_footprint(pid: u32) -> Option<u64> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: proc_pid_rusage writes a `rusage_info_v2` for the RUSAGE_INFO_V2
    // flavor into the provided buffer; we pass a pointer to a zeroed,
    // correctly-typed struct. The `rusage_info_t` param type is `*mut
    // *mut c_void`, so we cast our struct pointer through it.
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V2,
            std::ptr::addr_of_mut!(info).cast::<libc::rusage_info_t>(),
        )
    };
    if rc != 0 {
        return None;
    }
    Some(info.ri_phys_footprint)
}

#[cfg(target_os = "macos")]
fn process_pss_bytes_impl(pid: u32) -> Option<u64> {
    proc_phys_footprint(pid)
}

// ===========================================================================
// Fallback: any other target. Every metric is a capability gap.
// ===========================================================================

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn group_member_pids(_pgid: i32) -> Option<Vec<u32>> {
    tracing::debug!(
        "process-inspector: process-group membership unsupported on this platform \
         — resource trio null"
    );
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn group_pss_bytes(_pids: &[u32]) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[tokio::test]
    async fn sample_processes_empty_set_reports_zeroes() {
        let pids = BTreeSet::new();
        let sample = sample_processes(&pids).await;
        assert_eq!(sample, SampledProcesses::empty());
    }

    /// On the host platform (macOS in CI here, Linux on the cross-compiled
    /// target), sampling this test process's own process group must yield a
    /// non-null trio: this process exists, so membership, a memory figure,
    /// and a CPU% are all readable.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn samples_own_process_group_with_non_null_trio() {
        // getpgrp() returns this process's group id.
        // SAFETY: getpgrp takes no arguments and only reads kernel state.
        let pgid = unsafe { libc::getpgrp() };
        let sample = sample_process_group(pgid).await;

        assert!(
            sample.process_count.is_some_and(|c| c >= 1),
            "own pgid must contain at least this process: {:?}",
            sample.process_count
        );
        assert!(
            sample.memory_bytes.is_some_and(|m| m > 0),
            "own process must report a non-zero proportional memory figure: {:?}",
            sample.memory_bytes
        );
        assert!(
            sample.cpu_pct.is_some(),
            "cpu_pct must be a real (possibly 0.0) sample, not null, for a live group"
        );
    }

    /// A pgid that does not name any live process group yields a structurally
    /// honest sample: zero members, and memory null (nothing to sum) — never
    /// a panic.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn nonexistent_group_does_not_panic() {
        // A very high pgid that is overwhelmingly unlikely to exist.
        let sample = sample_process_group(2_000_000_000).await;
        // process_count is Some(0) (membership enumeration succeeded, found
        // none) on both platforms; memory is null (no members to sum).
        assert!(sample.memory_bytes.is_none() || sample.memory_bytes == Some(0));
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_pss_bytes_impl(pid: u32) -> Option<u64> {
    let _ = pid;
    tracing::debug!("process-inspector: per-process memory_bytes unsupported on this host");
    None
}
