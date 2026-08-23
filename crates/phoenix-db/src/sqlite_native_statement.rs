use crate::sqlite_workload::{
    SqliteAccessKind, SqliteObservation, SqliteObservationCounting, SqliteWorkloadCategory,
    SqliteWorkloadCollector,
};
use libsqlite3_sys as ffi;
#[cfg(test)]
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqliteConnection;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::Arc;

const TRACE_MASK: u32 = ffi::SQLITE_TRACE_STMT | ffi::SQLITE_TRACE_PROFILE;
const CONTEXT_FUNCTION_NAME: &str = "__phoenix_native_statement_context";
const MAIN_DB_NAME: &[u8] = b"main\0";
const STATEMENT_CACHE_SIZE: usize = 256;

#[derive(Debug)]
pub(crate) struct NativeStatementCallbackContext {
    collector: SqliteWorkloadCollector,
    active_read_concurrency: u32,
    prepare_state: PrepareState,
    prepare_pending: bool,
    statement_metadata: [CachedStatementMetadata; STATEMENT_CACHE_SIZE],
    writer_admitted_at: Option<Instant>,
    current_writer_category: SqliteWorkloadCategory,
    #[cfg(test)]
    drop_counter: Option<Arc<AtomicUsize>>,
}

#[derive(Debug, Clone, Copy)]
struct PrepareState {
    category: SqliteWorkloadCategory,
    access: Option<SqliteAccessKind>,
}

impl PrepareState {
    const EMPTY: Self = Self {
        category: SqliteWorkloadCategory::Other,
        access: None,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CachedStatementMetadata {
    statement_identity: usize,
    category: SqliteWorkloadCategory,
    access: Option<SqliteAccessKind>,
}

impl CachedStatementMetadata {
    const EMPTY: Self = Self {
        statement_identity: 0,
        category: SqliteWorkloadCategory::Other,
        access: None,
    };
}

impl NativeStatementCallbackContext {
    fn new(collector: SqliteWorkloadCollector) -> Self {
        Self {
            collector,
            active_read_concurrency: 0,
            prepare_state: PrepareState::EMPTY,
            prepare_pending: false,
            statement_metadata: [CachedStatementMetadata::EMPTY; STATEMENT_CACHE_SIZE],
            writer_admitted_at: None,
            current_writer_category: SqliteWorkloadCategory::Other,
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
            prepare_state: PrepareState::EMPTY,
            prepare_pending: false,
            statement_metadata: [CachedStatementMetadata::EMPTY; STATEMENT_CACHE_SIZE],
            writer_admitted_at: None,
            current_writer_category: SqliteWorkloadCategory::Other,
            drop_counter: Some(drop_counter),
        }
    }

    fn reset_prepare_state(&mut self) {
        self.prepare_state = PrepareState::EMPTY;
    }

    fn note_prepare_metadata(
        &mut self,
        category: SqliteWorkloadCategory,
        access: Option<SqliteAccessKind>,
    ) {
        self.prepare_pending = true;
        self.prepare_state.category = category_precedence(self.prepare_state.category, category);
        self.prepare_state.access = access_precedence(self.prepare_state.access, access);
    }

    fn cache_statement_metadata(
        &mut self,
        statement_identity: usize,
        category: SqliteWorkloadCategory,
        access: Option<SqliteAccessKind>,
    ) {
        let slot = statement_cache_slot(statement_identity);
        let entry = &mut self.statement_metadata[slot];
        if entry.statement_identity == 0
            || entry.statement_identity == statement_identity
            || self.prepare_pending
        {
            *entry = CachedStatementMetadata {
                statement_identity,
                category,
                access,
            };
        } else {
            *entry = CachedStatementMetadata {
                statement_identity,
                category: SqliteWorkloadCategory::Other,
                access: None,
            };
            self.collector.record_classification_gap();
        }
    }

    fn lookup_statement_metadata(
        &self,
        statement_identity: usize,
    ) -> Option<CachedStatementMetadata> {
        let entry = self.statement_metadata[statement_cache_slot(statement_identity)];
        (entry.statement_identity == statement_identity).then_some(entry)
    }

    fn take_prepare_metadata_for_statement(
        &mut self,
        statement_identity: usize,
    ) -> CachedStatementMetadata {
        let metadata = CachedStatementMetadata {
            statement_identity,
            category: self.prepare_state.category,
            access: self.prepare_state.access,
        };
        self.reset_prepare_state();
        self.prepare_pending = false;
        self.cache_statement_metadata(statement_identity, metadata.category, metadata.access);
        metadata
    }

    fn note_writer_statement(
        &mut self,
        category: SqliteWorkloadCategory,
        txn_state: c_int,
    ) -> bool {
        if txn_state != ffi::SQLITE_TXN_WRITE {
            return false;
        }
        self.current_writer_category = category_precedence(self.current_writer_category, category);
        let first_admission = self.writer_admitted_at.is_none();
        if first_admission {
            self.writer_admitted_at = Some(Instant::now());
        }
        first_admission
    }

    fn finish_writer_transaction(&mut self) {
        let Some(writer_admitted_at) = self.writer_admitted_at.take() else {
            return;
        };
        self.collector.record_writer_occupancy(
            unix_now_micros(),
            self.current_writer_category,
            writer_admitted_at.elapsed(),
        );
        self.current_writer_category = SqliteWorkloadCategory::Other;
    }

    fn record_writer_gap(&mut self) {
        self.collector.record_writer_occupancy_gap();
        self.current_writer_category = SqliteWorkloadCategory::Other;
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
    let authorizer_rc =
        unsafe { ffi::sqlite3_set_authorizer(db, Some(authorizer_callback), context.cast()) };
    if authorizer_rc != ffi::SQLITE_OK {
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
            "sqlite3_set_authorizer failed: {authorizer_rc}"
        )));
    }
    let trace_rc =
        unsafe { ffi::sqlite3_trace_v2(db, TRACE_MASK, Some(profile_callback), context.cast()) };
    if trace_rc != ffi::SQLITE_OK {
        unsafe {
            ffi::sqlite3_trace_v2(db, 0, None, ptr::null_mut());
            ffi::sqlite3_set_authorizer(db, None, ptr::null_mut());
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

unsafe extern "C" fn authorizer_callback(
    context: *mut c_void,
    action_code: c_int,
    arg1: *const c_char,
    _arg2: *const c_char,
    _db_name: *const c_char,
    _trigger_or_view: *const c_char,
) -> c_int {
    if context.is_null() {
        return ffi::SQLITE_OK;
    }
    let context = unsafe { &mut *(context.cast::<NativeStatementCallbackContext>()) };
    if action_code == ffi::SQLITE_SELECT {
        return ffi::SQLITE_OK;
    }
    if let Some((category, access)) = classify_authorizer_action(action_code, arg1) {
        context.note_prepare_metadata(category, access);
    }
    ffi::SQLITE_OK
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
    let statement = statement.cast::<ffi::sqlite3_stmt>();
    let statement_identity = statement as usize;
    let readonly = unsafe { ffi::sqlite3_stmt_readonly(statement) } == 1;
    if trace == ffi::SQLITE_TRACE_STMT {
        let metadata = if context.prepare_pending {
            context.take_prepare_metadata_for_statement(statement_identity)
        } else {
            context
                .lookup_statement_metadata(statement_identity)
                .unwrap_or_else(|| context.take_prepare_metadata_for_statement(statement_identity))
        };
        let access = metadata.access.unwrap_or_else(|| {
            if readonly {
                SqliteAccessKind::Read
            } else {
                SqliteAccessKind::Write
            }
        });
        if access == SqliteAccessKind::Read {
            context.active_read_concurrency = context.collector.begin_native_read();
        }
        return 0;
    }
    if trace != ffi::SQLITE_TRACE_PROFILE || elapsed_nanos.is_null() {
        return 0;
    }
    let nanos = unsafe { *(elapsed_nanos.cast::<ffi::sqlite3_int64>()) };
    let latency = Duration::from_nanos(u64::try_from(nanos).unwrap_or_default());
    let metadata = context.lookup_statement_metadata(statement_identity);
    let access = metadata
        .and_then(|metadata| metadata.access)
        .unwrap_or_else(|| {
            if readonly {
                SqliteAccessKind::Read
            } else {
                SqliteAccessKind::Write
            }
        });
    let read_connection_time = if access == SqliteAccessKind::Read {
        latency
    } else {
        Duration::ZERO
    };
    let db = unsafe { ffi::sqlite3_db_handle(statement) };
    let category = metadata
        .map(|metadata| metadata.category)
        .unwrap_or(SqliteWorkloadCategory::Other);
    let txn_state = if db.is_null() {
        ffi::SQLITE_TXN_NONE
    } else {
        unsafe { ffi::sqlite3_txn_state(db, MAIN_DB_NAME.as_ptr().cast()) }
    };
    if txn_state == ffi::SQLITE_TXN_WRITE {
        let first_write_admission = context.note_writer_statement(category, txn_state);
        if first_write_admission && access == SqliteAccessKind::Write {
            context.collector.record_writer_occupancy_gap();
        }
    } else if txn_state == ffi::SQLITE_TXN_NONE {
        if context.writer_admitted_at.is_some() {
            context.finish_writer_transaction();
        } else if access == SqliteAccessKind::Write {
            context.record_writer_gap();
        }
    }
    let read_concurrency = context.active_read_concurrency;
    if access == SqliteAccessKind::Read {
        context.collector.end_native_read();
        context.active_read_concurrency = 0;
    }
    let observation = SqliteObservation {
        completed_at_unix_micros: unix_now_micros(),
        category,
        access,
        latency,
        pool_wait: Duration::ZERO,
        write_admission_wait: Duration::ZERO,
        writer_held: Duration::ZERO,
        read_connection_time,
        retry_count: 0,
        retry_backoff: Duration::ZERO,
        writer_concurrency: 0,
        read_concurrency,
        counting: SqliteObservationCounting::BaselineStatement,
    };
    context.collector.record(observation);
    0
}

fn statement_cache_slot(statement_identity: usize) -> usize {
    (statement_identity >> 3) & (STATEMENT_CACHE_SIZE - 1)
}

fn classify_authorizer_action(
    action_code: c_int,
    object_name: *const c_char,
) -> Option<(SqliteWorkloadCategory, Option<SqliteAccessKind>)> {
    match action_code {
        ffi::SQLITE_READ => Some((
            classify_schema_object(object_name),
            Some(SqliteAccessKind::Read),
        )),
        ffi::SQLITE_INSERT | ffi::SQLITE_UPDATE | ffi::SQLITE_DELETE => Some((
            classify_schema_object(object_name),
            Some(SqliteAccessKind::Write),
        )),
        ffi::SQLITE_TRANSACTION => Some((
            SqliteWorkloadCategory::Maintenance,
            classify_transaction_access(object_name),
        )),
        ffi::SQLITE_SAVEPOINT
        | ffi::SQLITE_PRAGMA
        | ffi::SQLITE_ANALYZE
        | ffi::SQLITE_ATTACH
        | ffi::SQLITE_DETACH
        | ffi::SQLITE_ALTER_TABLE
        | ffi::SQLITE_REINDEX
        | ffi::SQLITE_CREATE_TABLE
        | ffi::SQLITE_CREATE_INDEX
        | ffi::SQLITE_CREATE_TRIGGER
        | ffi::SQLITE_CREATE_VIEW
        | ffi::SQLITE_CREATE_VTABLE
        | ffi::SQLITE_DROP_TABLE
        | ffi::SQLITE_DROP_INDEX
        | ffi::SQLITE_DROP_TRIGGER
        | ffi::SQLITE_DROP_VIEW
        | ffi::SQLITE_DROP_VTABLE => Some((SqliteWorkloadCategory::Maintenance, None)),
        _ => None,
    }
}

fn classify_schema_object(object_name: *const c_char) -> SqliteWorkloadCategory {
    if object_name.is_null() {
        return SqliteWorkloadCategory::Other;
    }
    let Some(name) = c_string(object_name) else {
        return SqliteWorkloadCategory::Other;
    };
    if is_fts_name(name) {
        return SqliteWorkloadCategory::Fts;
    }
    if is_message_persistence_name(name) {
        return SqliteWorkloadCategory::MessagePersistence;
    }
    if is_workflow_name(name) {
        return SqliteWorkloadCategory::DurableWorkflows;
    }
    if is_runtime_state_name(name) {
        return SqliteWorkloadCategory::RuntimeState;
    }
    if is_pr_project_name(name) {
        return SqliteWorkloadCategory::PrProjectData;
    }
    if is_maintenance_name(name) {
        return SqliteWorkloadCategory::Maintenance;
    }
    SqliteWorkloadCategory::Other
}

fn is_fts_name(name: &str) -> bool {
    name == "message_fts"
        || name == "message_fts_data"
        || name == "message_fts_idx"
        || name == "message_fts_docsize"
        || name == "message_fts_config"
        || name == "message_fts_rows"
        || name.starts_with("idx_message_fts_")
}

fn is_message_persistence_name(name: &str) -> bool {
    matches!(
        name,
        "messages"
            | "message_files"
            | "message_images"
            | "steering_messages"
            | "steering_acceptance_receipts"
            | "steering_message_files"
            | "steering_message_images"
            | "turn_usage"
            | "llm_request_metrics"
    )
}

fn is_workflow_name(name: &str) -> bool {
    name == "workflows"
        || name.starts_with("workflow_")
        || name.starts_with("wake_")
        || name.starts_with("direct_turn_")
        || name.starts_with("durable_turn")
}

fn classify_transaction_access(object_name: *const c_char) -> Option<SqliteAccessKind> {
    let Some(name) = c_string(object_name) else {
        return None;
    };
    match name {
        "BEGIN" | "COMMIT" | "ROLLBACK" => Some(SqliteAccessKind::Write),
        _ => None,
    }
}

fn c_string<'a>(raw: *const c_char) -> Option<&'a str> {
    if raw.is_null() {
        return None;
    }
    (unsafe { CStr::from_ptr(raw) }).to_str().ok()
}

fn is_runtime_state_name(name: &str) -> bool {
    name == "conversations"
        || name.starts_with("conversation_creation_")
        || name == "continuation_dispatch_intents"
        || name == "startup_parent_actions"
        || name == "fork_proposals"
        || name == "chain_qa"
        || name == "share_tokens"
        || name == "auth_sessions"
        || name == "mcp_disabled_servers"
        || name == "mcp_oauth_tokens"
        || name == "sub_agent_personas"
        || name == "notification_settings"
        || name == "app_settings"
}

fn is_pr_project_name(name: &str) -> bool {
    name == "projects"
        || name == "git_repositories"
        || name.starts_with("git_repository_")
        || name == "work_scopes"
        || name.starts_with("work_scope_")
}

fn is_maintenance_name(name: &str) -> bool {
    name == "sqlite_schema"
        || name == "sqlite_master"
        || name == "sqlite_temp_schema"
        || name == "sqlite_temp_master"
        || name == "_sqlx_migrations"
        || name == "sqlite_sequence"
}

fn category_precedence(
    current: SqliteWorkloadCategory,
    incoming: SqliteWorkloadCategory,
) -> SqliteWorkloadCategory {
    if category_rank(incoming) < category_rank(current) {
        incoming
    } else {
        current
    }
}

const fn access_precedence(
    current: Option<SqliteAccessKind>,
    incoming: Option<SqliteAccessKind>,
) -> Option<SqliteAccessKind> {
    match (current, incoming) {
        (Some(SqliteAccessKind::Write), _) | (_, Some(SqliteAccessKind::Write)) => {
            Some(SqliteAccessKind::Write)
        }
        (Some(SqliteAccessKind::Read), _) | (_, Some(SqliteAccessKind::Read)) => {
            Some(SqliteAccessKind::Read)
        }
        (None, None) => None,
    }
}

const fn category_rank(category: SqliteWorkloadCategory) -> u8 {
    match category {
        SqliteWorkloadCategory::Fts => 0,
        SqliteWorkloadCategory::MessagePersistence => 1,
        SqliteWorkloadCategory::DurableWorkflows => 2,
        SqliteWorkloadCategory::RuntimeState => 3,
        SqliteWorkloadCategory::PrProjectData => 4,
        SqliteWorkloadCategory::Maintenance => 5,
        SqliteWorkloadCategory::Other => 6,
    }
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
    use crate::SqliteLatencyBin;
    use crate::SqliteOutcome;
    use crate::{Database, SqliteSnapshotWindow, SqliteWorkloadAggregateReport};
    use sqlx::sqlite::{SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
    use sqlx::{Connection, Execute, Executor, Statement};
    use std::str::FromStr;
    use tokio::sync::oneshot;
    use tokio::time::sleep;

    fn category_totals(
        report: &SqliteWorkloadAggregateReport,
        access: SqliteAccessKind,
        category: SqliteWorkloadCategory,
    ) -> crate::BucketCategoryTotals {
        report.totals[access.index()][category.index()]
    }

    fn report_now(collector: &SqliteWorkloadCollector) -> SqliteWorkloadAggregateReport {
        collector.aggregate_report(SqliteSnapshotWindow::OneHour, unix_now_micros())
    }

    async fn connect_raw_pool(
        path: &str,
        collector: SqliteWorkloadCollector,
        busy_timeout: Duration,
    ) -> sqlx::SqlitePool {
        let opts = SqliteConnectOptions::from_str(&format!("sqlite:{path}?mode=rwc"))
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(busy_timeout)
            .foreign_keys(true);
        SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect(move |conn, _meta| {
                let collector = collector.clone();
                Box::pin(async move { install_native_statement_baseline(conn, collector).await })
            })
            .connect_with(opts)
            .await
            .unwrap()
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

        let report = report_now(&collector);
        let writes = category_totals(
            &report,
            SqliteAccessKind::Write,
            SqliteWorkloadCategory::Maintenance,
        );
        let other_writes = category_totals(
            &report,
            SqliteAccessKind::Write,
            SqliteWorkloadCategory::Other,
        );
        let reads = category_totals(
            &report,
            SqliteAccessKind::Read,
            SqliteWorkloadCategory::Other,
        );
        assert!(
            writes.baseline_statement_count >= 1,
            "expected create baseline write"
        );
        assert!(
            other_writes.baseline_statement_count >= 1,
            "expected insert write"
        );
        assert!(
            reads.baseline_statement_count >= 1,
            "expected select baseline read"
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
    fn classifies_durable_turn_tables_as_durable_workflows() {
        let durable_turn_state = CString::new("durable_turn_state").unwrap();
        let durable_turn_events = CString::new("durable_turn_events").unwrap();

        assert_eq!(
            classify_schema_object(durable_turn_state.as_ptr()),
            SqliteWorkloadCategory::DurableWorkflows
        );
        assert_eq!(
            classify_schema_object(durable_turn_events.as_ptr()),
            SqliteWorkloadCategory::DurableWorkflows
        );
    }

    #[tokio::test]
    async fn classifies_representative_domain_tables() {
        let db = Database::open_in_memory().await.unwrap();
        sqlx::query("INSERT INTO messages (message_id, conversation_id, sequence_id, message_type, content, created_at) VALUES ('m1','c1',1,'user','{}','2026-01-01T00:00:00Z')")
            .execute(db.pool())
            .await
            .unwrap_err();
        sqlx::query("SELECT COUNT(*) FROM conversations")
            .fetch_one(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO projects (id, canonical_path, main_ref, created_at) VALUES ('p1','/tmp/p1','main','2026-01-01T00:00:00Z')")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO workflows (workflow_id, profile_kind, profile_version, runtime_acceptance_enabled, external_acceptance_enabled, version, generation, status, snapshot_codec_family, snapshot_codec_version, snapshot_payload, created_at, updated_at) VALUES (1,'wake',1,1,0,0,0,'Active','wake',1,X'00',1,1)")
            .execute(db.pool())
            .await
            .unwrap();
        let report =
            db.sqlite_workload_aggregate_report(SqliteSnapshotWindow::OneHour, unix_now_micros());
        assert!(
            report.totals[SqliteAccessKind::Read.index()]
                [SqliteWorkloadCategory::RuntimeState.index()]
            .baseline_statement_count
                >= 1
        );
        assert!(
            report.totals[SqliteAccessKind::Write.index()]
                [SqliteWorkloadCategory::PrProjectData.index()]
            .baseline_statement_count
                >= 1
        );
        assert!(
            report.totals[SqliteAccessKind::Write.index()]
                [SqliteWorkloadCategory::DurableWorkflows.index()]
            .baseline_statement_count
                >= 1
        );
    }

    #[test]
    fn mixed_statement_uses_fts_precedence() {
        let collector = SqliteWorkloadCollector::new();
        let mut context = NativeStatementCallbackContext::new(collector);
        context.note_prepare_metadata(SqliteWorkloadCategory::PrProjectData, None);
        context.note_prepare_metadata(SqliteWorkloadCategory::MessagePersistence, None);
        context.note_prepare_metadata(SqliteWorkloadCategory::Fts, None);

        assert_eq!(
            context.take_prepare_metadata_for_statement(8).category,
            SqliteWorkloadCategory::Fts
        );
    }

    #[test]
    fn nested_select_callbacks_preserve_outer_prepare_category() {
        let collector = SqliteWorkloadCollector::new();
        let mut context = NativeStatementCallbackContext::new(collector);
        context.note_prepare_metadata(SqliteWorkloadCategory::PrProjectData, None);
        context.note_prepare_metadata(SqliteWorkloadCategory::Other, None);

        assert_eq!(
            context.take_prepare_metadata_for_statement(9).category,
            SqliteWorkloadCategory::PrProjectData
        );
    }

    #[test]
    fn begin_commit_and_rollback_classify_as_write_access() {
        let begin = CString::new("BEGIN").unwrap();
        let commit = CString::new("COMMIT").unwrap();
        let rollback = CString::new("ROLLBACK").unwrap();

        assert_eq!(
            classify_authorizer_action(ffi::SQLITE_TRANSACTION, begin.as_ptr()),
            Some((
                SqliteWorkloadCategory::Maintenance,
                Some(SqliteAccessKind::Write)
            ))
        );
        assert_eq!(
            classify_authorizer_action(ffi::SQLITE_TRANSACTION, commit.as_ptr()),
            Some((
                SqliteWorkloadCategory::Maintenance,
                Some(SqliteAccessKind::Write)
            ))
        );
        assert_eq!(
            classify_authorizer_action(ffi::SQLITE_TRANSACTION, rollback.as_ptr()),
            Some((
                SqliteWorkloadCategory::Maintenance,
                Some(SqliteAccessKind::Write)
            ))
        );
    }

    #[test]
    fn new_prepare_generation_replaces_metadata_at_reused_pointer() {
        let collector = SqliteWorkloadCollector::new();
        let mut context = NativeStatementCallbackContext::new(collector);
        context.cache_statement_metadata(8, SqliteWorkloadCategory::PrProjectData, None);
        context.note_prepare_metadata(SqliteWorkloadCategory::Fts, Some(SqliteAccessKind::Read));

        let metadata = context.take_prepare_metadata_for_statement(8);
        assert_eq!(metadata.category, SqliteWorkloadCategory::Fts);
        assert_eq!(metadata.access, Some(SqliteAccessKind::Read));
        assert_eq!(context.lookup_statement_metadata(8), Some(metadata));
    }

    #[tokio::test]
    async fn cached_statement_reuse_keeps_category() {
        let collector = SqliteWorkloadCollector::new();
        let mut conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::from_str("sqlite::memory:").unwrap(),
        )
        .await
        .unwrap();
        install_native_statement_baseline(&mut conn, collector.clone())
            .await
            .unwrap();
        conn.execute("CREATE TABLE projects (id TEXT PRIMARY KEY)")
            .await
            .unwrap();
        let stmt = conn
            .prepare(sqlx::query::<sqlx::Sqlite>("SELECT id FROM projects").sql())
            .await
            .unwrap();
        stmt.query().fetch_all(&mut conn).await.unwrap();
        stmt.query().fetch_all(&mut conn).await.unwrap();
        let report = report_now(&collector);
        assert!(
            report.totals[SqliteAccessKind::Read.index()]
                [SqliteWorkloadCategory::PrProjectData.index()]
            .baseline_statement_count
                >= 2
        );
    }

    #[tokio::test]
    async fn unknown_schema_objects_fall_back_to_other() {
        let collector = SqliteWorkloadCollector::new();
        let mut conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::from_str("sqlite::memory:").unwrap(),
        )
        .await
        .unwrap();
        install_native_statement_baseline(&mut conn, collector.clone())
            .await
            .unwrap();
        conn.execute("CREATE TABLE mystery (id INTEGER PRIMARY KEY)")
            .await
            .unwrap();
        conn.execute("INSERT INTO mystery DEFAULT VALUES")
            .await
            .unwrap();
        let report = report_now(&collector);
        assert!(
            report.totals[SqliteAccessKind::Write.index()][SqliteWorkloadCategory::Other.index()]
                .baseline_statement_count
                >= 1
        );
    }

    #[test]
    fn bounded_cache_collision_increments_gap_and_degrades_to_other() {
        let collector = SqliteWorkloadCollector::new();
        let mut context = NativeStatementCallbackContext::new(collector.clone());
        let first = 8usize;
        let collision = first + (STATEMENT_CACHE_SIZE << 3);
        context.cache_statement_metadata(first, SqliteWorkloadCategory::Fts, None);
        context.cache_statement_metadata(collision, SqliteWorkloadCategory::DurableWorkflows, None);

        assert!(context.lookup_statement_metadata(first).is_none());
        assert_eq!(
            context
                .lookup_statement_metadata(collision)
                .map(|metadata| metadata.category),
            Some(SqliteWorkloadCategory::Other)
        );
        assert_eq!(report_now(&collector).classification_gap_count, 1);
    }

    #[test]
    fn native_profile_records_baseline_without_outcome_or_unmeasured_waits() {
        let collector = SqliteWorkloadCollector::new();
        collector.record(SqliteObservation {
            completed_at_unix_micros: unix_now_micros(),
            category: SqliteWorkloadCategory::Other,
            access: SqliteAccessKind::Write,
            latency: Duration::from_millis(3),
            pool_wait: Duration::from_millis(7),
            write_admission_wait: Duration::from_millis(11),
            writer_held: Duration::ZERO,
            read_connection_time: Duration::ZERO,
            retry_count: 0,
            retry_backoff: Duration::ZERO,
            writer_concurrency: 0,
            read_concurrency: 0,
            counting: SqliteObservationCounting::BaselineStatement,
        });

        let report = report_now(&collector);
        let access = SqliteAccessKind::Write.index();
        let category = SqliteWorkloadCategory::Other.index();
        let totals = report.totals[access][category];
        assert_eq!(operation_count(&report.outcomes[access][category]), 0);
        assert_eq!(totals.pool_wait_micros, 0);
        assert_eq!(totals.write_admission_wait_micros, 0);
        assert_eq!(
            report.pool_wait_histogram[access][category]
                [SqliteLatencyBin::from_duration(Duration::from_millis(7)).index()],
            0
        );
        assert_eq!(
            report.write_admission_wait_histogram[access][category]
                [SqliteLatencyBin::from_duration(Duration::from_millis(11)).index()],
            0
        );
        assert_eq!(
            report.latency_histogram[access][category]
                [SqliteLatencyBin::from_duration(Duration::from_millis(3)).index()],
            1
        );
    }

    #[tokio::test]
    async fn native_constraint_failure_does_not_fabricate_success_outcome() {
        let collector = SqliteWorkloadCollector::new();
        let mut conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::from_str("sqlite::memory:").unwrap(),
        )
        .await
        .unwrap();
        install_native_statement_baseline(&mut conn, collector.clone())
            .await
            .unwrap();
        sqlx::query("CREATE TABLE constrained (id INTEGER PRIMARY KEY, value TEXT UNIQUE)")
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO constrained (value) VALUES ('same')")
            .execute(&mut conn)
            .await
            .unwrap();
        let before = report_now(&collector);
        assert!(
            sqlx::query("INSERT INTO constrained (value) VALUES ('same')")
                .execute(&mut conn)
                .await
                .is_err()
        );
        let after = report_now(&collector);
        let success_before: u64 = before
            .outcomes
            .iter()
            .flat_map(|categories| categories.iter())
            .map(|outcomes| outcomes[SqliteOutcome::Success.index()])
            .sum();
        let success_after: u64 = after
            .outcomes
            .iter()
            .flat_map(|categories| categories.iter())
            .map(|outcomes| outcomes[SqliteOutcome::Success.index()])
            .sum();
        assert_eq!(success_after, success_before);
        assert!(
            after
                .totals
                .iter()
                .flat_map(|categories| categories.iter())
                .map(|totals| totals.baseline_statement_count)
                .sum::<u64>()
                >= before
                    .totals
                    .iter()
                    .flat_map(|categories| categories.iter())
                    .map(|totals| totals.baseline_statement_count)
                    .sum::<u64>()
        );
    }

    #[tokio::test]
    async fn transaction_control_profiles_as_write_access() {
        let collector = SqliteWorkloadCollector::new();
        let mut conn = SqliteConnection::connect_with(
            &SqliteConnectOptions::from_str("sqlite::memory:").unwrap(),
        )
        .await
        .unwrap();
        install_native_statement_baseline(&mut conn, collector.clone())
            .await
            .unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::query("ROLLBACK").execute(&mut conn).await.unwrap();
        let report = report_now(&collector);
        let write_baseline: u64 = report.totals[SqliteAccessKind::Write.index()]
            .iter()
            .map(|totals| totals.baseline_statement_count)
            .sum();
        assert!(write_baseline >= 2);
    }

    #[tokio::test]
    async fn immediate_holder_gets_occupancy_and_busy_victim_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer_occupancy_busy.db");
        let holder_collector = SqliteWorkloadCollector::new();
        let victim_collector = SqliteWorkloadCollector::new();
        let holder = connect_raw_pool(
            path.to_str().unwrap(),
            holder_collector.clone(),
            Duration::from_secs(5),
        )
        .await;
        let victim = connect_raw_pool(
            path.to_str().unwrap(),
            victim_collector.clone(),
            Duration::ZERO,
        )
        .await;
        sqlx::query("CREATE TABLE messages (id INTEGER PRIMARY KEY, body TEXT NOT NULL)")
            .execute(&holder)
            .await
            .unwrap();

        sqlx::query("BEGIN IMMEDIATE")
            .execute(&holder)
            .await
            .unwrap();
        sqlx::query("INSERT INTO messages (body) VALUES ('holder')")
            .execute(&holder)
            .await
            .unwrap();
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let holder_clone = holder.clone();
        let holder_task = tokio::spawn(async move {
            started_tx.send(()).ok();
            // test-timing-allow: elapsed writer occupancy is the behavior under test
            sleep(Duration::from_millis(25)).await;
            let _ = release_rx.await;
            sqlx::query("ROLLBACK")
                .execute(&holder_clone)
                .await
                .unwrap();
        });
        let _ = started_rx.await;
        let victim_gap_before = report_now(&victim_collector).writer_occupancy_gap_count;
        let err = sqlx::query("INSERT INTO messages (body) VALUES ('victim')")
            .execute(&victim)
            .await
            .unwrap_err();
        let code = err
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .and_then(|code| code.parse::<i32>().ok())
            .unwrap_or_default();
        assert_eq!(code & 0xff, ffi::SQLITE_BUSY);
        release_tx.send(()).ok();
        holder_task.await.unwrap();

        let holder_report = report_now(&holder_collector);
        let victim_report = report_now(&victim_collector);
        let holder_totals = category_totals(
            &holder_report,
            SqliteAccessKind::Write,
            SqliteWorkloadCategory::MessagePersistence,
        );
        assert!(holder_totals.writer_held_micros > 0);
        assert!(victim_report.writer_occupancy_gap_count >= victim_gap_before);
        assert_eq!(
            category_totals(
                &victim_report,
                SqliteAccessKind::Write,
                SqliteWorkloadCategory::MessagePersistence,
            )
            .writer_held_micros,
            0
        );
    }

    #[tokio::test]
    async fn deferred_transaction_write_records_gap_and_occupancy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer_occupancy_deferred.db");
        let collector = SqliteWorkloadCollector::new();
        let pool = connect_raw_pool(
            path.to_str().unwrap(),
            collector.clone(),
            Duration::from_secs(5),
        )
        .await;
        let mut connection = pool.acquire().await.unwrap();
        sqlx::query("CREATE TABLE messages (id INTEGER PRIMARY KEY, body TEXT NOT NULL)")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("BEGIN DEFERRED")
            .execute(&mut *connection)
            .await
            .unwrap();
        // test-timing-allow: elapsed writer occupancy is the behavior under test
        sleep(Duration::from_millis(10)).await;
        sqlx::query("INSERT INTO messages (body) VALUES ('deferred')")
            .execute(&mut *connection)
            .await
            .unwrap();
        // test-timing-allow: elapsed writer occupancy is the behavior under test
        sleep(Duration::from_millis(10)).await;
        sqlx::query("COMMIT")
            .execute(&mut *connection)
            .await
            .unwrap();

        let report = report_now(&collector);
        let totals = category_totals(
            &report,
            SqliteAccessKind::Write,
            SqliteWorkloadCategory::MessagePersistence,
        );
        assert!(totals.writer_held_micros > 0);
        assert!(report.writer_occupancy_gap_count >= 1);
    }

    #[tokio::test]
    async fn autocommit_write_records_gap_without_occupancy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer_occupancy_autocommit.db");
        let collector = SqliteWorkloadCollector::new();
        let pool = connect_raw_pool(
            path.to_str().unwrap(),
            collector.clone(),
            Duration::from_secs(5),
        )
        .await;
        sqlx::query("CREATE TABLE messages (id INTEGER PRIMARY KEY, body TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO messages (body) VALUES ('auto')")
            .execute(&pool)
            .await
            .unwrap();

        let report = report_now(&collector);
        let totals = category_totals(
            &report,
            SqliteAccessKind::Write,
            SqliteWorkloadCategory::MessagePersistence,
        );
        assert_eq!(totals.writer_held_micros, 0);
        assert!(report.writer_occupancy_gap_count >= 1);
    }

    #[tokio::test]
    async fn nested_fts_message_precedence_uses_fts_for_transaction_occupancy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer_occupancy_precedence.db");
        let collector = SqliteWorkloadCollector::new();
        let pool = connect_raw_pool(
            path.to_str().unwrap(),
            collector.clone(),
            Duration::from_secs(5),
        )
        .await;
        sqlx::query("CREATE TABLE messages (id INTEGER PRIMARY KEY, body TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE VIRTUAL TABLE message_fts USING fts5(body)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE message_fts_rows (fts_rowid INTEGER PRIMARY KEY, conversation_id TEXT, message_id TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("BEGIN IMMEDIATE").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO messages (body) VALUES ('message')")
            .execute(&pool)
            .await
            .unwrap();
        // test-timing-allow: elapsed writer occupancy is the behavior under test
        sleep(Duration::from_millis(5)).await;
        sqlx::query("INSERT INTO message_fts (body) VALUES ('fts')")
            .execute(&pool)
            .await
            .unwrap();
        // test-timing-allow: elapsed writer occupancy is the behavior under test
        sleep(Duration::from_millis(5)).await;
        sqlx::query("COMMIT").execute(&pool).await.unwrap();
        let report = report_now(&collector);
        let fts = category_totals(
            &report,
            SqliteAccessKind::Write,
            SqliteWorkloadCategory::Fts,
        );
        let msg = category_totals(
            &report,
            SqliteAccessKind::Write,
            SqliteWorkloadCategory::MessagePersistence,
        );
        assert!(fts.writer_held_micros > 0);
        assert_eq!(msg.writer_held_micros, 0);
    }

    #[tokio::test]
    async fn total_writer_occupancy_does_not_exceed_covered_wall_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer_occupancy_bound.db");
        let collector = SqliteWorkloadCollector::new();
        let pool = connect_raw_pool(
            path.to_str().unwrap(),
            collector.clone(),
            Duration::from_secs(5),
        )
        .await;
        sqlx::query("CREATE TABLE messages (id INTEGER PRIMARY KEY, body TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("BEGIN IMMEDIATE").execute(&pool).await.unwrap();
        // test-timing-allow: elapsed writer occupancy is the behavior under test
        sleep(Duration::from_millis(20)).await;
        sqlx::query("INSERT INTO messages (body) VALUES ('bounded')")
            .execute(&pool)
            .await
            .unwrap();
        // test-timing-allow: elapsed writer occupancy is the behavior under test
        sleep(Duration::from_millis(20)).await;
        sqlx::query("ROLLBACK").execute(&pool).await.unwrap();

        let report = report_now(&collector);
        let total_writer_held: u64 = SqliteWorkloadCategory::ALL
            .iter()
            .map(|category| {
                report.totals[SqliteAccessKind::Write.index()][category.index()].writer_held_micros
            })
            .sum();
        assert!(total_writer_held <= report.covered_uptime_micros);
    }

    #[test]
    fn source_never_inspects_sql_text_or_expanded_sql() {
        let source = include_str!("sqlite_native_statement.rs");
        let disallow_sql = ["sqlite3_", "sql", "("].concat();
        let disallow_expanded_sql = ["sqlite3_", "expanded_sql", "("].concat();
        assert!(!source.contains(&disallow_sql));
        assert!(!source.contains(&disallow_expanded_sql));
        let forbidden_owned_conversion = ["CString", "::", "from_raw"].concat();
        assert!(!source.contains(&forbidden_owned_conversion));
    }
}
