use libsqlite3_sys as ffi;
use phoenix_core::domain::instance_identity::InstanceId;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteConnection, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
};
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::OnceCell;

use crate::sqlite_native_statement::install_native_statement_baseline;
use crate::{ddl, migrations, Database, DbError, DbResult, SqliteWorkloadCollector};

const MAIN_DB_NAME: &[u8] = b"main\0";
const SQLITE_HEADER_MAGIC: &[u8] = b"SQLite format 3\0";
const MIN_SQLITE_BYTES: usize = 4096;
const TEMPLATE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

static IN_MEMORY_TEMPLATE: OnceCell<Vec<u8>> = OnceCell::const_new();

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
    let dir = std::env::temp_dir().join(format!("phoenix-db-mem-tpl-{uid}"));
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
    let dir = std::env::temp_dir().join("phoenix-db-mem-tpl");
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
        unsafe {
            ffi::sqlite3_free(buf.cast());
        }
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
}
