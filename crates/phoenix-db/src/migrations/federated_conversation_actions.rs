use phoenix_core::domain::product_conversation::ProductConversationId;
use sqlx::{Connection, SqlitePool};

use super::{DbError, DbResult, Migration, WritableSchemaGuard};

#[cfg(test)]
mod tests;

fn live_locator(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|value| !value.trim().is_empty() && !value.contains('\0'))
}

fn live_creation_locators(locators: &[&[u8]; 4]) -> bool {
    locators.iter().all(|bytes| live_locator(bytes))
        && std::str::from_utf8(locators[0])
            .is_ok_and(|value| ProductConversationId::parse(value.to_owned()).is_ok())
}

fn live_rename_locators(locators: &[&[u8]; 4]) -> bool {
    locators.iter().all(|bytes| live_locator(bytes))
}

const CREATE_KIND_CHECK: &str = "objective_origin IN ('unknown_historical', 'user_api', 'slack_gateway', 'internal_conversation')";
const CREATE_LEGACY_PROVENANCE_CHECK: &str = r"CHECK ((objective_origin = 'internal_conversation'
        AND objective_source_product_conversation_id IS NOT NULL AND typeof(objective_source_product_conversation_id) = 'text' AND length(trim(objective_source_product_conversation_id)) > 0
        AND objective_source_transcript_id IS NOT NULL AND typeof(objective_source_transcript_id) = 'text' AND length(trim(objective_source_transcript_id)) > 0
        AND objective_source_message_id IS NOT NULL AND typeof(objective_source_message_id) = 'text' AND length(trim(objective_source_message_id)) > 0
        AND objective_source_tool_use_id IS NOT NULL AND typeof(objective_source_tool_use_id) = 'text' AND length(trim(objective_source_tool_use_id)) > 0)
      OR (objective_origin IN ('unknown_historical', 'user_api', 'slack_gateway')
        AND objective_source_product_conversation_id IS NULL AND objective_source_transcript_id IS NULL
        AND objective_source_message_id IS NULL AND objective_source_tool_use_id IS NULL))";
const RUST_WHITESPACE_SQL: &str = "char(9,10,11,12,13,32,133,160,5760,8192,8193,8194,8195,8196,8197,8198,8199,8200,8201,8202,8232,8233,8239,8287,12288)";

type HistoricalLocatorRow = (i64, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);

fn locators_complete(columns: &[&str]) -> String {
    columns
        .iter()
        .map(|column| {
            format!(
                "typeof({column}) = 'text' AND length(trim({column}, {RUST_WHITESPACE_SQL})) > 0 AND instr(CAST({column} AS BLOB), x'00') = 0"
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn creation_locators_complete() -> String {
    locators_complete(&[
        "objective_source_product_conversation_id",
        "objective_source_transcript_id",
        "objective_source_message_id",
        "objective_source_tool_use_id",
    ])
}

fn rename_locators_complete() -> String {
    locators_complete(&[
        "source_product_conversation_id",
        "source_transcript_id",
        "source_message_id",
        "source_tool_use_id",
    ])
}

fn instance_uuid_check(column: &str) -> String {
    format!(
        "typeof({column}) = 'text' AND length({column}) = 36 AND length(CAST({column} AS BLOB)) = 36 AND {column} = lower({column}) AND substr({column},9,1) = '-' AND substr({column},14,1) = '-' AND substr({column},15,1) = '4' AND substr({column},19,1) = '-' AND substr({column},20,1) IN ('8','9','a','b') AND substr({column},24,1) = '-' AND length(replace({column},'-','')) = 32 AND replace({column},'-','') NOT GLOB '*[^0-9a-f]*'"
    )
}

fn legacy_creation_locators() -> String {
    [
        "objective_source_product_conversation_id",
        "objective_source_transcript_id",
        "objective_source_message_id",
        "objective_source_tool_use_id",
    ]
    .into_iter()
    .map(|column| format!("typeof({column}) = 'text' AND length(trim({column})) > 0"))
    .collect::<Vec<_>>()
    .join(" AND ")
}

fn creation_provenance_check() -> String {
    let complete = creation_locators_complete();
    let legacy_locators = legacy_creation_locators();
    let uuid_check = instance_uuid_check("objective_source_instance_id");
    format!(
        "CHECK ((objective_origin IN ('internal_conversation', 'federated_conversation') AND COALESCE(({complete}), 0) AND objective_source_instance_id IS NOT NULL AND {uuid_check} AND objective_slack_request_id IS NULL) OR (objective_origin = 'historical_internal' AND {legacy_locators} AND objective_source_instance_id IS NULL AND objective_slack_request_id IS NULL) OR (objective_origin IN ('unknown_historical', 'user_api') AND objective_source_product_conversation_id IS NULL AND objective_source_transcript_id IS NULL AND objective_source_message_id IS NULL AND objective_source_tool_use_id IS NULL AND objective_source_instance_id IS NULL AND objective_slack_request_id IS NULL) OR (objective_origin = 'slack_gateway' AND objective_source_product_conversation_id IS NULL AND objective_source_transcript_id IS NULL AND objective_source_message_id IS NULL AND objective_source_tool_use_id IS NULL AND objective_source_instance_id IS NULL AND typeof(objective_slack_request_id) = 'text'))"
    )
}

fn rename_provenance_check() -> String {
    let complete = rename_locators_complete();
    let uuid_check = instance_uuid_check("source_instance_id");
    format!(
        "CHECK ((COALESCE(({complete}), 0) AND source_instance_id IS NOT NULL AND {uuid_check}) OR (source_instance_id IS NULL AND typeof(source_product_conversation_id) = 'text' AND typeof(source_transcript_id) = 'text' AND typeof(source_message_id) = 'text' AND typeof(source_tool_use_id) = 'text'))"
    )
}

fn qualify_schema(sql: &str, creation: bool) -> DbResult<String> {
    let mut sql = sql.to_owned();
    if creation {
        if sql.matches(CREATE_KIND_CHECK).count() != 1
            || sql.matches(CREATE_LEGACY_PROVENANCE_CHECK).count() != 1
        {
            return Err(DbError::Serialization(
                "migration 158 unexpected creation origin constraints".into(),
            ));
        }
        sql = sql.replace(
            CREATE_KIND_CHECK,
            "objective_origin IN ('unknown_historical', 'user_api', 'slack_gateway', 'internal_conversation', 'historical_internal', 'federated_conversation')",
        );
    } else {
        for column in [
            "source_product_conversation_id",
            "source_transcript_id",
            "source_message_id",
            "source_tool_use_id",
        ] {
            let nonempty = format!("AND length({column}) > 0");
            if sql.matches(&nonempty).count() != 1 {
                return Err(DbError::Serialization(format!(
                    "migration 158 unexpected rename locator constraint for {column}"
                )));
            }
        }
    }
    let check = if creation {
        creation_provenance_check()
    } else {
        rename_provenance_check()
    };
    if creation {
        sql = sql.replace(CREATE_LEGACY_PROVENANCE_CHECK, &check);
    } else {
        let end = sql
            .rfind(')')
            .ok_or_else(|| DbError::Serialization("migration 158 missing table body".into()))?;
        sql.insert_str(end, &format!(",\n    {check}\n"));
    }
    Ok(sql)
}

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut guard = WritableSchemaGuard::enable(pool).await?;
    let result: DbResult<()> = async {
        let mut tx = guard.connection().begin().await?;
        let local: String = sqlx::query_scalar("SELECT instance_id FROM instance_identity WHERE singleton_key = 1").fetch_one(&mut *tx).await?;
        local.parse::<phoenix_core::domain::instance_identity::InstanceId>()
            .map_err(|error| DbError::Serialization(format!("migration 158 invalid local instance identity: {error}")))?;
        let creation_rows: Vec<HistoricalLocatorRow> = sqlx::query_as(
            "SELECT rowid, CAST(objective_source_product_conversation_id AS BLOB), CAST(objective_source_transcript_id AS BLOB), CAST(objective_source_message_id AS BLOB), CAST(objective_source_tool_use_id AS BLOB) FROM product_creation_jobs WHERE objective_origin = 'internal_conversation'",
        )
        .fetch_all(&mut *tx).await?;
        let qualified_creations: Vec<i64> = creation_rows.into_iter()
            .filter(|(_, product, transcript, message, tool)| live_creation_locators(&[product, transcript, message, tool]))
            .map(|(rowid, _, _, _, _)| rowid)
            .collect();
        let rename_rows: Vec<HistoricalLocatorRow> = sqlx::query_as(
            "SELECT rowid, CAST(source_product_conversation_id AS BLOB), CAST(source_transcript_id AS BLOB), CAST(source_message_id AS BLOB), CAST(source_tool_use_id AS BLOB) FROM product_conversation_rename_receipts",
        )
        .fetch_all(&mut *tx).await?;
        let qualified_renames: Vec<i64> = rename_rows.into_iter()
            .filter(|(_, product, transcript, message, tool)| live_rename_locators(&[product, transcript, message, tool]))
            .map(|(rowid, _, _, _, _)| rowid)
            .collect();
        sqlx::raw_sql("ALTER TABLE product_creation_jobs ADD COLUMN objective_source_instance_id TEXT;
            ALTER TABLE product_conversation_rename_receipts ADD COLUMN source_instance_id TEXT;")
            .execute(&mut *tx).await?;
        // Updating only these definitions retains every existing FK, index, and trigger in place.
        for (table, creation) in [("product_creation_jobs", true), ("product_conversation_rename_receipts", false)] {
            let sql: String = sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1")
                .bind(table).fetch_one(&mut *tx).await?;
            sqlx::query("UPDATE sqlite_schema SET sql=?2 WHERE type='table' AND name=?1")
                .bind(table).bind(qualify_schema(&sql, creation)?).execute(&mut *tx).await?;
        }
        let version: i64 = sqlx::query_scalar("PRAGMA schema_version").fetch_one(&mut *tx).await?;
        sqlx::query(sqlx::AssertSqlSafe(format!("PRAGMA schema_version = {}", version + 1))).execute(&mut *tx).await?;
        sqlx::query("SELECT objective_source_instance_id FROM product_creation_jobs LIMIT 0").execute(&mut *tx).await?;
        sqlx::query("SELECT source_instance_id FROM product_conversation_rename_receipts LIMIT 0").execute(&mut *tx).await?;
        sqlx::query("UPDATE product_creation_jobs SET objective_origin = 'historical_internal' WHERE objective_origin = 'internal_conversation'")
            .execute(&mut *tx).await?;
        for rowid in qualified_creations {
            sqlx::query("UPDATE product_creation_jobs SET objective_origin = 'internal_conversation', objective_source_instance_id = ?1 WHERE rowid = ?2")
                .bind(&local).bind(rowid).execute(&mut *tx).await?;
        }
        for rowid in qualified_renames {
            sqlx::query("UPDATE product_conversation_rename_receipts SET source_instance_id = ?1 WHERE rowid = ?2")
                .bind(&local).bind(rowid).execute(&mut *tx).await?;
        }
        let integrity: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_all(&mut *tx)
            .await?;
        if integrity.len() != 1 || integrity[0] != "ok" {
            return Err(DbError::Serialization(format!(
                "migration 158 produced an invalid schema: {}",
                integrity.join("; ")
            )));
        }
        sqlx::query("INSERT INTO _migrations(version,name) VALUES (?1,?2)")
            .bind(migration.version).bind(migration.name).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }.await;
    let restore = guard.disable().await;
    result?;
    restore
}
