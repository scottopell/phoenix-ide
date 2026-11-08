//! Bounded, opt-in storage leaf; no logging, SQLite, or startup integration.
//! The first segment carries an exclusive advisory lock for the writer's lifetime.
//! A complete frame is not a durability claim: only successful `sync_data`
//! advances `committed`. After a crash, even a valid unsynced tail may be lost.

use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{mpsc, Arc};

const SEGMENT_BYTES: u64 = 2 * 1024 * 1024;
const FRAME_BYTES: usize = 1024;
const HEADER_BYTES: usize = 6;
const NAMES: [&str; 2] = ["resources.0.frames", "resources.1.frames"];

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct ProcessIdentity {
    pub pid: u32,
    pub started_unix_ms: u64,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum MetricFailure {
    Unsupported,
    Permission,
    Read,
    Parse,
}

/// The observation timestamp permits consumers to compute age independently of
/// the sample timestamp. Unavailable is not zero and does not reuse an old value.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum ByteReading {
    Available {
        bytes: u64,
        observed_unix_ms: u64,
    },
    Unavailable {
        observed_unix_ms: u64,
        code: MetricFailure,
    },
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum ProportionalSource {
    LinuxSmapsRollupPss,
    MacPhysicalFootprint,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct ProportionalReading {
    pub source: ProportionalSource,
    pub reading: ByteReading,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct CheapCounts {
    pub conversations: Option<u64>,
    pub active_tools: Option<u64>,
    pub child_processes: Option<u64>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) struct Sample {
    pub sampled_unix_ms: u64,
    pub process_uptime_ms: u64,
    pub current_rss: ByteReading,
    pub proportional: Option<ProportionalReading>,
    pub high_water_rss: ByteReading,
    pub counts: Option<CheapCounts>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    version: u8,
    sequence: u64,
    process: ProcessIdentity,
    sample: Sample,
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
    sender: mpsc::SyncSender<Sample>,
    counters: Arc<Counters>,
}

impl Handle {
    /// Never waits for disk or channel capacity. One record may be pending in
    /// addition to the writer's in-flight record. Drop only disconnects, not joins.
    pub(crate) fn try_record(&self, sample: Sample) {
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
/// Only the two fixed filenames are opened; the argument is directory config,
/// not a payload-controlled filename. No environment variable enables this leaf.
pub(crate) fn start(directory: &Path, process: ProcessIdentity) -> io::Result<Handle> {
    let (storage, sequence) = Storage::open(directory)?;
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
        if record.version != 1 || record.sequence <= sequence {
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
    fn open(directory: &Path) -> io::Result<(Self, u64)> {
        std::fs::create_dir_all(directory)?;
        let open = |name| {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(directory.join(name))
        };
        let mut files = [open(NAMES[0])?, open(NAMES[1])?];
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

fn run(
    mut storage: impl Sink,
    mut sequence: u64,
    process: ProcessIdentity,
    receiver: mpsc::Receiver<Sample>,
    counters: &Counters,
) {
    for sample in receiver {
        let Some(next) = sequence.checked_add(1) else {
            counters
                .failure
                .store(FailureCode::SequenceExhausted as u64, Relaxed);
            return;
        };
        let bytes = match frame(&Record {
            version: 1,
            sequence: next,
            process,
            sample,
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
        sequence = next;
        counters.last_committed_sequence.store(sequence, Relaxed);
        counters.committed.fetch_add(1, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(sequence: u64) -> Record {
        let reading = ByteReading::Available {
            bytes: 42,
            observed_unix_ms: 10,
        };
        Record {
            version: 1,
            sequence,
            process: ProcessIdentity {
                pid: 7,
                started_unix_ms: 5,
            },
            sample: Sample {
                sampled_unix_ms: 11,
                process_uptime_ms: 6,
                current_rss: reading,
                proportional: None,
                high_water_rss: reading,
                counts: None,
            },
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
            tx.send(record(1).sample).unwrap();
            tx.send(record(2).sample).unwrap();
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
            handle.try_record(record(1).sample);
        }
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.sampled, 100);
        assert_eq!(snapshot.queued, 1);
        assert_eq!(snapshot.dropped_full, 99);
        receiver.recv().unwrap();
        drop(receiver);
        handle.try_record(record(1).sample);
        assert_eq!(handle.snapshot().dropped_disconnected, 1);
        drop(handle);
    }

    #[test]
    fn frame_and_serialization_bounds() {
        let mut r = record(u64::MAX);
        let reading = ByteReading::Available {
            bytes: u64::MAX,
            observed_unix_ms: u64::MAX,
        };
        r.sample.current_rss = reading;
        r.sample.high_water_rss = reading;
        r.sample.proportional = Some(ProportionalReading {
            source: ProportionalSource::LinuxSmapsRollupPss,
            reading,
        });
        r.sample.counts = Some(CheapCounts {
            conversations: Some(u64::MAX),
            active_tools: Some(u64::MAX),
            child_processes: Some(u64::MAX),
        });
        assert!(frame(&r).unwrap().len() <= FRAME_BYTES);
        let mut bounded = BoundedJson(Vec::new());
        assert!(bounded.write_all(&[0; FRAME_BYTES]).is_err());
        assert!(bounded.0.is_empty());
        assert_eq!(scan(&[255u8; FRAME_BYTES][..]).unwrap(), (0, 0));
    }

    #[test]
    fn sequence_exhaustion_never_wraps_or_writes() {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(record(1).sample).unwrap();
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
                    std::fs::metadata(directory.path().join(name))
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
