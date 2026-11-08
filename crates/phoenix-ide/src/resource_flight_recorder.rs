//! Bounded resource history, independent of SQLite and structured-log storage.
//! The first segment carries an exclusive advisory lock for the writer's lifetime.
//! A complete frame is not a durability claim: only successful `sync_data`
//! advances `committed`. After a crash, even a valid unsynced tail may be lost.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{mpsc, Arc};

mod sampling;
pub(crate) use sampling::{initialize, spawn_sampler};

const SEGMENT_BYTES: u64 = 2 * 1024 * 1024;
const FRAME_BYTES: usize = 1024;
const HEADER_BYTES: usize = 6;
const DIRECTORY: &str = "resource-flight-recorder";
const NAMES: [&str; 2] = ["resources.0.frames", "resources.1.frames"];

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct ProcessIdentity {
    #[serde(rename = "p")]
    pub pid: u32,
    #[serde(rename = "t")]
    pub started_unix_ms: Option<u64>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum MemorySource {
    #[serde(rename = "p")]
    LinuxPss,
    #[serde(rename = "f")]
    MacPhysicalFootprint,
    #[serde(rename = "u")]
    Unavailable,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum HostPressure {
    #[serde(rename = "n")]
    Normal,
    #[serde(rename = "w")]
    Warning,
    #[serde(rename = "c")]
    Critical,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum Phase {
    #[serde(rename = "s")]
    Startup,
    #[serde(rename = "r")]
    Running,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct Sample {
    #[serde(rename = "k")]
    pub phase: Phase,
    #[serde(rename = "t")]
    pub sampled_unix_ms: Option<u64>,
    #[serde(rename = "u")]
    pub process_uptime_ms: u64,
    #[serde(rename = "h")]
    pub host_uptime_secs: Option<u64>,
    #[serde(rename = "g")]
    pub generation_unix_ms: Option<u64>,
    #[serde(rename = "d")]
    pub disk_unix_ms: Option<u64>,
    #[serde(rename = "m")]
    pub memory_source: MemorySource,
    #[serde(rename = "r")]
    pub current_rss_bytes: Option<u64>,
    #[serde(rename = "p")]
    pub proportional_bytes: Option<u64>,
    #[serde(rename = "l")]
    pub lifetime_max_rss_bytes: Option<u64>,
    #[serde(rename = "b")]
    pub host_bytes: [Option<u64>; 4],
    #[serde(rename = "v")]
    pub host_pressure: Option<HostPressure>,
    #[serde(rename = "s")]
    pub disk_available_bytes: [Option<u64>; 3],
    #[serde(rename = "a")]
    pub managed_proportional_bytes: Option<u64>,
    #[serde(rename = "n")]
    pub managed_pid_count: Option<u64>,
    #[serde(rename = "w")]
    pub live_materialized_handles: Option<u64>,
    #[serde(rename = "e")]
    pub last_failure_unix_ms: Option<u64>,
    #[serde(rename = "c")]
    pub sqlite_extended_code: Option<i32>,
}

#[derive(Clone, Copy)]
pub(crate) struct Observation {
    pub sample: Sample,
    pub failure_counts: [Option<u64>; 11],
}

#[derive(Serialize, Deserialize)]
struct Record {
    #[serde(rename = "v")]
    version: u8,
    #[serde(rename = "q")]
    sequence: u64,
    #[serde(rename = "i")]
    process: ProcessIdentity,
    #[serde(rename = "x")]
    sample: Sample,
    #[serde(rename = "f")]
    failure_deltas: [Option<u64>; 11],
    #[serde(rename = "z")]
    recorder_drop_deltas: [Option<u64>; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub(crate) enum FailureCode {
    None = 0,
    Encode = 1,
    Oversized = 2,
    Io = 3,
    SequenceExhausted = 4,
    Disconnected = 5,
}

#[derive(Default)]
struct Counters {
    sampled: AtomicU64,
    queued: AtomicU64,
    dropped_full: AtomicU64,
    dropped_disconnected: AtomicU64,
    dropped_encode: AtomicU64,
    dropped_oversized: AtomicU64,
    appended: AtomicU64,
    committed: AtomicU64,
    io_errors: AtomicU64,
    last_committed_sequence: AtomicU64,
    failure: AtomicU64,
}

/// Session-local counts, read independently (not an atomic multi-field snapshot).
/// `sampled` counts calls, `appended` complete writes, `committed` synced writes.
/// Queued/in-flight records and unsynced bytes can be lost on exit. Recovery
/// validates bytes, but cannot retrospectively prove their pre-crash sync state.
pub(crate) struct CounterSnapshot {
    pub sampled: u64,
    pub queued: u64,
    pub dropped_full: u64,
    pub dropped_disconnected: u64,
    pub dropped_encode: u64,
    pub dropped_oversized: u64,
    pub appended: u64,
    pub committed: u64,
    pub io_errors: u64,
    pub last_committed_sequence: u64,
    pub failure: FailureCode,
}

pub(crate) struct Handle {
    sender: mpsc::SyncSender<Observation>,
    counters: Arc<Counters>,
}

impl Handle {
    /// Never waits for disk or channel capacity. One record may be pending in
    /// addition to the writer's in-flight record. Drop only disconnects, not joins.
    pub(crate) fn try_record(&self, sample: Observation) {
        self.counters.sampled.fetch_add(1, Relaxed);
        match self.sender.try_send(sample) {
            Ok(()) => {
                self.counters.queued.fetch_add(1, Relaxed);
            }
            Err(mpsc::TrySendError::Full(_)) => {
                self.counters.dropped_full.fetch_add(1, Relaxed);
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.counters.dropped_disconnected.fetch_add(1, Relaxed);
                let _ = self.counters.failure.compare_exchange(
                    FailureCode::None as u64,
                    FailureCode::Disconnected as u64,
                    Relaxed,
                    Relaxed,
                );
            }
        }
    }

    pub(crate) fn snapshot(&self) -> CounterSnapshot {
        let c = &self.counters;
        CounterSnapshot {
            sampled: c.sampled.load(Relaxed),
            queued: c.queued.load(Relaxed),
            dropped_full: c.dropped_full.load(Relaxed),
            dropped_disconnected: c.dropped_disconnected.load(Relaxed),
            dropped_encode: c.dropped_encode.load(Relaxed),
            dropped_oversized: c.dropped_oversized.load(Relaxed),
            appended: c.appended.load(Relaxed),
            committed: c.committed.load(Relaxed),
            io_errors: c.io_errors.load(Relaxed),
            last_committed_sequence: c.last_committed_sequence.load(Relaxed),
            failure: match c.failure.load(Relaxed) {
                0 => FailureCode::None,
                1 => FailureCode::Encode,
                2 => FailureCode::Oversized,
                3 => FailureCode::Io,
                4 => FailureCode::SequenceExhausted,
                _ => FailureCode::Disconnected,
            },
        }
    }
}

/// Optional startup: errors are returned, never logged or made process-fatal.
/// `data_directory` must already exist, be owned by the effective user, and not
/// permit group/other writes. Its mode is never changed. Storage creates a fixed
/// `resource-flight-recorder/` child (0700) holding the two fixed files (0600).
/// Existing unsafe entries are rejected, not repaired. Unix only; no environment
/// variable enables this leaf.
pub(crate) fn start(data_directory: &Path, process: &ProcessIdentity) -> io::Result<Handle> {
    let process = *process;
    let (storage, sequence) = Storage::open(data_directory)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let counters = Arc::new(Counters::default());
    let worker_counters = counters.clone();
    std::thread::Builder::new()
        .name("resource-recorder".into())
        .spawn(move || {
            run(storage, sequence, process, receiver, &worker_counters);
        })?;
    Ok(Handle { sender, counters })
}

// FNV-1a detects accidental corruption, not malicious modifications.
fn checksum(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c9dc5u32, |h, b| {
        (h ^ u32::from(*b)).wrapping_mul(0x01000193)
    })
}

struct BoundedJson(Vec<u8>);
impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > FRAME_BYTES - HEADER_BYTES - self.0.len() {
            return Err(io::Error::other("record bound"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn frame(record: &Record) -> Result<Vec<u8>, FailureCode> {
    let mut json = BoundedJson(Vec::new());
    if let Err(error) = serde_json::to_writer(&mut json, record) {
        return Err(if error.is_io() {
            FailureCode::Oversized
        } else {
            FailureCode::Encode
        });
    }
    let mut bytes = Vec::with_capacity(HEADER_BYTES + json.0.len());
    bytes.extend_from_slice(&(json.0.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&checksum(&json.0).to_le_bytes());
    bytes.extend_from_slice(&json.0);
    Ok(bytes)
}

/// Scans at most one segment; first invalid frame ends the valid prefix.
fn scan(reader: impl Read) -> io::Result<(u64, u64)> {
    let mut bytes = Vec::new();
    reader.take(SEGMENT_BYTES).read_to_end(&mut bytes)?;
    let (mut offset, mut sequence) = (0usize, 0u64);
    while bytes.len() - offset >= HEADER_BYTES {
        let length = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        let end = offset + HEADER_BYTES + length;
        if length == 0 || length + HEADER_BYTES > FRAME_BYTES || end > bytes.len() {
            break;
        }
        let payload = &bytes[offset + HEADER_BYTES..end];
        let hash = u32::from_le_bytes(bytes[offset + 2..offset + 6].try_into().unwrap());
        if checksum(payload) != hash {
            break;
        }
        let Ok(record) = serde_json::from_slice::<Record>(payload) else {
            break;
        };
        if record.version != 2 || record.sequence <= sequence {
            break;
        }
        sequence = record.sequence;
        offset = end;
    }
    Ok((offset as u64, sequence))
}

// This small seam exercises write_all/short writes and sync failures without
// filling a host disk. Rotation is a storage operation, not a policy callback.
trait Sink {
    fn append(&mut self, frame: &[u8]) -> io::Result<()>;
    fn sync(&mut self) -> io::Result<()>;
}

struct Storage {
    files: [File; 2],
    active: usize,
    length: u64,
}

impl Storage {
    #[cfg(not(unix))]
    fn open(_: &Path) -> io::Result<(Self, u64)> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "resource recorder requires Unix no-follow storage",
        ))
    }

    #[cfg(unix)]
    fn open(data_directory: &Path) -> io::Result<(Self, u64)> {
        use nix::fcntl::{open, openat, OFlag};
        use nix::sys::stat::{mkdirat, Mode};
        use std::os::unix::fs::MetadataExt;

        let directory_flags =
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
        let parent = File::from(open(data_directory, directory_flags, Mode::empty())?);
        let metadata = parent.metadata()?;
        validate_owner(metadata.uid())?;
        if !metadata.is_dir() || metadata.mode() & 0o7022 != 0 {
            return Err(unsafe_storage("unsafe recorder data directory"));
        }
        match mkdirat(&parent, DIRECTORY, Mode::S_IRWXU) {
            Ok(()) | Err(nix::errno::Errno::EEXIST) => {}
            Err(error) => return Err(error.into()),
        }
        let directory = File::from(openat(&parent, DIRECTORY, directory_flags, Mode::empty())?);
        let metadata = directory.metadata()?;
        validate_owner(metadata.uid())?;
        if !metadata.is_dir() || metadata.mode() & 0o7777 != 0o700 {
            return Err(unsafe_storage("recorder directory must be private (0700)"));
        }
        let open_segment = |name: &str| -> io::Result<File> {
            // O_NONBLOCK prevents opening a pre-existing FIFO from stalling startup.
            let flags = OFlag::O_RDWR | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK;
            let descriptor = match openat(
                &directory,
                name,
                flags | OFlag::O_CREAT | OFlag::O_EXCL,
                Mode::S_IRUSR | Mode::S_IWUSR,
            ) {
                Ok(descriptor) => descriptor,
                Err(nix::errno::Errno::EEXIST) => openat(&directory, name, flags, Mode::empty())?,
                Err(error) => return Err(error.into()),
            };
            Ok(File::from(descriptor))
        };
        let mut files = [open_segment(NAMES[0])?, open_segment(NAMES[1])?];
        validate_segments(&files)?;
        fs2::FileExt::try_lock_exclusive(&files[0])?;
        let prefixes = [scan(&mut files[0])?, scan(&mut files[1])?];
        let active = usize::from(prefixes[1].1 > prefixes[0].1);
        // Discard only invalid tails, never the complete prefix on reopen.
        for (file, prefix) in files.iter_mut().zip(prefixes) {
            file.set_len(prefix.0)?;
            file.seek(SeekFrom::Start(prefix.0))?;
        }
        Ok((
            Self {
                files,
                active,
                length: prefixes[active].0,
            },
            prefixes[active].1,
        ))
    }
}

#[cfg(unix)]
fn unsafe_storage(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(unix)]
fn validate_owner(uid: u32) -> io::Result<()> {
    // SAFETY: geteuid has no arguments or memory safety preconditions.
    if uid != unsafe { libc::geteuid() } {
        return Err(unsafe_storage(
            "recorder entry is not owned by the effective user",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_segments(files: &[File; 2]) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = [files[0].metadata()?, files[1].metadata()?];
    for entry in &metadata {
        validate_owner(entry.uid())?;
        if !entry.is_file() || entry.nlink() != 1 || entry.mode() & 0o7777 != 0o600 {
            return Err(unsafe_storage(
                "recorder segment must be regular, single-link, private (0600)",
            ));
        }
        if entry.len() > SEGMENT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "recorder segment exceeds bound",
            ));
        }
    }
    if (metadata[0].dev(), metadata[0].ino()) == (metadata[1].dev(), metadata[1].ino()) {
        return Err(unsafe_storage("recorder segments share an inode"));
    }
    Ok(())
}

fn append_bytes(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    writer.write_all(bytes)
}

impl Sink for Storage {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > FRAME_BYTES {
            return Err(io::Error::other("frame bound"));
        }
        if self.length + bytes.len() as u64 > SEGMENT_BYTES {
            let next = 1 - self.active;
            // The previous active segment remains intact if truncate/write fails.
            self.files[next].set_len(0)?;
            self.files[next].seek(SeekFrom::Start(0))?;
            self.active = next;
            self.length = 0;
        }
        append_bytes(&mut self.files[self.active], bytes)?;
        self.length += bytes.len() as u64;
        Ok(())
    }
    fn sync(&mut self) -> io::Result<()> {
        self.files[self.active].sync_data()
    }
}

fn nonzero_deltas<const N: usize>(
    current: [Option<u64>; N],
    committed: [Option<u64>; N],
) -> [Option<u64>; N] {
    std::array::from_fn(|index| {
        current[index]?
            .checked_sub(committed[index].unwrap_or(0))
            .filter(|delta| *delta != 0)
    })
}

fn run(
    mut storage: impl Sink,
    mut sequence: u64,
    process: ProcessIdentity,
    receiver: mpsc::Receiver<Observation>,
    counters: &Counters,
) {
    let mut failure_baseline = [Some(0); 11];
    let mut drop_baseline = [Some(0); 4];
    for observation in receiver {
        let drop_counts = [
            Some(counters.dropped_full.load(Relaxed)),
            Some(counters.dropped_disconnected.load(Relaxed)),
            Some(counters.dropped_encode.load(Relaxed)),
            Some(counters.dropped_oversized.load(Relaxed)),
        ];
        let Some(next) = sequence.checked_add(1) else {
            counters
                .failure
                .store(FailureCode::SequenceExhausted as u64, Relaxed);
            return;
        };
        let bytes = match frame(&Record {
            version: 2,
            sequence: next,
            process,
            sample: observation.sample,
            failure_deltas: nonzero_deltas(observation.failure_counts, failure_baseline),
            recorder_drop_deltas: nonzero_deltas(drop_counts, drop_baseline),
        }) {
            Ok(bytes) => bytes,
            Err(code) => {
                match code {
                    FailureCode::Oversized => &counters.dropped_oversized,
                    _ => &counters.dropped_encode,
                }
                .fetch_add(1, Relaxed);
                counters.failure.store(code as u64, Relaxed);
                continue;
            }
        };
        // Stop permanently on the first storage failure; no repeated disk work.
        if storage.append(&bytes).is_err() {
            counters.io_errors.fetch_add(1, Relaxed);
            counters.failure.store(FailureCode::Io as u64, Relaxed);
            return;
        }
        counters.appended.fetch_add(1, Relaxed);
        if storage.sync().is_err() {
            counters.io_errors.fetch_add(1, Relaxed);
            counters.failure.store(FailureCode::Io as u64, Relaxed);
            return;
        }
        failure_baseline = std::array::from_fn(|index| {
            observation.failure_counts[index].or(failure_baseline[index])
        });
        drop_baseline = drop_counts;
        sequence = next;
        counters.last_committed_sequence.store(sequence, Relaxed);
        counters.committed.fetch_add(1, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(sequence: u64) -> Record {
        Record {
            version: 2,
            sequence,
            process: ProcessIdentity {
                pid: 7,
                started_unix_ms: Some(5),
            },
            sample: Sample {
                phase: Phase::Running,
                sampled_unix_ms: Some(11),
                process_uptime_ms: 6,
                host_uptime_secs: None,
                generation_unix_ms: None,
                disk_unix_ms: None,
                memory_source: MemorySource::Unavailable,
                current_rss_bytes: None,
                proportional_bytes: None,
                lifetime_max_rss_bytes: None,
                host_bytes: [None; 4],
                host_pressure: None,
                disk_available_bytes: [None; 3],
                managed_proportional_bytes: None,
                managed_pid_count: None,
                live_materialized_handles: None,
                last_failure_unix_ms: None,
                sqlite_extended_code: None,
            },
            failure_deltas: [None; 11],
            recorder_drop_deltas: [None; 4],
        }
    }

    fn observation(sequence: u64) -> Observation {
        Observation {
            sample: record(sequence).sample,
            failure_counts: [Some(0); 11],
        }
    }

    #[test]
    fn corrupt_and_torn_tails_stop_at_complete_prefix() {
        let first = frame(&record(1)).unwrap();
        let second = frame(&record(2)).unwrap();
        for cut in 0..second.len() {
            let mut bytes = first.clone();
            bytes.extend_from_slice(&second[..cut]);
            assert_eq!(scan(bytes.as_slice()).unwrap(), (first.len() as u64, 1));
        }
        let mut bytes = first.clone();
        let mut corrupt = second;
        corrupt[HEADER_BYTES + 3] ^= 1;
        bytes.extend(corrupt);
        assert_eq!(scan(bytes.as_slice()).unwrap(), (first.len() as u64, 1));
    }

    #[derive(Default)]
    struct Fake {
        bytes: Vec<u8>,
        capacity: usize,
        sync_fails: bool,
    }
    impl Write for Fake {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let count = bytes.len().min(3).min(self.capacity - self.bytes.len());
            if count == 0 {
                return Err(io::Error::other("fake full"));
            }
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Sink for Fake {
        fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
            append_bytes(self, bytes)
        }
        fn sync(&mut self) -> io::Result<()> {
            if self.sync_fails {
                Err(io::Error::other("fake sync"))
            } else {
                Ok(())
            }
        }
    }
    impl Sink for &mut Fake {
        fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
            (**self).append(bytes)
        }
        fn sync(&mut self) -> io::Result<()> {
            (**self).sync()
        }
    }

    #[test]
    fn short_writes_complete_and_full_stops_without_retry() {
        let bytes = frame(&record(1)).unwrap();
        let mut fake = Fake {
            capacity: FRAME_BYTES,
            ..Fake::default()
        };
        append_bytes(&mut fake, &bytes).unwrap();
        assert_eq!(fake.bytes, bytes);
        for (capacity, sync_fails, appended) in [(17, false, 0), (FRAME_BYTES, true, 1)] {
            let mut fake = Fake {
                capacity,
                sync_fails,
                ..Fake::default()
            };
            let (tx, rx) = mpsc::sync_channel(2);
            tx.send(observation(1)).unwrap();
            tx.send(observation(2)).unwrap();
            drop(tx);
            let counters = Counters::default();
            run(&mut fake, 0, record(1).process, rx, &counters);
            assert_eq!(counters.io_errors.load(Relaxed), 1);
            assert_eq!(counters.appended.load(Relaxed), appended);
            assert_eq!(counters.committed.load(Relaxed), 0);
            assert_eq!(
                fake.bytes.len(),
                if sync_fails { bytes.len() } else { capacity }
            );
        }
    }

    #[test]
    fn stalled_queue_is_one_slot_and_drop_never_joins() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let handle = Handle {
            sender,
            counters: Arc::new(Counters::default()),
        };
        for _ in 0..100 {
            handle.try_record(observation(1));
        }
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.sampled, 100);
        assert_eq!(snapshot.queued, 1);
        assert_eq!(snapshot.dropped_full, 99);
        receiver.recv().unwrap();
        drop(receiver);
        handle.try_record(observation(1));
        assert_eq!(handle.snapshot().dropped_disconnected, 1);
        drop(handle);
    }

    #[test]
    fn frame_and_serialization_bounds() {
        let mut r = record(u64::MAX);
        r.process.pid = u32::MAX;
        r.process.started_unix_ms = Some(u64::MAX);
        r.sample.sampled_unix_ms = Some(u64::MAX);
        r.sample.process_uptime_ms = u64::MAX;
        r.sample.host_uptime_secs = Some(u64::MAX);
        r.sample.generation_unix_ms = Some(u64::MAX);
        r.sample.disk_unix_ms = Some(u64::MAX);
        r.sample.memory_source = MemorySource::MacPhysicalFootprint;
        r.sample.current_rss_bytes = Some(u64::MAX);
        r.sample.proportional_bytes = Some(u64::MAX);
        r.sample.lifetime_max_rss_bytes = Some(u64::MAX);
        r.sample.host_bytes = [Some(u64::MAX); 4];
        r.sample.host_pressure = Some(HostPressure::Critical);
        r.sample.disk_available_bytes = [Some(u64::MAX); 3];
        r.sample.managed_proportional_bytes = Some(u64::MAX);
        r.sample.managed_pid_count = Some(u64::MAX);
        r.sample.live_materialized_handles = Some(u64::MAX);
        r.sample.last_failure_unix_ms = Some(u64::MAX);
        r.sample.sqlite_extended_code = Some(i32::MIN);
        r.failure_deltas = [Some(u64::MAX); 11];
        r.recorder_drop_deltas = [Some(u64::MAX); 4];
        assert!(frame(&r).unwrap().len() <= FRAME_BYTES);
        let value = serde_json::to_value(&r).unwrap();
        assert_eq!(value["i"]["t"], u64::MAX);
        for key in ["t", "g", "d"] {
            assert_eq!(value["x"][key], u64::MAX);
        }
        r.process.started_unix_ms = None;
        r.sample.sampled_unix_ms = None;
        r.sample.generation_unix_ms = None;
        r.sample.disk_unix_ms = None;
        assert!(frame(&r).unwrap().len() <= FRAME_BYTES);
        let value = serde_json::to_value(&r).unwrap();
        assert_eq!(value["i"].get("t"), Some(&serde_json::Value::Null));
        for key in ["t", "g", "d"] {
            assert_eq!(value["x"].get(key), Some(&serde_json::Value::Null));
        }
        let decoded: Record = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.process.started_unix_ms, None);
        assert_eq!(decoded.sample.sampled_unix_ms, None);
        assert_eq!(decoded.sample.generation_unix_ms, None);
        assert_eq!(decoded.sample.disk_unix_ms, None);
        let mut bounded = BoundedJson(Vec::new());
        assert!(bounded.write_all(&[0; FRAME_BYTES]).is_err());
        assert!(bounded.0.is_empty());
        assert_eq!(scan(&[255u8; FRAME_BYTES][..]).unwrap(), (0, 0));
    }

    #[test]
    fn failure_and_drop_deltas_follow_only_synced_samples() {
        let mut fake = Fake {
            capacity: FRAME_BYTES * 3,
            ..Fake::default()
        };
        let (tx, rx) = mpsc::sync_channel(3);
        for count in [7, 7, 9] {
            let mut value = observation(1);
            value.failure_counts[0] = Some(count);
            tx.send(value).unwrap();
        }
        drop(tx);
        let counters = Counters::default();
        counters.dropped_full.store(12, Relaxed);
        run(&mut fake, 0, record(1).process, rx, &counters);
        assert_eq!(counters.committed.load(Relaxed), 3);
        let bytes = fake.bytes.lock().unwrap();
        let mut offset = 0;
        for (expected, drops) in [(Some(7), Some(12)), (None, None), (Some(2), None)] {
            let length = usize::from(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]));
            let value: Record = serde_json::from_slice(
                &bytes[offset + HEADER_BYTES..offset + HEADER_BYTES + length],
            )
            .unwrap();
            assert_eq!(value.failure_deltas[0], expected);
            assert_eq!(value.recorder_drop_deltas[0], drops);
            assert!(value.failure_deltas[1..].iter().all(Option::is_none));
            offset += HEADER_BYTES + length;
        }
        assert_eq!(offset, bytes.len());
        assert_eq!(
            nonzero_deltas([Some(0), None], [Some(9), Some(9)]),
            [None, None]
        );
    }

    #[test]
    fn restart_and_reboot_evidence_does_not_invent_missing_samples() {
        let mut first = record(1);
        first.sample.host_uptime_secs = Some(1000);
        let mut second = record(2);
        second.process.pid = 8;
        second.process.started_unix_ms = Some(5000);
        second.sample.sampled_unix_ms = Some(5001);
        second.sample.host_uptime_secs = Some(3);
        second.sample.process_uptime_ms = 1;
        second.sample.phase = Phase::Startup;
        let mut bytes = frame(&first).unwrap();
        bytes.extend(frame(&second).unwrap());
        assert_eq!(scan(&bytes[..]).unwrap(), (bytes.len() as u64, 2));
        let value = serde_json::to_value(second).unwrap();
        assert_eq!(value["i"]["p"], 8);
        assert_eq!(value["x"]["h"], 3);
        assert_eq!(value["x"]["k"], "s");
        assert!(value["x"]["g"].is_null());
        assert!(value["x"]["l"].is_null());
        assert!(value["x"].get("gap_duration").is_none());
    }

    #[test]
    fn persisted_record_has_a_closed_primitive_and_fixed_enum_allowlist() {
        fn inspect(value: &serde_json::Value) {
            match value {
                serde_json::Value::String(text) => {
                    assert!(["s", "r", "p", "f", "u", "n", "w", "c"].contains(&text.as_str()))
                }
                serde_json::Value::Array(values) => values.iter().for_each(inspect),
                serde_json::Value::Object(values) => values.values().for_each(inspect),
                _ => {}
            }
        }
        let value = serde_json::to_value(record(1)).unwrap();
        inspect(&value);
        let keys = value["x"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            std::collections::BTreeSet::from([
                "k", "t", "u", "h", "g", "d", "m", "r", "p", "l", "b", "v", "s", "a", "n", "w",
                "e", "c"
            ])
        );
    }

    #[test]
    fn sequence_exhaustion_never_wraps_or_writes() {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(observation(1)).unwrap();
        drop(tx);
        let counters = Counters::default();
        let mut fake = Fake {
            capacity: FRAME_BYTES,
            ..Fake::default()
        };
        run(&mut fake, u64::MAX, record(1).process, rx, &counters);
        assert!(fake.bytes.is_empty());
        assert_eq!(
            counters.failure.load(Relaxed),
            FailureCode::SequenceExhausted as u64
        );
    }

    #[cfg(unix)]
    mod storage_safety {
        use super::*;
        use std::fs::{self, OpenOptions};
        use std::os::unix::fs::{symlink, MetadataExt, OpenOptionsExt, PermissionsExt};
        use std::path::PathBuf;

        // Deliberately torn tails reveal any premature truncation on refusal.
        fn fixture() -> (tempfile::TempDir, [PathBuf; 2], Vec<u8>) {
            let root = tempfile::tempdir().unwrap();
            drop(Storage::open(root.path()).unwrap());
            let paths = NAMES.map(|name| root.path().join(DIRECTORY).join(name));
            let mut sentinel = frame(&record(1)).unwrap();
            sentinel.extend_from_slice(b"torn sentinel");
            for path in &paths {
                fs::write(path, &sentinel).unwrap();
            }
            (root, paths, sentinel)
        }

        fn private_file(path: &Path, bytes: &[u8]) -> File {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .unwrap();
            file.write_all(bytes).unwrap();
            file
        }

        #[test]
        fn symlink_segments_never_touch_targets_or_other_segment() {
            for slot in 0..2 {
                let (root, paths, sentinel) = fixture();
                let outside = tempfile::tempdir().unwrap();
                let target = outside.path().join("target");
                let file = private_file(&target, &sentinel);
                let inode = file.metadata().unwrap().ino();
                fs::remove_file(&paths[slot]).unwrap();
                symlink(&target, &paths[slot]).unwrap();
                assert!(Storage::open(root.path()).is_err());
                assert_eq!(fs::read(&target).unwrap(), sentinel);
                assert_eq!(file.metadata().unwrap().ino(), inode);
                assert_eq!(fs::read(&paths[1 - slot]).unwrap(), sentinel);
                assert!(fs::symlink_metadata(&paths[slot]).unwrap().is_symlink());
            }
        }

        #[test]
        fn hardlink_segments_never_touch_targets_or_other_segment() {
            for slot in 0..2 {
                let (root, paths, sentinel) = fixture();
                let outside = tempfile::tempdir().unwrap();
                let target = outside.path().join("target");
                let file = private_file(&target, &sentinel);
                fs::remove_file(&paths[slot]).unwrap();
                fs::hard_link(&target, &paths[slot]).unwrap();
                assert!(Storage::open(root.path()).is_err());
                assert_eq!(fs::read(&target).unwrap(), sentinel);
                assert_eq!(file.metadata().unwrap().nlink(), 2);
                assert_eq!(fs::read(&paths[1 - slot]).unwrap(), sentinel);
            }
        }

        #[test]
        fn same_inode_is_rejected_even_without_multiple_links() {
            let (root, paths, sentinel) = fixture();
            fs::remove_file(&paths[1]).unwrap();
            fs::hard_link(&paths[0], &paths[1]).unwrap();
            assert!(Storage::open(root.path()).is_err());
            assert_eq!(fs::read(&paths[0]).unwrap(), sentinel);
            assert_eq!(fs::read(&paths[1]).unwrap(), sentinel);
            fs::remove_file(&paths[1]).unwrap();
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&paths[0])
                .unwrap();
            assert_eq!(file.metadata().unwrap().nlink(), 1);
            let alias = file.try_clone().unwrap();
            assert!(validate_segments(&[file, alias]).is_err());
            assert_eq!(fs::read(&paths[0]).unwrap(), sentinel);
        }

        #[test]
        fn fifo_and_directory_are_refused_without_touching_other_segment() {
            use nix::sys::stat::Mode;
            use nix::unistd::mkfifo;

            for slot in 0..2 {
                for fifo in [false, true] {
                    let (root, paths, sentinel) = fixture();
                    fs::remove_file(&paths[slot]).unwrap();
                    if fifo {
                        mkfifo(&paths[slot], Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
                    } else {
                        fs::create_dir(&paths[slot]).unwrap();
                    }
                    assert!(Storage::open(root.path()).is_err());
                    assert_eq!(fs::read(&paths[1 - slot]).unwrap(), sentinel);
                    let metadata = fs::symlink_metadata(&paths[slot]).unwrap();
                    assert_eq!(metadata.is_dir(), !fifo);
                }
            }
        }

        #[test]
        fn permissive_segment_is_refused_without_chmod_or_truncation() {
            for slot in 0..2 {
                for mode in [0o640, 0o606, 0o666, 0o4600] {
                    let (root, paths, sentinel) = fixture();
                    fs::set_permissions(&paths[slot], fs::Permissions::from_mode(mode)).unwrap();
                    assert!(Storage::open(root.path()).is_err());
                    assert_eq!(fs::metadata(&paths[slot]).unwrap().mode() & 0o7777, mode);
                    for path in &paths {
                        assert_eq!(fs::read(path).unwrap(), sentinel);
                    }
                }
            }
        }

        #[test]
        fn unsafe_directories_are_refused_without_chmod_or_truncation() {
            for recorder_child in [false, true] {
                for mode in [0o755, 0o770, 0o777] {
                    if !recorder_child && mode == 0o755 {
                        continue; // A read/search-only public data parent is supported below.
                    }
                    let (root, paths, sentinel) = fixture();
                    let directory = if recorder_child {
                        root.path().join(DIRECTORY)
                    } else {
                        root.path().to_path_buf()
                    };
                    fs::set_permissions(&directory, fs::Permissions::from_mode(mode)).unwrap();
                    assert!(Storage::open(root.path()).is_err());
                    assert_eq!(fs::metadata(&directory).unwrap().mode() & 0o7777, mode);
                    for path in &paths {
                        assert_eq!(fs::read(path).unwrap(), sentinel);
                    }
                }
            }
        }

        #[test]
        fn symlink_directories_are_refused_without_touching_targets() {
            let (root, paths, sentinel) = fixture();
            let links = tempfile::tempdir().unwrap();
            let parent_link = links.path().join("data");
            symlink(root.path(), &parent_link).unwrap();
            assert!(Storage::open(&parent_link).is_err());
            let child_parent = tempfile::tempdir().unwrap();
            symlink(
                root.path().join(DIRECTORY),
                child_parent.path().join(DIRECTORY),
            )
            .unwrap();
            assert!(Storage::open(child_parent.path()).is_err());
            for path in &paths {
                assert_eq!(fs::read(path).unwrap(), sentinel);
            }
        }

        #[test]
        fn non_directory_and_missing_parent_are_refused() {
            let root = tempfile::tempdir().unwrap();
            let parent_file = root.path().join("file");
            private_file(&parent_file, b"unchanged");
            assert!(Storage::open(&parent_file).is_err());
            assert_eq!(fs::read(&parent_file).unwrap(), b"unchanged");
            let missing = root.path().join("missing");
            assert!(Storage::open(&missing).is_err());
            assert!(!missing.exists());
            let child = root.path().join(DIRECTORY);
            private_file(&child, b"unchanged child");
            assert!(Storage::open(root.path()).is_err());
            assert_eq!(fs::read(child).unwrap(), b"unchanged child");
        }

        #[test]
        fn public_read_only_data_parent_gets_private_child_without_chmod() {
            let root = tempfile::tempdir().unwrap();
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
            let parent_file = root.path().join(NAMES[0]);
            private_file(&parent_file, b"not a recorder file");
            drop(Storage::open(root.path()).unwrap());
            assert_eq!(fs::metadata(root.path()).unwrap().mode() & 0o7777, 0o755);
            assert_eq!(
                fs::metadata(root.path().join(DIRECTORY)).unwrap().mode() & 0o7777,
                0o700
            );
            assert_eq!(fs::read(parent_file).unwrap(), b"not a recorder file");
            for name in NAMES {
                assert_eq!(
                    fs::metadata(root.path().join(DIRECTORY).join(name))
                        .unwrap()
                        .mode()
                        & 0o7777,
                    0o600
                );
            }
        }

        #[test]
        fn foreign_owner_is_refused_with_chown_fixture_when_privileged() {
            // SAFETY: geteuid has no arguments or memory safety preconditions.
            let uid = unsafe { libc::geteuid() };
            assert!(validate_owner(uid).is_ok());
            assert!(validate_owner(uid.wrapping_add(1)).is_err());
            if uid != 0 {
                // Foreign-owned temporary fixtures require privilege; the UID
                // predicate above is still exercised on unprivileged test hosts.
                return;
            }
            for entry in 0..4 {
                let (root, paths, sentinel) = fixture();
                let child = root.path().join(DIRECTORY);
                let path: &Path = match entry {
                    0 | 1 => &paths[entry],
                    2 => &child,
                    _ => root.path(),
                };
                std::os::unix::fs::chown(path, Some(1), None).unwrap();
                assert!(Storage::open(root.path()).is_err());
                assert_eq!(fs::metadata(path).unwrap().uid(), 1);
                for segment in &paths {
                    assert_eq!(fs::read(segment).unwrap(), sentinel);
                }
            }
        }

        #[test]
        fn oversized_segments_are_refused_without_reading_or_truncating() {
            for slot in 0..2 {
                for length in [SEGMENT_BYTES + 1, 1 << 40] {
                    let (root, paths, sentinel) = fixture();
                    let mut file = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&paths[slot])
                        .unwrap();
                    file.set_len(length).unwrap(); // Sparse fixture, not disk exhaustion.
                    file.seek(SeekFrom::Start(length - 4)).unwrap();
                    file.write_all(b"tail").unwrap();
                    let inode = file.metadata().unwrap().ino();
                    assert!(Storage::open(root.path()).is_err());
                    assert_eq!(file.metadata().unwrap().len(), length);
                    assert_eq!(file.metadata().unwrap().ino(), inode);
                    file.seek(SeekFrom::Start(0)).unwrap();
                    let mut head = vec![0; sentinel.len()];
                    file.read_exact(&mut head).unwrap();
                    assert_eq!(head, sentinel);
                    file.seek(SeekFrom::Start(length - 4)).unwrap();
                    let mut tail = [0; 4];
                    file.read_exact(&mut tail).unwrap();
                    assert_eq!(&tail, b"tail");
                    assert_eq!(fs::read(&paths[1 - slot]).unwrap(), sentinel);
                }
            }
        }

        #[test]
        fn rotation_reopen_after_truncate_partial_and_complete_first_frame() {
            use std::fs::FileTimes;
            use std::time::{Duration, SystemTime};

            for cut in 0..3 {
                let (root, paths, _) = fixture();
                fs::write(&paths[0], frame(&record(100)).unwrap()).unwrap();
                fs::write(&paths[1], frame(&record(1)).unwrap()).unwrap();
                let (mut storage, mut sequence) = Storage::open(root.path()).unwrap();
                let next_frame = loop {
                    let bytes = frame(&record(sequence + 1)).unwrap();
                    if storage.length + bytes.len() as u64 > SEGMENT_BYTES {
                        break bytes;
                    }
                    storage.append(&bytes).unwrap();
                    sequence += 1;
                };
                storage.sync().unwrap();
                assert_eq!(storage.active, 0);
                let intact = fs::read(&paths[0]).unwrap();
                // Exact rotation crash cuts, before any reopen; no fault framework.
                storage.files[1].set_len(0).unwrap();
                storage.files[1].seek(SeekFrom::Start(0)).unwrap();
                let written = match cut {
                    0 => 0,
                    1 => next_frame.len() / 2,
                    _ => next_frame.len(),
                };
                storage.files[1].write_all(&next_frame[..written]).unwrap();
                // Make mtime point to the WRONG active segment at each cut.
                let older = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
                let newer = older + Duration::from_secs(10);
                for (index, file) in storage.files.iter().enumerate() {
                    let is_newer = index == usize::from(cut != 2);
                    file.set_times(FileTimes::new().set_modified(if is_newer {
                        newer
                    } else {
                        older
                    }))
                    .unwrap();
                }
                drop(storage);
                let (mut reopened, recovered) = Storage::open(root.path()).unwrap();
                let complete = cut == 2;
                assert_eq!(recovered, sequence + u64::from(complete));
                assert_eq!(reopened.active, usize::from(complete));
                assert_eq!(fs::read(&paths[0]).unwrap(), intact);
                assert_eq!(
                    fs::read(&paths[1]).unwrap(),
                    if complete { next_frame } else { Vec::new() }
                );
                reopened
                    .append(&frame(&record(recovered + 1)).unwrap())
                    .unwrap();
                reopened.sync().unwrap();
                assert_eq!(fs::read(&paths[0]).unwrap(), intact);
                drop(reopened);
                assert_eq!(Storage::open(root.path()).unwrap().1, recovered + 1);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn reopen_rotation_and_byte_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let (mut storage, sequence) = Storage::open(directory.path()).unwrap();
        assert_eq!(sequence, 0);
        assert!(Storage::open(directory.path()).is_err());
        let mut sequence = 0;
        // Cross two rotations using small bounded real files, not disk exhaustion.
        while sequence < 20_000 {
            sequence += 1;
            let bytes = frame(&record(sequence)).unwrap();
            let old_active = storage.active;
            let old_length = storage.length;
            storage.append(&bytes).unwrap();
            if storage.active != old_active {
                assert_eq!(
                    storage.files[old_active].metadata().unwrap().len(),
                    old_length
                );
            }
            assert!(storage
                .files
                .iter()
                .all(|f| f.metadata().unwrap().len() <= SEGMENT_BYTES));
        }
        storage.sync().unwrap();
        let active = storage.active;
        let length = storage.length;
        storage.files[active].write_all(&[1, 2, 3]).unwrap();
        drop(storage);
        let (mut reopened, recovered) = Storage::open(directory.path()).unwrap();
        assert_eq!(recovered, sequence);
        assert_eq!(reopened.length, length);
        reopened
            .append(&frame(&record(sequence + 1)).unwrap())
            .unwrap();
        reopened.sync().unwrap();
        drop(reopened);
        assert_eq!(Storage::open(directory.path()).unwrap().1, sequence + 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in NAMES {
                assert_eq!(
                    std::fs::metadata(directory.path().join(DIRECTORY).join(name))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
    }
}
