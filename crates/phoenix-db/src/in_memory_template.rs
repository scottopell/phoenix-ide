use libsqlite3_sys as ffi;
use phoenix_core::domain::instance_identity::InstanceId;
use phoenix_core::runtime_env::PhoenixRuntimeEnvironment;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteConnection, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
};
#[cfg(test)]
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;
#[cfg(test)]
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::OnceCell;

use crate::sqlite_native_statement::install_native_statement_baseline;
use crate::{ddl, migrations, Database, DbError, DbResult, SqliteWorkloadCollector};

const MAIN_DB_NAME: &[u8] = b"main\0";
const SQLITE_HEADER_MAGIC: &[u8] = b"SQLite format 3\0";
const MIN_SQLITE_BYTES: usize = 4096;
const TEMPLATE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

static IN_MEMORY_TEMPLATE: OnceCell<Vec<u8>> = OnceCell::const_new();
#[cfg(test)]
static PREFIX_TEMPLATES: OnceLock<Mutex<HashMap<u32, Arc<OnceCell<Vec<u8>>>>>> = OnceLock::new();

fn template_disabled() -> bool {
    matches!(
        std::env::var("PHOENIX_DB_DISABLE_MEM_TEMPLATE").as_deref(),
        Ok("1" | "true" | "TRUE")
    )
}

fn template_fingerprint() -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"phoenix-db-in-memory-template-v1\0");
    for part in [
        ddl::SCHEMA,
        ddl::MIGRATION_TYPED_STATE,
        ddl::MIGRATION_RENAME_MESSAGE_ID,
        ddl::MIGRATION_REMOVE_UNKNOWN_ERROR_KIND,
        ddl::MIGRATION_CREATE_PROJECTS,
        ddl::MIGRATION_CREATE_MCP_DISABLED_SERVERS,
        ddl::MIGRATION_CREATE_SHARE_TOKENS,
    ] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    let mig_digest = migrations::compiled_migration_digest();
    hasher.update((mig_digest.len() as u64).to_be_bytes());
    hasher.update(mig_digest.as_bytes());

    let sqlite_ver = unsafe { ffi::sqlite3_libversion_number() };
    hasher.update(sqlite_ver.to_be_bytes());

    hasher.update(env!("PHOENIX_DB_GIT_SHA").as_bytes());
    hasher.update(b"\0");
    hasher.update(env!("PHOENIX_DB_GIT_DIRTY").as_bytes());
    hasher.update(b"\0");

    // Bind the template key to the exact running executable identity so any
    // recompile (including Rust-only migration helper edits on a dirty tree)
    // produces a distinct cache key and can never reuse a prior build's image.
    if let Ok(exe) = std::env::current_exe() {
        let exe = fs::canonicalize(&exe).unwrap_or(exe);
        hasher.update(exe.as_os_str().as_encoded_bytes());
        hasher.update(b"\0");
        if let Ok(meta) = fs::metadata(&exe) {
            hasher.update(meta.len().to_be_bytes());
            if let Ok(modified) = meta.modified() {
                if let Ok(since_epoch) = modified.duration_since(UNIX_EPOCH) {
                    hasher.update(since_epoch.as_nanos().to_be_bytes());
                }
            }
        }
    }

    hasher
        .finalize()
        .iter()
        .take(16)
        .fold(String::with_capacity(32), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(unix)]
fn private_template_dir() -> Option<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    let uid = unsafe { libc::geteuid() };
    let dir = PhoenixRuntimeEnvironment::detect()
        .tmp_root()
        .join(format!("phoenix-db-mem-tpl-{uid}"));
    if !dir.exists() {
        let _ = fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir);
    }
    let meta = fs::symlink_metadata(&dir).ok()?;
    if !meta.file_type().is_dir() || meta.uid() != uid {
        return None;
    }
    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    Some(dir)
}

#[cfg(not(unix))]
fn private_template_dir() -> Option<PathBuf> {
    let dir = PhoenixRuntimeEnvironment::detect()
        .tmp_root()
        .join("phoenix-db-mem-tpl");
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn is_valid_sqlite_image(bytes: &[u8]) -> bool {
    bytes.len() >= MIN_SQLITE_BYTES
        && bytes.len().is_multiple_of(512)
        && bytes.starts_with(SQLITE_HEADER_MAGIC)
}

fn try_load_cached_template(path: &Path) -> Option<Vec<u8>> {
    let bytes = fs::read(path).ok()?;
    is_valid_sqlite_image(&bytes).then_some(bytes)
}

fn prune_stale_templates(dir: &Path, keep_name: &std::ffi::OsStr) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        if entry.file_name() == keep_name {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let stale = meta
            .modified()
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_some_and(|age| age > TEMPLATE_MAX_AGE);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn try_store_cached_template(path: &Path, bytes: &[u8]) {
    let Some(parent) = path.parent() else {
        return;
    };
    let Some(file_name) = path.file_name() else {
        return;
    };
    let pid = std::process::id();
    let nonce = uuid::Uuid::new_v4().simple();
    let tmp = parent.join(format!(".tpl-{pid}-{nonce}.tmp"));

    let mut open_opts = fs::OpenOptions::new();
    open_opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_opts.mode(0o600);
    }
    let Ok(mut file) = open_opts.open(&tmp) else {
        return;
    };
    if file.write_all(bytes).is_err() {
        drop(file);
        let _ = fs::remove_file(&tmp);
        return;
    }
    drop(file);
    if fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
    } else {
        prune_stale_templates(parent, file_name);
    }
}

async fn serialize_connection(conn: &mut SqliteConnection) -> DbResult<Vec<u8>> {
    let mut locked = conn.lock_handle().await?;
    let db_ptr = locked.as_raw_handle().as_ptr();
    let mut size: ffi::sqlite3_int64 = 0;
    let raw_ptr =
        unsafe { ffi::sqlite3_serialize(db_ptr, MAIN_DB_NAME.as_ptr().cast(), &raw mut size, 0) };
    if raw_ptr.is_null() {
        return Err(DbError::Serialization(
            "sqlite3_serialize returned null for in-memory template".to_string(),
        ));
    }
    let len = usize::try_from(size).map_err(|_| {
        unsafe { ffi::sqlite3_free(raw_ptr.cast()) };
        DbError::Serialization(format!("sqlite3_serialize returned invalid size: {size}"))
    })?;
    let bytes = unsafe { std::slice::from_raw_parts(raw_ptr, len) }.to_vec();
    unsafe { ffi::sqlite3_free(raw_ptr.cast()) };
    if !is_valid_sqlite_image(&bytes) {
        return Err(DbError::Serialization(format!(
            "sqlite3_serialize produced invalid image ({} bytes)",
            bytes.len()
        )));
    }
    Ok(bytes)
}

async fn deserialize_into_connection(
    conn: &mut SqliteConnection,
    bytes: &[u8],
) -> Result<(), sqlx::Error> {
    let db_size = ffi::sqlite3_int64::try_from(bytes.len())
        .map_err(|_| sqlx::Error::Protocol("in-memory SQLite template too large".to_string()))?;
    let alloc_bytes = ffi::sqlite3_uint64::try_from(bytes.len())
        .map_err(|_| sqlx::Error::Protocol("in-memory SQLite template too large".to_string()))?;

    let mut locked = conn.lock_handle().await?;
    let db_ptr = locked.as_raw_handle().as_ptr();

    let buf = unsafe { ffi::sqlite3_malloc64(alloc_bytes) }.cast::<u8>();
    if buf.is_null() {
        return Err(sqlx::Error::Protocol(
            "sqlite3_malloc64 failed while restoring in-memory template".to_string(),
        ));
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
    }
    // SQLITE_DESERIALIZE_FREEONCLOSE transfers ownership of `buf` to SQLite:
    // on both success (connection close) and every runtime error path inside
    // sqlite3_deserialize (`end_deserialize:`), SQLite frees `buf` itself, so
    // the caller must NOT call sqlite3_free on error.
    let flags = ffi::SQLITE_DESERIALIZE_FREEONCLOSE | ffi::SQLITE_DESERIALIZE_RESIZEABLE;
    let rc = unsafe {
        ffi::sqlite3_deserialize(
            db_ptr,
            MAIN_DB_NAME.as_ptr().cast(),
            buf,
            db_size,
            db_size,
            flags,
        )
    };
    if rc != ffi::SQLITE_OK {
        return Err(sqlx::Error::Protocol(format!(
            "sqlite3_deserialize failed with status {rc}"
        )));
    }
    Ok(())
}

async fn build_fresh_template_bytes() -> DbResult<Vec<u8>> {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    let collector = SqliteWorkloadCollector::new();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await?;
    let db = Database::new_with_generated_target_binding(pool, String::new(), collector);
    db.run_migrations().await?;
    migrations::run_pending_migrations(&db.pool).await?;
    let mut conn = db.pool.acquire().await?;
    serialize_connection(&mut conn).await
}

async fn load_or_build_template() -> DbResult<&'static [u8]> {
    let bytes = IN_MEMORY_TEMPLATE
        .get_or_try_init(|| async {
            let cache_path = private_template_dir()
                .map(|dir| dir.join(format!("tpl-{}.sqlite", template_fingerprint())));
            if let Some(path) = cache_path.as_deref() {
                if let Some(cached) = try_load_cached_template(path) {
                    return Ok::<Vec<u8>, DbError>(cached);
                }
            }
            let fresh = build_fresh_template_bytes().await?;
            if let Some(path) = cache_path.as_deref() {
                try_store_cached_template(path, &fresh);
            }
            Ok(fresh)
        })
        .await?;
    Ok(bytes.as_slice())
}

#[cfg(test)]
async fn build_fresh_prefix_bytes(through: u32) -> DbResult<Vec<u8>> {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    let collector = SqliteWorkloadCollector::new();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await?;
    let db = Database::new_with_generated_target_binding(pool, String::new(), collector);
    db.run_migrations().await?;
    migrations::run_through_for_test(&db.pool, through).await?;
    let mut conn = db.pool.acquire().await?;
    serialize_connection(&mut conn).await
}

#[cfg(test)]
async fn load_or_build_prefix_template(through: u32) -> DbResult<Vec<u8>> {
    let cell = {
        let map = PREFIX_TEMPLATES.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .entry(through)
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone()
    };
    let bytes = cell
        .get_or_try_init(|| async {
            let cache_path = private_template_dir()
                .map(|dir| dir.join(format!("tpl-{}-v{through}.sqlite", template_fingerprint())));
            if let Some(path) = cache_path.as_deref() {
                if let Some(cached) = try_load_cached_template(path) {
                    return Ok::<Vec<u8>, DbError>(cached);
                }
            }
            let fresh = build_fresh_prefix_bytes(through).await?;
            if let Some(path) = cache_path.as_deref() {
                try_store_cached_template(path, &fresh);
            }
            Ok(fresh)
        })
        .await?;
    Ok(bytes.clone())
}

#[cfg(test)]
async fn refresh_instance_identity_if_present(
    conn: &mut SqliteConnection,
    fresh_instance_id: &str,
) -> Result<(), sqlx::Error> {
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'instance_identity'",
    )
    .persistent(false)
    .fetch_optional(&mut *conn)
    .await?;
    if exists.is_some() {
        sqlx::query("UPDATE instance_identity SET instance_id = ?1 WHERE singleton_key = 1")
            .bind(fresh_instance_id)
            .persistent(false)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) async fn open_in_memory_prefix_for_test(through: u32) -> DbResult<Database> {
    if template_disabled() {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);
        let sqlite_workload_collector = SqliteWorkloadCollector::new();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect({
                let sqlite_workload_collector = sqlite_workload_collector.clone();
                move |conn, _meta| {
                    let sqlite_workload_collector = sqlite_workload_collector.clone();
                    Box::pin(async move {
                        install_native_statement_baseline(conn, sqlite_workload_collector).await
                    })
                }
            })
            .connect_with(opts)
            .await?;
        let db = Database::new_with_generated_target_binding(
            pool,
            String::new(),
            sqlite_workload_collector,
        );
        db.run_migrations().await?;
        migrations::run_through_for_test(&db.pool, through).await?;
        return Ok(db);
    }

    let template_bytes = Arc::new(load_or_build_prefix_template(through).await?);
    let fresh_instance_id = InstanceId::new().to_string();
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    let sqlite_workload_collector = SqliteWorkloadCollector::new();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .after_connect({
            let sqlite_workload_collector = sqlite_workload_collector.clone();
            move |conn, _meta| {
                let sqlite_workload_collector = sqlite_workload_collector.clone();
                let template_bytes = template_bytes.clone();
                let fresh_instance_id = fresh_instance_id.clone();
                Box::pin(async move {
                    deserialize_into_connection(conn, &template_bytes).await?;
                    refresh_instance_identity_if_present(conn, &fresh_instance_id).await?;
                    install_native_statement_baseline(conn, sqlite_workload_collector).await
                })
            }
        })
        .connect_with(opts)
        .await?;
    Ok(Database::new_with_generated_target_binding(
        pool,
        String::new(),
        sqlite_workload_collector,
    ))
}

#[cfg(test)]
pub(crate) async fn open_prefix_for_test(path: &str, through: u32) -> DbResult<Database> {
    let target = Path::new(path);
    let seeded = if template_disabled() || target.exists() {
        false
    } else {
        let template_bytes = load_or_build_prefix_template(through).await?;
        seed_bytes_atomically(target, &template_bytes)?
    };
    if !seeded {
        let db = Database::open(path).await?;
        migrations::run_through_for_test(db.pool(), through).await?;
        db.restrict_file_permissions();
        return Ok(db);
    }
    let opts = SqliteConnectOptions::from_str(&format!("sqlite:{path}?mode=rwc"))?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    let sqlite_workload_collector = SqliteWorkloadCollector::new();
    let pool = SqlitePoolOptions::new()
        .after_connect({
            let sqlite_workload_collector = sqlite_workload_collector.clone();
            move |conn, _meta| {
                let sqlite_workload_collector = sqlite_workload_collector.clone();
                Box::pin(async move {
                    install_native_statement_baseline(conn, sqlite_workload_collector).await
                })
            }
        })
        .connect_with(opts)
        .await?;
    let db = Database::new_with_generated_target_binding(
        pool,
        path.to_string(),
        sqlite_workload_collector,
    );
    {
        let mut conn = db.pool().acquire().await?;
        refresh_instance_identity_if_present(&mut conn, &InstanceId::new().to_string()).await?;
    }
    db.restrict_file_permissions();
    Ok(db)
}

pub(crate) async fn open_in_memory_fresh() -> DbResult<Database> {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    // In-memory SQLite DBs are per-connection, so limit to 1 connection
    let sqlite_workload_collector = SqliteWorkloadCollector::new();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .after_connect({
            let sqlite_workload_collector = sqlite_workload_collector.clone();
            move |conn, _meta| {
                let sqlite_workload_collector = sqlite_workload_collector.clone();
                Box::pin(async move {
                    install_native_statement_baseline(conn, sqlite_workload_collector).await
                })
            }
        })
        .connect_with(opts)
        .await?;
    let db =
        Database::new_with_generated_target_binding(pool, String::new(), sqlite_workload_collector);
    db.run_migrations().await?;
    migrations::run_pending_migrations(&db.pool).await?;
    Ok(db)
}

pub(crate) async fn open_in_memory_from_template() -> DbResult<Database> {
    if template_disabled() {
        return open_in_memory_fresh().await;
    }
    let template_bytes = load_or_build_template().await?;
    let fresh_instance_id = InstanceId::new().to_string();

    let opts = SqliteConnectOptions::from_str("sqlite::memory:")?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    let sqlite_workload_collector = SqliteWorkloadCollector::new();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .after_connect({
            let sqlite_workload_collector = sqlite_workload_collector.clone();
            move |conn, _meta| {
                let sqlite_workload_collector = sqlite_workload_collector.clone();
                let fresh_instance_id = fresh_instance_id.clone();
                Box::pin(async move {
                    deserialize_into_connection(conn, template_bytes).await?;
                    sqlx::query(
                        "UPDATE instance_identity SET instance_id = ?1 WHERE singleton_key = 1",
                    )
                    .bind(&fresh_instance_id)
                    .persistent(false)
                    .execute(&mut *conn)
                    .await?;
                    install_native_statement_baseline(conn, sqlite_workload_collector).await
                })
            }
        })
        .connect_with(opts)
        .await?;
    Ok(Database::new_with_generated_target_binding(
        pool,
        String::new(),
        sqlite_workload_collector,
    ))
}

#[cfg(any(test, feature = "test-support"))]
fn seed_bytes_atomically(path: &Path, template_bytes: &[u8]) -> DbResult<bool> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let pid = std::process::id();
    let nonce = uuid::Uuid::new_v4().simple();
    let tmp = parent.join(format!(".seed-{pid}-{nonce}.tmp"));

    let mut open_opts = fs::OpenOptions::new();
    open_opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_opts.mode(0o600);
    }
    let mut file = open_opts.open(&tmp).map_err(|error| {
        DbError::Serialization(format!(
            "failed to create staging file for test database template at {}: {error}",
            tmp.display()
        ))
    })?;
    if let Err(error) = file.write_all(template_bytes) {
        drop(file);
        let _ = fs::remove_file(&tmp);
        return Err(DbError::Serialization(format!(
            "failed to write test database template at {}: {error}",
            tmp.display()
        )));
    }
    drop(file);

    // Publish atomically without clobbering an existing file: hard_link fails
    // with AlreadyExists if another caller created `path` first.
    let link_result = fs::hard_link(&tmp, path);
    let _ = fs::remove_file(&tmp);
    match link_result {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
            // Fallback for filesystems without hard links.
            let mut fallback_opts = fs::OpenOptions::new();
            fallback_opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                fallback_opts.mode(0o600);
            }
            let mut fallback = match fallback_opts.open(path) {
                Ok(file) => file,
                Err(open_err) if open_err.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Ok(false);
                }
                Err(open_err) => {
                    return Err(DbError::Serialization(format!(
                        "failed to seed test database template at {}: {open_err}",
                        path.display()
                    )));
                }
            };
            if let Err(write_err) = fallback.write_all(template_bytes) {
                drop(fallback);
                let _ = fs::remove_file(path);
                return Err(DbError::Serialization(format!(
                    "failed to write test database template at {}: {write_err}",
                    path.display()
                )));
            }
            Ok(true)
        }
        Err(error) => Err(DbError::Serialization(format!(
            "failed to publish test database template at {}: {error}",
            path.display()
        ))),
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) async fn seed_migrated_file_for_test(path: &Path) -> DbResult<bool> {
    if template_disabled() || path.exists() {
        return Ok(false);
    }
    let template_bytes = load_or_build_template().await?;
    seed_bytes_atomically(path, template_bytes)
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) async fn open_migrated_for_test(path: &str) -> DbResult<Database> {
    let seeded = seed_migrated_file_for_test(Path::new(path)).await?;
    if !seeded {
        let db = Database::open(path).await?;
        migrations::run_pending_migrations(db.pool()).await?;
        db.restrict_file_permissions();
        return Ok(db);
    }
    let opts = SqliteConnectOptions::from_str(&format!("sqlite:{path}?mode=rwc"))?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    let sqlite_workload_collector = SqliteWorkloadCollector::new();
    let pool = SqlitePoolOptions::new()
        .after_connect({
            let sqlite_workload_collector = sqlite_workload_collector.clone();
            move |conn, _meta| {
                let sqlite_workload_collector = sqlite_workload_collector.clone();
                Box::pin(async move {
                    install_native_statement_baseline(conn, sqlite_workload_collector).await
                })
            }
        })
        .connect_with(opts)
        .await?;
    let db = Database::new_with_generated_target_binding(
        pool,
        path.to_string(),
        sqlite_workload_collector,
    );
    sqlx::query("UPDATE instance_identity SET instance_id = ?1 WHERE singleton_key = 1")
        .bind(InstanceId::new().to_string())
        .persistent(false)
        .execute(db.pool())
        .await?;
    db.restrict_file_permissions();
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn template_open_in_memory_matches_fresh_migrated_schema_and_isolates_instances() {
        let fresh = open_in_memory_fresh().await.unwrap();
        let templated_a = open_in_memory_from_template().await.unwrap();
        let templated_b = open_in_memory_from_template().await.unwrap();

        let schema_query =
            "SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY type, name";
        let fresh_schema: Vec<(String, String, String, Option<String>)> =
            sqlx::query_as(schema_query)
                .fetch_all(fresh.pool())
                .await
                .unwrap();
        let templated_schema: Vec<(String, String, String, Option<String>)> =
            sqlx::query_as(schema_query)
                .fetch_all(templated_a.pool())
                .await
                .unwrap();
        assert_eq!(templated_schema, fresh_schema);

        let mig_query = "SELECT version, name FROM _migrations ORDER BY version";
        let fresh_migs: Vec<(i64, String)> = sqlx::query_as(mig_query)
            .fetch_all(fresh.pool())
            .await
            .unwrap();
        let templated_migs: Vec<(i64, String)> = sqlx::query_as(mig_query)
            .fetch_all(templated_a.pool())
            .await
            .unwrap();
        assert_eq!(templated_migs, fresh_migs);

        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(templated_a.pool())
            .await
            .unwrap();
        assert_eq!(fk, 1);

        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(templated_a.pool())
            .await
            .unwrap();
        assert_eq!(integrity, "ok");

        // Re-running pending migrations on a templated DB is a 0-op idempotent check.
        assert_eq!(
            migrations::run_pending_migrations(templated_a.pool())
                .await
                .unwrap(),
            0
        );

        // Each in-memory DB receives its own distinct instance identity and isolated state.
        let id_fresh = fresh.instance_id().await.unwrap();
        let id_a = templated_a.instance_id().await.unwrap();
        let id_b = templated_b.instance_id().await.unwrap();
        assert_ne!(id_a, id_b);
        assert_ne!(id_a, id_fresh);

        templated_a
            .create_conversation("iso-a", "iso-a", "/tmp", true, None, None)
            .await
            .unwrap();
        assert!(templated_b.get_conversation("iso-a").await.is_err());

        // Non-empty seed tables match fresh migrations (other than instance_identity and _migrations timestamps).
        let seq_query = "SELECT sequence_name, next_value FROM workflow_global_sequences ORDER BY sequence_name";
        let fresh_seqs: Vec<(String, i64)> = sqlx::query_as(seq_query)
            .fetch_all(fresh.pool())
            .await
            .unwrap();
        let templated_seqs: Vec<(String, i64)> = sqlx::query_as(seq_query)
            .fetch_all(templated_a.pool())
            .await
            .unwrap();
        assert_eq!(templated_seqs, fresh_seqs);
    }

    #[tokio::test]
    async fn open_migrated_for_test_matches_fresh_file_database_and_supports_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let fresh_path = dir.path().join("fresh.db");
        let templated_path = dir.path().join("templated.db");

        let fresh = Database::open(fresh_path.to_str().unwrap()).await.unwrap();
        migrations::run_pending_migrations(fresh.pool())
            .await
            .unwrap();

        let templated = Database::open_migrated_for_test(templated_path.to_str().unwrap())
            .await
            .unwrap();

        let schema_query =
            "SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY type, name";
        let fresh_schema: Vec<(String, String, String, Option<String>)> =
            sqlx::query_as(schema_query)
                .fetch_all(fresh.pool())
                .await
                .unwrap();
        let templated_schema: Vec<(String, String, String, Option<String>)> =
            sqlx::query_as(schema_query)
                .fetch_all(templated.pool())
                .await
                .unwrap();
        assert_eq!(templated_schema, fresh_schema);

        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(templated.pool())
            .await
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "wal");
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(templated.pool())
            .await
            .unwrap();
        assert_eq!(fk, 1);
        assert_eq!(
            migrations::run_pending_migrations(templated.pool())
                .await
                .unwrap(),
            0
        );

        let id_before = templated.instance_id().await.unwrap();
        assert_ne!(id_before, fresh.instance_id().await.unwrap());
        templated
            .create_conversation("reopen-conv", "reopen-conv", "/tmp", true, None, None)
            .await
            .unwrap();
        templated.pool().close().await;
        drop(templated);

        let reopened = Database::open(templated_path.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(reopened.instance_id().await.unwrap(), id_before);
        assert!(reopened.get_conversation("reopen-conv").await.is_ok());
    }

    #[tokio::test]
    async fn prefix_templates_match_fresh_run_through_for_test_schema_and_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let fresh_path = dir.path().join("fresh-142.db");
        let cached_path = dir.path().join("cached-142.db");

        let fresh = Database::open(fresh_path.to_str().unwrap()).await.unwrap();
        migrations::run_through_for_test(fresh.pool(), 142)
            .await
            .unwrap();

        let cached_file = open_prefix_for_test(cached_path.to_str().unwrap(), 142)
            .await
            .unwrap();
        let cached_mem = open_in_memory_prefix_for_test(142).await.unwrap();

        let schema_query =
            "SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY type, name";
        let fresh_schema: Vec<(String, String, String, Option<String>)> =
            sqlx::query_as(schema_query)
                .fetch_all(fresh.pool())
                .await
                .unwrap();
        let file_schema: Vec<(String, String, String, Option<String>)> =
            sqlx::query_as(schema_query)
                .fetch_all(cached_file.pool())
                .await
                .unwrap();
        let mem_schema: Vec<(String, String, String, Option<String>)> =
            sqlx::query_as(schema_query)
                .fetch_all(cached_mem.pool())
                .await
                .unwrap();
        assert_eq!(file_schema, fresh_schema);
        assert_eq!(mem_schema, fresh_schema);

        let mig_query = "SELECT version, name FROM _migrations ORDER BY version";
        let fresh_migs: Vec<(i64, String)> = sqlx::query_as(mig_query)
            .fetch_all(fresh.pool())
            .await
            .unwrap();
        let file_migs: Vec<(i64, String)> = sqlx::query_as(mig_query)
            .fetch_all(cached_file.pool())
            .await
            .unwrap();
        let mem_migs: Vec<(i64, String)> = sqlx::query_as(mig_query)
            .fetch_all(cached_mem.pool())
            .await
            .unwrap();
        assert_eq!(file_migs, fresh_migs);
        assert_eq!(mem_migs, fresh_migs);
        assert_ne!(
            cached_file.instance_id().await.unwrap(),
            cached_mem.instance_id().await.unwrap()
        );
    }
}
