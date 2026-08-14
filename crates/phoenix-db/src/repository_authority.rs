use crate::{DbError, DbResult};
use sqlx::{Connection, SqliteConnection, SqlitePool};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryAuthorityGeneration {
    Project,
    GitRepository,
}

impl RepositoryAuthorityGeneration {
    fn parse(value: i64) -> DbResult<Self> {
        match value {
            1 => Ok(Self::Project),
            2 => Ok(Self::GitRepository),
            _ => Err(DbError::RepositoryAuthorityGenerationCorrupt { value }),
        }
    }
}

impl std::fmt::Display for RepositoryAuthorityGeneration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Project => "project generation 1",
            Self::GitRepository => "git-repository generation 2",
        })
    }
}

pub(crate) async fn probe_existing_generation(
    path: &str,
) -> DbResult<Option<RepositoryAuthorityGeneration>> {
    if !std::path::Path::new(path).exists() {
        return Ok(None);
    }
    let options = sqlx::sqlite::SqliteConnectOptions::from_str(&format!("sqlite:{path}?mode=ro"))?
        .read_only(true);
    let mut connection = SqliteConnection::connect_with(&options).await?;
    read_optional_connection_generation(&mut connection).await
}

pub(crate) async fn read_optional_connection_generation(
    connection: &mut SqliteConnection,
) -> DbResult<Option<RepositoryAuthorityGeneration>> {
    let table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'repository_authority_generation'
         )",
    )
    .fetch_one(&mut *connection)
    .await?;
    if !table_exists {
        let ledger_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = '_migrations'
             )",
        )
        .fetch_one(&mut *connection)
        .await?;
        if ledger_exists {
            let migration_recorded: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM _migrations WHERE version = 66)")
                    .fetch_one(&mut *connection)
                    .await?;
            if migration_recorded {
                return Err(DbError::RepositoryAuthorityGenerationRowMissing);
            }
        }
        return Ok(None);
    }
    let value = sqlx::query_scalar::<_, i64>(
        "SELECT generation FROM repository_authority_generation WHERE singleton = 1",
    )
    .fetch_optional(connection)
    .await?
    .ok_or(DbError::RepositoryAuthorityGenerationRowMissing)?;
    RepositoryAuthorityGeneration::parse(value).map(Some)
}

pub(crate) fn authority_db_error(error: sqlx::Error) -> DbError {
    let sqlx::Error::Configuration(source) = error else {
        return DbError::Sqlx(error);
    };
    match source.downcast::<DbError>() {
        Ok(error) => *error,
        Err(source) => DbError::Sqlx(sqlx::Error::Configuration(source)),
    }
}

fn authority_sqlx_error(error: DbError) -> sqlx::Error {
    sqlx::Error::Configuration(Box::new(error))
}

pub(crate) async fn require_connection_generation(
    connection: &mut SqliteConnection,
    expected: RepositoryAuthorityGeneration,
) -> Result<(), sqlx::Error> {
    let actual = read_optional_connection_generation(connection)
        .await
        .map_err(authority_sqlx_error)?
        .ok_or_else(|| {
            authority_sqlx_error(DbError::RepositoryAuthorityGenerationMissing { expected })
        })?;
    if actual == expected {
        Ok(())
    } else {
        Err(authority_sqlx_error(
            DbError::RepositoryAuthorityGenerationMismatch { expected, actual },
        ))
    }
}

pub(crate) async fn project_bootstrap_connection_is_eligible(
    connection: &mut SqliteConnection,
) -> Result<bool, sqlx::Error> {
    let generation = read_optional_connection_generation(connection)
        .await
        .map_err(authority_sqlx_error)?;
    Ok(matches!(
        generation,
        None | Some(RepositoryAuthorityGeneration::Project)
    ))
}

pub(crate) async fn prepare_connection(
    connection: &mut SqliteConnection,
    expected: RepositoryAuthorityGeneration,
) -> Result<(), sqlx::Error> {
    require_connection_generation(connection, expected).await?;
    sqlx::raw_sql("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
        .execute(connection)
        .await?;
    Ok(())
}

pub(crate) async fn connection_generation_matches(
    connection: &mut SqliteConnection,
    expected: RepositoryAuthorityGeneration,
) -> Result<bool, sqlx::Error> {
    let actual = read_optional_connection_generation(connection)
        .await
        .map_err(authority_sqlx_error)?
        .ok_or_else(|| {
            authority_sqlx_error(DbError::RepositoryAuthorityGenerationMissing { expected })
        })?;
    Ok(actual == expected)
}

pub(crate) async fn read_generation(pool: &SqlitePool) -> DbResult<RepositoryAuthorityGeneration> {
    let value: i64 = sqlx::query_scalar(
        "SELECT generation
         FROM repository_authority_generation
         WHERE singleton = 1",
    )
    .fetch_one(pool)
    .await
    .map_err(authority_db_error)?;
    RepositoryAuthorityGeneration::parse(value)
}

pub(crate) async fn require_generation(
    pool: &SqlitePool,
    expected: RepositoryAuthorityGeneration,
) -> DbResult<()> {
    let actual = read_generation(pool).await?;
    if actual == expected {
        Ok(())
    } else {
        Err(DbError::RepositoryAuthorityGenerationMismatch { expected, actual })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;

    #[tokio::test]
    async fn project_opener_bootstraps_and_requires_generation_one() {
        let db = Database::open_in_memory_project_authority().await.unwrap();
        assert_eq!(
            read_generation(db.pool()).await.unwrap(),
            RepositoryAuthorityGeneration::Project
        );
    }

    #[tokio::test]
    async fn role_specific_openers_reject_the_opposite_generation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("authority.db");
        let path = path.to_string_lossy().into_owned();
        let db = Database::open_project_authority(&path).await.unwrap();
        sqlx::query(
            "UPDATE repository_authority_generation
             SET generation = 2
             WHERE singleton = 1",
        )
        .execute(db.pool())
        .await
        .unwrap();
        drop(db);

        assert!(matches!(
            Database::open_project_authority(&path).await,
            Err(DbError::RepositoryAuthorityGenerationMismatch {
                expected: RepositoryAuthorityGeneration::Project,
                actual: RepositoryAuthorityGeneration::GitRepository,
            })
        ));
        Database::open_git_repository_authority(&path)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn project_opener_bootstraps_an_existing_legacy_database() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.db");
        let path = path.to_string_lossy().into_owned();
        let options =
            sqlx::sqlite::SqliteConnectOptions::from_str(&format!("sqlite:{path}?mode=rwc"))
                .unwrap();
        let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
        sqlx::query("CREATE TABLE legacy_marker (id INTEGER PRIMARY KEY)")
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();

        let db = Database::open_project_authority(&path).await.unwrap();
        assert_eq!(
            read_generation(db.pool()).await.unwrap(),
            RepositoryAuthorityGeneration::Project
        );
        let marker_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'legacy_marker'
             )",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(marker_exists);
    }

    #[tokio::test]
    async fn wrong_role_rejection_does_not_enable_wal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wrong-role.db");
        let path = path.to_string_lossy().into_owned();
        let options =
            sqlx::sqlite::SqliteConnectOptions::from_str(&format!("sqlite:{path}?mode=rwc"))
                .unwrap();
        let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE repository_authority_generation (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 generation INTEGER NOT NULL CHECK (generation IN (1, 2))
             );
             INSERT INTO repository_authority_generation (singleton, generation)
             VALUES (1, 1);",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        connection.close().await.unwrap();

        assert!(matches!(
            Database::open_git_repository_authority(&path).await,
            Err(DbError::RepositoryAuthorityGenerationMismatch { .. })
        ));

        let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(journal_mode, "delete");
    }

    #[tokio::test]
    async fn malformed_generation_rows_fail_closed() {
        for mutation in [
            "DELETE FROM repository_authority_generation",
            "UPDATE repository_authority_generation SET generation = 3",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("malformed.db");
            let path = path.to_string_lossy().into_owned();
            let db = Database::open_project_authority(&path).await.unwrap();
            sqlx::raw_sql(
                "DROP TRIGGER repository_authority_generation_no_delete;
                 DROP TRIGGER repository_authority_generation_monotonic_update;",
            )
            .execute(db.pool())
            .await
            .unwrap();
            let mut connection = db.pool().acquire().await.unwrap();
            sqlx::query("PRAGMA ignore_check_constraints = ON")
                .execute(&mut *connection)
                .await
                .unwrap();
            sqlx::query(mutation)
                .execute(&mut *connection)
                .await
                .unwrap();
            drop(connection);
            drop(db);

            let Err(error) = Database::open_project_authority(&path).await else {
                panic!("malformed authority generation must reject open");
            };
            assert!(matches!(
                error,
                DbError::RepositoryAuthorityGenerationRowMissing
                    | DbError::RepositoryAuthorityGenerationCorrupt { value: 3 }
            ));
        }
    }

    #[tokio::test]
    async fn post_migration_missing_generation_table_fails_before_writable_open() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing-table.db");
        let path = path.to_string_lossy().into_owned();
        let db = Database::open_project_authority(&path).await.unwrap();
        sqlx::query("DROP TABLE repository_authority_generation")
            .execute(db.pool())
            .await
            .unwrap();
        drop(db);

        assert!(matches!(
            Database::open_project_authority(&path).await,
            Err(DbError::RepositoryAuthorityGenerationRowMissing)
        ));
        let options =
            sqlx::sqlite::SqliteConnectOptions::from_str(&format!("sqlite:{path}?mode=ro"))
                .unwrap()
                .read_only(true);
        let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
        let table_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'repository_authority_generation'
             )",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert!(!table_exists);
    }

    #[tokio::test]
    async fn guarded_connection_rejects_wrong_role_before_enabling_wal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("guarded-race.db");
        let path = path.to_string_lossy().into_owned();
        let options =
            sqlx::sqlite::SqliteConnectOptions::from_str(&format!("sqlite:{path}?mode=rwc"))
                .unwrap();
        let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE repository_authority_generation (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 generation INTEGER NOT NULL CHECK (generation IN (1, 2))
             );
             INSERT INTO repository_authority_generation (singleton, generation)
             VALUES (1, 1);",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        let error = prepare_connection(
            &mut connection,
            RepositoryAuthorityGeneration::GitRepository,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, sqlx::Error::Configuration(_)));
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(journal_mode, "delete");
    }

    #[tokio::test]
    async fn project_pool_rejects_new_work_after_generation_flips() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stale.db");
        let path = path.to_string_lossy().into_owned();
        let db = Database::open_project_authority(&path).await.unwrap();
        sqlx::query(
            "UPDATE repository_authority_generation
             SET generation = 2
             WHERE singleton = 1",
        )
        .execute(db.pool())
        .await
        .unwrap();

        assert!(sqlx::query_scalar::<_, i64>("SELECT 1")
            .fetch_one(db.pool())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn project_pool_rejects_new_physical_connections_after_generation_flips() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stale-new-connection.db");
        let path = path.to_string_lossy().into_owned();
        let db = Database::open_project_authority(&path).await.unwrap();
        let held_connection = db.pool().acquire().await.unwrap();

        let options =
            sqlx::sqlite::SqliteConnectOptions::from_str(&format!("sqlite:{path}?mode=rw"))
                .unwrap();
        let mut activation_connection = SqliteConnection::connect_with(&options).await.unwrap();
        sqlx::query(
            "UPDATE repository_authority_generation
             SET generation = 2
             WHERE singleton = 1",
        )
        .execute(&mut activation_connection)
        .await
        .unwrap();

        assert!(sqlx::query_scalar::<_, i64>("SELECT 1")
            .fetch_one(db.pool())
            .await
            .is_err());
        drop(held_connection);
    }

    #[tokio::test]
    async fn git_repository_opener_never_bootstraps_an_unversioned_database() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("empty.db");
        let path = path.to_string_lossy().into_owned();

        assert!(matches!(
            Database::open_git_repository_authority(&path).await,
            Err(DbError::RepositoryAuthorityGenerationMissing {
                expected: RepositoryAuthorityGeneration::GitRepository,
            })
        ));
        assert!(!std::path::Path::new(&path).exists());
    }
}
