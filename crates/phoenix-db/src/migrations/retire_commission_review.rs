use serde::Deserialize;
use std::fmt::Write as _;

use sha2::{Digest, Sha256};
use sqlx::{Acquire, Row, Sqlite, SqlitePool, Transaction};

use phoenix_core::domain::db_schema::{Message, MessageContent, MessageType};
use phoenix_core::domain::llm_types::ContentBlock;
use phoenix_core::domain::sm_state::{AssistantMessage, ConvState};

use crate::{DbError, DbResult};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LegacyCommissionReviewState {
    AwaitingCommissionReviewApproval {
        tool_use_id: String,
        #[serde(rename = "request")]
        _request: LegacyCommissionReviewInput,
        #[serde(rename = "scope")]
        _scope: LegacyCommissionReviewScope,
        assistant_message: AssistantMessage,
    },
}

#[derive(Deserialize)]
struct LegacyCommissionReviewInput {
    #[serde(rename = "brief")]
    _brief: String,
    #[serde(rename = "focus")]
    _focus: Option<String>,
}

#[derive(Deserialize)]
struct LegacyCommissionReviewScope {
    #[serde(rename = "kind")]
    _kind: String,
    #[serde(rename = "repo_root")]
    _repo_root: String,
    #[serde(rename = "base")]
    _base: String,
    #[serde(rename = "head")]
    _head: String,
    #[serde(rename = "approved_head")]
    _approved_head: Option<String>,
    #[serde(rename = "approved_base")]
    _approved_base: Option<String>,
    #[serde(rename = "dirty")]
    _dirty: bool,
    #[serde(rename = "changed_files")]
    _changed_files: usize,
    #[serde(rename = "insertions")]
    _insertions: u64,
    #[serde(rename = "deletions")]
    _deletions: u64,
}

impl LegacyCommissionReviewState {
    fn into_parts(self) -> (String, AssistantMessage) {
        let Self::AwaitingCommissionReviewApproval {
            tool_use_id,
            _request: _,
            _scope: _,
            assistant_message,
        } = self;
        (tool_use_id, assistant_message)
    }
}

async fn insert_message(tx: &mut Transaction<'_, Sqlite>, message: &Message) -> DbResult<()> {
    let content = serde_json::to_string(&message.content.to_stored_json())
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let display_data = message
        .display_data
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    let usage_data = message
        .usage_data
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| DbError::Serialization(error.to_string()))?;
    sqlx::query(
        "INSERT INTO messages (
             message_id, conversation_id, sequence_id, message_type,
             content, display_data, usage_data, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(&message.message_id)
    .bind(&message.conversation_id)
    .bind(message.sequence_id)
    .bind(message.message_type.to_string())
    .bind(content)
    .bind(display_data)
    .bind(usage_data)
    .bind(message.created_at.to_rfc3339())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn validate_tool_call(
    conversation_id: &str,
    tool_use_id: &str,
    assistant_message: &AssistantMessage,
) -> DbResult<()> {
    let matching_calls = assistant_message
        .content
        .iter()
        .filter(|block| {
            matches!(
                block,
                ContentBlock::ToolUse { id, name, .. }
                    if id == tool_use_id && name == "commission_review"
            )
        })
        .count();
    if matching_calls != 1 {
        return Err(DbError::Serialization(format!(
            "migration 68 expected exactly one commission_review tool use {tool_use_id} in conversation {conversation_id}, found {matching_calls}"
        )));
    }
    Ok(())
}

fn recovery_result_id(conversation_id: &str, tool_use_id: &str) -> String {
    let digest = Sha256::digest(format!(
        "commission-review-retirement\0{conversation_id}\0{tool_use_id}"
    ));
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("commission-review-retired-{hex}")
}

const RECOVERY_SETTLEMENT_SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS conversation_recovery_settlements (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    terminal_message_id TEXT NOT NULL REFERENCES messages(message_id) ON DELETE CASCADE,
    reason TEXT NOT NULL CHECK (reason IN ('retired_tool_call')),
    created_at TEXT NOT NULL
);

CREATE TRIGGER IF NOT EXISTS conversation_recovery_settlements_message_owner_insert
BEFORE INSERT ON conversation_recovery_settlements
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM messages
    WHERE message_id = NEW.terminal_message_id
      AND conversation_id = NEW.conversation_id
)
BEGIN
    SELECT RAISE(ABORT, 'recovery settlement message must belong to conversation');
END;

CREATE TRIGGER IF NOT EXISTS conversation_recovery_settlements_message_owner_update
BEFORE UPDATE ON conversation_recovery_settlements
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM messages
    WHERE message_id = NEW.terminal_message_id
      AND conversation_id = NEW.conversation_id
)
BEGIN
    SELECT RAISE(ABORT, 'recovery settlement message must belong to conversation');
END;
";

async fn recover_pending_reviews(tx: &mut Transaction<'_, Sqlite>) -> DbResult<()> {
    sqlx::raw_sql(RECOVERY_SETTLEMENT_SCHEMA)
        .execute(&mut **tx)
        .await?;
    let rows = sqlx::query(
        "SELECT id, state FROM conversations
         WHERE state_kind = 'awaiting_commission_review_approval'
         ORDER BY id",
    )
    .fetch_all(&mut **tx)
    .await?;

    for row in rows {
        let conversation_id: String = row.get("id");
        let state_json: String = row.get("state");
        let legacy: LegacyCommissionReviewState = serde_json::from_str(&state_json).map_err(
            |error| {
                DbError::Serialization(format!(
                    "migration 68 cannot decode pending commission review state for conversation {conversation_id}: {error}"
                ))
            },
        )?;
        let (tool_use_id, assistant_message) = legacy.into_parts();
        validate_tool_call(&conversation_id, &tool_use_id, &assistant_message)?;
        sqlx::query(
            "DELETE FROM messages
             WHERE conversation_id = ?1 AND message_id = ?2",
        )
        .bind(&conversation_id)
        .bind(&assistant_message.message_id)
        .execute(&mut **tx)
        .await?;

        remove_matching_tool_results(tx, &conversation_id, &tool_use_id).await?;

        let next_sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence_id), 0) + 1 FROM messages WHERE conversation_id = ?1",
        )
        .bind(&conversation_id)
        .fetch_one(&mut **tx)
        .await?;
        insert_message(
            tx,
            &Message {
                message_id: assistant_message.message_id,
                conversation_id: conversation_id.clone(),
                sequence_id: next_sequence,
                message_type: MessageType::Agent,
                content: MessageContent::agent(assistant_message.content),
                display_data: assistant_message.display_data,
                usage_data: assistant_message.usage,
                created_at: assistant_message.created_at,
            },
        )
        .await?;
        let result_message_id = recovery_result_id(&conversation_id, &tool_use_id);
        insert_message(
            tx,
            &Message {
                message_id: result_message_id.clone(),
                conversation_id: conversation_id.clone(),
                sequence_id: next_sequence + 1,
                message_type: MessageType::Tool,
                content: MessageContent::tool(
                    &tool_use_id,
                    "commission_review is unavailable because the capability was retired",
                    true,
                ),
                display_data: None,
                usage_data: None,
                created_at: chrono::Utc::now(),
            },
        )
        .await?;

        sqlx::query(
            "INSERT INTO conversation_recovery_settlements (
                 conversation_id, terminal_message_id, reason, created_at
             ) VALUES (?1, ?2, 'retired_tool_call', STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        )
        .bind(&conversation_id)
        .bind(result_message_id)
        .execute(&mut **tx)
        .await?;

        let idle_state = serde_json::to_string(&ConvState::Idle)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        sqlx::query(
            "UPDATE conversations
             SET state = ?1, state_kind = 'idle',
                 state_updated_at = STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?2",
        )
        .bind(idle_state)
        .bind(conversation_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn remove_matching_tool_results(
    tx: &mut Transaction<'_, Sqlite>,
    conversation_id: &str,
    tool_use_id: &str,
) -> DbResult<()> {
    for message_id in matching_tool_result_ids(tx, conversation_id, tool_use_id).await? {
        sqlx::query("DELETE FROM messages WHERE conversation_id = ?1 AND message_id = ?2")
            .bind(conversation_id)
            .bind(message_id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn matching_tool_result_ids(
    tx: &mut Transaction<'_, Sqlite>,
    conversation_id: &str,
    tool_use_id: &str,
) -> DbResult<Vec<String>> {
    let rows = sqlx::query(
        "SELECT message_id, content FROM messages
         WHERE conversation_id = ?1 AND message_type = 'tool'",
    )
    .bind(conversation_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut matching = Vec::new();
    for row in rows {
        let message_id: String = row.get("message_id");
        let content: String = row.get("content");
        let value = serde_json::from_str::<serde_json::Value>(&content).map_err(|error| {
            DbError::Serialization(format!(
                "migration 68 cannot decode tool message {message_id} for conversation {conversation_id}: {error}"
            ))
        })?;
        let MessageContent::Tool(tool) = MessageContent::from_stored_json(MessageType::Tool, value)
            .map_err(DbError::Serialization)?
        else {
            return Err(DbError::Serialization(format!(
                "migration 68 found non-tool content in tool message {message_id} for conversation {conversation_id}"
            )));
        };
        if tool.tool_use_id == tool_use_id {
            matching.push(message_id);
        }
    }
    Ok(matching)
}

struct SchemaDependencies {
    local_objects: Vec<String>,
    views: Vec<(String, String)>,
    cross_table_triggers: Vec<(String, String)>,
}

async fn detach_schema_dependencies(
    tx: &mut Transaction<'_, Sqlite>,
) -> DbResult<SchemaDependencies> {
    let local_objects = sqlx::query_scalar(
        "SELECT sql FROM sqlite_schema
         WHERE tbl_name = 'conversations'
           AND type IN ('index', 'trigger')
           AND sql IS NOT NULL
         ORDER BY type, name",
    )
    .fetch_all(&mut **tx)
    .await?;
    let cross_table_triggers: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_schema
         WHERE type = 'trigger' AND tbl_name <> 'conversations'
           AND sql IS NOT NULL AND instr(lower(sql), 'conversations') > 0
         ORDER BY name",
    )
    .fetch_all(&mut **tx)
    .await?;
    let views: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_schema
         WHERE type = 'view' AND sql IS NOT NULL
           AND instr(lower(sql), 'conversations') > 0
         ORDER BY name",
    )
    .fetch_all(&mut **tx)
    .await?;
    for (object_type, objects) in [("TRIGGER", &cross_table_triggers), ("VIEW", &views)] {
        for (name, _) in objects {
            let quoted_name = format!("\"{}\"", name.replace('"', "\"\""));
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DROP {object_type} {quoted_name}"
            )))
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(SchemaDependencies {
        local_objects,
        views,
        cross_table_triggers,
    })
}

async fn restore_schema_dependencies(
    tx: &mut Transaction<'_, Sqlite>,
    dependencies: SchemaDependencies,
) -> DbResult<()> {
    for sql in dependencies.local_objects {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut **tx)
            .await?;
    }
    for (_, sql) in dependencies.views {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut **tx)
            .await?;
    }
    for (_, sql) in dependencies.cross_table_triggers {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn rebuild_conversations(tx: &mut Transaction<'_, Sqlite>) -> DbResult<()> {
    let table_sql: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'conversations'",
    )
    .fetch_one(&mut **tx)
    .await?;
    if !table_sql.contains("awaiting_commission_review_approval") {
        return Ok(());
    }
    let contracted_sql = table_sql.replacen("'awaiting_commission_review_approval',", "", 1);
    if contracted_sql == table_sql || contracted_sql.contains("awaiting_commission_review_approval")
    {
        return Err(DbError::Serialization(
            "migration 68 could not contract the conversations state_kind domain".to_string(),
        ));
    }
    let opening_paren = contracted_sql.find('(').ok_or_else(|| {
        DbError::Serialization("migration 68 found malformed conversations table SQL".to_string())
    })?;
    let table_body = contracted_sql.get(opening_paren..).ok_or_else(|| {
        DbError::Serialization("migration 68 found malformed UTF-8 table SQL".to_string())
    })?;
    let create_new_sql = format!("CREATE TABLE conversations_new {table_body}");
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('conversations') ORDER BY cid")
            .fetch_all(&mut **tx)
            .await?;
    if columns.is_empty() {
        return Err(DbError::Serialization(
            "migration 68 found no conversations columns".to_string(),
        ));
    }
    let quoted_columns = columns
        .iter()
        .map(|column| format!("\"{}\"", column.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let copy_sql = format!(
        "INSERT INTO conversations_new ({quoted_columns}) SELECT {quoted_columns} FROM conversations"
    );
    let dependencies = detach_schema_dependencies(tx).await?;

    sqlx::raw_sql(sqlx::AssertSqlSafe(create_new_sql))
        .execute(&mut **tx)
        .await?;
    sqlx::raw_sql(sqlx::AssertSqlSafe(copy_sql))
        .execute(&mut **tx)
        .await?;
    sqlx::raw_sql(
        "DROP TABLE conversations; ALTER TABLE conversations_new RENAME TO conversations;",
    )
    .execute(&mut **tx)
    .await?;
    restore_schema_dependencies(tx, dependencies).await?;
    let violation: Option<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_optional(&mut **tx)
        .await?;
    if let Some((table, rowid, parent, foreign_key)) = violation {
        return Err(DbError::Serialization(format!(
            "migration 68 foreign key check failed: table={table} rowid={rowid} parent={parent} foreign_key={foreign_key}"
        )));
    }
    Ok(())
}

pub(super) async fn run(pool: &SqlitePool, version: u32, name: &str) -> DbResult<()> {
    let mut connection = pool.acquire().await?;
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await?;
    let migration_result = async {
        let mut tx = connection.begin().await?;
        recover_pending_reviews(&mut tx).await?;
        rebuild_conversations(&mut tx).await?;
        sqlx::query("INSERT INTO _migrations (version, name) VALUES (?1, ?2)")
            .bind(version)
            .bind(name)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok::<(), DbError>(())
    }
    .await;
    let restore_result = sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *connection)
        .await;
    migration_result?;
    restore_result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    const LEGACY_CONVERSATIONS_TABLE: &str = r#"
        CREATE TABLE conversations (
            id TEXT PRIMARY KEY,
            slug TEXT UNIQUE,
            parent_conversation_id TEXT,
            user_initiated BOOLEAN NOT NULL,
            state TEXT NOT NULL DEFAULT '{"type":"idle"}',
            state_kind TEXT NOT NULL DEFAULT 'idle'
                CHECK (state_kind IN (
                    'idle', 'llm_requesting', 'tool_executing', 'cancelling_tool',
                    'awaiting_sub_agents', 'cancelling_sub_agents', 'error',
                    'awaiting_continuation', 'recoverable_continuation_failure',
                    'awaiting_recovery', 'awaiting_task_approval', 'awaiting_user_response',
                    'awaiting_commission_review_approval', 'context_exhausted', 'handed_off',
                    'terminal', 'completed', 'failed', 'provisioning', 'creation_failed',
                    'creation_cancelled', 'seeded_llm_requesting'
                )),
            state_updated_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            archived BOOLEAN NOT NULL DEFAULT 0,
            model TEXT,
            project_id TEXT REFERENCES projects(id),
            title TEXT,
            desired_base_branch TEXT,
            seed_parent_id TEXT,
            seed_label TEXT,
            steering_queue TEXT NOT NULL DEFAULT '[]',
            continued_in_conv_id TEXT REFERENCES conversations(id),
            chain_name TEXT,
            llm_language TEXT NOT NULL DEFAULT 'phoenix-native',
            spawned_from_conversation_id TEXT,
            cm_kind TEXT,
            cm_task_id TEXT,
            cm_task_title TEXT,
            cm_next_taskmd_id_hint TEXT,
            clear_watermark INTEGER NOT NULL DEFAULT 0,
            transcript_generation INTEGER NOT NULL DEFAULT 1,
            runtime_role TEXT NOT NULL DEFAULT 'user'
                CHECK (runtime_role IN ('user', 'sub_agent', 'coordinator')),
            work_scope_id TEXT REFERENCES work_scopes(id),
            sub_agent_cwd_override TEXT,
            coordinator_head INTEGER NOT NULL DEFAULT 0 CHECK (coordinator_head IN (0, 1)),
            effort TEXT CHECK (effort IS NULL OR effort IN ('none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max')),
            service_tier TEXT NOT NULL DEFAULT 'standard' CHECK (service_tier IN ('standard', 'fast')),
            FOREIGN KEY (parent_conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );
    "#;

    const PENDING_STATE: &str = r#"{
        "type":"awaiting_commission_review_approval",
        "tool_use_id":"tool-review",
        "request":{"brief":"Review it","focus":"correctness"},
        "scope":{
            "kind":"committed_branch_diff",
            "repo_root":"/repo",
            "base":"origin/main",
            "head":"HEAD",
            "approved_head":null,
            "approved_base":"main",
            "dirty":false,
            "changed_files":1,
            "insertions":2,
            "deletions":1
        },
        "assistant_message":{
            "message_id":"assistant-review",
            "content":[{
                "type":"tool_use",
                "id":"tool-review",
                "name":"commission_review",
                "input":{"brief":"Review it","focus":"correctness"}
            }],
            "usage":{"input_tokens":12,"output_tokens":3},
            "display_data":{"retry_count":1},
            "created_at":"2026-01-01T00:00:01Z"
        }
    }"#;

    async fn conversation_columns(
        pool: &SqlitePool,
    ) -> Vec<(String, String, i64, Option<String>, i64)> {
        sqlx::query("PRAGMA table_info(conversations)")
            .fetch_all(pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| {
                (
                    row.get("name"),
                    row.get("type"),
                    row.get("notnull"),
                    row.get("dflt_value"),
                    row.get("pk"),
                )
            })
            .collect()
    }

    async fn legacy_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(
            r"
            PRAGMA foreign_keys = ON;
            CREATE TABLE projects (id TEXT PRIMARY KEY);
            CREATE TABLE work_scopes (id TEXT PRIMARY KEY);
            CREATE TABLE git_repositories (id TEXT PRIMARY KEY);
            CREATE TABLE _migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            ",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(LEGACY_CONVERSATIONS_TABLE)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            r"
            CREATE INDEX idx_conversations_slug ON conversations(slug);
            CREATE INDEX idx_conversations_state_kind ON conversations(state_kind);
            CREATE TRIGGER conversations_touch_updated
            AFTER UPDATE ON conversations
            BEGIN
                SELECT NEW.updated_at;
            END;
            CREATE TABLE messages (
                message_id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                sequence_id INTEGER NOT NULL,
                message_type TEXT NOT NULL,
                content TEXT NOT NULL,
                display_data TEXT,
                usage_data TEXT,
                created_at TEXT NOT NULL
            );
            CREATE UNIQUE INDEX messages_conversation_sequence
                ON messages(conversation_id, sequence_id);
            ",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert_pending(pool: &SqlitePool, state: &str) {
        sqlx::query(
            "INSERT INTO conversations (
                 id, slug, user_initiated, state, state_kind, state_updated_at,
                 created_at, updated_at
             ) VALUES (
                 'pending', 'pending', 1, ?1,
                 'awaiting_commission_review_approval',
                 '2026-01-01T00:00:01Z', '2026-01-01T00:00:00Z',
                 '2026-01-01T00:00:01Z'
             )",
        )
        .bind(state)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (
                 message_id, conversation_id, sequence_id, message_type, content, created_at
             ) VALUES ('prior', 'pending', 4, 'user', '{\"text\":\"review this\"}', '2026-01-01T00:00:00Z')",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn run_migration(pool: &SqlitePool) -> DbResult<()> {
        run(pool, 68, "retire_commission_review_approvals").await
    }

    #[tokio::test]
    async fn pending_approval_decodes_and_becomes_error_paired_idle_transcript() {
        let pool = legacy_pool().await;
        insert_pending(&pool, PENDING_STATE).await;

        run_migration(&pool).await.unwrap();

        let state: (String, String) =
            sqlx::query_as("SELECT state, state_kind FROM conversations WHERE id = 'pending'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, (r#"{"type":"idle"}"#.into(), "idle".into()));
        let messages: Vec<(i64, String, String, String)> = sqlx::query_as(
            "SELECT sequence_id, message_id, message_type, content
             FROM messages WHERE conversation_id = 'pending' ORDER BY sequence_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(
            (
                messages[1].0,
                messages[1].1.as_str(),
                messages[1].2.as_str()
            ),
            (5, "assistant-review", "agent")
        );
        assert_eq!(
            (
                messages[2].0,
                messages[2].1.as_str(),
                messages[2].2.as_str()
            ),
            (
                6,
                recovery_result_id("pending", "tool-review").as_str(),
                "tool"
            )
        );
        let result: serde_json::Value = serde_json::from_str(&messages[2].3).unwrap();
        assert_eq!(result["tool_use_id"], "tool-review");
        assert_eq!(result["is_error"], true);
        assert!(result["content"].as_str().unwrap().contains("retired"));
        let settlement: (String, String) = sqlx::query_as(
            "SELECT terminal_message_id, reason
             FROM conversation_recovery_settlements
             WHERE conversation_id = 'pending'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(settlement.0, recovery_result_id("pending", "tool-review"));
        assert_eq!(settlement.1, "retired_tool_call");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _migrations WHERE version = 68")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        let objects: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_schema
             WHERE tbl_name = 'conversations' AND type IN ('index', 'trigger')",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(objects.iter().any(|name| name == "idx_conversations_slug"));
        assert!(objects
            .iter()
            .any(|name| name == "idx_conversations_state_kind"));
        assert!(objects
            .iter()
            .any(|name| name == "conversations_touch_updated"));
    }

    #[tokio::test]
    async fn malformed_pending_state_aborts_without_fabricating_transcript() {
        let pool = legacy_pool().await;
        insert_pending(
            &pool,
            r#"{"type":"awaiting_commission_review_approval","tool_use_id":7}"#,
        )
        .await;

        let error = run_migration(&pool).await.unwrap_err();
        assert!(error
            .to_string()
            .contains("cannot decode pending commission review state"));
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT state_kind FROM conversations WHERE id = 'pending'",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            "awaiting_commission_review_approval"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM messages WHERE conversation_id = 'pending'",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _migrations WHERE version = 68")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn stale_success_and_materialized_assistant_are_replaced_in_order() {
        let pool = legacy_pool().await;
        insert_pending(&pool, PENDING_STATE).await;
        for (id, sequence, message_type, content) in [
            (
                "assistant-review",
                7,
                "agent",
                r#"[{"type":"tool_use","id":"tool-review","name":"commission_review","input":{}}]"#,
            ),
            (
                "stale-success",
                6,
                "tool",
                r#"{"tool_use_id":"tool-review","content":"looks good","is_error":false}"#,
            ),
            (
                "stale-error",
                8,
                "tool",
                r#"{"tool_use_id":"tool-review","content":"old error","is_error":true}"#,
            ),
        ] {
            sqlx::query(
                "INSERT INTO messages (
                     message_id, conversation_id, sequence_id, message_type, content, created_at
                 ) VALUES (?1, 'pending', ?2, ?3, ?4, '2026-01-01T00:00:02Z')",
            )
            .bind(id)
            .bind(sequence)
            .bind(message_type)
            .bind(content)
            .execute(&pool)
            .await
            .unwrap();
        }

        run_migration(&pool).await.unwrap();

        let rows: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT sequence_id, message_id, content FROM messages
             WHERE conversation_id = 'pending' ORDER BY sequence_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[1].0, rows[1].1.as_str()), (5, "assistant-review"));
        assert_eq!(
            (rows[2].0, rows[2].1.as_str()),
            (6, recovery_result_id("pending", "tool-review").as_str())
        );
        let result: serde_json::Value = serde_json::from_str(&rows[2].2).unwrap();
        assert_eq!(result["is_error"], true);
        assert_ne!(result["content"], "looks good");
    }

    #[tokio::test]
    async fn migration_069_backfills_tail_settlement_for_already_migrated_database() {
        let pool = legacy_pool().await;
        insert_pending(&pool, PENDING_STATE).await;
        run_migration(&pool).await.unwrap();
        sqlx::raw_sql("DROP TABLE conversation_recovery_settlements;")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::raw_sql(super::super::MIGRATION_069)
            .execute(&pool)
            .await
            .unwrap();

        let settlement: (String, String) = sqlx::query_as(
            "SELECT terminal_message_id, reason
             FROM conversation_recovery_settlements
             WHERE conversation_id = 'pending'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(settlement.0, recovery_result_id("pending", "tool-review"));
        assert_eq!(settlement.1, "retired_tool_call");
    }

    #[tokio::test]
    async fn upgraded_schema_matches_fresh_conversation_columns() {
        let upgraded = legacy_pool().await;
        run_migration(&upgraded).await.unwrap();
        let fresh = crate::Database::open_in_memory().await.unwrap();

        assert_eq!(
            conversation_columns(&upgraded).await,
            conversation_columns(fresh.pool()).await
        );
        let fresh_sql: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'conversations'",
        )
        .fetch_one(fresh.pool())
        .await
        .unwrap();
        let upgraded_sql: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'conversations'",
        )
        .fetch_one(&upgraded)
        .await
        .unwrap();
        for allowed in [
            "creation_failed",
            "creation_cancelled",
            "seeded_llm_requesting",
            "sub_agent",
            "coordinator",
            "fast",
        ] {
            assert!(fresh_sql.contains(allowed));
            assert!(upgraded_sql.contains(allowed));
        }
    }

    #[tokio::test]
    async fn upgraded_schema_rejects_retired_state_kind() {
        let pool = legacy_pool().await;
        run_migration(&pool).await.unwrap();

        let table_sql: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'conversations'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!table_sql.contains("awaiting_commission_review_approval"));
        let error = sqlx::query(
            "INSERT INTO conversations (
                 id, slug, user_initiated, state, state_kind, state_updated_at,
                 created_at, updated_at
             ) VALUES (
                 'retired', 'retired', 1, '{\"type\":\"idle\"}',
                 'awaiting_commission_review_approval', '2026-01-01',
                 '2026-01-01', '2026-01-01'
             )",
        )
        .execute(&pool)
        .await
        .unwrap_err();
        assert!(error.to_string().contains("CHECK constraint failed"));
    }
}
