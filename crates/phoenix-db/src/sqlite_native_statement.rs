use crate::sqlite_workload::{
    SqliteAccessKind, SqliteObservation, SqliteOutcome, SqliteWorkloadCategory,
    SqliteWorkloadCollector,
};
use libsqlite3_sys as ffi;
use sqlx::sqlite::SqliteConnection;
use std::ffi::CString;
use std::os::raw::{c_int, c_void};
use std::ptr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::Arc;

const TRACE_MASK: u32 = (ffi::SQLITE_TRACE_STMT | ffi::SQLITE_TRACE_PROFILE) as u32;
const CONTEXT_FUNCTION_NAME: &str = "__phoenix_native_statement_context";

#[derive(Debug)]
pub(crate) struct NativeStatementCallbackContext {
    collector: SqliteWorkloadCollector,
    active_read_concurrency: u32,
    #[cfg(test)]
    drop_counter: Option<Arc<AtomicUsize>>,
}

impl NativeStatementCallbackContext {
    fn new(collector: SqliteWorkloadCollector) -> Self {
        Self {
            collector,
            active_read_concurrency: 0,
            #[cfg(test)]
            drop_counter: None,
        }
    }

    #[cfg(test)]
    fn with_drop_counter(
        collector: SqliteWorkloadCollector,
        drop_counter: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            collector,
            active_read_concurrency: 0,
            drop_counter: Some(drop_counter),
        }
    }
}

impl Drop for NativeStatementCallbackContext {
    fn drop(&mut self) {
        #[cfg(test)]
        if let Some(counter) = &self.drop_counter {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    }
}

pub(crate) async fn install_native_statement_baseline(
    conn: &mut SqliteConnection,
    collector: SqliteWorkloadCollector,
) -> Result<(), sqlx::Error> {
    install_native_statement_baseline_with_context(
        conn,
        NativeStatementCallbackContext::new(collector),
    )
    .await
}

#[cfg(test)]
pub(crate) async fn install_native_statement_baseline_with_drop_counter(
    conn: &mut SqliteConnection,
    collector: SqliteWorkloadCollector,
    drop_counter: Arc<AtomicUsize>,
) -> Result<(), sqlx::Error> {
    install_native_statement_baseline_with_context(
        conn,
        NativeStatementCallbackContext::with_drop_counter(collector, drop_counter),
    )
    .await
}

async fn install_native_statement_baseline_with_context(
    conn: &mut SqliteConnection,
    context: NativeStatementCallbackContext,
) -> Result<(), sqlx::Error> {
    let mut locked = conn.lock_handle().await?;
    let db = locked.as_raw_handle().as_ptr();
    let context = Box::into_raw(Box::new(context));
    let name = CString::new(CONTEXT_FUNCTION_NAME).expect("context function name contains no nul");
    let create_rc = unsafe {
        ffi::sqlite3_create_function_v2(
            db,
            name.as_ptr(),
            0,
            ffi::SQLITE_UTF8,
            context.cast(),
            Some(noop_function),
            None,
            None,
            Some(destroy_context),
        )
    };
    if create_rc != ffi::SQLITE_OK {
        return Err(sqlx::Error::Protocol(format!(
            "sqlite3_create_function_v2 failed: {create_rc}"
        )));
    }
    let trace_rc =
        unsafe { ffi::sqlite3_trace_v2(db, TRACE_MASK, Some(profile_callback), context.cast()) };
    if trace_rc != ffi::SQLITE_OK {
        unsafe {
            ffi::sqlite3_create_function_v2(
                db,
                name.as_ptr(),
                0,
                ffi::SQLITE_UTF8,
                ptr::null_mut(),
                None,
                None,
                None,
                None,
            );
        }
        return Err(sqlx::Error::Protocol(format!(
            "sqlite3_trace_v2 failed: {trace_rc}"
        )));
    }
    Ok(())
}

unsafe extern "C" fn profile_callback(
    trace: u32,
    context: *mut c_void,
    statement: *mut c_void,
    elapsed_nanos: *mut c_void,
) -> c_int {
    if context.is_null() || statement.is_null() {
        return 0;
    }
    let context = unsafe { &mut *(context.cast::<NativeStatementCallbackContext>()) };
    let readonly = unsafe { ffi::sqlite3_stmt_readonly(statement.cast()) } == 1;
    if trace == ffi::SQLITE_TRACE_STMT as u32 {
        if readonly {
            context.active_read_concurrency = context.collector.begin_native_read();
        }
        return 0;
    }
    if trace != ffi::SQLITE_TRACE_PROFILE as u32 || elapsed_nanos.is_null() {
        return 0;
    }
    let nanos = unsafe { *(elapsed_nanos.cast::<ffi::sqlite3_int64>()) };
    let latency = Duration::from_nanos(u64::try_from(nanos).unwrap_or_default());
    let access = if readonly {
        SqliteAccessKind::Read
    } else {
        SqliteAccessKind::Write
    };
    let read_connection_time = if readonly { latency } else { Duration::ZERO };
    let db = unsafe { ffi::sqlite3_db_handle(statement.cast()) };
    let primary_code = if db.is_null() {
        ffi::SQLITE_ERROR
    } else {
        (unsafe { ffi::sqlite3_errcode(db) }) & 0xff
    };
    let outcome = match primary_code {
        ffi::SQLITE_OK | ffi::SQLITE_ROW | ffi::SQLITE_DONE => SqliteOutcome::Success,
        ffi::SQLITE_BUSY => SqliteOutcome::Busy,
        ffi::SQLITE_LOCKED => SqliteOutcome::Locked,
        _ => SqliteOutcome::OtherFailure,
    };
    let read_concurrency = context.active_read_concurrency;
    if readonly {
        context.collector.end_native_read();
        context.active_read_concurrency = 0;
    }
    let observation = SqliteObservation {
        completed_at_unix_micros: unix_now_micros(),
        category: SqliteWorkloadCategory::Other,
        access,
        outcome,
        latency,
        pool_wait: Duration::ZERO,
        write_admission_wait: Duration::ZERO,
        writer_held: Duration::ZERO,
        read_connection_time,
        retry_count: 0,
        retry_backoff: Duration::ZERO,
        writer_concurrency: 0,
        read_concurrency,
        baseline_statement_count: 1,
    };
    context.collector.record(observation);
    0
}

unsafe extern "C" fn noop_function(
    _context: *mut ffi::sqlite3_context,
    _argc: c_int,
    _argv: *mut *mut ffi::sqlite3_value,
) {
}

unsafe extern "C" fn destroy_context(context: *mut c_void) {
    if !context.is_null() {
        unsafe {
            drop(Box::from_raw(
                context.cast::<NativeStatementCallbackContext>(),
            ));
        }
    }
}

fn unix_now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SqliteSnapshotWindow, SqliteWorkloadAggregateReport};
    use sqlx::sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
    };
    use std::str::FromStr;

    fn other_totals(
        report: &SqliteWorkloadAggregateReport,
        access: SqliteAccessKind,
    ) -> crate::BucketCategoryTotals {
        report.totals[access.index()][SqliteWorkloadCategory::Other.index()]
    }

    #[tokio::test]
    async fn native_statement_profile_records_read_and_write_baseline() {
        let collector = SqliteWorkloadCollector::new();
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect({
                let collector = collector.clone();
                move |conn, _meta| {
                    let collector = collector.clone();
                    Box::pin(
                        async move { install_native_statement_baseline(conn, collector).await },
                    )
                }
            })
            .connect_with(opts)
            .await
            .unwrap();

        sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t (v) VALUES ('x')")
            .execute(&pool)
            .await
            .unwrap();
        let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM t")
            .fetch_one(&pool)
            .await
            .unwrap();

        let report = collector.aggregate_report(SqliteSnapshotWindow::OneHour, unix_now_micros());
        let writes = other_totals(&report, SqliteAccessKind::Write);
        let reads = other_totals(&report, SqliteAccessKind::Read);
        assert!(
            writes.baseline_statement_count >= 2,
            "expected create + insert baseline writes, got {writes:?}"
        );
        assert!(
            reads.baseline_statement_count >= 1,
            "expected select baseline read, got {reads:?}"
        );
        assert_eq!(writes.writer_held_micros, 0);
        assert_eq!(writes.write_admission_wait_micros, 0);
        assert_eq!(reads.write_admission_wait_micros, 0);
        assert!(reads.read_connection_micros >= reads.latency_micros.saturating_sub(5_000));
    }

    #[tokio::test]
    async fn callback_context_drops_when_connection_closes() {
        let collector = SqliteWorkloadCollector::new();
        let drops = Arc::new(AtomicUsize::new(0));
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect({
                let collector = collector.clone();
                let drops = drops.clone();
                move |conn, _meta| {
                    let collector = collector.clone();
                    let drops = drops.clone();
                    Box::pin(async move {
                        install_native_statement_baseline_with_drop_counter(conn, collector, drops)
                            .await
                    })
                }
            })
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::query("SELECT 1").execute(&pool).await.unwrap();
        pool.close().await;
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn source_never_inspects_sql_text_or_expanded_sql() {
        let source = include_str!("sqlite_native_statement.rs");
        let disallow_sql = ["sqlite3_", "sql", "("].concat();
        let disallow_expanded_sql = ["sqlite3_", "expanded_sql", "("].concat();
        assert!(!source.contains(&disallow_sql));
        assert!(!source.contains(&disallow_expanded_sql));
    }
}
