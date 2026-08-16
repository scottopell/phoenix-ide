use crate::{DbError, DbResult};
use std::time::{Duration, Instant};
use tracing::field;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SqliteOperation {
    DirectTurnTerminalSettlement,
    FtsDeleteConversation,
    FtsDeleteMessage,
    FtsHideMessage,
    FtsIndexMessage,
    FtsReconcileUpsert,
    FtsUpsert,
}

impl SqliteOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::DirectTurnTerminalSettlement => "direct_turn.terminal_settlement",
            Self::FtsDeleteConversation => "fts.delete_conversation",
            Self::FtsDeleteMessage => "fts.delete_message",
            Self::FtsHideMessage => "fts.hide_message",
            Self::FtsIndexMessage => "fts.index_message",
            Self::FtsReconcileUpsert => "fts.reconcile_upsert",
            Self::FtsUpsert => "fts.upsert",
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
    started_at: Instant,
}

impl SqliteTelemetry {
    pub(crate) fn new(operation: SqliteOperation) -> Self {
        Self {
            operation,
            started_at: Instant::now(),
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
            db.elapsed_ms = elapsed_ms,
            db.phase_elapsed_ms = phase_elapsed_ms,
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
                db_error_kind = error_kind,
                db_elapsed_ms = elapsed_ms,
                db_phase_elapsed_ms = phase_elapsed_ms,
                "SQLite operation failed without a database result code"
            );
        }
    }
}

fn elapsed_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
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
    use super::test_support::EventCapture;
    use super::*;
    use crate::retrieval::fts_upsert;
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
    fn telemetry_vocabulary_is_bounded() {
        assert_eq!(
            SqliteOperation::DirectTurnTerminalSettlement.as_str(),
            "direct_turn.terminal_settlement"
        );
        assert_eq!(SqliteOperation::FtsUpsert.as_str(), "fts.upsert");
        assert_eq!(SqlitePhase::LocatorLookup.as_str(), "locator_lookup");
        assert_eq!(SqlitePhase::Commit.as_str(), "commit");
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
            Some("fts.delete_conversation")
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
            Some("fts.index_message")
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
