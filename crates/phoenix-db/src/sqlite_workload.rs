use std::sync::{Arc, Mutex};
use std::time::Duration;

const BUCKET_SECONDS: u64 = 60;
pub(crate) const BUCKET_COUNT: usize = 1_441;
const MICROS_PER_SECOND: u64 = 1_000_000;
const MICROS_PER_MINUTE: u64 = BUCKET_SECONDS * MICROS_PER_SECOND;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum SqliteWorkloadCategory {
    MessagePersistence,
    DurableWorkflows,
    Fts,
    RuntimeState,
    PrProjectData,
    Maintenance,
    Other,
}

impl SqliteWorkloadCategory {
    pub(crate) const ALL: [Self; 7] = [
        Self::MessagePersistence,
        Self::DurableWorkflows,
        Self::Fts,
        Self::RuntimeState,
        Self::PrProjectData,
        Self::Maintenance,
        Self::Other,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum SqliteAccessKind {
    Read,
    Write,
}

impl SqliteAccessKind {
    pub(crate) const ALL: [Self; 2] = [Self::Read, Self::Write];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum SqliteOutcome {
    Success,
    Busy,
    Locked,
    PoolTimeout,
    OtherTimeout,
    OtherFailure,
    Abandoned,
}

impl SqliteOutcome {
    pub(crate) const ALL: [Self; 7] = [
        Self::Success,
        Self::Busy,
        Self::Locked,
        Self::PoolTimeout,
        Self::OtherTimeout,
        Self::OtherFailure,
        Self::Abandoned,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum SqliteSnapshotWindow {
    OneHour,
    SixHours,
    TwentyFourHours,
}

impl SqliteSnapshotWindow {
    pub(crate) const ALL: [Self; 3] = [Self::OneHour, Self::SixHours, Self::TwentyFourHours];

    pub(crate) const fn minutes(self) -> usize {
        match self {
            Self::OneHour => 60,
            Self::SixHours => 360,
            Self::TwentyFourHours => 1_440,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum SqliteLatencyBin {
    Under1Ms,
    Ms1To4,
    Ms5To19,
    Ms20To99,
    Ms100To249,
    Ms250To999,
    Ms1000Plus,
}

impl SqliteLatencyBin {
    pub(crate) const ALL: [Self; 7] = [
        Self::Under1Ms,
        Self::Ms1To4,
        Self::Ms5To19,
        Self::Ms20To99,
        Self::Ms100To249,
        Self::Ms250To999,
        Self::Ms1000Plus,
    ];

    const fn index(self) -> usize {
        self as usize
    }

    pub(crate) const fn from_duration(duration: Duration) -> Self {
        let millis = duration.as_millis();
        if millis < 1 {
            Self::Under1Ms
        } else if millis < 5 {
            Self::Ms1To4
        } else if millis < 20 {
            Self::Ms5To19
        } else if millis < 100 {
            Self::Ms20To99
        } else if millis < 250 {
            Self::Ms100To249
        } else if millis < 1_000 {
            Self::Ms250To999
        } else {
            Self::Ms1000Plus
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SqliteObservation {
    pub(crate) completed_at_unix_micros: u64,
    pub(crate) category: SqliteWorkloadCategory,
    pub(crate) access: SqliteAccessKind,
    pub(crate) outcome: SqliteOutcome,
    pub(crate) latency: Duration,
    pub(crate) writer_held: Duration,
    pub(crate) read_connection_time: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct BucketCategoryTotals {
    pub(crate) operation_count: u64,
    pub(crate) writer_held_micros: u64,
    pub(crate) read_connection_micros: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SqliteBucketSnapshot {
    pub(crate) minute_start_unix_micros: u64,
    pub(crate) totals: Box<
        [[BucketCategoryTotals; SqliteWorkloadCategory::ALL.len()]; SqliteAccessKind::ALL.len()],
    >,
    pub(crate) outcomes: Box<
        [[[u64; SqliteOutcome::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    pub(crate) latency_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SqliteWorkloadSnapshot {
    pub(crate) window: SqliteSnapshotWindow,
    pub(crate) covered_minutes: usize,
    pub(crate) buckets: Vec<SqliteBucketSnapshot>,
}

#[derive(Debug)]
struct SqliteBucket {
    minute_start_unix_micros: u64,
    totals: Box<
        [[BucketCategoryTotals; SqliteWorkloadCategory::ALL.len()]; SqliteAccessKind::ALL.len()],
    >,
    outcomes: Box<
        [[[u64; SqliteOutcome::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    latency_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
}

impl Default for SqliteBucket {
    fn default() -> Self {
        Self {
            minute_start_unix_micros: 0,
            totals: Box::new(
                [[BucketCategoryTotals::default(); SqliteWorkloadCategory::ALL.len()];
                    SqliteAccessKind::ALL.len()],
            ),
            outcomes: Box::new(
                [[[0; SqliteOutcome::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
                    SqliteAccessKind::ALL.len()],
            ),
            latency_histogram: Box::new(
                [[[0; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
                    SqliteAccessKind::ALL.len()],
            ),
        }
    }
}

impl SqliteBucket {
    fn reset_to(&mut self, minute_start_unix_micros: u64) {
        *self = Self {
            minute_start_unix_micros,
            ..Self::default()
        };
    }

    fn snapshot(&self) -> SqliteBucketSnapshot {
        SqliteBucketSnapshot {
            minute_start_unix_micros: self.minute_start_unix_micros,
            totals: self.totals.clone(),
            outcomes: self.outcomes.clone(),
            latency_histogram: self.latency_histogram.clone(),
        }
    }
}

#[derive(Debug)]
struct InnerCollector {
    buckets: Box<[SqliteBucket; BUCKET_COUNT]>,
}

impl Default for InnerCollector {
    fn default() -> Self {
        Self {
            buckets: Box::new(std::array::from_fn(|_| SqliteBucket::default())),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SqliteWorkloadCollector {
    inner: Arc<Mutex<InnerCollector>>,
}

impl SqliteWorkloadCollector {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn shared_id(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }

    pub(crate) fn record(&self, observation: SqliteObservation) {
        let mut inner = self
            .inner
            .lock()
            .expect("sqlite workload collector mutex poisoned");
        inner.record(observation);
    }

    pub(crate) fn snapshot(
        &self,
        window: SqliteSnapshotWindow,
        now_unix_micros: u64,
    ) -> SqliteWorkloadSnapshot {
        let inner = self
            .inner
            .lock()
            .expect("sqlite workload collector mutex poisoned");
        inner.snapshot(window, now_unix_micros)
    }
}

impl InnerCollector {
    fn record(&mut self, observation: SqliteObservation) {
        let completed_minute_start = minute_start(observation.completed_at_unix_micros);
        let bucket = self.bucket_mut(completed_minute_start);
        let access_index = observation.access.index();
        let category_index = observation.category.index();
        bucket.totals[access_index][category_index].operation_count += 1;
        bucket.outcomes[access_index][category_index][observation.outcome.index()] += 1;
        bucket.latency_histogram[access_index][category_index]
            [SqliteLatencyBin::from_duration(observation.latency).index()] += 1;

        self.distribute_duration(
            completed_minute_start,
            observation.writer_held,
            observation.access,
            observation.category,
            true,
        );
        self.distribute_duration(
            completed_minute_start,
            observation.read_connection_time,
            observation.access,
            observation.category,
            false,
        );
    }

    fn distribute_duration(
        &mut self,
        completed_minute_start: u64,
        duration: Duration,
        access: SqliteAccessKind,
        category: SqliteWorkloadCategory,
        writer: bool,
    ) {
        let mut remaining = duration_to_micros(duration);
        if remaining == 0 {
            return;
        }
        let access_index = access.index();
        let category_index = category.index();
        let completion_offset = MICROS_PER_MINUTE - 1;
        let mut segment_end = completed_minute_start + completion_offset;
        while remaining > 0 {
            let segment_start = minute_start(segment_end);
            let used_in_bucket = ((segment_end - segment_start) + 1).min(remaining);
            let bucket = self.bucket_mut(segment_start);
            let totals = &mut bucket.totals[access_index][category_index];
            if writer {
                totals.writer_held_micros += used_in_bucket;
            } else {
                totals.read_connection_micros += used_in_bucket;
            }
            remaining -= used_in_bucket;
            if remaining == 0 || segment_start == 0 {
                break;
            }
            segment_end = segment_start - 1;
        }
    }

    fn snapshot(
        &self,
        window: SqliteSnapshotWindow,
        now_unix_micros: u64,
    ) -> SqliteWorkloadSnapshot {
        let current_minute_start = minute_start(now_unix_micros);
        let available_minutes =
            ((current_minute_start / MICROS_PER_MINUTE) + 1).min(BUCKET_COUNT as u64) as usize;
        let covered_minutes = available_minutes.min(window.minutes() + 1);
        let start_minute = current_minute_start
            .saturating_sub((covered_minutes.saturating_sub(1) as u64) * MICROS_PER_MINUTE);
        let mut buckets = Vec::with_capacity(covered_minutes);
        for offset in 0..covered_minutes {
            let minute_start_unix_micros = start_minute + (offset as u64) * MICROS_PER_MINUTE;
            let bucket = &self.buckets[slot_for(minute_start_unix_micros)];
            if bucket.minute_start_unix_micros == minute_start_unix_micros {
                buckets.push(bucket.snapshot());
            } else {
                buckets.push(
                    SqliteBucket {
                        minute_start_unix_micros,
                        ..SqliteBucket::default()
                    }
                    .snapshot(),
                );
            }
        }
        SqliteWorkloadSnapshot {
            window,
            covered_minutes,
            buckets,
        }
    }

    fn bucket_mut(&mut self, minute_start_unix_micros: u64) -> &mut SqliteBucket {
        let slot = slot_for(minute_start_unix_micros);
        let bucket = &mut self.buckets[slot];
        if bucket.minute_start_unix_micros != minute_start_unix_micros {
            bucket.reset_to(minute_start_unix_micros);
        }
        bucket
    }
}

fn minute_start(unix_micros: u64) -> u64 {
    unix_micros - (unix_micros % MICROS_PER_MINUTE)
}

fn slot_for(minute_start_unix_micros: u64) -> usize {
    ((minute_start_unix_micros / MICROS_PER_MINUTE) % (BUCKET_COUNT as u64)) as usize
}

fn duration_to_micros(duration: Duration) -> u64 {
    duration.as_micros().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    const M: u64 = MICROS_PER_MINUTE;

    #[test]
    fn collector_has_fixed_bucket_count_and_closed_vocabularies() {
        assert_eq!(BUCKET_COUNT, 1_441);
        assert_eq!(SqliteWorkloadCategory::ALL.len(), 7);
        assert_eq!(SqliteAccessKind::ALL.len(), 2);
        assert_eq!(SqliteOutcome::ALL.len(), 7);
        assert_eq!(SqliteLatencyBin::ALL.len(), 7);
        assert_eq!(SqliteSnapshotWindow::ALL.len(), 3);
        assert_eq!(SqliteSnapshotWindow::OneHour.minutes(), 60);
        assert_eq!(SqliteSnapshotWindow::SixHours.minutes(), 360);
        assert_eq!(SqliteSnapshotWindow::TwentyFourHours.minutes(), 1_440);
    }

    #[test]
    fn latency_bins_are_fixed_and_bounded() {
        assert_eq!(
            SqliteLatencyBin::from_duration(Duration::from_micros(999)),
            SqliteLatencyBin::Under1Ms
        );
        assert_eq!(
            SqliteLatencyBin::from_duration(Duration::from_millis(1)),
            SqliteLatencyBin::Ms1To4
        );
        assert_eq!(
            SqliteLatencyBin::from_duration(Duration::from_millis(5)),
            SqliteLatencyBin::Ms5To19
        );
        assert_eq!(
            SqliteLatencyBin::from_duration(Duration::from_millis(20)),
            SqliteLatencyBin::Ms20To99
        );
        assert_eq!(
            SqliteLatencyBin::from_duration(Duration::from_millis(100)),
            SqliteLatencyBin::Ms100To249
        );
        assert_eq!(
            SqliteLatencyBin::from_duration(Duration::from_millis(250)),
            SqliteLatencyBin::Ms250To999
        );
        assert_eq!(
            SqliteLatencyBin::from_duration(Duration::from_millis(1_000)),
            SqliteLatencyBin::Ms1000Plus
        );
    }

    #[test]
    fn writer_duration_splits_across_minute_boundary() {
        let collector = SqliteWorkloadCollector::new();
        collector.record(SqliteObservation {
            completed_at_unix_micros: (2 * M) + 10_000_000,
            category: SqliteWorkloadCategory::Fts,
            access: SqliteAccessKind::Write,
            outcome: SqliteOutcome::Success,
            latency: Duration::from_millis(30),
            writer_held: Duration::from_secs(70),
            read_connection_time: Duration::ZERO,
        });

        let snapshot = collector.snapshot(SqliteSnapshotWindow::OneHour, 2 * M);
        let fts = SqliteWorkloadCategory::Fts.index();
        let write = SqliteAccessKind::Write.index();
        assert_eq!(
            snapshot.buckets.last().unwrap().totals[write][fts].writer_held_micros,
            M
        );
        assert_eq!(
            snapshot.buckets[snapshot.buckets.len() - 2].totals[write][fts].writer_held_micros,
            10_000_000
        );
    }

    #[test]
    fn read_duration_splits_without_affecting_writer_occupancy() {
        let collector = SqliteWorkloadCollector::new();
        collector.record(SqliteObservation {
            completed_at_unix_micros: (3 * M) + 5_000_000,
            category: SqliteWorkloadCategory::RuntimeState,
            access: SqliteAccessKind::Read,
            outcome: SqliteOutcome::Success,
            latency: Duration::from_millis(12),
            writer_held: Duration::ZERO,
            read_connection_time: Duration::from_secs(65),
        });

        let snapshot = collector.snapshot(SqliteSnapshotWindow::OneHour, 3 * M);
        let cat = SqliteWorkloadCategory::RuntimeState.index();
        let read = SqliteAccessKind::Read.index();
        assert_eq!(
            snapshot.buckets.last().unwrap().totals[read][cat].read_connection_micros,
            M
        );
        assert_eq!(
            snapshot.buckets[snapshot.buckets.len() - 2].totals[read][cat].read_connection_micros,
            5_000_000
        );
        assert_eq!(
            snapshot.buckets.last().unwrap().totals[read][cat].writer_held_micros,
            0
        );
    }

    #[test]
    fn snapshot_windows_are_fixed_and_cover_available_history() {
        let collector = SqliteWorkloadCollector::new();
        collector.record(SqliteObservation {
            completed_at_unix_micros: (1_000 * M) + 1,
            category: SqliteWorkloadCategory::Maintenance,
            access: SqliteAccessKind::Write,
            outcome: SqliteOutcome::Busy,
            latency: Duration::from_secs(2),
            writer_held: Duration::from_secs(1),
            read_connection_time: Duration::ZERO,
        });

        let one = collector.snapshot(SqliteSnapshotWindow::OneHour, 1_000 * M);
        let six = collector.snapshot(SqliteSnapshotWindow::SixHours, 1_000 * M);
        let day = collector.snapshot(SqliteSnapshotWindow::TwentyFourHours, 1_000 * M);
        assert_eq!(one.covered_minutes, 61);
        assert_eq!(six.covered_minutes, 361);
        assert_eq!(day.covered_minutes, 1_001);
        assert_eq!(one.buckets.len(), 61);
        assert_eq!(six.buckets.len(), 361);
        assert_eq!(day.buckets.len(), 1_001);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn database_clones_share_one_collector_instance() {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        let db = Database::from_pool_for_tests(pool, String::new());
        let cloned = db.clone();
        assert_eq!(
            db.sqlite_workload_collector().shared_id(),
            cloned.sqlite_workload_collector().shared_id()
        );
    }

    #[test]
    fn completion_bucket_receives_count_outcome_and_histogram() {
        let collector = SqliteWorkloadCollector::new();
        let completed_at = (4 * M) + 42;
        collector.record(SqliteObservation {
            completed_at_unix_micros: completed_at,
            category: SqliteWorkloadCategory::MessagePersistence,
            access: SqliteAccessKind::Write,
            outcome: SqliteOutcome::Locked,
            latency: Duration::from_millis(130),
            writer_held: Duration::from_secs(1),
            read_connection_time: Duration::ZERO,
        });
        let snapshot = collector.snapshot(SqliteSnapshotWindow::OneHour, 4 * M);
        let bucket = snapshot.buckets.last().unwrap();
        let access = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::MessagePersistence.index();
        assert_eq!(bucket.totals[access][category].operation_count, 1);
        assert_eq!(
            bucket.outcomes[access][category][SqliteOutcome::Locked.index()],
            1
        );
        assert_eq!(
            bucket.latency_histogram[access][category][SqliteLatencyBin::Ms100To249.index()],
            1
        );
    }
}
