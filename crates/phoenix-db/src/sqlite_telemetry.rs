#[cfg(test)]
use crate::sqlite_workload::operation_count;
use crate::sqlite_workload::{
    SqliteAccessKind, SqliteOutcome, SqliteWaitMeasurement, SqliteWorkloadCategory,
    SqliteWorkloadCollector, TypedOutcomeObservation,
};
use crate::{DbError, DbResult};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::field;

const SLOW_POOL_ACQUISITION: Duration = Duration::from_millis(100);
const SLOW_TRANSACTION: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SqliteOperation {
    ConversationDelete,
    CreateTaskApprovalHandoff,
    DirectTurnTerminalSettlement,
    FtsReconcileUpsert,
    FtsUpsert,
    UpdateMessageDisplayData,
}

impl SqliteOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ConversationDelete => "conversation.delete",
            Self::CreateTaskApprovalHandoff => "conversation.task_approval_handoff",
            Self::DirectTurnTerminalSettlement => "direct_turn.terminal_settlement",
            Self::FtsReconcileUpsert => "fts.reconcile_upsert",
            Self::FtsUpsert => "fts.upsert",
            Self::UpdateMessageDisplayData => "message.update_display_data",
        }
    }
}

impl SqliteWorkloadCategory {
    const fn as_str(self) -> &'static str {
        match self {
            Self::MessagePersistence => "message_persistence",
            Self::DurableWorkflows => "durable_workflows",
            Self::Fts => "fts",
            Self::RuntimeState => "runtime_state",
            Self::PrProjectData => "pr_project_data",
            Self::Maintenance => "maintenance",
            Self::Other => "other",
        }
    }
}

impl SqliteAccessKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

#[cfg(test)]
impl SqliteOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Busy => "busy",
            Self::Locked => "locked",
            Self::PoolTimeout => "pool_timeout",
            Self::OtherTimeout => "other_timeout",
            Self::OtherFailure => "other_failure",
            Self::Abandoned => "abandoned",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SqlitePhase {
    TransactionAcquisition,
    Statement,
    LocatorLookup,
    FtsRowDelete,
    LocatorDelete,
    FtsInsert,
    LocatorInsert,
    Commit,
    Rollback,
}

impl SqlitePhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::TransactionAcquisition => "transaction_acquisition",
            Self::Statement => "statement",
            Self::LocatorLookup => "locator_lookup",
            Self::FtsRowDelete => "fts_row_delete",
            Self::LocatorDelete => "locator_delete",
            Self::FtsInsert => "fts_insert",
            Self::LocatorInsert => "locator_insert",
            Self::Commit => "commit",
            Self::Rollback => "rollback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SqliteTransactionOutcome {
    Committed,
    RolledBack,
}

impl SqliteTransactionOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Committed => "committed",
            Self::RolledBack => "rolled_back",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlowSqlitePhase {
    PoolAcquisition,
    Transaction,
    PoolAcquisitionAndTransaction,
}

impl SlowSqlitePhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::PoolAcquisition => "pool_acquisition",
            Self::Transaction => "transaction",
            Self::PoolAcquisitionAndTransaction => "pool_acquisition_and_transaction",
        }
    }
}

#[must_use = "transaction timing must be completed with the transaction"]
pub(crate) struct SuccessfulTransactionTiming {
    acquisition_elapsed: Duration,
    admission_elapsed: Duration,
    transaction_started_at: Instant,
}

#[must_use = "pool acquisition timing must be attached to its transaction"]
pub(crate) struct SuccessfulPoolAcquisitionTiming {
    acquisition_elapsed: Duration,
}

impl SuccessfulPoolAcquisitionTiming {
    fn admitted(self, admission_elapsed: Duration) -> SuccessfulTransactionTiming {
        SuccessfulTransactionTiming {
            acquisition_elapsed: self.acquisition_elapsed,
            admission_elapsed,
            transaction_started_at: Instant::now(),
        }
    }
}

impl SuccessfulTransactionTiming {
    #[cfg(test)]
    fn from_durations(
        acquisition_elapsed: Duration,
        admission_elapsed: Duration,
        transaction_started_at: Instant,
    ) -> Self {
        Self {
            acquisition_elapsed,
            admission_elapsed,
            transaction_started_at,
        }
    }

    fn complete_at(self, transaction_ended_at: Instant) -> CompletedTransactionTiming {
        CompletedTransactionTiming {
            acquisition_elapsed: self.acquisition_elapsed,
            write_admission_wait_elapsed: self.admission_elapsed,
            transaction_elapsed: transaction_ended_at
                .saturating_duration_since(self.transaction_started_at),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_field_names)]
struct CompletedTransactionTiming {
    acquisition_elapsed: Duration,
    write_admission_wait_elapsed: Duration,
    transaction_elapsed: Duration,
}

impl CompletedTransactionTiming {
    const fn slow_phase(self) -> Option<SlowSqlitePhase> {
        match (
            self.acquisition_elapsed.as_millis() >= SLOW_POOL_ACQUISITION.as_millis(),
            self.transaction_elapsed.as_millis() >= SLOW_TRANSACTION.as_millis(),
        ) {
            (true, true) => Some(SlowSqlitePhase::PoolAcquisitionAndTransaction),
            (true, false) => Some(SlowSqlitePhase::PoolAcquisition),
            (false, true) => Some(SlowSqlitePhase::Transaction),
            (false, false) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SqliteResultCodes {
    primary: i32,
    extended: i32,
}

impl SqliteResultCodes {
    fn from_error(error: &sqlx::Error) -> Option<Self> {
        let extended = error.as_database_error()?.code()?.parse::<i32>().ok()?;
        Some(Self {
            primary: extended & 0xff,
            extended,
        })
    }
}

const fn sqlx_error_kind(error: &sqlx::Error) -> &'static str {
    match error {
        sqlx::Error::Database(_) => "database",
        sqlx::Error::PoolTimedOut => "pool_timeout",
        sqlx::Error::PoolClosed => "pool_closed",
        sqlx::Error::WorkerCrashed => "worker_crashed",
        sqlx::Error::Io(_) => "io",
        sqlx::Error::Tls(_) => "tls",
        sqlx::Error::Protocol(_) => "protocol",
        sqlx::Error::RowNotFound => "row_not_found",
        sqlx::Error::TypeNotFound { .. } => "type_not_found",
        sqlx::Error::ColumnIndexOutOfBounds { .. } => "column_index_out_of_bounds",
        sqlx::Error::ColumnNotFound(_) => "column_not_found",
        sqlx::Error::ColumnDecode { .. } => "column_decode",
        sqlx::Error::Encode(_) => "encode",
        sqlx::Error::Decode(_) => "decode",
        sqlx::Error::Configuration(_) => "configuration",
        sqlx::Error::InvalidArgument(_) => "invalid_argument",
        sqlx::Error::AnyDriverError(_) => "any_driver",
        sqlx::Error::InvalidSavePointStatement => "invalid_savepoint",
        sqlx::Error::BeginFailed => "begin_failed",
        sqlx::Error::Migrate(_) => "migrate",
        _ => "other",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProvenWaits {
    None,
    Pool(Duration),
    PoolAndAdmission { pool: Duration, admission: Duration },
}

impl ProvenWaits {
    const fn measurement(self) -> SqliteWaitMeasurement {
        match self {
            Self::None => SqliteWaitMeasurement::Unavailable,
            Self::Pool(pool_wait) => SqliteWaitMeasurement::PoolOnly { pool_wait },
            Self::PoolAndAdmission { pool, admission } => SqliteWaitMeasurement::PoolAndAdmission {
                pool_wait: pool,
                admission_wait: admission,
            },
        }
    }
}

pub(crate) struct SqliteTelemetry {
    operation: SqliteOperation,
    category: SqliteWorkloadCategory,
    access: SqliteAccessKind,
    collector: Option<SqliteWorkloadCollector>,
    started_at: Instant,
    outcome_recorded: AtomicBool,
    lifecycle_completed: AtomicBool,
    proven_waits: Mutex<ProvenWaits>,
}

impl Drop for SqliteTelemetry {
    fn drop(&mut self) {
        if self.lifecycle_completed.load(Ordering::Acquire)
            || self.outcome_recorded.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let Some(collector) = &self.collector else {
            return;
        };
        collector.record_typed_outcome(TypedOutcomeObservation {
            category: self.category,
            access: self.access,
            latency: self.started_at.elapsed(),
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            outcome: SqliteOutcome::Abandoned,
            waits: self.proven_waits().measurement(),
        });
    }
}

impl SqliteTelemetry {
    pub(crate) fn new(operation: SqliteOperation) -> Self {
        Self::without_collector(
            operation,
            SqliteWorkloadCategory::Other,
            SqliteAccessKind::Write,
        )
    }

    pub(crate) fn with_collector(
        operation: SqliteOperation,
        category: SqliteWorkloadCategory,
        access: SqliteAccessKind,
        collector: SqliteWorkloadCollector,
    ) -> Self {
        Self {
            operation,
            category,
            access,
            collector: Some(collector),
            started_at: Instant::now(),
            outcome_recorded: AtomicBool::new(false),
            lifecycle_completed: AtomicBool::new(false),
            proven_waits: Mutex::new(ProvenWaits::None),
        }
    }

    pub(crate) fn without_collector(
        operation: SqliteOperation,
        category: SqliteWorkloadCategory,
        access: SqliteAccessKind,
    ) -> Self {
        Self {
            operation,
            category,
            access,
            collector: None,
            started_at: Instant::now(),
            outcome_recorded: AtomicBool::new(false),
            lifecycle_completed: AtomicBool::new(false),
            proven_waits: Mutex::new(ProvenWaits::None),
        }
    }

    pub(crate) async fn observe_sqlx<T>(
        &self,
        phase: SqlitePhase,
        operation: impl std::future::Future<Output = Result<T, sqlx::Error>>,
    ) -> Result<T, sqlx::Error> {
        assert!(
            !matches!(phase, SqlitePhase::Commit | SqlitePhase::Rollback),
            "transaction terminals require typed commit/rollback observation",
        );
        let phase_started_at = Instant::now();
        let result = operation.await;
        match &result {
            Ok(_) if matches!(phase, SqlitePhase::Commit | SqlitePhase::Rollback) => {
                self.lifecycle_completed.store(true, Ordering::Release);
            }
            Err(error) => self.record_failure(phase, phase_started_at.elapsed(), error),
            Ok(_) => {}
        }
        result
    }

    pub(crate) async fn observe_db<T>(
        &self,
        phase: SqlitePhase,
        operation: impl std::future::Future<Output = DbResult<T>>,
    ) -> DbResult<T> {
        assert!(
            !matches!(phase, SqlitePhase::Commit | SqlitePhase::Rollback),
            "transaction terminals require typed commit/rollback observation",
        );
        let phase_started_at = Instant::now();
        let result = operation.await;
        match &result {
            Ok(_) if matches!(phase, SqlitePhase::Commit | SqlitePhase::Rollback) => {
                self.lifecycle_completed.store(true, Ordering::Release);
            }
            Err(DbError::Sqlx(error)) => {
                self.record_failure(phase, phase_started_at.elapsed(), error);
            }
            Err(_) | Ok(_) => {}
        }
        result
    }

    pub(crate) async fn observe_pool_acquisition_sqlx<T>(
        &self,
        operation: impl std::future::Future<Output = Result<T, sqlx::Error>>,
    ) -> Result<(T, SuccessfulPoolAcquisitionTiming), sqlx::Error> {
        let started_at = Instant::now();
        match operation.await {
            Ok(value) => {
                let acquisition_elapsed = started_at.elapsed();
                self.set_proven_waits(ProvenWaits::Pool(acquisition_elapsed));
                Ok((
                    value,
                    SuccessfulPoolAcquisitionTiming {
                        acquisition_elapsed,
                    },
                ))
            }
            Err(error) => {
                let acquisition_elapsed = started_at.elapsed();
                self.set_proven_waits(ProvenWaits::Pool(acquisition_elapsed));
                self.record_failure(
                    SqlitePhase::TransactionAcquisition,
                    acquisition_elapsed,
                    &error,
                );
                Err(error)
            }
        }
    }

    pub(crate) async fn observe_transaction_admission_db<T>(
        &self,
        pool: SuccessfulPoolAcquisitionTiming,
        operation: impl std::future::Future<Output = DbResult<T>>,
    ) -> DbResult<(T, SuccessfulTransactionTiming)> {
        let started_at = Instant::now();
        match operation.await {
            Ok(value) => {
                let admission_elapsed = started_at.elapsed();
                self.set_proven_waits(ProvenWaits::PoolAndAdmission {
                    pool: pool.acquisition_elapsed,
                    admission: admission_elapsed,
                });
                Ok((value, pool.admitted(admission_elapsed)))
            }
            Err(DbError::Sqlx(error)) => {
                let admission_elapsed = started_at.elapsed();
                self.set_proven_waits(ProvenWaits::PoolAndAdmission {
                    pool: pool.acquisition_elapsed,
                    admission: admission_elapsed,
                });
                self.record_failure(
                    SqlitePhase::TransactionAcquisition,
                    admission_elapsed,
                    &error,
                );
                Err(DbError::Sqlx(error))
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn observe_commit_db<T>(
        &self,
        timing: SuccessfulTransactionTiming,
        operation: impl std::future::Future<Output = DbResult<T>>,
    ) -> DbResult<T> {
        self.observe_transaction_completion_db(
            timing,
            SqlitePhase::Commit,
            SqliteTransactionOutcome::Committed,
            SqliteOutcome::Success,
            operation,
        )
        .await
    }

    pub(crate) async fn observe_rollback_db<T>(
        &self,
        timing: SuccessfulTransactionTiming,
        operation: impl std::future::Future<Output = DbResult<T>>,
    ) -> DbResult<T> {
        self.observe_transaction_completion_db(
            timing,
            SqlitePhase::Rollback,
            SqliteTransactionOutcome::RolledBack,
            SqliteOutcome::Success,
            operation,
        )
        .await
    }

    pub(crate) async fn observe_failure_rollback_db<T>(
        &self,
        timing: SuccessfulTransactionTiming,
        operation: impl std::future::Future<Output = DbResult<T>>,
    ) -> DbResult<T> {
        self.observe_transaction_completion_db(
            timing,
            SqlitePhase::Rollback,
            SqliteTransactionOutcome::RolledBack,
            SqliteOutcome::OtherFailure,
            operation,
        )
        .await
    }

    fn finish_successful_transaction(
        &self,
        timing: CompletedTransactionTiming,
        transaction_outcome: SqliteTransactionOutcome,
        operation_outcome: SqliteOutcome,
    ) {
        self.lifecycle_completed.store(true, Ordering::Release);
        self.record_typed_outcome(TypedOutcomeObservation {
            category: self.category,
            access: self.access,
            latency: self.started_at.elapsed(),
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            outcome: operation_outcome,
            waits: SqliteWaitMeasurement::PoolAndAdmission {
                pool_wait: timing.acquisition_elapsed,
                admission_wait: timing.write_admission_wait_elapsed,
            },
        });
        self.record_slow_success(timing, transaction_outcome);
    }

    async fn observe_transaction_completion_db<T>(
        &self,
        timing: SuccessfulTransactionTiming,
        phase: SqlitePhase,
        transaction_outcome: SqliteTransactionOutcome,
        operation_outcome: SqliteOutcome,
        operation: impl std::future::Future<Output = DbResult<T>>,
    ) -> DbResult<T> {
        let phase_started_at = Instant::now();
        match operation.await {
            Ok(value) => {
                self.finish_successful_transaction(
                    timing.complete_at(Instant::now()),
                    transaction_outcome,
                    operation_outcome,
                );
                Ok(value)
            }
            Err(error) => {
                if let DbError::Sqlx(sqlx_error) = &error {
                    self.record_failure(phase, phase_started_at.elapsed(), sqlx_error);
                } else {
                    self.lifecycle_completed.store(true, Ordering::Release);
                    self.record_typed_outcome(TypedOutcomeObservation {
                        category: self.category,
                        access: self.access,
                        latency: self.started_at.elapsed(),
                        retry_count: 0,
                        retry_backoff: Duration::ZERO,
                        outcome: SqliteOutcome::OtherFailure,
                        waits: self.proven_waits().measurement(),
                    });
                }
                Err(error)
            }
        }
    }

    fn record_slow_success(
        &self,
        timing: CompletedTransactionTiming,
        outcome: SqliteTransactionOutcome,
    ) {
        let Some(slow_phase) = timing.slow_phase() else {
            return;
        };
        let pool_acquisition_ms = elapsed_millis(timing.acquisition_elapsed);
        let transaction_ms = elapsed_millis(timing.transaction_elapsed);
        let parent = tracing::Span::current();
        let span = tracing::warn_span!(
            target: "phoenix_db::otel",
            parent: &parent,
            "db.slow_operation",
            db.system = "sqlite",
            db.operation = self.operation.as_str(),
            db.category = self.category.as_str(),
            db.access = self.access.as_str(),
            db.outcome = outcome.as_str(),
            db.slow_phase = slow_phase.as_str(),
            db.pool_acquisition_ms = trace_millis(pool_acquisition_ms),
            db.transaction_ms = trace_millis(transaction_ms),
        );
        let _entered = span.enter();
        tracing::warn!(
            target: "phoenix_db::observability",
            db_system = "sqlite",
            db_operation = self.operation.as_str(),
            db_category = self.category.as_str(),
            db_access = self.access.as_str(),
            db_outcome = outcome.as_str(),
            db_slow_phase = slow_phase.as_str(),
            db_pool_acquisition_ms = pool_acquisition_ms,
            db_transaction_ms = transaction_ms,
            "slow successful SQLite operation"
        );
    }

    fn record_failure(&self, phase: SqlitePhase, phase_elapsed: Duration, error: &sqlx::Error) {
        self.lifecycle_completed.store(true, Ordering::Release);
        let elapsed_ms = elapsed_millis(self.started_at.elapsed());
        let phase_elapsed_ms = elapsed_millis(phase_elapsed);
        let codes = SqliteResultCodes::from_error(error);
        let error_kind = sqlx_error_kind(error);
        let parent = tracing::Span::current();
        let span = tracing::error_span!(
            target: "phoenix_db::otel",
            parent: &parent,
            "db.failure",
            db.system = "sqlite",
            db.operation = self.operation.as_str(),
            db.phase = phase.as_str(),
            db.error.kind = error_kind,
            db.category = self.category.as_str(),
            db.access = self.access.as_str(),
            db.elapsed_ms = trace_millis(elapsed_ms),
            db.phase_elapsed_ms = trace_millis(phase_elapsed_ms),
            db.sqlite.primary_code = field::Empty,
            db.sqlite.extended_code = field::Empty,
            otel.status_code = "ERROR",
        );
        if let Some(codes) = codes {
            span.record("db.sqlite.primary_code", i64::from(codes.primary));
            span.record("db.sqlite.extended_code", i64::from(codes.extended));
        }

        let _entered = span.enter();
        if let Some(codes) = codes {
            tracing::error!(
                target: "phoenix_db::observability",
                db_system = "sqlite",
                db_operation = self.operation.as_str(),
                db_phase = phase.as_str(),
                db_category = self.category.as_str(),
                db_access = self.access.as_str(),
                db_error_kind = error_kind,
                db_elapsed_ms = elapsed_ms,
                db_phase_elapsed_ms = phase_elapsed_ms,
                db_sqlite_primary_code = codes.primary,
                db_sqlite_extended_code = codes.extended,
                "SQLite operation failed"
            );
        } else {
            tracing::error!(
                target: "phoenix_db::observability",
                db_system = "sqlite",
                db_operation = self.operation.as_str(),
                db_phase = phase.as_str(),
                db_category = self.category.as_str(),
                db_access = self.access.as_str(),
                db_error_kind = error_kind,
                db_elapsed_ms = elapsed_ms,
                db_phase_elapsed_ms = phase_elapsed_ms,
                "SQLite operation failed without a database result code"
            );
        }
        self.record_typed_outcome(TypedOutcomeObservation {
            category: self.category,
            access: self.access,
            latency: self.started_at.elapsed(),
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            outcome: classify_outcome(error),
            waits: self.proven_waits().measurement(),
        });
    }

    fn proven_waits(&self) -> ProvenWaits {
        *self
            .proven_waits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set_proven_waits(&self, waits: ProvenWaits) {
        *self
            .proven_waits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = waits;
    }

    fn record_typed_outcome(&self, observation: TypedOutcomeObservation) {
        if self.outcome_recorded.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(collector) = &self.collector {
            collector.record_typed_outcome(observation);
        }
    }
}

#[allow(clippy::match_same_arms)]
fn classify_outcome(error: &sqlx::Error) -> SqliteOutcome {
    match error {
        sqlx::Error::PoolTimedOut => SqliteOutcome::PoolTimeout,
        sqlx::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => {
            SqliteOutcome::OtherTimeout
        }
        sqlx::Error::Database(_) => {
            match SqliteResultCodes::from_error(error).map(|codes| codes.primary) {
                Some(5) => SqliteOutcome::Busy,
                Some(6) => SqliteOutcome::Locked,
                _ => SqliteOutcome::OtherFailure,
            }
        }
        sqlx::Error::Configuration(_)
        | sqlx::Error::InvalidArgument(_)
        | sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::Protocol(_)
        | sqlx::Error::RowNotFound
        | sqlx::Error::TypeNotFound { .. }
        | sqlx::Error::ColumnIndexOutOfBounds { .. }
        | sqlx::Error::ColumnNotFound(_)
        | sqlx::Error::ColumnDecode { .. }
        | sqlx::Error::Encode(_)
        | sqlx::Error::Decode(_)
        | sqlx::Error::AnyDriverError(_)
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed
        | sqlx::Error::Migrate(_)
        | sqlx::Error::InvalidSavePointStatement
        | sqlx::Error::BeginFailed => SqliteOutcome::OtherFailure,
        #[allow(unreachable_patterns)]
        _ => SqliteOutcome::OtherFailure,
    }
}

fn elapsed_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn trace_millis(elapsed_ms: u64) -> i64 {
    i64::try_from(elapsed_ms).unwrap_or(i64::MAX)
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id};
    use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

    #[derive(Clone, Default)]
    pub(crate) struct EventCapture {
        events: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
    }

    impl EventCapture {
        pub(crate) fn events(&self) -> Vec<BTreeMap<String, String>> {
            self.events.lock().expect("event capture lock").clone()
        }
    }

    impl<S> Layer<S> for EventCapture
    where
        S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>,
    {
        fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
            if event.metadata().target() != "phoenix_db::observability" {
                return;
            }
            let mut fields = BTreeMap::new();
            event.record(&mut FieldCapture(&mut fields));
            self.events.lock().expect("event capture lock").push(fields);
        }
    }

    #[derive(Clone, Default)]
    pub(crate) struct SpanCapture {
        spans: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
    }

    impl SpanCapture {
        pub(crate) fn spans(&self) -> Vec<BTreeMap<String, String>> {
            self.spans.lock().expect("span capture lock").clone()
        }
    }

    impl<S> Layer<S> for SpanCapture
    where
        S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>,
    {
        fn on_new_span(&self, attributes: &Attributes<'_>, _id: &Id, _context: Context<'_, S>) {
            if attributes.metadata().target() != "phoenix_db::otel" {
                return;
            }
            let mut fields = BTreeMap::from([(
                "span.name".to_owned(),
                attributes.metadata().name().to_owned(),
            )]);
            attributes.record(&mut FieldCapture(&mut fields));
            self.spans.lock().expect("span capture lock").push(fields);
        }
    }

    struct FieldCapture<'a>(&'a mut BTreeMap<String, String>);

    impl Visit for FieldCapture<'_> {
        fn record_i64(&mut self, field: &Field, value: i64) {
            self.0.insert(field.name().to_owned(), value.to_string());
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.insert(field.name().to_owned(), value.to_string());
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name().to_owned(), value.to_owned());
        }

        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name().to_owned(), format!("{value:?}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{EventCapture, SpanCapture};
    use super::*;
    use crate::retrieval::fts_upsert;
    use crate::sqlite_workload::unix_now_micros;
    use crate::sqlite_workload::SqliteSnapshotWindow;
    use crate::{Database, Message, MessageContent, MessageType};
    use chrono::Utc;
    use libsqlite3_sys as ffi;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;
    use tracing_subscriber::prelude::*;

    #[test]
    fn primary_code_is_derived_from_extended_code() {
        let codes = SqliteResultCodes {
            primary: ffi::SQLITE_BUSY_SNAPSHOT & 0xff,
            extended: ffi::SQLITE_BUSY_SNAPSHOT,
        };
        assert_eq!(codes.primary, ffi::SQLITE_BUSY);
        assert_eq!(codes.extended, ffi::SQLITE_BUSY_SNAPSHOT);
    }

    #[test]
    fn workload_outcome_vocabulary_is_bounded() {
        assert_eq!(SqliteOutcome::Busy.as_str(), "busy");
        assert_eq!(SqliteOutcome::Locked.as_str(), "locked");
        assert_eq!(ffi::SQLITE_BUSY_SNAPSHOT & 0xff, ffi::SQLITE_BUSY);
        assert_eq!(ffi::SQLITE_LOCKED_SHAREDCACHE & 0xff, ffi::SQLITE_LOCKED);
    }

    #[test]
    fn telemetry_vocabulary_is_bounded() {
        assert_eq!(
            SqliteOperation::DirectTurnTerminalSettlement.as_str(),
            "direct_turn.terminal_settlement"
        );
        assert_eq!(SqliteOperation::FtsUpsert.as_str(), "fts.upsert");
        assert_eq!(
            SqliteOperation::CreateTaskApprovalHandoff.as_str(),
            "conversation.task_approval_handoff"
        );
        assert_eq!(
            SqliteOperation::UpdateMessageDisplayData.as_str(),
            "message.update_display_data"
        );
        assert_eq!(SqlitePhase::LocatorLookup.as_str(), "locator_lookup");
        assert_eq!(SqlitePhase::Commit.as_str(), "commit");
        assert_eq!(SqlitePhase::Rollback.as_str(), "rollback");
        assert_eq!(SqliteTransactionOutcome::Committed.as_str(), "committed");
        assert_eq!(SqliteTransactionOutcome::RolledBack.as_str(), "rolled_back");
        assert_eq!(
            SlowSqlitePhase::PoolAcquisition.as_str(),
            "pool_acquisition"
        );
        assert_eq!(SlowSqlitePhase::Transaction.as_str(), "transaction");
        assert_eq!(
            SlowSqlitePhase::PoolAcquisitionAndTransaction.as_str(),
            "pool_acquisition_and_transaction"
        );
    }

    #[test]
    fn transaction_timing_separates_pool_wait_and_excludes_retry_backoff() {
        let transaction_started_at = Instant::now();
        let timing = SuccessfulTransactionTiming::from_durations(
            Duration::from_millis(120),
            Duration::from_millis(35),
            transaction_started_at,
        )
        .complete_at(transaction_started_at + Duration::from_millis(40));

        assert_eq!(timing.acquisition_elapsed, Duration::from_millis(120));
        assert_eq!(
            timing.write_admission_wait_elapsed,
            Duration::from_millis(35)
        );
        assert_eq!(timing.transaction_elapsed, Duration::from_millis(40));
    }

    #[test]
    fn slow_thresholds_are_inclusive_and_fast_success_emits_nothing() {
        let fast_acquisition = Duration::from_millis(99);
        let fast_transaction = Duration::from_millis(249);
        assert_eq!(
            CompletedTransactionTiming {
                acquisition_elapsed: fast_acquisition,
                write_admission_wait_elapsed: Duration::ZERO,
                transaction_elapsed: fast_transaction,
            }
            .slow_phase(),
            None
        );
        assert_eq!(
            CompletedTransactionTiming {
                acquisition_elapsed: SLOW_POOL_ACQUISITION,
                write_admission_wait_elapsed: Duration::ZERO,
                transaction_elapsed: fast_transaction,
            }
            .slow_phase(),
            Some(SlowSqlitePhase::PoolAcquisition)
        );
        assert_eq!(
            CompletedTransactionTiming {
                acquisition_elapsed: fast_acquisition,
                write_admission_wait_elapsed: Duration::ZERO,
                transaction_elapsed: SLOW_TRANSACTION,
            }
            .slow_phase(),
            Some(SlowSqlitePhase::Transaction)
        );
        assert_eq!(
            CompletedTransactionTiming {
                acquisition_elapsed: SLOW_POOL_ACQUISITION,
                write_admission_wait_elapsed: Duration::ZERO,
                transaction_elapsed: SLOW_TRANSACTION,
            }
            .slow_phase(),
            Some(SlowSqlitePhase::PoolAcquisitionAndTransaction)
        );

        let events = EventCapture::default();
        let spans = SpanCapture::default();
        let subscriber = tracing_subscriber::registry()
            .with(events.clone())
            .with(spans.clone());
        tracing::subscriber::with_default(subscriber, || {
            SqliteTelemetry::new(SqliteOperation::DirectTurnTerminalSettlement)
                .record_slow_success(
                    CompletedTransactionTiming {
                        acquisition_elapsed: fast_acquisition,
                        write_admission_wait_elapsed: Duration::ZERO,
                        transaction_elapsed: fast_transaction,
                    },
                    SqliteTransactionOutcome::Committed,
                );
        });
        assert!(events.events().is_empty());
        assert!(spans.spans().is_empty());
    }

    #[test]
    fn slow_success_emits_one_bounded_privacy_safe_signal() {
        let events = EventCapture::default();
        let spans = SpanCapture::default();
        let subscriber = tracing_subscriber::registry()
            .with(events.clone())
            .with(spans.clone());
        tracing::subscriber::with_default(subscriber, || {
            SqliteTelemetry::new(SqliteOperation::DirectTurnTerminalSettlement)
                .record_slow_success(
                    CompletedTransactionTiming {
                        acquisition_elapsed: Duration::from_millis(150),
                        write_admission_wait_elapsed: Duration::ZERO,
                        transaction_elapsed: Duration::from_millis(300),
                    },
                    SqliteTransactionOutcome::RolledBack,
                );
        });

        let captured_events = events.events();
        assert_eq!(captured_events.len(), 1);
        let event = &captured_events[0];
        assert_eq!(
            event.get("db_operation").map(String::as_str),
            Some("direct_turn.terminal_settlement")
        );
        assert_eq!(
            event.get("db_slow_phase").map(String::as_str),
            Some("pool_acquisition_and_transaction")
        );
        assert_eq!(
            event.get("db_pool_acquisition_ms").map(String::as_str),
            Some("150")
        );
        assert_eq!(
            event.get("db_transaction_ms").map(String::as_str),
            Some("300")
        );

        let captured_spans = spans.spans();
        assert_eq!(captured_spans.len(), 1);
        let span = &captured_spans[0];
        assert_eq!(
            span.get("span.name").map(String::as_str),
            Some("db.slow_operation")
        );
        assert_eq!(span.get("db.system").map(String::as_str), Some("sqlite"));
        assert_eq!(
            span.get("db.outcome").map(String::as_str),
            Some("rolled_back")
        );
        assert_eq!(
            span.get("db.pool_acquisition_ms").map(String::as_str),
            Some("150")
        );
        assert_eq!(
            span.get("db.transaction_ms").map(String::as_str),
            Some("300")
        );
        let rendered = format!("{captured_events:?}{captured_spans:?}");
        for forbidden in ["conversation_id", "message_id", "SELECT", "/Users/"] {
            assert!(!rendered.contains(forbidden));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn failed_transaction_boundaries_emit_only_failure_telemetry() {
        let events = EventCapture::default();
        let spans = SpanCapture::default();
        let subscriber = tracing_subscriber::registry()
            .with(events.clone())
            .with(spans.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        let result = SqliteTelemetry::new(SqliteOperation::DirectTurnTerminalSettlement)
            .observe_pool_acquisition_sqlx(async { Err::<(), _>(sqlx::Error::PoolClosed) })
            .await;

        assert!(matches!(result, Err(sqlx::Error::PoolClosed)));
        let origin = Instant::now();
        let telemetry = SqliteTelemetry::new(SqliteOperation::DirectTurnTerminalSettlement);
        let commit = telemetry
            .observe_commit_db(
                SuccessfulTransactionTiming::from_durations(Duration::ZERO, Duration::ZERO, origin),
                async { Err::<(), _>(DbError::Sqlx(sqlx::Error::PoolClosed)) },
            )
            .await;
        let rollback = telemetry
            .observe_rollback_db(
                SuccessfulTransactionTiming::from_durations(Duration::ZERO, Duration::ZERO, origin),
                async { Err::<(), _>(DbError::Sqlx(sqlx::Error::PoolClosed)) },
            )
            .await;
        assert!(matches!(
            commit,
            Err(DbError::Sqlx(sqlx::Error::PoolClosed))
        ));
        assert!(matches!(
            rollback,
            Err(DbError::Sqlx(sqlx::Error::PoolClosed))
        ));

        let captured_events = events.events();
        assert_eq!(captured_events.len(), 3);
        assert_eq!(
            captured_events
                .iter()
                .map(|event| event.get("db_phase").map(String::as_str))
                .collect::<Vec<_>>(),
            [
                Some("transaction_acquisition"),
                Some("commit"),
                Some("rollback")
            ]
        );
        let captured_spans = spans.spans();
        assert_eq!(captured_spans.len(), 3);
        assert!(captured_spans.iter().all(|span| span
            .get("span.name")
            .is_some_and(|name| name == "db.failure")));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn successful_transaction_completion_records_once_with_aggregate_observation() {
        let telemetry = SqliteTelemetry::with_collector(
            SqliteOperation::ConversationDelete,
            SqliteWorkloadCategory::MessagePersistence,
            SqliteAccessKind::Write,
            SqliteWorkloadCollector::new(),
        );
        let acquisition_started_at = Instant::now()
            .checked_sub(Duration::from_millis(50))
            .unwrap();
        let transaction_started_at = acquisition_started_at + Duration::from_millis(20);

        telemetry
            .observe_commit_db(
                SuccessfulTransactionTiming::from_durations(
                    transaction_started_at.saturating_duration_since(acquisition_started_at),
                    Duration::ZERO,
                    transaction_started_at,
                ),
                async { Ok::<(), _>(()) },
            )
            .await
            .unwrap();

        let snapshot = telemetry
            .collector
            .as_ref()
            .unwrap()
            .aggregate_report(SqliteSnapshotWindow::OneHour, unix_now_micros());
        let access = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::MessagePersistence.index();
        assert_eq!(operation_count(&snapshot.outcomes[access][category]), 1);
        assert_eq!(
            snapshot.outcomes[access][category][SqliteOutcome::Success.index()],
            1
        );
        assert!(snapshot.totals[access][category].pool_wait_micros >= 20_000);
        assert_eq!(snapshot.totals[access][category].writer_held_micros, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_fts_lock_contention_records_exact_phase_and_codes_without_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contention.db");
        let db = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::migrations::run_pending_migrations(db.pool())
            .await
            .unwrap();

        let options =
            SqliteConnectOptions::from_str(&format!("sqlite:{}?mode=rw", path.to_string_lossy()))
                .unwrap()
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(Duration::ZERO)
                .foreign_keys(true);
        let contending_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();

        let mut writer = db.pool().acquire().await.unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *writer)
            .await
            .unwrap();

        let capture = EventCapture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let error = fts_upsert(
            &contending_pool,
            &Message {
                message_id: "sensitive-message-id".to_string(),
                conversation_id: "busy-conversation".to_string(),
                sequence_id: 1,
                message_type: MessageType::User,
                content: MessageContent::user("sensitive payload sentinel"),
                display_data: None,
                usage_data: None,
                created_at: Utc::now(),
            },
            db.sqlite_workload_collector.clone(),
        )
        .await
        .unwrap_err();
        sqlx::query("ROLLBACK").execute(&mut *writer).await.unwrap();

        let codes = SqliteResultCodes::from_error(&error).expect("SQLite result codes");
        assert_eq!(codes.primary, ffi::SQLITE_BUSY);

        let events = capture.events();
        assert_eq!(events.len(), 1, "one failed database phase is recorded");
        let event = &events[0];
        assert_eq!(
            event.get("db_operation").map(String::as_str),
            Some("fts.upsert")
        );
        assert_eq!(
            event.get("db_phase").map(String::as_str),
            Some("transaction_acquisition")
        );
        assert_eq!(
            event.get("db_sqlite_primary_code").map(String::as_str),
            Some("5")
        );
        assert_eq!(
            event.get("db_sqlite_extended_code").map(String::as_str),
            Some(codes.extended.to_string()).as_deref()
        );
        assert_eq!(
            event.get("db_error_kind").map(String::as_str),
            Some("database")
        );
        assert!(!event.contains_key("db_attempt"));
        assert!(!event.contains_key("db_retry_count"));
        let rendered = format!("{event:?}");
        assert!(!rendered.contains("sensitive-message-id"));
        assert!(!rendered.contains("sensitive payload sentinel"));
    }

    #[tokio::test]
    async fn successful_commit_disarms_abandoned_outcome() {
        let collector = SqliteWorkloadCollector::new();
        let telemetry = SqliteTelemetry::with_collector(
            SqliteOperation::FtsUpsert,
            SqliteWorkloadCategory::Fts,
            SqliteAccessKind::Write,
            collector.clone(),
        );
        let timing = SuccessfulTransactionTiming::from_durations(
            Duration::ZERO,
            Duration::ZERO,
            Instant::now(),
        );
        telemetry
            .observe_commit_db(timing, async { Ok::<(), DbError>(()) })
            .await
            .unwrap();
        drop(telemetry);

        let snapshot = collector.aggregate_report(SqliteSnapshotWindow::OneHour, unix_now_micros());
        let access = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::Fts.index();
        assert_eq!(
            snapshot.outcomes[access][category][SqliteOutcome::Abandoned.index()],
            0
        );
    }

    #[tokio::test]
    #[should_panic(expected = "transaction terminals require typed commit/rollback observation")]
    async fn generic_observation_rejects_transaction_terminals() {
        let telemetry = SqliteTelemetry::new(SqliteOperation::FtsUpsert);
        telemetry
            .observe_db(SqlitePhase::Commit, async { Ok::<(), DbError>(()) })
            .await
            .unwrap();
    }

    #[test]
    fn unfinished_telemetry_records_one_abandoned_operation() {
        let collector = SqliteWorkloadCollector::new();
        {
            let _telemetry = SqliteTelemetry::with_collector(
                SqliteOperation::FtsUpsert,
                SqliteWorkloadCategory::Fts,
                SqliteAccessKind::Write,
                collector.clone(),
            );
        }
        let snapshot = collector.aggregate_report(SqliteSnapshotWindow::OneHour, unix_now_micros());
        let access = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::Fts.index();
        assert_eq!(
            snapshot.outcomes[access][category][SqliteOutcome::Abandoned.index()],
            1
        );
        assert_eq!(
            snapshot.outcomes[access][category][SqliteOutcome::Abandoned.index()],
            1
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn failed_transaction_completion_records_once() {
        let telemetry = SqliteTelemetry::with_collector(
            SqliteOperation::ConversationDelete,
            SqliteWorkloadCategory::MessagePersistence,
            SqliteAccessKind::Write,
            SqliteWorkloadCollector::new(),
        );
        let origin = Instant::now();

        let result = telemetry
            .observe_commit_db(
                SuccessfulTransactionTiming::from_durations(Duration::ZERO, Duration::ZERO, origin),
                async { Err::<(), _>(DbError::Sqlx(sqlx::Error::PoolClosed)) },
            )
            .await;

        assert!(matches!(
            result,
            Err(DbError::Sqlx(sqlx::Error::PoolClosed))
        ));
        let snapshot = telemetry
            .collector
            .as_ref()
            .unwrap()
            .aggregate_report(SqliteSnapshotWindow::OneHour, unix_now_micros());
        let access = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::MessagePersistence.index();
        assert_eq!(operation_count(&snapshot.outcomes[access][category]), 1);
        assert_eq!(
            snapshot.outcomes[access][category][SqliteOutcome::OtherFailure.index()],
            1
        );
    }

    fn assert_wait_samples(
        collector: &SqliteWorkloadCollector,
        expected_pool: u64,
        expected_admission: u64,
    ) {
        let report = collector.aggregate_report(SqliteSnapshotWindow::OneHour, unix_now_micros());
        let write = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::MessagePersistence.index();
        assert_eq!(
            report.pool_wait_histogram[write][category]
                .iter()
                .sum::<u64>(),
            expected_pool
        );
        assert_eq!(
            report.write_admission_wait_histogram[write][category]
                .iter()
                .sum::<u64>(),
            expected_admission
        );
    }

    #[tokio::test]
    async fn post_pool_failure_and_later_failure_keep_proven_wait_boundaries() {
        let collector = SqliteWorkloadCollector::new();
        let telemetry = SqliteTelemetry::with_collector(
            SqliteOperation::ConversationDelete,
            SqliteWorkloadCategory::MessagePersistence,
            SqliteAccessKind::Write,
            collector.clone(),
        );
        let ((), pool) = telemetry
            .observe_pool_acquisition_sqlx(async { Ok::<_, sqlx::Error>(()) })
            .await
            .unwrap();
        let result = telemetry
            .observe_transaction_admission_db(pool, async {
                Err::<(), _>(DbError::Sqlx(sqlx::Error::BeginFailed))
            })
            .await;
        assert!(result.is_err());
        assert_wait_samples(&collector, 1, 1);

        let collector = SqliteWorkloadCollector::new();
        let telemetry = SqliteTelemetry::with_collector(
            SqliteOperation::ConversationDelete,
            SqliteWorkloadCategory::MessagePersistence,
            SqliteAccessKind::Write,
            collector.clone(),
        );
        let ((), pool) = telemetry
            .observe_pool_acquisition_sqlx(async { Ok::<_, sqlx::Error>(()) })
            .await
            .unwrap();
        let ((), _timing) = telemetry
            .observe_transaction_admission_db(pool, async { Ok::<_, DbError>(()) })
            .await
            .unwrap();
        let _ = telemetry
            .observe_db(SqlitePhase::Statement, async {
                Err::<(), _>(DbError::Sqlx(sqlx::Error::PoolClosed))
            })
            .await;
        assert_wait_samples(&collector, 1, 1);
    }

    #[tokio::test]
    async fn abandonment_after_admission_keeps_proven_wait_boundaries() {
        let collector = SqliteWorkloadCollector::new();
        {
            let telemetry = SqliteTelemetry::with_collector(
                SqliteOperation::ConversationDelete,
                SqliteWorkloadCategory::MessagePersistence,
                SqliteAccessKind::Write,
                collector.clone(),
            );
            let ((), pool) = telemetry
                .observe_pool_acquisition_sqlx(async { Ok::<_, sqlx::Error>(()) })
                .await
                .unwrap();
            let _ = telemetry
                .observe_transaction_admission_db(pool, async { Ok::<_, DbError>(()) })
                .await
                .unwrap();
        }
        assert_wait_samples(&collector, 1, 1);
    }

    fn message_persistence_outcomes(db: &Database) -> [u64; SqliteOutcome::ALL.len()] {
        db.sqlite_workload_collector
            .aggregate_report(SqliteSnapshotWindow::OneHour, unix_now_micros())
            .outcomes[SqliteAccessKind::Write.index()]
            [SqliteWorkloadCategory::MessagePersistence.index()]
    }

    fn assert_one_outcome_delta(
        before: [u64; SqliteOutcome::ALL.len()],
        after: [u64; SqliteOutcome::ALL.len()],
        expected: SqliteOutcome,
    ) {
        for outcome in SqliteOutcome::ALL {
            assert_eq!(
                after[outcome.index()] - before[outcome.index()],
                u64::from(outcome == expected),
                "unexpected {outcome:?} delta",
            );
        }
    }

    #[tokio::test]
    async fn conversation_delete_commit_records_exactly_one_success() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("delete-success", "Delete", "/tmp", true, None, None)
            .await
            .unwrap();
        let before = message_persistence_outcomes(&db);

        db.delete_conversation("delete-success").await.unwrap();

        assert_one_outcome_delta(
            before,
            message_persistence_outcomes(&db),
            SqliteOutcome::Success,
        );
    }

    #[tokio::test]
    async fn conversation_delete_missing_rolls_back_with_one_failure_outcome() {
        let db = Database::open_in_memory().await.unwrap();
        let before = message_persistence_outcomes(&db);

        let error = db.delete_conversation("delete-missing").await.unwrap_err();

        assert!(matches!(error, DbError::ConversationNotFound(id) if id == "delete-missing"));
        assert_one_outcome_delta(
            before,
            message_persistence_outcomes(&db),
            SqliteOutcome::OtherFailure,
        );
    }

    #[tokio::test]
    async fn update_display_data_commit_records_exactly_one_success() {
        let db = Database::open_in_memory().await.unwrap();
        db.create_conversation("display-success", "Display", "/tmp", true, None, None)
            .await
            .unwrap();
        db.add_message(
            "display-success-message",
            "display-success",
            &MessageContent::tool("tool", "result", false),
            None,
            None,
        )
        .await
        .unwrap();
        let before = message_persistence_outcomes(&db);

        db.update_message_display_data(
            "display-success-message",
            &serde_json::json!({"hidden": false}),
        )
        .await
        .unwrap();

        assert_one_outcome_delta(
            before,
            message_persistence_outcomes(&db),
            SqliteOutcome::Success,
        );
    }

    #[tokio::test]
    async fn update_display_data_missing_rolls_back_with_one_failure_outcome() {
        let db = Database::open_in_memory().await.unwrap();
        let before = message_persistence_outcomes(&db);

        let error = db
            .update_message_display_data("display-missing", &serde_json::json!({"hidden": false}))
            .await
            .unwrap_err();

        assert!(matches!(error, DbError::MessageNotFound(id) if id == "display-missing"));
        assert_one_outcome_delta(
            before,
            message_persistence_outcomes(&db),
            SqliteOutcome::OtherFailure,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn caller_owned_fts_transaction_records_acquisition_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("caller-owned.db");
        let setup = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::migrations::run_pending_migrations(setup.pool())
            .await
            .unwrap();

        let options =
            SqliteConnectOptions::from_str(&format!("sqlite:{}?mode=rw", path.to_string_lossy()))
                .unwrap()
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(Duration::ZERO)
                .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let db = Database::from_pool_for_tests(pool.clone(), path.to_string_lossy().into_owned());
        pool.close().await;

        let capture = EventCapture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let error = db.delete_conversation("unread-under-pool-contention").await;

        assert!(matches!(
            error,
            Err(crate::DbError::Sqlx(sqlx::Error::PoolClosed))
        ));
        let outcomes = message_persistence_outcomes(&db);
        assert_eq!(operation_count(&outcomes), 1);
        assert_eq!(outcomes[SqliteOutcome::OtherFailure.index()], 1);
        assert_eq!(outcomes[SqliteOutcome::Abandoned.index()], 0);
        let events = capture.events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].get("db_operation").map(String::as_str),
            Some("conversation.delete")
        );
        assert_eq!(
            events[0].get("db_phase").map(String::as_str),
            Some("transaction_acquisition")
        );
        assert_eq!(
            events[0].get("db_error_kind").map(String::as_str),
            Some("pool_closed")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn display_update_records_transaction_admission_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("caller-owned-statement.db");
        let setup = Database::open(path.to_str().unwrap()).await.unwrap();
        crate::migrations::run_pending_migrations(setup.pool())
            .await
            .unwrap();
        setup
            .create_conversation("busy-conversation", "Busy", "/tmp", true, None, None)
            .await
            .unwrap();
        setup
            .add_message(
                "busy-message",
                "busy-conversation",
                &MessageContent::user("source payload sentinel"),
                None,
                None,
            )
            .await
            .unwrap();

        let options =
            SqliteConnectOptions::from_str(&format!("sqlite:{}?mode=rw", path.to_string_lossy()))
                .unwrap()
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(Duration::ZERO)
                .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let db = Database::from_pool_for_tests(pool, path.to_string_lossy().into_owned());
        let mut writer = setup.pool().acquire().await.unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *writer)
            .await
            .unwrap();

        let capture = EventCapture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let error = db
            .update_message_display_data("busy-message", &serde_json::json!({"hidden": false}))
            .await;
        sqlx::query("ROLLBACK").execute(&mut *writer).await.unwrap();

        assert!(matches!(error, Err(crate::DbError::Sqlx(_))));
        let outcomes = message_persistence_outcomes(&db);
        assert_eq!(operation_count(&outcomes), 1);
        assert_eq!(outcomes[SqliteOutcome::Busy.index()], 1);
        assert_eq!(outcomes[SqliteOutcome::Abandoned.index()], 0);
        let events = capture.events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].get("db_operation").map(String::as_str),
            Some("message.update_display_data")
        );
        assert_eq!(
            events[0].get("db_phase").map(String::as_str),
            Some("transaction_acquisition")
        );
        assert_eq!(
            events[0].get("db_sqlite_primary_code").map(String::as_str),
            Some("5")
        );
        assert!(!format!("{:?}", events[0]).contains("source payload sentinel"));
    }
}
