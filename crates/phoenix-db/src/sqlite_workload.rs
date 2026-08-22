use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BUCKET_SECONDS: u64 = 60;
pub(crate) const BUCKET_COUNT: usize = 1_441;
const MICROS_PER_SECOND: u64 = 1_000_000;
const MICROS_PER_MINUTE: u64 = BUCKET_SECONDS * MICROS_PER_SECOND;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SqliteWorkloadCategory {
    MessagePersistence,
    DurableWorkflows,
    Fts,
    RuntimeState,
    PrProjectData,
    Maintenance,
    Other,
}

impl SqliteWorkloadCategory {
    pub const ALL: [Self; 7] = [
        Self::MessagePersistence,
        Self::DurableWorkflows,
        Self::Fts,
        Self::RuntimeState,
        Self::PrProjectData,
        Self::Maintenance,
        Self::Other,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SqliteAccessKind {
    Read,
    Write,
}

impl SqliteAccessKind {
    pub const ALL: [Self; 2] = [Self::Read, Self::Write];

    pub const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SqliteOutcome {
    Success,
    Busy,
    Locked,
    PoolTimeout,
    OtherTimeout,
    OtherFailure,
    Abandoned,
}

impl SqliteOutcome {
    pub const ALL: [Self; 7] = [
        Self::Success,
        Self::Busy,
        Self::Locked,
        Self::PoolTimeout,
        Self::OtherTimeout,
        Self::OtherFailure,
        Self::Abandoned,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SqliteSnapshotWindow {
    OneHour,
    SixHours,
    TwentyFourHours,
}

impl SqliteSnapshotWindow {
    pub const ALL: [Self; 3] = [Self::OneHour, Self::SixHours, Self::TwentyFourHours];

    pub const fn minutes(self) -> usize {
        match self {
            Self::OneHour => 60,
            Self::SixHours => 360,
            Self::TwentyFourHours => 1_440,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SqliteLatencyBin {
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

    pub(crate) const fn index(self) -> usize {
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
    pub(crate) pool_wait: Duration,
    pub(crate) writer_held: Duration,
    pub(crate) read_connection_time: Duration,
    pub(crate) retry_count: u64,
    pub(crate) retry_backoff: Duration,
    pub(crate) writer_concurrency: u32,
    pub(crate) read_concurrency: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BucketCategoryTotals {
    pub operation_count: u64,
    pub pool_wait_micros: u64,
    pub writer_held_micros: u64,
    pub read_connection_micros: u64,
    pub retry_count: u64,
    pub retry_backoff_micros: u64,
    pub abandoned_count: u64,
    pub writer_concurrency_peak: u32,
    pub read_concurrency_peak: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteBucketSnapshot {
    pub minute_start_unix_micros: u64,
    pub totals: Box<
        [[BucketCategoryTotals; SqliteWorkloadCategory::ALL.len()]; SqliteAccessKind::ALL.len()],
    >,
    pub outcomes: Box<
        [[[u64; SqliteOutcome::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    pub latency_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteWorkloadSnapshot {
    pub window: SqliteSnapshotWindow,
    pub covered_minutes: usize,
    pub process_started_at_unix_micros: u64,
    pub process_uptime_micros: u64,
    pub covered_uptime_micros: u64,
    pub buckets: Vec<SqliteBucketSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteWorkloadAggregateReport {
    pub requested_window: SqliteSnapshotWindow,
    pub covered_minutes: usize,
    pub process_started_at_unix_micros: u64,
    pub process_uptime_micros: u64,
    pub covered_uptime_micros: u64,
    pub totals: Box<
        [[BucketCategoryTotals; SqliteWorkloadCategory::ALL.len()]; SqliteAccessKind::ALL.len()],
    >,
    pub outcomes: Box<
        [[[u64; SqliteOutcome::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    pub latency_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
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
    process_started_at_unix_micros: u64,
    process_started_at: Instant,
    buckets: Box<[SqliteBucket; BUCKET_COUNT]>,
}

impl Default for InnerCollector {
    fn default() -> Self {
        Self {
            process_started_at_unix_micros: unix_now_micros(),
            process_started_at: Instant::now(),
            buckets: Box::new(std::array::from_fn(|_| SqliteBucket::default())),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SqliteWorkloadCollector {
    inner: Arc<Mutex<InnerCollector>>,
}

impl SqliteWorkloadCollector {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
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

    pub fn snapshot(
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

    pub fn aggregate_report(
        &self,
        window: SqliteSnapshotWindow,
        now_unix_micros: u64,
    ) -> SqliteWorkloadAggregateReport {
        let snapshot = self.snapshot(window, now_unix_micros);
        let mut totals = Box::new(
            [[BucketCategoryTotals::default(); SqliteWorkloadCategory::ALL.len()];
                SqliteAccessKind::ALL.len()],
        );
        let mut outcomes = Box::new(
            [[[0; SqliteOutcome::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
                SqliteAccessKind::ALL.len()],
        );
        let mut latency_histogram = Box::new(
            [[[0; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
                SqliteAccessKind::ALL.len()],
        );
        for bucket in &snapshot.buckets {
            for access in SqliteAccessKind::ALL {
                for category in SqliteWorkloadCategory::ALL {
                    let a = access.index();
                    let c = category.index();
                    let dst = &mut totals[a][c];
                    let src = bucket.totals[a][c];
                    dst.operation_count += src.operation_count;
                    dst.pool_wait_micros += src.pool_wait_micros;
                    dst.writer_held_micros += src.writer_held_micros;
                    dst.read_connection_micros += src.read_connection_micros;
                    dst.retry_count += src.retry_count;
                    dst.retry_backoff_micros += src.retry_backoff_micros;
                    dst.abandoned_count += src.abandoned_count;
                    dst.writer_concurrency_peak =
                        dst.writer_concurrency_peak.max(src.writer_concurrency_peak);
                    dst.read_concurrency_peak =
                        dst.read_concurrency_peak.max(src.read_concurrency_peak);
                    for outcome in SqliteOutcome::ALL {
                        outcomes[a][c][outcome.index()] += bucket.outcomes[a][c][outcome.index()];
                    }
                    for bin in SqliteLatencyBin::ALL {
                        latency_histogram[a][c][bin.index()] +=
                            bucket.latency_histogram[a][c][bin.index()];
                    }
                }
            }
        }
        SqliteWorkloadAggregateReport {
            requested_window: snapshot.window,
            covered_minutes: snapshot.covered_minutes,
            process_started_at_unix_micros: snapshot.process_started_at_unix_micros,
            process_uptime_micros: snapshot.process_uptime_micros,
            covered_uptime_micros: snapshot.covered_uptime_micros,
            totals,
            outcomes,
            latency_histogram,
        }
    }
}

impl InnerCollector {
    fn record(&mut self, observation: SqliteObservation) {
        let completed_minute_start = minute_start(observation.completed_at_unix_micros);
        let bucket = self.bucket_mut(completed_minute_start);
        let access_index = observation.access.index();
        let category_index = observation.category.index();
        let totals = &mut bucket.totals[access_index][category_index];
        totals.operation_count += 1;
        totals.pool_wait_micros += duration_to_micros(observation.pool_wait);
        totals.retry_count += observation.retry_count;
        totals.retry_backoff_micros += duration_to_micros(observation.retry_backoff);
        totals.writer_concurrency_peak = totals
            .writer_concurrency_peak
            .max(observation.writer_concurrency);
        totals.read_concurrency_peak = totals
            .read_concurrency_peak
            .max(observation.read_concurrency);
        if observation.outcome == SqliteOutcome::Abandoned {
            totals.abandoned_count += 1;
        }
        bucket.outcomes[access_index][category_index][observation.outcome.index()] += 1;
        bucket.latency_histogram[access_index][category_index]
            [SqliteLatencyBin::from_duration(observation.latency).index()] += 1;

        self.distribute_duration(
            observation.completed_at_unix_micros,
            observation.writer_held,
            observation.access,
            observation.category,
            true,
        );
        self.distribute_duration(
            observation.completed_at_unix_micros,
            observation.read_connection_time,
            observation.access,
            observation.category,
            false,
        );
    }

    fn distribute_duration(
        &mut self,
        completed_at_unix_micros: u64,
        duration: Duration,
        access: SqliteAccessKind,
        category: SqliteWorkloadCategory,
        writer: bool,
    ) {
        let total = duration_to_micros(duration);
        if total == 0 {
            return;
        }
        let access_index = access.index();
        let category_index = category.index();
        let mut remaining = total;
        let mut segment_end_exclusive = completed_at_unix_micros;
        while remaining > 0 {
            let segment_start = segment_end_exclusive.saturating_sub(remaining);
            let bucket_start = minute_start(segment_end_exclusive.saturating_sub(1));
            if segment_end_exclusive <= bucket_start {
                segment_end_exclusive = bucket_start;
                continue;
            }
            let used_in_bucket =
                segment_end_exclusive.saturating_sub(segment_start.max(bucket_start));
            let bucket = self.bucket_mut(bucket_start);
            let totals = &mut bucket.totals[access_index][category_index];
            if writer {
                totals.writer_held_micros += used_in_bucket;
            } else {
                totals.read_connection_micros += used_in_bucket;
            }
            remaining -= used_in_bucket;
            if remaining == 0 || bucket_start == 0 {
                break;
            }
            segment_end_exclusive = bucket_start;
        }
    }

    fn snapshot(
        &self,
        window: SqliteSnapshotWindow,
        now_unix_micros: u64,
    ) -> SqliteWorkloadSnapshot {
        let process_uptime_micros = duration_to_micros(self.process_started_at.elapsed());
        let process_end_unix_micros = self
            .process_started_at_unix_micros
            .saturating_add(process_uptime_micros);
        let effective_now_unix_micros = now_unix_micros.min(process_end_unix_micros);
        let requested_covered_uptime_micros =
            (window.minutes() as u64).saturating_mul(MICROS_PER_MINUTE);
        let total_uptime_micros =
            effective_now_unix_micros.saturating_sub(self.process_started_at_unix_micros);
        let covered_uptime_micros = total_uptime_micros.min(requested_covered_uptime_micros);
        let covered_minutes = usize::try_from(covered_uptime_micros / MICROS_PER_MINUTE)
            .unwrap_or(usize::MAX)
            .saturating_add(1)
            .min(BUCKET_COUNT);
        let current_minute_start = minute_start(effective_now_unix_micros);
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
            process_started_at_unix_micros: self.process_started_at_unix_micros,
            process_uptime_micros,
            covered_uptime_micros,
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

fn unix_now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(duration_to_micros)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    const M: u64 = MICROS_PER_MINUTE;

    fn collector_started_at(
        started_at_unix_micros: u64,
        uptime: Duration,
    ) -> SqliteWorkloadCollector {
        let collector = SqliteWorkloadCollector::new();
        {
            let mut inner = collector
                .inner
                .lock()
                .expect("sqlite workload collector mutex poisoned");
            inner.process_started_at_unix_micros = started_at_unix_micros;
            inner.process_started_at = Instant::now() - uptime;
        }
        collector
    }

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
    fn writer_duration_splits_from_exact_completion_timestamp() {
        let collector = collector_started_at(0, Duration::from_secs(3 * 60));
        collector.record(SqliteObservation {
            completed_at_unix_micros: (2 * M) + 10_000_000,
            category: SqliteWorkloadCategory::Fts,
            access: SqliteAccessKind::Write,
            outcome: SqliteOutcome::Success,
            latency: Duration::from_millis(30),
            pool_wait: Duration::ZERO,
            writer_held: Duration::from_secs(70),
            read_connection_time: Duration::ZERO,
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 1,
            read_concurrency: 0,
        });

        let snapshot =
            collector.snapshot(SqliteSnapshotWindow::TwentyFourHours, (2 * M) + 10_000_000);
        let fts = SqliteWorkloadCategory::Fts.index();
        let write = SqliteAccessKind::Write.index();
        let current = snapshot
            .buckets
            .iter()
            .find(|bucket| bucket.minute_start_unix_micros == 2 * M)
            .unwrap();
        assert_eq!(current.totals[write][fts].writer_held_micros, 10_000_000);
        let all_writer_micros: Vec<(u64, u64)> = snapshot
            .buckets
            .iter()
            .map(|bucket| {
                (
                    bucket.minute_start_unix_micros,
                    bucket.totals[write][fts].writer_held_micros,
                )
            })
            .filter(|(_, micros)| *micros > 0)
            .collect();
        assert_eq!(
            all_writer_micros,
            vec![(M, 60_000_000), (2 * M, 10_000_000)]
        );
    }

    #[test]
    fn read_duration_splits_without_affecting_writer_occupancy() {
        let collector = collector_started_at(0, Duration::from_secs(4 * 60));
        collector.record(SqliteObservation {
            completed_at_unix_micros: (3 * M) + 5_000_000,
            category: SqliteWorkloadCategory::RuntimeState,
            access: SqliteAccessKind::Read,
            outcome: SqliteOutcome::Success,
            latency: Duration::from_millis(12),
            pool_wait: Duration::ZERO,
            writer_held: Duration::ZERO,
            read_connection_time: Duration::from_secs(65),
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 0,
            read_concurrency: 2,
        });

        let snapshot =
            collector.snapshot(SqliteSnapshotWindow::TwentyFourHours, (3 * M) + 5_000_000);
        let cat = SqliteWorkloadCategory::RuntimeState.index();
        let read = SqliteAccessKind::Read.index();
        let current = snapshot
            .buckets
            .iter()
            .find(|bucket| bucket.minute_start_unix_micros == 3 * M)
            .unwrap();
        assert_eq!(current.totals[read][cat].read_connection_micros, 5_000_000);
        let all_read_micros: Vec<(u64, u64)> = snapshot
            .buckets
            .iter()
            .map(|bucket| {
                (
                    bucket.minute_start_unix_micros,
                    bucket.totals[read][cat].read_connection_micros,
                )
            })
            .filter(|(_, micros)| *micros > 0)
            .collect();
        assert_eq!(
            all_read_micros,
            vec![(2 * M, 60_000_000), (3 * M, 5_000_000)]
        );
        assert_eq!(current.totals[read][cat].writer_held_micros, 0);
    }

    #[test]
    fn snapshot_clips_coverage_to_process_uptime() {
        let collector = SqliteWorkloadCollector::new();
        {
            let mut inner = collector
                .inner
                .lock()
                .expect("sqlite workload collector mutex poisoned");
            inner.process_started_at_unix_micros = 10 * M;
            inner.process_started_at = Instant::now() - Duration::from_secs(150);
        }

        let snapshot = collector.snapshot(SqliteSnapshotWindow::OneHour, 20 * M);
        assert_eq!(snapshot.covered_minutes, 3);
        assert!(snapshot.covered_uptime_micros >= 150_000_000);
        assert!(snapshot.covered_uptime_micros < 151_000_000);
        assert_eq!(
            snapshot.buckets.first().unwrap().minute_start_unix_micros,
            10 * M
        );
        assert_eq!(
            snapshot.buckets.last().unwrap().minute_start_unix_micros,
            12 * M
        );

        let report = collector.aggregate_report(SqliteSnapshotWindow::OneHour, 20 * M);
        assert!(report.covered_uptime_micros >= 150_000_000);
        assert!(report.covered_uptime_micros < 151_000_000);
    }

    #[test]
    fn snapshot_clips_coverage_to_requested_duration() {
        let collector = SqliteWorkloadCollector::new();
        {
            let mut inner = collector
                .inner
                .lock()
                .expect("sqlite workload collector mutex poisoned");
            inner.process_started_at_unix_micros = 10 * M;
            inner.process_started_at = Instant::now() - Duration::from_secs(200 * 60);
        }

        let snapshot = collector.snapshot(SqliteSnapshotWindow::OneHour, 300 * M);
        assert_eq!(snapshot.covered_minutes, 61);
        assert_eq!(snapshot.covered_uptime_micros, 60 * M);
        assert_eq!(
            snapshot.buckets.first().unwrap().minute_start_unix_micros,
            150 * M
        );
        assert_eq!(
            snapshot.buckets.last().unwrap().minute_start_unix_micros,
            210 * M
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
            pool_wait: Duration::from_millis(3),
            writer_held: Duration::from_secs(1),
            read_connection_time: Duration::ZERO,
            retry_count: 4,
            retry_backoff: Duration::from_millis(20),
            writer_concurrency: 1,
            read_concurrency: 0,
        });

        let one = collector.snapshot(SqliteSnapshotWindow::OneHour, 1_000 * M);
        let six = collector.snapshot(SqliteSnapshotWindow::SixHours, 1_000 * M);
        let day = collector.snapshot(SqliteSnapshotWindow::TwentyFourHours, 1_000 * M);
        assert_eq!(one.covered_minutes, 1);
        assert_eq!(six.covered_minutes, 1);
        assert_eq!(day.covered_minutes, 1);
        assert_eq!(one.buckets.len(), 1);
        assert_eq!(six.buckets.len(), 1);
        assert_eq!(day.buckets.len(), 1);
        assert!(day.process_uptime_micros > 0 || day.process_started_at_unix_micros > 0);
        assert!(day.covered_uptime_micros <= day.process_uptime_micros);
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
    fn completion_bucket_receives_count_outcome_histogram_and_aggregates() {
        let collector = SqliteWorkloadCollector::new();
        let completed_at = (4 * M) + 42;
        collector.record(SqliteObservation {
            completed_at_unix_micros: completed_at,
            category: SqliteWorkloadCategory::MessagePersistence,
            access: SqliteAccessKind::Write,
            outcome: SqliteOutcome::Locked,
            latency: Duration::from_millis(130),
            pool_wait: Duration::from_millis(7),
            writer_held: Duration::from_secs(1),
            read_connection_time: Duration::ZERO,
            retry_count: 2,
            retry_backoff: Duration::from_millis(15),
            writer_concurrency: 1,
            read_concurrency: 0,
        });
        let snapshot = collector.snapshot(SqliteSnapshotWindow::OneHour, 4 * M);
        let bucket = snapshot.buckets.last().unwrap();
        let access = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::MessagePersistence.index();
        assert_eq!(bucket.totals[access][category].operation_count, 1);
        assert_eq!(bucket.totals[access][category].pool_wait_micros, 7_000);
        assert_eq!(bucket.totals[access][category].retry_count, 2);
        assert_eq!(bucket.totals[access][category].retry_backoff_micros, 15_000);
        assert_eq!(
            bucket.outcomes[access][category][SqliteOutcome::Locked.index()],
            1
        );
        assert_eq!(
            bucket.latency_histogram[access][category][SqliteLatencyBin::Ms100To249.index()],
            1
        );

        let report = collector.aggregate_report(SqliteSnapshotWindow::OneHour, 4 * M);
        assert_eq!(report.totals[access][category].operation_count, 1);
        assert_eq!(
            report.outcomes[access][category][SqliteOutcome::Locked.index()],
            1
        );
    }
}
