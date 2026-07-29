use libsqlite3_sys as ffi;
use serde::Serialize;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::time::{Duration, Instant};

const MAX_ROWS: usize = 200;
const MAX_BYTES: usize = 64 * 1024;
const MAX_DURATION: Duration = Duration::from_millis(750);
const MAX_SQL_BYTES: usize = 16 * 1024;
const MAX_COLUMNS: c_int = 64;
const PROGRESS_OPS: c_int = 1_000;

#[derive(Debug, thiserror::Error)]
pub enum CoordinatorQueryError {
    #[error("query contains a NUL byte")]
    InvalidQuery,
    #[error("database path contains a NUL byte")]
    InvalidPath,
    #[error("query denied by Coordinator read policy: {0}")]
    Denied(String),
    #[error("query must contain exactly one read-only statement")]
    MultipleStatements,
    #[error("query exceeded its execution budget")]
    BudgetExceeded,
    #[error("SQLite {0}")]
    Sqlite(CoordinatorSqliteDiagnostic),
    #[error("query worker failed")]
    WorkerFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorSqliteDiagnostic {
    pub phase: CoordinatorSqlitePhase,
    pub primary_code: c_int,
    pub extended_code: c_int,
    pub symbolic_code: &'static str,
    pub message: String,
    pub error_offset: Option<c_int>,
}

impl std::fmt::Display for CoordinatorSqliteDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} failed: {} ({}; primary_code={}; extended_code={}",
            self.phase, self.message, self.symbolic_code, self.primary_code, self.extended_code
        )?;
        if let Some(offset) = self.error_offset {
            write!(formatter, "; error_offset={offset}")?;
        }
        formatter.write_str(")")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinatorSqlitePhase {
    Open,
    Prepare,
    Execute,
}

impl std::fmt::Display for CoordinatorSqlitePhase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Open => "open",
            Self::Prepare => "prepare",
            Self::Execute => "execute",
        })
    }
}

#[derive(Debug, Serialize)]
pub struct CoordinatorQueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<CoordinatorCell>>,
    pub truncated: bool,
    pub row_limit: usize,
    pub byte_limit: usize,
    pub elapsed_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CoordinatorCell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob { bytes: usize },
}

struct AuthorizerState {
    denied: Option<String>,
}

struct ProgressState {
    deadline: Instant,
}

/// Execute one engine-authorized, bounded read statement.
///
/// # Errors
///
/// Returns a policy, budget, path, or `SQLite` execution error.
#[allow(clippy::too_many_lines)]
pub fn execute_coordinator_query(
    path: &str,
    sql: &str,
) -> Result<CoordinatorQueryResult, CoordinatorQueryError> {
    if sql.len() > MAX_SQL_BYTES {
        return Err(CoordinatorQueryError::BudgetExceeded);
    }
    let path = CString::new(path).map_err(|_| CoordinatorQueryError::InvalidPath)?;
    let sql = CString::new(sql).map_err(|_| CoordinatorQueryError::InvalidQuery)?;
    let started = Instant::now();
    let mut db = ptr::null_mut();
    let open_flags = ffi::SQLITE_OPEN_READONLY | ffi::SQLITE_OPEN_NOMUTEX;
    let rc = unsafe { ffi::sqlite3_open_v2(path.as_ptr(), &raw mut db, open_flags, ptr::null()) };
    if rc != ffi::SQLITE_OK {
        let diagnostic = sqlite_diagnostic(db, rc, CoordinatorSqlitePhase::Open);
        if !db.is_null() {
            unsafe { ffi::sqlite3_close(db) };
        }
        return Err(CoordinatorQueryError::Sqlite(diagnostic));
    }
    let mut connection = ConnectionGuard(db);
    let mut authorizer = Box::new(AuthorizerState { denied: None });
    let mut progress = Box::new(ProgressState {
        deadline: started + MAX_DURATION,
    });
    unsafe {
        ffi::sqlite3_extended_result_codes(connection.0, 1);
        ffi::sqlite3_limit(connection.0, ffi::SQLITE_LIMIT_COLUMN, MAX_COLUMNS);
        ffi::sqlite3_set_authorizer(connection.0, Some(authorize), (&raw mut *authorizer).cast());
        ffi::sqlite3_progress_handler(
            connection.0,
            PROGRESS_OPS,
            Some(check_progress),
            (&raw mut *progress).cast(),
        );
    }

    let mut statement = ptr::null_mut();
    let mut tail = ptr::null();
    let rc = unsafe {
        ffi::sqlite3_prepare_v2(
            connection.0,
            sql.as_ptr(),
            -1,
            &raw mut statement,
            &raw mut tail,
        )
    };
    if rc != ffi::SQLITE_OK {
        return Err(classify_error(
            &authorizer,
            connection.0,
            rc,
            CoordinatorSqlitePhase::Prepare,
        ));
    }
    let statement = StatementGuard(statement);
    if statement.0.is_null() || tail_has_statement(tail) {
        return Err(CoordinatorQueryError::MultipleStatements);
    }
    if unsafe { ffi::sqlite3_stmt_readonly(statement.0) } != 1 {
        return Err(CoordinatorQueryError::Denied(
            "statement is not read-only".to_string(),
        ));
    }

    let column_count = unsafe { ffi::sqlite3_column_count(statement.0) };
    let columns = (0..column_count)
        .map(|index| unsafe { c_string(ffi::sqlite3_column_name(statement.0, index)) })
        .collect::<Vec<_>>();
    let mut rows = Vec::new();
    let mut bytes = columns.iter().map(String::len).sum::<usize>();
    let mut truncated = false;
    loop {
        let rc = unsafe { ffi::sqlite3_step(statement.0) };
        match rc {
            ffi::SQLITE_ROW => {
                if rows.len() >= MAX_ROWS {
                    truncated = true;
                    break;
                }
                let mut row = Vec::with_capacity(usize::try_from(column_count).unwrap_or_default());
                for index in 0..column_count {
                    let cell = read_cell(statement.0, index);
                    bytes = bytes.saturating_add(cell_size(&cell));
                    if bytes > MAX_BYTES {
                        truncated = true;
                        break;
                    }
                    row.push(cell);
                }
                if truncated {
                    break;
                }
                rows.push(row);
            }
            ffi::SQLITE_DONE => break,
            ffi::SQLITE_INTERRUPT => return Err(CoordinatorQueryError::BudgetExceeded),
            _ => {
                return Err(classify_error(
                    &authorizer,
                    connection.0,
                    rc,
                    CoordinatorSqlitePhase::Execute,
                ));
            }
        }
    }
    unsafe {
        ffi::sqlite3_progress_handler(connection.0, 0, None, ptr::null_mut());
        ffi::sqlite3_set_authorizer(connection.0, None, ptr::null_mut());
    }
    drop(statement);
    connection.close();
    let mut result = CoordinatorQueryResult {
        columns,
        rows,
        truncated,
        row_limit: MAX_ROWS,
        byte_limit: MAX_BYTES,
        elapsed_ms: started.elapsed().as_millis(),
    };
    while serde_json::to_vec(&result).map_or(usize::MAX, |json| json.len()) > MAX_BYTES {
        if result.rows.pop().is_none() {
            return Err(CoordinatorQueryError::BudgetExceeded);
        }
        result.truncated = true;
    }
    Ok(result)
}

unsafe extern "C" fn authorize(
    user_data: *mut c_void,
    action: c_int,
    arg1: *const c_char,
    arg2: *const c_char,
    _database: *const c_char,
    _trigger: *const c_char,
) -> c_int {
    let state = unsafe { &mut *user_data.cast::<AuthorizerState>() };
    let object = unsafe { optional_c_string(arg1) };
    let detail = unsafe { optional_c_string(arg2) };
    let allowed = match action {
        ffi::SQLITE_SELECT | ffi::SQLITE_RECURSIVE => true,
        ffi::SQLITE_READ => object.as_deref().is_some_and(object_allowed),
        ffi::SQLITE_FUNCTION => detail.as_deref().is_some_and(function_allowed),
        _ => false,
    };
    if allowed {
        ffi::SQLITE_OK
    } else {
        state.denied = Some(format!(
            "operation {action} on {}{}",
            object.as_deref().unwrap_or("unknown object"),
            detail
                .as_deref()
                .map(|value| format!("/{value}"))
                .unwrap_or_default()
        ));
        ffi::SQLITE_DENY
    }
}

unsafe extern "C" fn check_progress(user_data: *mut c_void) -> c_int {
    let state = unsafe { &*user_data.cast::<ProgressState>() };
    i32::from(Instant::now() >= state.deadline)
}

fn object_allowed(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !name.starts_with("sqlite_") && !name.starts_with("message_fts_") && name != "message_fts"
}

fn function_allowed(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "abs"
            | "avg"
            | "coalesce"
            | "concat"
            | "concat_ws"
            | "count"
            | "date"
            | "datetime"
            | "format"
            | "glob"
            | "hex"
            | "ifnull"
            | "iif"
            | "instr"
            | "json"
            | "json_array"
            | "json_array_length"
            | "json_extract"
            | "json_object"
            | "json_quote"
            | "json_type"
            | "json_valid"
            | "julianday"
            | "length"
            | "like"
            | "lower"
            | "ltrim"
            | "max"
            | "min"
            | "nullif"
            | "printf"
            | "quote"
            | "replace"
            | "round"
            | "rtrim"
            | "sign"
            | "strftime"
            | "substr"
            | "substring"
            | "sum"
            | "time"
            | "total"
            | "trim"
            | "typeof"
            | "unhex"
            | "unicode"
            | "unixepoch"
            | "upper"
            | "zeroblob"
    )
}

fn tail_has_statement(mut tail: *const c_char) -> bool {
    if tail.is_null() {
        return false;
    }
    unsafe {
        while *tail != 0 {
            let byte = tail.cast::<u8>().read();
            if !byte.is_ascii_whitespace() && byte != b';' {
                return true;
            }
            tail = tail.add(1);
        }
    }
    false
}

fn classify_error(
    authorizer: &AuthorizerState,
    db: *mut ffi::sqlite3,
    rc: c_int,
    phase: CoordinatorSqlitePhase,
) -> CoordinatorQueryError {
    if rc == ffi::SQLITE_AUTH || authorizer.denied.is_some() {
        CoordinatorQueryError::Denied(
            authorizer
                .denied
                .clone()
                .unwrap_or_else(|| "unauthorized operation".to_string()),
        )
    } else if rc == ffi::SQLITE_INTERRUPT {
        CoordinatorQueryError::BudgetExceeded
    } else {
        CoordinatorQueryError::Sqlite(sqlite_diagnostic(db, rc, phase))
    }
}

fn sqlite_diagnostic(
    db: *mut ffi::sqlite3,
    rc: c_int,
    phase: CoordinatorSqlitePhase,
) -> CoordinatorSqliteDiagnostic {
    let extended_code = if db.is_null() {
        rc
    } else {
        unsafe { ffi::sqlite3_extended_errcode(db) }
    };
    let primary_code = extended_code & 0xff;
    let message = if db.is_null() {
        unsafe { c_string(ffi::sqlite3_errstr(rc)) }
    } else {
        unsafe { c_string(ffi::sqlite3_errmsg(db)) }
    };
    let error_offset = if db.is_null() {
        None
    } else {
        let offset = unsafe { ffi::sqlite3_error_offset(db) };
        (offset >= 0).then_some(offset)
    };
    CoordinatorSqliteDiagnostic {
        phase,
        primary_code,
        extended_code,
        symbolic_code: sqlite_symbolic_code(extended_code),
        message,
        error_offset,
    }
}

#[allow(clippy::too_many_lines)]
fn sqlite_symbolic_code(code: c_int) -> &'static str {
    match code {
        ffi::SQLITE_ERROR_MISSING_COLLSEQ => "SQLITE_ERROR_MISSING_COLLSEQ",
        ffi::SQLITE_ERROR_RETRY => "SQLITE_ERROR_RETRY",
        ffi::SQLITE_ERROR_SNAPSHOT => "SQLITE_ERROR_SNAPSHOT",
        ffi::SQLITE_ERROR_RESERVESIZE => "SQLITE_ERROR_RESERVESIZE",
        ffi::SQLITE_ERROR_KEY => "SQLITE_ERROR_KEY",
        ffi::SQLITE_ERROR_UNABLE => "SQLITE_ERROR_UNABLE",
        ffi::SQLITE_BUSY_RECOVERY => "SQLITE_BUSY_RECOVERY",
        ffi::SQLITE_BUSY_SNAPSHOT => "SQLITE_BUSY_SNAPSHOT",
        ffi::SQLITE_BUSY_TIMEOUT => "SQLITE_BUSY_TIMEOUT",
        ffi::SQLITE_LOCKED_SHAREDCACHE => "SQLITE_LOCKED_SHAREDCACHE",
        ffi::SQLITE_LOCKED_VTAB => "SQLITE_LOCKED_VTAB",
        ffi::SQLITE_READONLY_RECOVERY => "SQLITE_READONLY_RECOVERY",
        ffi::SQLITE_READONLY_CANTLOCK => "SQLITE_READONLY_CANTLOCK",
        ffi::SQLITE_READONLY_ROLLBACK => "SQLITE_READONLY_ROLLBACK",
        ffi::SQLITE_READONLY_DBMOVED => "SQLITE_READONLY_DBMOVED",
        ffi::SQLITE_READONLY_CANTINIT => "SQLITE_READONLY_CANTINIT",
        ffi::SQLITE_READONLY_DIRECTORY => "SQLITE_READONLY_DIRECTORY",
        ffi::SQLITE_ABORT_ROLLBACK => "SQLITE_ABORT_ROLLBACK",
        ffi::SQLITE_IOERR_READ => "SQLITE_IOERR_READ",
        ffi::SQLITE_IOERR_SHORT_READ => "SQLITE_IOERR_SHORT_READ",
        ffi::SQLITE_IOERR_WRITE => "SQLITE_IOERR_WRITE",
        ffi::SQLITE_IOERR_FSYNC => "SQLITE_IOERR_FSYNC",
        ffi::SQLITE_IOERR_DIR_FSYNC => "SQLITE_IOERR_DIR_FSYNC",
        ffi::SQLITE_IOERR_TRUNCATE => "SQLITE_IOERR_TRUNCATE",
        ffi::SQLITE_IOERR_FSTAT => "SQLITE_IOERR_FSTAT",
        ffi::SQLITE_IOERR_UNLOCK => "SQLITE_IOERR_UNLOCK",
        ffi::SQLITE_IOERR_RDLOCK => "SQLITE_IOERR_RDLOCK",
        ffi::SQLITE_IOERR_DELETE => "SQLITE_IOERR_DELETE",
        ffi::SQLITE_IOERR_BLOCKED => "SQLITE_IOERR_BLOCKED",
        ffi::SQLITE_IOERR_NOMEM => "SQLITE_IOERR_NOMEM",
        ffi::SQLITE_IOERR_ACCESS => "SQLITE_IOERR_ACCESS",
        ffi::SQLITE_IOERR_CHECKRESERVEDLOCK => "SQLITE_IOERR_CHECKRESERVEDLOCK",
        ffi::SQLITE_IOERR_LOCK => "SQLITE_IOERR_LOCK",
        ffi::SQLITE_IOERR_CLOSE => "SQLITE_IOERR_CLOSE",
        ffi::SQLITE_IOERR_DIR_CLOSE => "SQLITE_IOERR_DIR_CLOSE",
        ffi::SQLITE_IOERR_SHMOPEN => "SQLITE_IOERR_SHMOPEN",
        ffi::SQLITE_IOERR_SHMSIZE => "SQLITE_IOERR_SHMSIZE",
        ffi::SQLITE_IOERR_SHMLOCK => "SQLITE_IOERR_SHMLOCK",
        ffi::SQLITE_IOERR_SHMMAP => "SQLITE_IOERR_SHMMAP",
        ffi::SQLITE_IOERR_SEEK => "SQLITE_IOERR_SEEK",
        ffi::SQLITE_IOERR_DELETE_NOENT => "SQLITE_IOERR_DELETE_NOENT",
        ffi::SQLITE_IOERR_MMAP => "SQLITE_IOERR_MMAP",
        ffi::SQLITE_IOERR_GETTEMPPATH => "SQLITE_IOERR_GETTEMPPATH",
        ffi::SQLITE_IOERR_CONVPATH => "SQLITE_IOERR_CONVPATH",
        ffi::SQLITE_IOERR_VNODE => "SQLITE_IOERR_VNODE",
        ffi::SQLITE_IOERR_AUTH => "SQLITE_IOERR_AUTH",
        ffi::SQLITE_IOERR_BEGIN_ATOMIC => "SQLITE_IOERR_BEGIN_ATOMIC",
        ffi::SQLITE_IOERR_COMMIT_ATOMIC => "SQLITE_IOERR_COMMIT_ATOMIC",
        ffi::SQLITE_IOERR_ROLLBACK_ATOMIC => "SQLITE_IOERR_ROLLBACK_ATOMIC",
        ffi::SQLITE_IOERR_DATA => "SQLITE_IOERR_DATA",
        ffi::SQLITE_IOERR_CORRUPTFS => "SQLITE_IOERR_CORRUPTFS",
        ffi::SQLITE_IOERR_IN_PAGE => "SQLITE_IOERR_IN_PAGE",
        ffi::SQLITE_IOERR_BADKEY => "SQLITE_IOERR_BADKEY",
        ffi::SQLITE_IOERR_CODEC => "SQLITE_IOERR_CODEC",
        ffi::SQLITE_CANTOPEN_NOTEMPDIR => "SQLITE_CANTOPEN_NOTEMPDIR",
        ffi::SQLITE_CANTOPEN_ISDIR => "SQLITE_CANTOPEN_ISDIR",
        ffi::SQLITE_CANTOPEN_FULLPATH => "SQLITE_CANTOPEN_FULLPATH",
        ffi::SQLITE_CANTOPEN_CONVPATH => "SQLITE_CANTOPEN_CONVPATH",
        ffi::SQLITE_CANTOPEN_DIRTYWAL => "SQLITE_CANTOPEN_DIRTYWAL",
        ffi::SQLITE_CANTOPEN_SYMLINK => "SQLITE_CANTOPEN_SYMLINK",
        ffi::SQLITE_CORRUPT_VTAB => "SQLITE_CORRUPT_VTAB",
        ffi::SQLITE_CORRUPT_SEQUENCE => "SQLITE_CORRUPT_SEQUENCE",
        ffi::SQLITE_CORRUPT_INDEX => "SQLITE_CORRUPT_INDEX",
        ffi::SQLITE_CONSTRAINT_CHECK => "SQLITE_CONSTRAINT_CHECK",
        ffi::SQLITE_CONSTRAINT_COMMITHOOK => "SQLITE_CONSTRAINT_COMMITHOOK",
        ffi::SQLITE_CONSTRAINT_FOREIGNKEY => "SQLITE_CONSTRAINT_FOREIGNKEY",
        ffi::SQLITE_CONSTRAINT_FUNCTION => "SQLITE_CONSTRAINT_FUNCTION",
        ffi::SQLITE_CONSTRAINT_NOTNULL => "SQLITE_CONSTRAINT_NOTNULL",
        ffi::SQLITE_CONSTRAINT_PRIMARYKEY => "SQLITE_CONSTRAINT_PRIMARYKEY",
        ffi::SQLITE_CONSTRAINT_TRIGGER => "SQLITE_CONSTRAINT_TRIGGER",
        ffi::SQLITE_CONSTRAINT_UNIQUE => "SQLITE_CONSTRAINT_UNIQUE",
        ffi::SQLITE_CONSTRAINT_VTAB => "SQLITE_CONSTRAINT_VTAB",
        ffi::SQLITE_CONSTRAINT_ROWID => "SQLITE_CONSTRAINT_ROWID",
        ffi::SQLITE_CONSTRAINT_PINNED => "SQLITE_CONSTRAINT_PINNED",
        ffi::SQLITE_CONSTRAINT_DATATYPE => "SQLITE_CONSTRAINT_DATATYPE",
        ffi::SQLITE_NOTICE_RECOVER_WAL => "SQLITE_NOTICE_RECOVER_WAL",
        ffi::SQLITE_NOTICE_RECOVER_ROLLBACK => "SQLITE_NOTICE_RECOVER_ROLLBACK",
        ffi::SQLITE_NOTICE_RBU => "SQLITE_NOTICE_RBU",
        ffi::SQLITE_WARNING_AUTOINDEX => "SQLITE_WARNING_AUTOINDEX",
        ffi::SQLITE_AUTH_USER => "SQLITE_AUTH_USER",
        _ => match code & 0xff {
            ffi::SQLITE_ERROR => "SQLITE_ERROR",
            ffi::SQLITE_INTERNAL => "SQLITE_INTERNAL",
            ffi::SQLITE_PERM => "SQLITE_PERM",
            ffi::SQLITE_ABORT => "SQLITE_ABORT",
            ffi::SQLITE_BUSY => "SQLITE_BUSY",
            ffi::SQLITE_LOCKED => "SQLITE_LOCKED",
            ffi::SQLITE_NOMEM => "SQLITE_NOMEM",
            ffi::SQLITE_READONLY => "SQLITE_READONLY",
            ffi::SQLITE_INTERRUPT => "SQLITE_INTERRUPT",
            ffi::SQLITE_IOERR => "SQLITE_IOERR",
            ffi::SQLITE_CORRUPT => "SQLITE_CORRUPT",
            ffi::SQLITE_NOTFOUND => "SQLITE_NOTFOUND",
            ffi::SQLITE_FULL => "SQLITE_FULL",
            ffi::SQLITE_CANTOPEN => "SQLITE_CANTOPEN",
            ffi::SQLITE_PROTOCOL => "SQLITE_PROTOCOL",
            ffi::SQLITE_EMPTY => "SQLITE_EMPTY",
            ffi::SQLITE_SCHEMA => "SQLITE_SCHEMA",
            ffi::SQLITE_TOOBIG => "SQLITE_TOOBIG",
            ffi::SQLITE_CONSTRAINT => "SQLITE_CONSTRAINT",
            ffi::SQLITE_MISMATCH => "SQLITE_MISMATCH",
            ffi::SQLITE_MISUSE => "SQLITE_MISUSE",
            ffi::SQLITE_NOLFS => "SQLITE_NOLFS",
            ffi::SQLITE_AUTH => "SQLITE_AUTH",
            ffi::SQLITE_FORMAT => "SQLITE_FORMAT",
            ffi::SQLITE_RANGE => "SQLITE_RANGE",
            ffi::SQLITE_NOTADB => "SQLITE_NOTADB",
            ffi::SQLITE_NOTICE => "SQLITE_NOTICE",
            ffi::SQLITE_WARNING => "SQLITE_WARNING",
            ffi::SQLITE_ROW => "SQLITE_ROW",
            ffi::SQLITE_DONE => "SQLITE_DONE",
            _ => "SQLITE_UNKNOWN",
        },
    }
}

unsafe fn optional_c_string(value: *const c_char) -> Option<String> {
    (!value.is_null()).then(|| unsafe { c_string(value) })
}

unsafe fn c_string(value: *const c_char) -> String {
    if value.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned()
    }
}

fn read_cell(statement: *mut ffi::sqlite3_stmt, index: c_int) -> CoordinatorCell {
    match unsafe { ffi::sqlite3_column_type(statement, index) } {
        ffi::SQLITE_INTEGER => {
            CoordinatorCell::Integer(unsafe { ffi::sqlite3_column_int64(statement, index) })
        }
        ffi::SQLITE_FLOAT => {
            CoordinatorCell::Real(unsafe { ffi::sqlite3_column_double(statement, index) })
        }
        ffi::SQLITE_TEXT => {
            let value = unsafe { ffi::sqlite3_column_text(statement, index) };
            let bytes =
                usize::try_from(unsafe { ffi::sqlite3_column_bytes(statement, index) }.max(0))
                    .unwrap_or_default();
            if value.is_null() {
                CoordinatorCell::Null
            } else {
                let slice = unsafe { std::slice::from_raw_parts(value, bytes) };
                CoordinatorCell::Text(String::from_utf8_lossy(slice).into_owned())
            }
        }
        ffi::SQLITE_BLOB => CoordinatorCell::Blob {
            bytes: usize::try_from(unsafe { ffi::sqlite3_column_bytes(statement, index) }.max(0))
                .unwrap_or_default(),
        },
        _ => CoordinatorCell::Null,
    }
}

fn cell_size(cell: &CoordinatorCell) -> usize {
    match cell {
        CoordinatorCell::Null => 4,
        CoordinatorCell::Integer(value) => value.to_string().len(),
        CoordinatorCell::Real(value) => value.to_string().len(),
        CoordinatorCell::Text(value) => value.len(),
        CoordinatorCell::Blob { .. } => 16,
    }
}

struct StatementGuard(*mut ffi::sqlite3_stmt);
impl Drop for StatementGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ffi::sqlite3_finalize(self.0) };
        }
    }
}

struct ConnectionGuard(*mut ffi::sqlite3);
impl ConnectionGuard {
    fn close(&mut self) {
        if !self.0.is_null() {
            unsafe { ffi::sqlite3_close(self.0) };
            self.0 = ptr::null_mut();
        }
    }
}
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("query.db");
        let db = rusqlite_for_test(&path);
        db.execute_batch(
            "CREATE TABLE conversations(id TEXT, state TEXT, state_updated_at TEXT, updated_at TEXT);\n             CREATE TABLE auth_sessions(id TEXT, token_hash TEXT);\n             CREATE TABLE future_secret_store(id TEXT, secret TEXT);\n             CREATE TABLE conversation_creation_jobs(id TEXT, claim_token TEXT, status TEXT);\n             CREATE TABLE workflow_external_acceptance_bindings(id TEXT, idempotency_key TEXT, receipt_handle BLOB, disposition_handle BLOB, status TEXT);\n             CREATE TABLE messages(message_id TEXT, content TEXT, display_data TEXT);\n             CREATE TABLE message_images(message_id TEXT, ordinal INTEGER, media_type TEXT, data TEXT);\n             CREATE TABLE workflow_effects(id TEXT, intent_payload BLOB, status TEXT);\n             INSERT INTO conversations VALUES ('active', '{\"type\":\"tool_execution\",\"pending\":\"secret\"}', '2026-07-21', '2026-07-21');\n             INSERT INTO auth_sessions VALUES ('session', 'secret');",
        )
        .unwrap();
        (dir, path)
    }

    fn rusqlite_for_test(path: &std::path::Path) -> TestDb {
        TestDb::open(path)
    }

    struct TestDb(*mut ffi::sqlite3);
    impl TestDb {
        fn open(path: &std::path::Path) -> Self {
            let path = CString::new(path.to_string_lossy().as_bytes()).unwrap();
            let mut db = ptr::null_mut();
            assert_eq!(
                unsafe { ffi::sqlite3_open(path.as_ptr(), &raw mut db) },
                ffi::SQLITE_OK
            );
            Self(db)
        }
        fn execute_batch(&self, sql: &str) -> Result<(), String> {
            let sql = CString::new(sql).unwrap();
            let mut error = ptr::null_mut();
            let rc = unsafe {
                ffi::sqlite3_exec(self.0, sql.as_ptr(), None, ptr::null_mut(), &raw mut error)
            };
            if rc == ffi::SQLITE_OK {
                Ok(())
            } else {
                let message = unsafe { c_string(error) };
                unsafe { ffi::sqlite3_free(error.cast()) };
                Err(message)
            }
        }
    }
    impl Drop for TestDb {
        fn drop(&mut self) {
            unsafe { ffi::sqlite3_close(self.0) };
        }
    }

    #[test]
    fn reads_application_data_including_operator_only_rows() {
        let (_dir, path) = fixture();
        let db = rusqlite_for_test(&path);
        db.execute_batch("INSERT INTO messages VALUES ('hidden', 'operator-only text', '{}'); INSERT INTO future_secret_store VALUES ('secret-id', 'secret-value');").unwrap();
        for sql in [
            "SELECT id, state FROM conversations",
            "SELECT content FROM messages",
            "SELECT secret FROM future_secret_store",
            "SELECT token_hash FROM auth_sessions",
            "SELECT claim_token FROM conversation_creation_jobs",
            "SELECT idempotency_key, receipt_handle, disposition_handle FROM workflow_external_acceptance_bindings",
            "SELECT data FROM message_images",
            "SELECT intent_payload FROM workflow_effects",
        ] {
            execute_coordinator_query(path.to_str().unwrap(), sql).unwrap();
        }
    }

    #[test]
    fn reports_actionable_prepare_diagnostics() {
        let (_dir, path) = fixture();
        for (sql, expected_message, expected_offset) in [
            (
                "SELECT missing_column FROM conversations",
                "no such column: missing_column",
                Some(7),
            ),
            (
                "SELECT * FROM missing_table",
                "no such table: missing_table",
                None,
            ),
            ("SELEC 1", "near \"SELEC\": syntax error", Some(0)),
        ] {
            let error = execute_coordinator_query(path.to_str().unwrap(), sql).unwrap_err();
            let CoordinatorQueryError::Sqlite(diagnostic) = error else {
                panic!("expected SQLite diagnostic for {sql}: {error}");
            };
            assert_eq!(diagnostic.phase, CoordinatorSqlitePhase::Prepare);
            assert_eq!(diagnostic.primary_code, ffi::SQLITE_ERROR);
            assert_eq!(diagnostic.extended_code, ffi::SQLITE_ERROR);
            assert_eq!(diagnostic.symbolic_code, "SQLITE_ERROR");
            assert_eq!(diagnostic.message, expected_message);
            assert_eq!(diagnostic.error_offset, expected_offset);
            let rendered = diagnostic.to_string();
            assert!(rendered.contains(expected_message), "{rendered}");
            assert!(rendered.contains("primary_code=1"), "{rendered}");
            assert!(rendered.contains("extended_code=1"), "{rendered}");
        }
    }

    #[test]
    fn reports_busy_snapshot_with_extended_code() {
        let diagnostic = sqlite_diagnostic(
            ptr::null_mut(),
            ffi::SQLITE_BUSY_SNAPSHOT,
            CoordinatorSqlitePhase::Execute,
        );
        assert_eq!(diagnostic.primary_code, ffi::SQLITE_BUSY);
        assert_eq!(diagnostic.extended_code, ffi::SQLITE_BUSY_SNAPSHOT);
        assert_eq!(diagnostic.symbolic_code, "SQLITE_BUSY_SNAPSHOT");
        assert_eq!(diagnostic.message, "database is locked");
        assert_eq!(diagnostic.error_offset, None);
        assert!(diagnostic.to_string().contains("extended_code=517"));
    }

    #[test]
    fn preserves_non_locking_extended_symbolic_codes() {
        for (code, expected) in [
            (ffi::SQLITE_IOERR_READ, "SQLITE_IOERR_READ"),
            (ffi::SQLITE_CANTOPEN_ISDIR, "SQLITE_CANTOPEN_ISDIR"),
            (ffi::SQLITE_CORRUPT_VTAB, "SQLITE_CORRUPT_VTAB"),
            (
                ffi::SQLITE_CONSTRAINT_FOREIGNKEY,
                "SQLITE_CONSTRAINT_FOREIGNKEY",
            ),
            (
                ffi::SQLITE_ERROR_MISSING_COLLSEQ,
                "SQLITE_ERROR_MISSING_COLLSEQ",
            ),
        ] {
            assert_eq!(sqlite_symbolic_code(code), expected);
        }
    }

    #[test]
    fn preserves_error_offset_for_execute_phase_diagnostics() {
        let (_dir, path) = fixture();
        let db = rusqlite_for_test(&path);
        let sql = CString::new("SELEC 1").unwrap();
        let mut statement = ptr::null_mut();
        let rc = unsafe {
            ffi::sqlite3_prepare_v2(db.0, sql.as_ptr(), -1, &raw mut statement, ptr::null_mut())
        };
        assert_ne!(rc, ffi::SQLITE_OK);

        let diagnostic = sqlite_diagnostic(db.0, rc, CoordinatorSqlitePhase::Execute);

        assert_eq!(diagnostic.error_offset, Some(0));
        if !statement.is_null() {
            unsafe { ffi::sqlite3_finalize(statement) };
        }
    }

    #[test]
    fn reports_real_database_lock_diagnostic() {
        let (_dir, path) = fixture();
        let writer = rusqlite_for_test(&path);
        writer.execute_batch("BEGIN EXCLUSIVE").unwrap();

        let error = execute_coordinator_query(path.to_str().unwrap(), "SELECT 1").unwrap_err();

        writer.execute_batch("ROLLBACK").unwrap();
        let CoordinatorQueryError::Sqlite(diagnostic) = error else {
            panic!("expected SQLite lock diagnostic: {error}");
        };
        assert_eq!(diagnostic.primary_code, ffi::SQLITE_BUSY);
        assert!(matches!(
            diagnostic.symbolic_code,
            "SQLITE_BUSY" | "SQLITE_BUSY_RECOVERY" | "SQLITE_BUSY_SNAPSHOT" | "SQLITE_BUSY_TIMEOUT"
        ));
        assert!(diagnostic.message.contains("locked"), "{diagnostic}");
    }

    #[test]
    fn denies_writes_attach_pragmas_and_multiple_statements() {
        let (_dir, path) = fixture();
        for sql in [
            "UPDATE conversations SET id = 'changed'",
            "ATTACH DATABASE '/tmp/other.db' AS other",
            "PRAGMA query_only",
        ] {
            assert!(
                matches!(
                    execute_coordinator_query(path.to_str().unwrap(), sql),
                    Err(CoordinatorQueryError::Denied(_))
                ),
                "{sql}"
            );
        }
        assert!(matches!(
            execute_coordinator_query(path.to_str().unwrap(), "SELECT 1; SELECT 2"),
            Err(CoordinatorQueryError::MultipleStatements)
        ));
    }

    #[test]
    fn distinguishes_trailing_delimiters_from_another_statement() {
        let (_dir, path) = fixture();
        execute_coordinator_query(path.to_str().unwrap(), "SELECT 1; \t\n;").unwrap();
        assert!(matches!(
            execute_coordinator_query(path.to_str().unwrap(), "SELECT 1; SELECT 2"),
            Err(CoordinatorQueryError::MultipleStatements)
        ));
    }

    #[test]
    fn denies_schema_and_shadow_table_bypasses() {
        let (_dir, path) = fixture();
        for sql in [
            "SELECT sql FROM sqlite_schema",
            "SELECT * FROM sqlite_dbpage",
            "SELECT readfile('/etc/passwd')",
            "SELECT load_extension('/tmp/evil')",
        ] {
            assert!(
                matches!(
                    execute_coordinator_query(path.to_str().unwrap(), sql),
                    Err(CoordinatorQueryError::Denied(_) | CoordinatorQueryError::Sqlite(_))
                ),
                "{sql}"
            );
        }
    }

    #[test]
    fn denies_fts_index_and_shadow_storage() {
        let (_dir, path) = fixture();
        let db = rusqlite_for_test(&path);
        db.execute_batch("CREATE VIRTUAL TABLE message_fts USING fts5(text); INSERT INTO message_fts(text) VALUES ('wake progress');").unwrap();
        for sql in [
            "SELECT text FROM message_fts WHERE message_fts MATCH 'wake'",
            "SELECT c0 FROM message_fts_content",
        ] {
            assert!(matches!(
                execute_coordinator_query(path.to_str().unwrap(), sql),
                Err(CoordinatorQueryError::Denied(_))
            ));
        }
    }

    #[test]
    fn budgets_the_serialized_result_and_sql_shape() {
        let (_dir, path) = fixture();
        let alias = "x".repeat(MAX_SQL_BYTES);
        assert!(matches!(
            execute_coordinator_query(path.to_str().unwrap(), &format!("SELECT 1 AS '{alias}'")),
            Err(CoordinatorQueryError::BudgetExceeded)
        ));
        let aliases = (0..=MAX_COLUMNS)
            .map(|index| format!("1 AS c{index}"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(
            execute_coordinator_query(path.to_str().unwrap(), &format!("SELECT {aliases}"))
                .is_err()
        );
    }

    #[test]
    fn bounds_rows_bytes_and_recursive_work() {
        let (_dir, path) = fixture();
        let rows = execute_coordinator_query(
            path.to_str().unwrap(),
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000) SELECT x FROM n",
        )
        .unwrap();
        assert_eq!(rows.rows.len(), MAX_ROWS);
        assert!(rows.truncated);

        let bytes =
            execute_coordinator_query(path.to_str().unwrap(), "SELECT printf('%.*c', 70000, 'x')")
                .unwrap();
        assert!(!bytes
            .rows
            .iter()
            .flatten()
            .any(|cell| matches!(cell, CoordinatorCell::Text(value) if value.len() > MAX_BYTES)));
    }
}
