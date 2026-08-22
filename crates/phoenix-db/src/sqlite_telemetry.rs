use crate::sqlite_workload::{
    SqliteAccessKind, SqliteObservation, SqliteOutcome, SqliteWorkloadCategory,
    SqliteWorkloadCollector,
};
use crate::{DbError, DbResult};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::field;

const SLOW_POOL_ACQUISITION: Duration = Duration::from_millis(100);
const SLOW_TRANSACTION: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SqliteOperation {
    ConversationDelete,
    CreateTaskApprovalHandoff,
    DirectTurnTerminalSettlement,
    FtsDeleteConversation,
    FtsDeleteMessage,
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
            Self::FtsDeleteConversation => "fts.delete_conversation",
            Self::FtsDeleteMessage => "fts.delete_message",
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
    transaction_started_at: Instant,
    admitted_transaction_start_elapsed: Duration,
}

#[must_use = "pool acquisition timing must be attached to its transaction"]
pub(crate) struct SuccessfulPoolAcquisitionTiming {
    acquisition_elapsed: Duration,
}

impl SuccessfulPoolAcquisitionTiming {
    pub(crate) fn transaction_started(self) -> SuccessfulTransactionTiming {
        SuccessfulTransactionTiming {
            acquisition_elapsed: self.acquisition_elapsed,
            transaction_started_at: Instant::now(),
            admitted_transaction_start_elapsed: self.acquisition_elapsed,
        }
    }
}

impl SuccessfulTransactionTiming {
    #[cfg(test)]
    fn from_boundaries(acquisition_started_at: Instant, transaction_started_at: Instant) -> Self {
        Self {
            acquisition_elapsed: transaction_started_at
                .saturating_duration_since(acquisition_started_at),
            transaction_started_at,
            admitted_transaction_start_elapsed: transaction_started_at
                .saturating_duration_since(acquisition_started_at),
        }
    }

    fn complete_at(
        self,
        transaction_ended_at: Instant,
        total_elapsed: Duration,
    ) -> CompletedTransactionTiming {
        let transaction_elapsed =
            transaction_ended_at.saturating_duration_since(self.transaction_started_at);
        let admitted_transaction_envelope = self
            .admitted_transaction_start_elapsed
            .saturating_add(transaction_elapsed);
        CompletedTransactionTiming {
            acquisition_elapsed: self.acquisition_elapsed,
            write_admission_wait_elapsed: total_elapsed
                .saturating_sub(self.acquisition_elapsed)
                .saturating_sub(admitted_transaction_envelope),
            transaction_elapsed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

pub(crate) struct SqliteTelemetry {
    operation: SqliteOperation,
    category: SqliteWorkloadCategory,
    access: SqliteAccessKind,
    collector: Option<SqliteWorkloadCollector>,
    started_at: Instant,
    outcome_recorded: AtomicBool,
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
        }
    }

    pub(crate) async fn observe_sqlx<T>(
        &self,
        phase: SqlitePhase,
        operation: impl std::future::Future<Output = Result<T, sqlx::Error>>,
    ) -> Result<T, sqlx::Error> {
        let phase_started_at = Instant::now();
        let result = operation.await;
        result.inspect_err(|error| {
            self.record_failure(phase, phase_started_at.elapsed(), error);
        })
    }

    pub(crate) async fn observe_db<T>(
        &self,
        phase: SqlitePhase,
        operation: impl std::future::Future<Output = DbResult<T>>,
    ) -> DbResult<T> {
        let phase_started_at = Instant::now();
        let result = operation.await;
        result.inspect_err(|error| {
            if let DbError::Sqlx(sqlx_error) = error {
                self.record_failure(phase, phase_started_at.elapsed(), sqlx_error);
            }
        })
    }

    pub(crate) async fn observe_pool_acquisition_sqlx<T>(
        &self,
        operation: impl std::future::Future<Output = Result<T, sqlx::Error>>,
    ) -> Result<(T, SuccessfulPoolAcquisitionTiming), sqlx::Error> {
        let acquisition_started_at = Instant::now();
        let value = self
            .observe_sqlx(SqlitePhase::TransactionAcquisition, operation)
            .await?;
        Ok((
            value,
            SuccessfulPoolAcquisitionTiming {
                acquisition_elapsed: acquisition_started_at.elapsed(),
            },
        ))
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
            operation,
        )
        .await
    }

    fn finish_successful_transaction(
        &self,
        timing: CompletedTransactionTiming,
        outcome: SqliteTransactionOutcome,
    ) {
        self.record_observation(SqliteObservation {
            completed_at_unix_micros: unix_now_micros(),
            category: self.category,
            access: self.access,
            outcome: SqliteOutcome::Success,
            latency: self.started_at.elapsed(),
            pool_wait: timing.acquisition_elapsed,
            write_admission_wait: timing.write_admission_wait_elapsed,
            writer_held: timing.transaction_elapsed,
            read_connection_time: Duration::ZERO,
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 1,
            read_concurrency: 0,
        });
        self.record_slow_success(timing, outcome);
    }

    async fn observe_transaction_completion_db<T>(
        &self,
        timing: SuccessfulTransactionTiming,
        phase: SqlitePhase,
        outcome: SqliteTransactionOutcome,
        operation: impl std::future::Future<Output = DbResult<T>>,
    ) -> DbResult<T> {
        let value = self.observe_db(phase, operation).await?;
        self.finish_successful_transaction(
            timing.complete_at(Instant::now(), self.started_at.elapsed()),
            outcome,
        );
        Ok(value)
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
        self.record_observation(SqliteObservation {
            completed_at_unix_micros: unix_now_micros(),
            category: self.category,
            access: self.access,
            outcome: classify_outcome(error),
            latency: self.started_at.elapsed(),
            pool_wait: if phase == SqlitePhase::TransactionAcquisition {
                phase_elapsed
            } else {
                Duration::ZERO
            },
            write_admission_wait: if phase == SqlitePhase::TransactionAcquisition {
                Duration::ZERO
            } else {
                self.started_at.elapsed().saturating_sub(phase_elapsed)
            },
            writer_held: Duration::ZERO,
            read_connection_time: Duration::ZERO,
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 0,
            read_concurrency: 0,
        });
    }

    fn record_observation(&self, observation: SqliteObservation) {
        if self.outcome_recorded.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(collector) = &self.collector {
            collector.record(observation);
        }
    }
}

fn unix_now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0)
}

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
        let origin = Instant::now();
        let retry_backoff = Duration::from_secs(5);
        let acquisition_started_at = origin + retry_backoff;
        let transaction_started_at = acquisition_started_at + Duration::from_millis(120);
        let timing = SuccessfulTransactionTiming::from_boundaries(
            acquisition_started_at,
            transaction_started_at,
        )
        .complete_at(
            transaction_started_at + Duration::from_millis(40),
            Duration::from_millis(160),
        );

        assert_eq!(timing.acquisition_elapsed, Duration::from_millis(120));
        assert_eq!(timing.transaction_elapsed, Duration::from_millis(40));
        assert_ne!(
            timing.transaction_elapsed,
            retry_backoff + Duration::from_millis(160)
        );
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
                SuccessfulTransactionTiming::from_boundaries(origin, origin),
                async { Err::<(), _>(DbError::Sqlx(sqlx::Error::PoolClosed)) },
            )
            .await;
        let rollback = telemetry
            .observe_rollback_db(
                SuccessfulTransactionTiming::from_boundaries(origin, origin),
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
        let acquisition_started_at = Instant::now() - Duration::from_millis(50);
        let transaction_started_at = acquisition_started_at + Duration::from_millis(20);

        telemetry
            .observe_commit_db(
                SuccessfulTransactionTiming::from_boundaries(
                    acquisition_started_at,
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
        assert_eq!(snapshot.totals[access][category].operation_count, 1);
        assert_eq!(
            snapshot.outcomes[access][category][SqliteOutcome::Success.index()],
            1
        );
        assert!(snapshot.totals[access][category].pool_wait_micros >= 20_000);
        assert!(snapshot.totals[access][category].writer_held_micros >= 30_000);
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
            Some("locator_delete")
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
                SuccessfulTransactionTiming::from_boundaries(origin, origin),
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
        assert_eq!(snapshot.totals[access][category].operation_count, 1);
        assert_eq!(
            snapshot.outcomes[access][category][SqliteOutcome::OtherFailure.index()],
            1
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
    async fn caller_owned_fts_transaction_records_first_source_write_failure() {
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
        let events = capture.events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].get("db_operation").map(String::as_str),
            Some("message.update_display_data")
        );
        assert_eq!(
            events[0].get("db_phase").map(String::as_str),
            Some("statement")
        );
        assert_eq!(
            events[0].get("db_sqlite_primary_code").map(String::as_str),
            Some("5")
        );
        assert!(!format!("{:?}", events[0]).contains("source payload sentinel"));
    }
}
