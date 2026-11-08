use sqlx::{Connection, SqlitePool};

use super::{DbError, DbResult, Migration, WritableSchemaGuard};

#[cfg(test)]
mod tests;

const CREATE_KIND_CHECK: &str = "objective_origin IN ('unknown_historical', 'user_api', 'slack_gateway', 'internal_conversation')";
const CREATE_NATIVE_CHECK: &str = "objective_origin = 'internal_conversation'";

fn qualify_schema(sql: &str, creation: bool) -> DbResult<String> {
    let mut sql = sql.to_owned();
    if creation {
        if sql.matches(CREATE_KIND_CHECK).count() != 1
            || sql.matches(CREATE_NATIVE_CHECK).count() != 1
        {
            return Err(DbError::Serialization(
                "migration 158 unexpected creation origin constraints".into(),
            ));
        }
        sql = sql.replace(CREATE_KIND_CHECK, "objective_origin IN ('unknown_historical', 'user_api', 'slack_gateway', 'internal_conversation', 'federated_conversation')")
            .replace(CREATE_NATIVE_CHECK, "objective_origin IN ('internal_conversation', 'federated_conversation')");
    }
    let column = if creation {
        "objective_source_instance_id"
    } else {
        "source_instance_id"
    };
    let uuid_check = format!("typeof({column}) = 'text' AND length({column}) = 36 AND {column} = lower({column}) AND substr({column},9,1) = '-' AND substr({column},14,1) = '-' AND substr({column},15,1) = '4' AND substr({column},19,1) = '-' AND substr({column},20,1) IN ('8','9','a','b') AND substr({column},24,1) = '-' AND length(replace({column},'-','')) = 32 AND replace({column},'-','') NOT GLOB '*[^0-9a-f]*'");
    let check = if creation {
        format!("CHECK ((objective_origin IN ('internal_conversation', 'federated_conversation') AND {column} IS NOT NULL AND {uuid_check} AND objective_slack_request_id IS NULL AND length(trim(objective_source_product_conversation_id)) > 0 AND length(trim(objective_source_transcript_id)) > 0 AND length(trim(objective_source_message_id)) > 0 AND length(trim(objective_source_tool_use_id)) > 0) OR (objective_origin IN ('unknown_historical', 'user_api', 'slack_gateway') AND {column} IS NULL))")
    } else {
        format!("CHECK (length(trim(source_product_conversation_id)) > 0 AND length(trim(source_transcript_id)) > 0 AND length(trim(source_message_id)) > 0 AND length(trim(source_tool_use_id)) > 0 AND {column} IS NOT NULL AND {uuid_check})")
    };
    let end = sql
        .rfind(')')
        .ok_or_else(|| DbError::Serialization("migration 158 missing table body".into()))?;
    sql.insert_str(end, &format!(",\n    {check}\n"));
    Ok(sql)
}

pub(super) async fn run(pool: &SqlitePool, migration: &Migration) -> DbResult<()> {
    let mut guard = WritableSchemaGuard::enable(pool).await?;
    let result: DbResult<()> = async {
        let mut tx = guard.connection().begin().await?;
        let local: String = sqlx::query_scalar("SELECT instance_id FROM instance_identity WHERE singleton_key = 1").fetch_one(&mut *tx).await?;
        local.parse::<phoenix_core::domain::instance_identity::InstanceId>()
            .map_err(|error| DbError::Serialization(format!("migration 158 invalid local instance identity: {error}")))?;
        let incomplete_native_creations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM product_creation_jobs WHERE objective_origin = 'internal_conversation' AND (length(trim(objective_source_product_conversation_id)) = 0 OR length(trim(objective_source_transcript_id)) = 0 OR length(trim(objective_source_message_id)) = 0 OR length(trim(objective_source_tool_use_id)) = 0)")
            .fetch_one(&mut *tx).await?;
        let incomplete_rename_receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM product_conversation_rename_receipts WHERE length(trim(source_product_conversation_id)) = 0 OR length(trim(source_transcript_id)) = 0 OR length(trim(source_message_id)) = 0 OR length(trim(source_tool_use_id)) = 0")
            .fetch_one(&mut *tx).await?;
        if incomplete_native_creations != 0 || incomplete_rename_receipts != 0 {
            return Err(DbError::Serialization(format!("migration 158 cannot qualify incomplete historical native locators (creation jobs: {incomplete_native_creations}, rename receipts: {incomplete_rename_receipts})")));
        }
        sqlx::raw_sql("ALTER TABLE product_creation_jobs ADD COLUMN objective_source_instance_id TEXT;
            ALTER TABLE product_conversation_rename_receipts ADD COLUMN source_instance_id TEXT;
            UPDATE product_creation_jobs SET objective_source_instance_id = (SELECT instance_id FROM instance_identity WHERE singleton_key = 1)
            WHERE objective_origin = 'internal_conversation';
            UPDATE product_conversation_rename_receipts SET source_instance_id = (SELECT instance_id FROM instance_identity WHERE singleton_key = 1);")
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
        sqlx::query("INSERT INTO _migrations(version,name) VALUES (?1,?2)")
            .bind(migration.version).bind(migration.name).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }.await;
    let restore = guard.disable().await;
    result?;
    restore
}
