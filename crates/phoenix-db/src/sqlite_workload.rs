use std::collections::HashMap;
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

    #[must_use]
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

    #[must_use]
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

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

#[must_use]
pub const fn operation_count(outcomes: &[u64; SqliteOutcome::ALL.len()]) -> u64 {
    let mut total: u64 = 0;
    let mut index = 0;
    while index < SqliteOutcome::ALL.len() {
        total = total.saturating_add(outcomes[index]);
        index += 1;
    }
    total
}

#[must_use]
pub const fn abandoned_count(outcomes: &[u64; SqliteOutcome::ALL.len()]) -> u64 {
    outcomes[SqliteOutcome::Abandoned.index()]
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

    #[must_use]
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
    pub const ALL: [Self; 7] = [
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
pub(crate) struct NativeStatementObservation {
    pub(crate) category: SqliteWorkloadCategory,
    pub(crate) access: SqliteAccessKind,
    pub(crate) statement_latency: Duration,
    pub(crate) read_connection_time: Duration,
    pub(crate) read_concurrency: u32,
}

#[derive(Debug)]
pub(crate) struct NativeReadToken {
    collector: SqliteWorkloadCollector,
    identity: u64,
    concurrency: u32,
    active: bool,
}

impl NativeReadToken {
    pub(crate) fn concurrency(&self) -> u32 {
        self.concurrency
    }

    pub(crate) fn finish(mut self) {
        self.release();
    }

    fn release(&mut self) {
        if self.active {
            self.collector.end_native_read(self.identity);
            self.active = false;
        }
    }
}

impl Drop for NativeReadToken {
    fn drop(&mut self) {
        self.release();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TypedOutcomeObservation {
    pub(crate) category: SqliteWorkloadCategory,
    pub(crate) access: SqliteAccessKind,
    pub(crate) latency: Duration,
    pub(crate) retry_count: u64,
    pub(crate) retry_backoff: Duration,
    pub(crate) outcome: SqliteOutcome,
    pub(crate) waits: SqliteWaitMeasurement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InnerObservation {
    pub(crate) completed_at_unix_micros: u64,
    pub(crate) category: SqliteWorkloadCategory,
    pub(crate) access: SqliteAccessKind,
    pub(crate) latency: Duration,
    pub(crate) writer_held: Duration,
    pub(crate) read_connection_time: Duration,
    pub(crate) retry_count: u64,
    pub(crate) retry_backoff: Duration,
    pub(crate) writer_concurrency: u32,
    pub(crate) read_concurrency: u32,
    pub(crate) counting: InnerObservationCounting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InnerObservationCounting {
    BaselineStatement,
    TypedOutcome {
        outcome: SqliteOutcome,
        waits: SqliteWaitMeasurement,
    },
    OccupancyOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SqliteWaitMeasurement {
    Unavailable,
    PoolOnly {
        pool_wait: Duration,
    },
    PoolAndAdmission {
        pool_wait: Duration,
        admission_wait: Duration,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BucketCategoryTotals {
    pub baseline_statement_count: u64,
    pub native_statement_latency_micros: u64,
    pub typed_attempt_latency_micros: u64,
    pub pool_wait_micros: u64,
    pub write_admission_wait_micros: u64,
    pub writer_held_micros: u64,
    pub read_connection_micros: u64,
    pub retry_count: u64,
    pub retry_backoff_micros: u64,
    pub writer_concurrency_peak: u32,
    pub read_concurrency_peak: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteBucketSnapshot {
    pub minute_start_unix_micros: u64,
    pub classification_gap_count: u64,
    pub writer_occupancy_gap_count: u64,
    pub totals: Box<
        [[BucketCategoryTotals; SqliteWorkloadCategory::ALL.len()]; SqliteAccessKind::ALL.len()],
    >,
    pub outcomes: Box<
        [[[u64; SqliteOutcome::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    pub native_statement_latency_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    pub typed_attempt_latency_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    pub pool_wait_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    pub write_admission_wait_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteWorkloadSnapshot {
    pub window: SqliteSnapshotWindow,
    pub bucket_count: usize,
    pub restart_truncated: bool,
    pub process_started_at_unix_micros: u64,
    pub process_uptime_micros: u64,
    pub covered_uptime_micros: u64,
    pub buckets: Vec<SqliteBucketSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteWorkloadAggregateReport {
    pub requested_window: SqliteSnapshotWindow,
    pub bucket_count: usize,
    pub restart_truncated: bool,
    pub process_started_at_unix_micros: u64,
    pub process_uptime_micros: u64,
    pub covered_uptime_micros: u64,
    pub classification_gap_count: u64,
    pub writer_occupancy_gap_count: u64,
    pub totals: Box<
        [[BucketCategoryTotals; SqliteWorkloadCategory::ALL.len()]; SqliteAccessKind::ALL.len()],
    >,
    pub outcomes: Box<
        [[[u64; SqliteOutcome::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    pub native_statement_latency_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    pub typed_attempt_latency_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    pub pool_wait_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    pub write_admission_wait_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampledSqliteWorkloadAggregateReport {
    pub sampled_at_unix_micros: u64,
    pub report: SqliteWorkloadAggregateReport,
}

#[derive(Clone)]
struct CollectorClock {
    unix_now_micros: Arc<dyn Fn() -> u64 + Send + Sync>,
    instant_now: Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl std::fmt::Debug for CollectorClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollectorClock").finish_non_exhaustive()
    }
}

impl Default for CollectorClock {
    fn default() -> Self {
        Self {
            unix_now_micros: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(duration_to_micros)
                    .unwrap_or(0)
            }),
            instant_now: Arc::new(Instant::now),
        }
    }
}

impl CollectorClock {
    fn unix_now_micros(&self) -> u64 {
        (self.unix_now_micros)()
    }

    fn instant_now(&self) -> Instant {
        (self.instant_now)()
    }
}

#[derive(Debug)]
struct SqliteBucket {
    minute_start_unix_micros: u64,
    classification_gap_count: u64,
    writer_occupancy_gap_count: u64,
    totals: Box<
        [[BucketCategoryTotals; SqliteWorkloadCategory::ALL.len()]; SqliteAccessKind::ALL.len()],
    >,
    outcomes: Box<
        [[[u64; SqliteOutcome::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    native_statement_latency_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    typed_attempt_latency_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,

    pool_wait_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
    write_admission_wait_histogram: Box<
        [[[u64; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
            SqliteAccessKind::ALL.len()],
    >,
}

impl Default for SqliteBucket {
    fn default() -> Self {
        Self {
            minute_start_unix_micros: 0,
            classification_gap_count: 0,
            writer_occupancy_gap_count: 0,
            totals: Box::new(
                [[BucketCategoryTotals::default(); SqliteWorkloadCategory::ALL.len()];
                    SqliteAccessKind::ALL.len()],
            ),
            outcomes: Box::new(
                [[[0; SqliteOutcome::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
                    SqliteAccessKind::ALL.len()],
            ),
            native_statement_latency_histogram: Box::new(
                [[[0; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
                    SqliteAccessKind::ALL.len()],
            ),
            typed_attempt_latency_histogram: Box::new(
                [[[0; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
                    SqliteAccessKind::ALL.len()],
            ),
            pool_wait_histogram: Box::new(
                [[[0; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
                    SqliteAccessKind::ALL.len()],
            ),
            write_admission_wait_histogram: Box::new(
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
            classification_gap_count: self.classification_gap_count,
            writer_occupancy_gap_count: self.writer_occupancy_gap_count,
            totals: self.totals.clone(),
            outcomes: self.outcomes.clone(),
            native_statement_latency_histogram: self.native_statement_latency_histogram.clone(),
            typed_attempt_latency_histogram: self.typed_attempt_latency_histogram.clone(),
            pool_wait_histogram: self.pool_wait_histogram.clone(),
            write_admission_wait_histogram: self.write_admission_wait_histogram.clone(),
        }
    }
}

#[derive(Debug)]
struct InnerCollector {
    process_started_at_unix_micros: u64,
    process_started_at: Instant,
    buckets: Box<[SqliteBucket; BUCKET_COUNT]>,
}

impl InnerCollector {
    fn new(clock: &CollectorClock) -> Self {
        Self {
            process_started_at_unix_micros: clock.unix_now_micros(),
            process_started_at: clock.instant_now(),
            buckets: Box::new(std::array::from_fn(|_| SqliteBucket::default())),
        }
    }
}

#[derive(Debug, Default)]
struct ActiveNativeReads {
    next_identity: u64,
    categories_by_identity: HashMap<u64, SqliteWorkloadCategory>,
}

#[derive(Debug, Clone)]
pub struct SqliteWorkloadCollector {
    inner: Arc<Mutex<InnerCollector>>,
    active_native_reads: Arc<Mutex<ActiveNativeReads>>,
    clock: CollectorClock,
}

impl Default for SqliteWorkloadCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl SqliteWorkloadCollector {
    #[must_use]
    pub fn new() -> Self {
        Self::with_clock(CollectorClock::default())
    }

    fn with_clock(clock: CollectorClock) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerCollector::new(&clock))),
            active_native_reads: Arc::new(Mutex::new(ActiveNativeReads::default())),
            clock,
        }
    }

    #[cfg(test)]
    pub(crate) fn shared_id(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }

    #[cfg(test)]
    pub(crate) fn active_native_reads_for_test(&self) -> u32 {
        let active = self
            .active_native_reads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .categories_by_identity
            .len();
        u32::try_from(active).unwrap_or(u32::MAX)
    }

    #[cfg(test)]
    fn record(&self, observation: InnerObservation) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.record(observation);
    }

    pub(crate) fn record_native_statement(&self, observation: NativeStatementObservation) {
        let completed_at_unix_micros = self.clock.unix_now_micros();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.record(InnerObservation {
            completed_at_unix_micros,
            category: observation.category,
            access: observation.access,
            latency: observation.statement_latency,
            writer_held: Duration::ZERO,
            read_connection_time: observation.read_connection_time,
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 0,
            read_concurrency: observation.read_concurrency,
            counting: InnerObservationCounting::BaselineStatement,
        });
    }

    pub(crate) fn record_typed_outcome(&self, observation: TypedOutcomeObservation) {
        let completed_at_unix_micros = self.clock.unix_now_micros();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.record(InnerObservation {
            completed_at_unix_micros,
            category: observation.category,
            access: observation.access,
            latency: observation.latency,
            writer_held: Duration::ZERO,
            read_connection_time: Duration::ZERO,
            retry_count: observation.retry_count,
            retry_backoff: observation.retry_backoff,
            writer_concurrency: 0,
            read_concurrency: 0,
            counting: InnerObservationCounting::TypedOutcome {
                outcome: observation.outcome,
                waits: observation.waits,
            },
        });
    }

    pub(crate) fn begin_native_read(&self, category: SqliteWorkloadCategory) -> NativeReadToken {
        let (identity, concurrency, active_categories) = {
            let mut active = self
                .active_native_reads
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            active.next_identity = active.next_identity.wrapping_add(1);
            let identity = active.next_identity;
            active.categories_by_identity.insert(identity, category);
            let concurrency =
                u32::try_from(active.categories_by_identity.len()).unwrap_or(u32::MAX);
            let mut active_categories = [false; SqliteWorkloadCategory::ALL.len()];
            for category in active.categories_by_identity.values() {
                active_categories[category.index()] = true;
            }
            (identity, concurrency, active_categories)
        };
        self.record_native_read_transition(concurrency, active_categories);
        NativeReadToken {
            collector: self.clone(),
            identity,
            concurrency,
            active: true,
        }
    }

    fn end_native_read(&self, identity: u64) {
        let (concurrency, active_categories) = {
            let mut active = self
                .active_native_reads
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            active.categories_by_identity.remove(&identity);
            let concurrency =
                u32::try_from(active.categories_by_identity.len()).unwrap_or(u32::MAX);
            let mut active_categories = [false; SqliteWorkloadCategory::ALL.len()];
            for category in active.categories_by_identity.values() {
                active_categories[category.index()] = true;
            }
            (concurrency, active_categories)
        };
        self.record_native_read_transition(concurrency, active_categories);
    }

    fn record_native_read_transition(
        &self,
        concurrency: u32,
        active_categories: [bool; SqliteWorkloadCategory::ALL.len()],
    ) {
        let now = self.clock.unix_now_micros();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bucket = inner.bucket_mut(minute_start(now));
        let read = SqliteAccessKind::Read.index();
        for category in SqliteWorkloadCategory::ALL {
            if active_categories[category.index()] {
                let totals = &mut bucket.totals[read][category.index()];
                totals.read_concurrency_peak = totals.read_concurrency_peak.max(concurrency);
            }
        }
    }

    pub(crate) fn record_classification_gap(&self) {
        let now = self.clock.unix_now_micros();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bucket = inner.bucket_mut(minute_start(now));
        bucket.classification_gap_count = bucket.classification_gap_count.saturating_add(1);
    }

    pub(crate) fn record_writer_occupancy_gap(&self) {
        let now = self.clock.unix_now_micros();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bucket = inner.bucket_mut(minute_start(now));
        bucket.writer_occupancy_gap_count = bucket.writer_occupancy_gap_count.saturating_add(1);
    }

    pub(crate) fn record_writer_occupancy(
        &self,
        category: SqliteWorkloadCategory,
        writer_held: Duration,
    ) {
        let completed_at_unix_micros = self.clock.unix_now_micros();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.record(InnerObservation {
            completed_at_unix_micros,
            category,
            access: SqliteAccessKind::Write,
            latency: Duration::ZERO,
            writer_held,
            read_connection_time: Duration::ZERO,
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 1,
            read_concurrency: 0,
            counting: InnerObservationCounting::OccupancyOnly,
        });
    }

    #[must_use]
    pub fn snapshot(
        &self,
        window: SqliteSnapshotWindow,
        now_unix_micros: u64,
    ) -> SqliteWorkloadSnapshot {
        let now_instant = self.clock.instant_now();
        self.snapshot_at(window, now_unix_micros, now_instant)
    }

    fn snapshot_at(
        &self,
        window: SqliteSnapshotWindow,
        now_unix_micros: u64,
        now_instant: Instant,
    ) -> SqliteWorkloadSnapshot {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.snapshot(window, now_unix_micros, now_instant)
    }

    /// Captures the report cutoff while holding the collector lock used by recorders.
    ///
    #[must_use]
    pub fn aggregate_report_now(
        &self,
        window: SqliteSnapshotWindow,
    ) -> SampledSqliteWorkloadAggregateReport {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sampled_at_unix_micros = self.clock.unix_now_micros();
        let snapshot = inner.snapshot(window, sampled_at_unix_micros, self.clock.instant_now());
        drop(inner);
        SampledSqliteWorkloadAggregateReport {
            sampled_at_unix_micros,
            report: Self::aggregate_snapshot(&snapshot),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn aggregate_fresh_collector_for_test(&self) -> SqliteWorkloadAggregateReport {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = self.clock.unix_now_micros();
        let process_uptime_micros = now.saturating_sub(inner.process_started_at_unix_micros);
        assert!(
            process_uptime_micros
                <= (SqliteSnapshotWindow::OneHour.minutes() as u64)
                    .saturating_mul(MICROS_PER_MINUTE),
            "fresh-collector test aggregation requires a collector younger than one hour"
        );
        let process_started_at_unix_micros = inner.process_started_at_unix_micros;
        let buckets: Vec<_> = inner
            .buckets
            .iter()
            .filter(|bucket| bucket.minute_start_unix_micros != 0)
            .map(SqliteBucket::snapshot)
            .collect();
        drop(inner);
        let snapshot = SqliteWorkloadSnapshot {
            window: SqliteSnapshotWindow::OneHour,
            bucket_count: buckets.len(),
            restart_truncated: false,
            process_started_at_unix_micros,
            process_uptime_micros,
            covered_uptime_micros: process_uptime_micros,
            buckets,
        };
        Self::aggregate_snapshot(&snapshot)
    }

    #[cfg(test)]
    fn aggregate_report_now_with_snapshot_hook(
        &self,
        window: SqliteSnapshotWindow,
        hook: impl FnOnce(u64),
    ) -> SampledSqliteWorkloadAggregateReport {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sampled_at_unix_micros = self.clock.unix_now_micros();
        let snapshot = inner.snapshot(window, sampled_at_unix_micros, self.clock.instant_now());
        hook(sampled_at_unix_micros);
        drop(inner);
        SampledSqliteWorkloadAggregateReport {
            sampled_at_unix_micros,
            report: Self::aggregate_snapshot(&snapshot),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn aggregate_report(
        &self,
        window: SqliteSnapshotWindow,
        now_unix_micros: u64,
    ) -> SqliteWorkloadAggregateReport {
        let now_instant = self.clock.instant_now();
        Self::aggregate_snapshot(&self.snapshot_at(window, now_unix_micros, now_instant))
    }

    fn aggregate_snapshot(snapshot: &SqliteWorkloadSnapshot) -> SqliteWorkloadAggregateReport {
        let mut totals = Box::new(
            [[BucketCategoryTotals::default(); SqliteWorkloadCategory::ALL.len()];
                SqliteAccessKind::ALL.len()],
        );
        let mut outcomes = Box::new(
            [[[0; SqliteOutcome::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
                SqliteAccessKind::ALL.len()],
        );
        let mut native_statement_latency_histogram = Box::new(
            [[[0; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
                SqliteAccessKind::ALL.len()],
        );
        let mut typed_attempt_latency_histogram = Box::new(
            [[[0; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
                SqliteAccessKind::ALL.len()],
        );
        let mut pool_wait_histogram = Box::new(
            [[[0; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
                SqliteAccessKind::ALL.len()],
        );
        let mut write_admission_wait_histogram = Box::new(
            [[[0; SqliteLatencyBin::ALL.len()]; SqliteWorkloadCategory::ALL.len()];
                SqliteAccessKind::ALL.len()],
        );
        let mut classification_gap_count = 0u64;
        let mut writer_occupancy_gap_count = 0u64;
        for bucket in &snapshot.buckets {
            classification_gap_count += bucket.classification_gap_count;
            writer_occupancy_gap_count += bucket.writer_occupancy_gap_count;
            for access in SqliteAccessKind::ALL {
                for category in SqliteWorkloadCategory::ALL {
                    let a = access.index();
                    let c = category.index();
                    let dst = &mut totals[a][c];
                    let src = bucket.totals[a][c];
                    dst.baseline_statement_count += src.baseline_statement_count;
                    dst.native_statement_latency_micros += src.native_statement_latency_micros;
                    dst.typed_attempt_latency_micros += src.typed_attempt_latency_micros;
                    dst.pool_wait_micros += src.pool_wait_micros;
                    dst.write_admission_wait_micros += src.write_admission_wait_micros;
                    dst.writer_held_micros += src.writer_held_micros;
                    dst.read_connection_micros += src.read_connection_micros;
                    dst.retry_count += src.retry_count;
                    dst.retry_backoff_micros += src.retry_backoff_micros;
                    dst.writer_concurrency_peak =
                        dst.writer_concurrency_peak.max(src.writer_concurrency_peak);
                    dst.read_concurrency_peak =
                        dst.read_concurrency_peak.max(src.read_concurrency_peak);
                    for outcome in SqliteOutcome::ALL {
                        outcomes[a][c][outcome.index()] += bucket.outcomes[a][c][outcome.index()];
                    }
                    for bin in SqliteLatencyBin::ALL {
                        native_statement_latency_histogram[a][c][bin.index()] +=
                            bucket.native_statement_latency_histogram[a][c][bin.index()];
                        typed_attempt_latency_histogram[a][c][bin.index()] +=
                            bucket.typed_attempt_latency_histogram[a][c][bin.index()];
                        pool_wait_histogram[a][c][bin.index()] +=
                            bucket.pool_wait_histogram[a][c][bin.index()];
                        write_admission_wait_histogram[a][c][bin.index()] +=
                            bucket.write_admission_wait_histogram[a][c][bin.index()];
                    }
                }
            }
        }
        SqliteWorkloadAggregateReport {
            requested_window: snapshot.window,
            bucket_count: snapshot.bucket_count,
            restart_truncated: snapshot.restart_truncated,
            process_started_at_unix_micros: snapshot.process_started_at_unix_micros,
            process_uptime_micros: snapshot.process_uptime_micros,
            covered_uptime_micros: snapshot.covered_uptime_micros,
            classification_gap_count,
            writer_occupancy_gap_count,
            totals,
            outcomes,
            native_statement_latency_histogram,
            typed_attempt_latency_histogram,
            pool_wait_histogram,
            write_admission_wait_histogram,
        }
    }
}

impl InnerCollector {
    fn record(&mut self, observation: InnerObservation) {
        let completed_minute_start = minute_start(observation.completed_at_unix_micros);
        let bucket = self.bucket_mut(completed_minute_start);
        let access_index = observation.access.index();
        let category_index = observation.category.index();
        let totals = &mut bucket.totals[access_index][category_index];
        match observation.counting {
            InnerObservationCounting::BaselineStatement => {
                totals.baseline_statement_count += 1;
                totals.native_statement_latency_micros += duration_to_micros(observation.latency);
                totals.read_concurrency_peak = totals
                    .read_concurrency_peak
                    .max(observation.read_concurrency);
                bucket.native_statement_latency_histogram[access_index][category_index]
                    [SqliteLatencyBin::from_duration(observation.latency).index()] += 1;
            }
            InnerObservationCounting::TypedOutcome { outcome, waits } => {
                totals.typed_attempt_latency_micros += duration_to_micros(observation.latency);
                totals.retry_count += observation.retry_count;
                totals.retry_backoff_micros += duration_to_micros(observation.retry_backoff);
                totals.writer_concurrency_peak = totals
                    .writer_concurrency_peak
                    .max(observation.writer_concurrency);
                totals.read_concurrency_peak = totals
                    .read_concurrency_peak
                    .max(observation.read_concurrency);
                bucket.outcomes[access_index][category_index][outcome.index()] += 1;
                bucket.typed_attempt_latency_histogram[access_index][category_index]
                    [SqliteLatencyBin::from_duration(observation.latency).index()] += 1;
                match waits {
                    SqliteWaitMeasurement::Unavailable => {}
                    SqliteWaitMeasurement::PoolOnly { pool_wait } => {
                        record_wait(
                            totals,
                            &mut bucket.pool_wait_histogram[access_index][category_index],
                            pool_wait,
                            false,
                        );
                    }
                    SqliteWaitMeasurement::PoolAndAdmission {
                        pool_wait,
                        admission_wait,
                    } => {
                        record_wait(
                            totals,
                            &mut bucket.pool_wait_histogram[access_index][category_index],
                            pool_wait,
                            false,
                        );
                        if observation.access == SqliteAccessKind::Write {
                            record_wait(
                                totals,
                                &mut bucket.write_admission_wait_histogram[access_index]
                                    [category_index],
                                admission_wait,
                                true,
                            );
                        }
                    }
                }
            }
            InnerObservationCounting::OccupancyOnly => {
                totals.writer_concurrency_peak = totals
                    .writer_concurrency_peak
                    .max(observation.writer_concurrency);
            }
        }

        self.distribute_duration(
            observation.completed_at_unix_micros,
            observation.writer_held,
            observation.access,
            observation.category,
            observation.writer_concurrency,
            true,
        );
        self.distribute_duration(
            observation.completed_at_unix_micros,
            observation.read_connection_time,
            observation.access,
            observation.category,
            observation.read_concurrency,
            false,
        );
    }

    fn distribute_duration(
        &mut self,
        completed_at_unix_micros: u64,
        duration: Duration,
        access: SqliteAccessKind,
        category: SqliteWorkloadCategory,
        concurrency: u32,
        writer: bool,
    ) {
        let total = duration_to_micros(duration);
        if total == 0 {
            return;
        }
        let access_index = access.index();
        let category_index = category.index();
        let retained_start = minute_start(completed_at_unix_micros)
            .saturating_sub(((BUCKET_COUNT as u64).saturating_sub(1)) * MICROS_PER_MINUTE);
        let start = completed_at_unix_micros
            .saturating_sub(total)
            .max(retained_start);
        let mut remaining = completed_at_unix_micros.saturating_sub(start);
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
                totals.writer_concurrency_peak = totals.writer_concurrency_peak.max(concurrency);
            } else {
                totals.read_connection_micros += used_in_bucket;
                totals.read_concurrency_peak = totals.read_concurrency_peak.max(concurrency);
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
        now_instant: Instant,
    ) -> SqliteWorkloadSnapshot {
        let process_uptime_micros =
            duration_to_micros(now_instant.saturating_duration_since(self.process_started_at));
        let effective_now_unix_micros = now_unix_micros;
        let requested_covered_uptime_micros =
            (window.minutes() as u64).saturating_mul(MICROS_PER_MINUTE);
        let wall_elapsed_micros =
            effective_now_unix_micros.saturating_sub(self.process_started_at_unix_micros);
        let total_uptime_micros = wall_elapsed_micros.min(process_uptime_micros);
        let available_start = effective_now_unix_micros
            .saturating_sub(total_uptime_micros)
            .max(effective_now_unix_micros.saturating_sub(requested_covered_uptime_micros));
        let first_complete_minute = minute_start(available_start).saturating_add(
            u64::from(!available_start.is_multiple_of(MICROS_PER_MINUTE)) * MICROS_PER_MINUTE,
        );
        let start_minute = if first_complete_minute >= effective_now_unix_micros {
            minute_start(effective_now_unix_micros)
        } else {
            first_complete_minute
        };
        let covered_start = if first_complete_minute >= effective_now_unix_micros {
            available_start
        } else {
            start_minute
        };
        let covered_uptime_micros = effective_now_unix_micros.saturating_sub(covered_start);
        let restart_truncated = total_uptime_micros < requested_covered_uptime_micros;
        let bucket_count = if covered_uptime_micros == 0 {
            0
        } else {
            let last_minute_start = minute_start(effective_now_unix_micros.saturating_sub(1));
            usize::try_from((last_minute_start.saturating_sub(start_minute)) / MICROS_PER_MINUTE)
                .unwrap_or(usize::MAX)
                .saturating_add(1)
                .min(BUCKET_COUNT)
        };
        let mut buckets = Vec::with_capacity(bucket_count);
        for offset in 0..bucket_count {
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
            bucket_count,
            restart_truncated,
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

fn record_wait(
    totals: &mut BucketCategoryTotals,
    histogram: &mut [u64; SqliteLatencyBin::ALL.len()],
    wait: Duration,
    admission: bool,
) {
    let micros = duration_to_micros(wait);
    if admission {
        totals.write_admission_wait_micros =
            totals.write_admission_wait_micros.saturating_add(micros);
    } else {
        totals.pool_wait_micros = totals.pool_wait_micros.saturating_add(micros);
    }
    histogram[SqliteLatencyBin::from_duration(wait).index()] += 1;
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
pub(crate) fn unix_now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(duration_to_micros)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SqlitePercentiles {
    pub sample_count: u64,
    pub p50_upper_bound_ms: Option<u64>,
    pub p95_upper_bound_ms: Option<u64>,
    pub p99_upper_bound_ms: Option<u64>,
}

impl SqliteLatencyBin {
    const fn upper_bound_ms(self) -> Option<u64> {
        match self {
            Self::Under1Ms => Some(1),
            Self::Ms1To4 => Some(5),
            Self::Ms5To19 => Some(20),
            Self::Ms20To99 => Some(100),
            Self::Ms100To249 => Some(250),
            Self::Ms250To999 => Some(1_000),
            Self::Ms1000Plus => None,
        }
    }
}

#[must_use]
pub fn approximate_percentiles_from_histogram(
    histogram: &[u64; SqliteLatencyBin::ALL.len()],
) -> SqlitePercentiles {
    let sample_count = histogram.iter().copied().sum();
    SqlitePercentiles {
        sample_count,
        p50_upper_bound_ms: approximate_percentile(histogram, sample_count, 50),
        p95_upper_bound_ms: approximate_percentile(histogram, sample_count, 95),
        p99_upper_bound_ms: approximate_percentile(histogram, sample_count, 99),
    }
}

fn approximate_percentile(
    histogram: &[u64; SqliteLatencyBin::ALL.len()],
    sample_count: u64,
    percentile: u64,
) -> Option<u64> {
    if sample_count == 0 {
        return None;
    }
    let target = sample_count.saturating_mul(percentile).saturating_add(99) / 100;
    let mut seen = 0u64;
    for bin in SqliteLatencyBin::ALL {
        seen = seen.saturating_add(histogram[bin.index()]);
        if seen >= target {
            return bin.upper_bound_ms();
        }
    }
    SqliteLatencyBin::Ms1000Plus.upper_bound_ms()
}

#[cfg(test)]
impl SqliteWorkloadCollector {
    fn with_test_clock(
        unix_now_micros: impl Fn() -> u64 + Send + Sync + 'static,
        instant_now: impl Fn() -> Instant + Send + Sync + 'static,
    ) -> Self {
        Self::with_clock(CollectorClock {
            unix_now_micros: Arc::new(unix_now_micros),
            instant_now: Arc::new(instant_now),
        })
    }
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
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.process_started_at_unix_micros = started_at_unix_micros;
            inner.process_started_at = Instant::now().checked_sub(uptime).unwrap();
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
        collector.record(InnerObservation {
            completed_at_unix_micros: (2 * M) + 10_000_000,
            category: SqliteWorkloadCategory::Fts,
            access: SqliteAccessKind::Write,
            latency: Duration::from_millis(30),
            writer_held: Duration::from_secs(70),
            read_connection_time: Duration::ZERO,
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 1,
            read_concurrency: 0,
            counting: InnerObservationCounting::TypedOutcome {
                outcome: SqliteOutcome::Success,
                waits: SqliteWaitMeasurement::Unavailable,
            },
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
        collector.record(InnerObservation {
            completed_at_unix_micros: (3 * M) + 5_000_000,
            category: SqliteWorkloadCategory::RuntimeState,
            access: SqliteAccessKind::Read,
            latency: Duration::from_millis(12),
            writer_held: Duration::ZERO,
            read_connection_time: Duration::from_secs(65),
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 0,
            read_concurrency: 2,
            counting: InnerObservationCounting::TypedOutcome {
                outcome: SqliteOutcome::Success,
                waits: SqliteWaitMeasurement::Unavailable,
            },
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
        let peaks: Vec<(u64, u32)> = snapshot
            .buckets
            .iter()
            .filter_map(|bucket| {
                let peak = bucket.totals[read][cat].read_concurrency_peak;
                (peak > 0).then_some((bucket.minute_start_unix_micros, peak))
            })
            .collect();
        assert_eq!(peaks, vec![(2 * M, 2), (3 * M, 2)]);
    }

    #[test]
    fn native_read_transition_peak_is_attributed_to_each_active_category() {
        let collector = SqliteWorkloadCollector::new();
        let first = collector.begin_native_read(SqliteWorkloadCategory::RuntimeState);
        collector.record_native_statement(NativeStatementObservation {
            category: SqliteWorkloadCategory::RuntimeState,
            access: SqliteAccessKind::Read,
            statement_latency: Duration::from_millis(1),
            read_connection_time: Duration::from_millis(1),
            read_concurrency: first.concurrency(),
        });
        let second = collector.begin_native_read(SqliteWorkloadCategory::Fts);
        collector.record_native_statement(NativeStatementObservation {
            category: SqliteWorkloadCategory::Fts,
            access: SqliteAccessKind::Read,
            statement_latency: Duration::from_millis(1),
            read_connection_time: Duration::from_millis(1),
            read_concurrency: second.concurrency(),
        });
        second.finish();
        first.finish();

        let report = collector.aggregate_report(
            SqliteSnapshotWindow::OneHour,
            collector.clock.unix_now_micros(),
        );
        let read = SqliteAccessKind::Read.index();
        assert_eq!(
            report.totals[read][SqliteWorkloadCategory::RuntimeState.index()].read_concurrency_peak,
            2,
        );
        assert_eq!(
            report.totals[read][SqliteWorkloadCategory::Fts.index()].read_concurrency_peak,
            2,
        );
    }

    #[test]
    fn non_minute_aligned_fixed_window_discards_leading_partial_minute() {
        for offset in [1, 30_000_000, M - 1] {
            let now = 100 * M + offset;
            let collector = collector_started_at(1, Duration::from_secs(24 * 60 * 60));

            let snapshot = collector.snapshot(SqliteSnapshotWindow::OneHour, now);

            assert!(!snapshot.restart_truncated);
            assert_eq!(snapshot.bucket_count, 60, "offset={offset}");
            assert_eq!(
                snapshot.covered_uptime_micros,
                59 * M + offset,
                "offset={offset}",
            );
            assert_eq!(
                snapshot.buckets.first().unwrap().minute_start_unix_micros,
                41 * M,
                "offset={offset}",
            );
            assert_eq!(
                snapshot.buckets.last().unwrap().minute_start_unix_micros,
                100 * M,
                "offset={offset}",
            );
        }
    }

    #[test]
    fn restart_boundary_coverage_begins_at_first_complete_minute_boundary() {
        let now = 100 * M + 1;
        let collector = collector_started_at(100 * M - 1, Duration::from_micros(2));

        let snapshot = {
            let inner = collector
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.snapshot(
                SqliteSnapshotWindow::OneHour,
                now,
                inner.process_started_at + Duration::from_micros(2),
            )
        };

        assert!(snapshot.restart_truncated);
        assert_eq!(snapshot.process_uptime_micros, 2);
        assert_eq!(snapshot.covered_uptime_micros, 1);
        assert_eq!(snapshot.bucket_count, 1);
        assert_eq!(snapshot.buckets[0].minute_start_unix_micros, 100 * M,);
    }

    #[test]
    fn snapshot_clips_coverage_to_process_uptime() {
        let collector = SqliteWorkloadCollector::new();
        {
            let mut inner = collector
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.process_started_at_unix_micros = 10 * M;
            inner.process_started_at = Instant::now()
                .checked_sub(Duration::from_secs(150))
                .unwrap();
        }

        let snapshot = collector.snapshot(SqliteSnapshotWindow::OneHour, 20 * M);
        assert_eq!(snapshot.bucket_count, 2);
        assert!(snapshot.restart_truncated);
        assert_eq!(snapshot.covered_uptime_micros, 2 * M);
        assert_eq!(
            snapshot.buckets.first().unwrap().minute_start_unix_micros,
            18 * M
        );
        assert_eq!(
            snapshot.buckets.last().unwrap().minute_start_unix_micros,
            19 * M
        );

        let report = collector.aggregate_report(SqliteSnapshotWindow::OneHour, 20 * M);
        assert_eq!(report.covered_uptime_micros, 2 * M);
    }

    #[test]
    fn snapshot_clips_coverage_to_requested_duration() {
        let collector = SqliteWorkloadCollector::new();
        {
            let mut inner = collector
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.process_started_at_unix_micros = 10 * M;
            inner.process_started_at = Instant::now()
                .checked_sub(Duration::from_secs(200 * 60))
                .unwrap();
        }

        let snapshot = collector.snapshot(SqliteSnapshotWindow::OneHour, 300 * M);
        assert_eq!(snapshot.bucket_count, 60);
        assert!(!snapshot.restart_truncated);
        assert_eq!(snapshot.covered_uptime_micros, 60 * M);
        assert_eq!(
            snapshot.buckets.first().unwrap().minute_start_unix_micros,
            240 * M
        );
        assert_eq!(
            snapshot.buckets.last().unwrap().minute_start_unix_micros,
            299 * M
        );
    }

    #[test]
    fn snapshot_windows_are_fixed_and_cover_available_history() {
        let collector = collector_started_at(999 * M, Duration::from_secs(2 * 60));
        collector.record(InnerObservation {
            completed_at_unix_micros: (1_000 * M) - 1,
            category: SqliteWorkloadCategory::Maintenance,
            access: SqliteAccessKind::Write,
            latency: Duration::from_secs(2),
            writer_held: Duration::from_secs(1),
            read_connection_time: Duration::ZERO,
            retry_count: 4,
            retry_backoff: Duration::from_millis(20),
            writer_concurrency: 1,
            read_concurrency: 0,
            counting: InnerObservationCounting::TypedOutcome {
                outcome: SqliteOutcome::Busy,
                waits: SqliteWaitMeasurement::Unavailable,
            },
        });

        let one = collector.snapshot(SqliteSnapshotWindow::OneHour, 1_000 * M);
        let six = collector.snapshot(SqliteSnapshotWindow::SixHours, 1_000 * M);
        let day = collector.snapshot(SqliteSnapshotWindow::TwentyFourHours, 1_000 * M);
        assert_eq!(one.bucket_count, 1);
        assert_eq!(six.bucket_count, 1);
        assert_eq!(day.bucket_count, 1);
        assert!(one.restart_truncated);
        assert!(six.restart_truncated);
        assert!(day.restart_truncated);
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
            db.sqlite_workload_collector.shared_id(),
            cloned.sqlite_workload_collector.shared_id()
        );
    }

    #[test]
    fn completion_bucket_receives_count_outcome_histogram_and_aggregates() {
        let collector = collector_started_at(3 * M, Duration::from_secs(2 * 60));
        let completed_at = (4 * M) + 42;
        collector.record(InnerObservation {
            completed_at_unix_micros: completed_at,
            category: SqliteWorkloadCategory::MessagePersistence,
            access: SqliteAccessKind::Write,
            latency: Duration::from_millis(130),
            writer_held: Duration::from_secs(1),
            read_connection_time: Duration::ZERO,
            retry_count: 2,
            retry_backoff: Duration::from_millis(15),
            writer_concurrency: 1,
            read_concurrency: 0,
            counting: InnerObservationCounting::TypedOutcome {
                outcome: SqliteOutcome::Locked,
                waits: SqliteWaitMeasurement::PoolAndAdmission {
                    pool_wait: Duration::from_millis(7),
                    admission_wait: Duration::from_millis(11),
                },
            },
        });
        let snapshot = collector.snapshot(SqliteSnapshotWindow::OneHour, completed_at);
        let bucket = snapshot.buckets.last().unwrap();
        let access = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::MessagePersistence.index();
        assert_eq!(operation_count(&bucket.outcomes[access][category]), 1);
        assert_eq!(bucket.totals[access][category].pool_wait_micros, 7_000);
        assert_eq!(
            bucket.totals[access][category].write_admission_wait_micros,
            11_000
        );
        assert_eq!(bucket.totals[access][category].retry_count, 2);
        assert_eq!(bucket.totals[access][category].retry_backoff_micros, 15_000);
        assert_eq!(
            bucket.outcomes[access][category][SqliteOutcome::Locked.index()],
            1
        );
        assert_eq!(
            bucket.native_statement_latency_histogram[access][category]
                [SqliteLatencyBin::Ms100To249.index()],
            0
        );
        assert_eq!(
            bucket.typed_attempt_latency_histogram[access][category]
                [SqliteLatencyBin::Ms100To249.index()],
            1
        );
        assert_eq!(
            bucket.totals[access][category].native_statement_latency_micros,
            0
        );
        assert_eq!(
            bucket.totals[access][category].typed_attempt_latency_micros,
            130_000
        );

        let report = collector.aggregate_report(SqliteSnapshotWindow::OneHour, completed_at);
        assert_eq!(operation_count(&report.outcomes[access][category]), 1);
        assert_eq!(
            report.outcomes[access][category][SqliteOutcome::Locked.index()],
            1
        );
    }

    #[test]
    fn mixed_sources_keep_latency_authorities_disjoint() {
        let collector = collector_started_at(0, Duration::from_secs(60));
        collector.record(InnerObservation {
            completed_at_unix_micros: M - 2,
            category: SqliteWorkloadCategory::RuntimeState,
            access: SqliteAccessKind::Read,
            latency: Duration::from_millis(12),
            writer_held: Duration::ZERO,
            read_connection_time: Duration::from_millis(30),
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 0,
            read_concurrency: 2,
            counting: InnerObservationCounting::BaselineStatement,
        });
        collector.record(InnerObservation {
            completed_at_unix_micros: M - 1,
            category: SqliteWorkloadCategory::RuntimeState,
            access: SqliteAccessKind::Read,
            latency: Duration::from_millis(45),
            writer_held: Duration::ZERO,
            read_connection_time: Duration::ZERO,
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 0,
            read_concurrency: 0,
            counting: InnerObservationCounting::TypedOutcome {
                outcome: SqliteOutcome::Success,
                waits: SqliteWaitMeasurement::Unavailable,
            },
        });
        collector.record_writer_occupancy(
            SqliteWorkloadCategory::RuntimeState,
            Duration::from_millis(20),
        );
        let report = collector.aggregate_report(SqliteSnapshotWindow::OneHour, M);
        let read = SqliteAccessKind::Read.index();
        let category = SqliteWorkloadCategory::RuntimeState.index();
        assert_eq!(report.totals[read][category].baseline_statement_count, 1);
        assert_eq!(operation_count(&report.outcomes[read][category]), 1);
        assert_eq!(
            report.totals[read][category].native_statement_latency_micros,
            12_000
        );
        assert_eq!(
            report.totals[read][category].typed_attempt_latency_micros,
            45_000
        );
        assert_eq!(
            report.native_statement_latency_histogram[read][category]
                .iter()
                .sum::<u64>(),
            1
        );
        assert_eq!(
            report.typed_attempt_latency_histogram[read][category]
                .iter()
                .sum::<u64>(),
            1
        );
    }

    #[test]
    fn minute_aligned_window_keeps_concurrent_read_duration_additive() {
        let collector = collector_started_at(0, Duration::from_secs(20 * 60));
        let completed_at = (10 * M) + 30_000_000;
        collector.record(InnerObservation {
            completed_at_unix_micros: completed_at,
            category: SqliteWorkloadCategory::Fts,
            access: SqliteAccessKind::Read,
            latency: Duration::from_millis(1),
            writer_held: Duration::ZERO,
            read_connection_time: Duration::from_secs(70),
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 1,
            read_concurrency: 0,
            counting: InnerObservationCounting::TypedOutcome {
                outcome: SqliteOutcome::Success,
                waits: SqliteWaitMeasurement::Unavailable,
            },
        });
        let snapshot = collector.snapshot(SqliteSnapshotWindow::OneHour, completed_at);
        let read = SqliteAccessKind::Read.index();
        let fts = SqliteWorkloadCategory::Fts.index();
        let relevant: Vec<(u64, u64)> = snapshot
            .buckets
            .iter()
            .map(|bucket| {
                (
                    bucket.minute_start_unix_micros,
                    bucket.totals[read][fts].read_connection_micros,
                )
            })
            .filter(|(_, micros)| *micros > 0)
            .collect();
        assert_eq!(relevant, vec![(9 * M, 40_000_000), (10 * M, 30_000_000)]);
    }

    #[test]
    fn sampled_cutoff_and_snapshot_share_the_lock_boundary() {
        use std::sync::mpsc;
        use std::thread;

        let collector = collector_started_at(
            unix_now_micros().saturating_sub(2 * M),
            Duration::from_secs(120),
        );
        let sampler = collector.clone();
        let (snapshot_taken_tx, snapshot_taken_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let sample_thread = thread::spawn(move || {
            sampler.aggregate_report_now_with_snapshot_hook(
                SqliteSnapshotWindow::OneHour,
                |cutoff| {
                    snapshot_taken_tx.send(cutoff).unwrap();
                    release_rx.recv().unwrap();
                },
            )
        });
        let cutoff = snapshot_taken_rx.recv().unwrap();
        let recorder = collector.clone();
        let record_thread = thread::spawn(move || {
            recorder.record_native_statement(NativeStatementObservation {
                category: SqliteWorkloadCategory::RuntimeState,
                access: SqliteAccessKind::Write,
                statement_latency: Duration::ZERO,
                read_connection_time: Duration::ZERO,
                read_concurrency: 0,
            });
        });
        release_tx.send(()).unwrap();
        let captured = sample_thread.join().unwrap();
        record_thread.join().unwrap();

        assert_eq!(captured.sampled_at_unix_micros, cutoff);
        let write = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::RuntimeState.index();
        assert_eq!(
            captured.report.totals[write][category].baseline_statement_count,
            0
        );
        assert_eq!(
            collector
                .aggregate_report_now(SqliteSnapshotWindow::OneHour)
                .report
                .totals[write][category]
                .baseline_statement_count,
            1
        );
    }

    #[test]
    fn recorder_uses_physical_completion_time_before_collector_lock_delay() {
        use std::sync::{mpsc, Arc, Barrier};
        use std::thread;

        use std::sync::atomic::{AtomicU64, Ordering};
        let now = Arc::new(AtomicU64::new(5 * M));
        let elapsed = Arc::new(AtomicU64::new(0));
        let instant = Instant::now();
        let (completion_captured_tx, completion_captured_rx) = mpsc::channel();
        let completion_signal = Arc::new(Mutex::new(None::<mpsc::Sender<()>>));
        let collector = SqliteWorkloadCollector::with_test_clock(
            {
                let now = Arc::clone(&now);
                let completion_signal = Arc::clone(&completion_signal);
                move || {
                    let captured = now.load(Ordering::SeqCst);
                    if let Some(signal) = completion_signal
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    {
                        signal.send(()).unwrap();
                    }
                    captured
                }
            },
            {
                let elapsed = Arc::clone(&elapsed);
                move || instant + Duration::from_micros(elapsed.load(Ordering::SeqCst))
            },
        );
        now.store(7 * M, Ordering::SeqCst);
        elapsed.store(2 * M, Ordering::SeqCst);
        *completion_signal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(completion_captured_tx);
        let guard = collector.inner.lock().unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let (recorded_tx, recorded_rx) = mpsc::channel();
        let recorder = collector.clone();
        let recorder_barrier = Arc::clone(&barrier);
        let record_thread = thread::spawn(move || {
            recorder_barrier.wait();
            recorder.record_native_statement(NativeStatementObservation {
                category: SqliteWorkloadCategory::RuntimeState,
                access: SqliteAccessKind::Write,
                statement_latency: Duration::ZERO,
                read_connection_time: Duration::ZERO,
                read_concurrency: 0,
            });
            recorded_tx.send(()).unwrap();
        });
        barrier.wait();
        completion_captured_rx.recv().unwrap();
        now.store(8 * M, Ordering::SeqCst);
        elapsed.store(3 * M, Ordering::SeqCst);
        drop(guard);
        recorded_rx.recv().unwrap();
        let snapshot = collector.snapshot(SqliteSnapshotWindow::OneHour, 8 * M);
        record_thread.join().unwrap();
        let write = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::RuntimeState.index();
        let physical_minute = snapshot
            .buckets
            .iter()
            .find(|bucket| bucket.minute_start_unix_micros == 7 * M)
            .unwrap();
        assert_eq!(
            physical_minute.totals[write][category].baseline_statement_count,
            1
        );
        let delayed_minute = snapshot
            .buckets
            .iter()
            .find(|bucket| bucket.minute_start_unix_micros == 8 * M);
        assert!(delayed_minute
            .is_none_or(|bucket| { bucket.totals[write][category].baseline_statement_count == 0 }));
    }

    #[test]
    fn pool_only_wait_does_not_fabricate_admission_sample() {
        let collector = collector_started_at(0, Duration::from_secs(60));
        collector.record(InnerObservation {
            completed_at_unix_micros: M - 1,
            category: SqliteWorkloadCategory::DurableWorkflows,
            access: SqliteAccessKind::Write,
            latency: Duration::from_millis(9),
            writer_held: Duration::ZERO,
            read_connection_time: Duration::ZERO,
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 0,
            read_concurrency: 0,
            counting: InnerObservationCounting::TypedOutcome {
                outcome: SqliteOutcome::PoolTimeout,
                waits: SqliteWaitMeasurement::PoolOnly {
                    pool_wait: Duration::from_millis(9),
                },
            },
        });
        let report = collector.aggregate_report(SqliteSnapshotWindow::OneHour, M);
        let write = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::DurableWorkflows.index();
        assert_eq!(
            report.pool_wait_histogram[write][category]
                .iter()
                .sum::<u64>(),
            1
        );
        assert_eq!(
            report.write_admission_wait_histogram[write][category]
                .iter()
                .sum::<u64>(),
            0
        );
    }

    #[test]
    fn unavailable_waits_add_neither_wait_total_nor_sample() {
        let collector = collector_started_at(0, Duration::from_secs(60));
        collector.record(InnerObservation {
            completed_at_unix_micros: M - 1,
            category: SqliteWorkloadCategory::RuntimeState,
            access: SqliteAccessKind::Write,
            latency: Duration::from_millis(5),
            writer_held: Duration::ZERO,
            read_connection_time: Duration::ZERO,
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 0,
            read_concurrency: 0,
            counting: InnerObservationCounting::TypedOutcome {
                outcome: SqliteOutcome::Abandoned,
                waits: SqliteWaitMeasurement::Unavailable,
            },
        });
        let report = collector.aggregate_report(SqliteSnapshotWindow::OneHour, M);
        let write = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::RuntimeState.index();
        assert_eq!(report.totals[write][category].pool_wait_micros, 0);
        assert_eq!(
            report.totals[write][category].write_admission_wait_micros,
            0
        );
        assert_eq!(
            report.pool_wait_histogram[write][category]
                .iter()
                .sum::<u64>(),
            0
        );
        assert_eq!(
            report.write_admission_wait_histogram[write][category]
                .iter()
                .sum::<u64>(),
            0
        );
    }

    #[test]
    fn poisoned_collector_lock_recovers_for_callback_recording() {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let collector = SqliteWorkloadCollector::new();
        let poison = collector.clone();
        let _ = catch_unwind(AssertUnwindSafe(move || {
            let _guard = poison.inner.lock().unwrap();
            panic!("poison collector for regression");
        }));
        collector.record_classification_gap();
        let report = collector
            .aggregate_report_now(SqliteSnapshotWindow::OneHour)
            .report;
        assert_eq!(report.classification_gap_count, 1);
    }

    #[test]
    fn expired_gap_counters_do_not_appear_in_new_window_reports() {
        let collector = collector_started_at(0, Duration::from_secs(2000 * 60));
        {
            let mut inner = collector.inner.lock().unwrap();
            inner.bucket_mut(0).classification_gap_count = 2;
            inner.bucket_mut(0).writer_occupancy_gap_count = 3;
        }
        let now = 2_000 * M;
        let report = collector.aggregate_report(SqliteSnapshotWindow::OneHour, now);
        assert_eq!(report.classification_gap_count, 0);
        assert_eq!(report.writer_occupancy_gap_count, 0);
    }

    #[test]
    fn duration_longer_than_ring_truncates_without_erasing_completion_bucket() {
        let collector =
            collector_started_at(0, Duration::from_secs((BUCKET_COUNT as u64 + 10) * 60));
        let completed_at = (2_000 * M) + 1;
        collector.record(InnerObservation {
            completed_at_unix_micros: completed_at,
            category: SqliteWorkloadCategory::RuntimeState,
            access: SqliteAccessKind::Read,
            latency: Duration::from_millis(1),
            writer_held: Duration::ZERO,
            read_connection_time: Duration::from_secs((BUCKET_COUNT as u64 + 10) * 60),
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 0,
            read_concurrency: 1,
            counting: InnerObservationCounting::TypedOutcome {
                outcome: SqliteOutcome::Success,
                waits: SqliteWaitMeasurement::Unavailable,
            },
        });
        let snapshot = collector.snapshot(SqliteSnapshotWindow::TwentyFourHours, completed_at);
        let read = SqliteAccessKind::Read.index();
        let cat = SqliteWorkloadCategory::RuntimeState.index();
        let completion_bucket = snapshot
            .buckets
            .iter()
            .find(|bucket| bucket.minute_start_unix_micros == 2_000 * M)
            .unwrap();
        assert_eq!(
            completion_bucket.totals[read][cat].read_connection_micros,
            1
        );
    }

    #[test]
    fn occupancy_only_observations_do_not_create_count_authorities() {
        let collector = collector_started_at(0, Duration::from_secs(5 * 60));
        let completed_at = 4 * M;
        collector.record(InnerObservation {
            completed_at_unix_micros: completed_at,
            category: SqliteWorkloadCategory::Maintenance,
            access: SqliteAccessKind::Write,
            latency: Duration::from_secs(1),
            writer_held: Duration::from_secs(5),
            read_connection_time: Duration::ZERO,
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 1,
            read_concurrency: 0,
            counting: InnerObservationCounting::OccupancyOnly,
        });
        let report = collector.aggregate_report(SqliteSnapshotWindow::OneHour, completed_at + 1);
        let access = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::Maintenance.index();
        assert_eq!(operation_count(&report.outcomes[access][category]), 0);
        assert_eq!(abandoned_count(&report.outcomes[access][category]), 0);
        assert_eq!(
            report.outcomes[access][category][SqliteOutcome::Abandoned.index()],
            0
        );
        assert_eq!(
            report.totals[access][category].writer_held_micros,
            5_000_000
        );
    }

    #[test]
    fn approximate_percentiles_use_upper_bounds_and_report_sample_count() {
        let histogram = [0, 1, 0, 18, 1, 0, 1];
        let percentiles = approximate_percentiles_from_histogram(&histogram);
        assert_eq!(percentiles.sample_count, 21);
        assert_eq!(percentiles.p50_upper_bound_ms, Some(100));
        assert_eq!(percentiles.p95_upper_bound_ms, Some(250));
        assert_eq!(percentiles.p99_upper_bound_ms, None);
    }

    #[test]
    fn snapshot_uses_exact_half_open_window_and_partial_edge_buckets() {
        let collector = collector_started_at(5 * M, Duration::from_secs(10 * 60));
        let now = (10 * M) + 30_000_000;
        let snapshot = collector.snapshot(SqliteSnapshotWindow::OneHour, now);
        assert_eq!(snapshot.covered_uptime_micros, now - (5 * M));
        assert_eq!(snapshot.bucket_count, 6);
        assert!(snapshot.restart_truncated);
        assert_eq!(
            snapshot.buckets.first().unwrap().minute_start_unix_micros,
            5 * M
        );
        assert_eq!(
            snapshot.buckets.last().unwrap().minute_start_unix_micros,
            10 * M
        );
    }
}
