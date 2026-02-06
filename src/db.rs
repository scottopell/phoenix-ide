//! Database module for Phoenix IDE
//!
//! Provides persistence for conversations and messages.

mod schema;

pub use schema::*;
use schema::{MIGRATION_ADD_LOCAL_ID, MIGRATION_LOCAL_ID_INDEX, MIGRATION_TYPED_STATE};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DbError {
    #[error("Database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Conversation not found: {0}")]
    ConversationNotFound(String),
    #[error("Slug already exists: {0}")]
    SlugExists(String),
}

pub type DbResult<T> = Result<T, DbError>;

/// Thread-safe database handle
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Open or create database at the given path
    pub fn open<P: AsRef<Path>>(path: P) -> DbResult<Self> {
        let conn = Connection::open(path)?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.run_migrations()?;
        Ok(db)
    }

    /// Open an in-memory database (for testing)
    #[allow(dead_code)] // Used in tests
    pub fn open_in_memory() -> DbResult<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.run_migrations()?;
        Ok(db)
    }

    fn run_migrations(&self) -> DbResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(SCHEMA)?;
        // Run typed state migration (idempotent - only affects non-JSON states)
        conn.execute_batch(MIGRATION_TYPED_STATE)?;
        
        // Try to add model column - ignore error if it already exists
        let _ = conn.execute("ALTER TABLE conversations ADD COLUMN model TEXT", []);
        
        // Try to add local_id column for idempotent message sends - ignore error if exists
        let _ = conn.execute_batch(MIGRATION_ADD_LOCAL_ID);
        // Always try to create the index (IF NOT EXISTS handles idempotency)
        let _ = conn.execute_batch(MIGRATION_LOCAL_ID_INDEX);
        
        Ok(())
    }

    // ==================== Conversation Operations ====================

    /// Create a new conversation
    pub fn create_conversation(
        &self,
        id: &str,
        slug: &str,
        cwd: &str,
        user_initiated: bool,
        parent_id: Option<&str>,
        model: Option<&str>,
    ) -> DbResult<Conversation> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now();
        let idle_state = serde_json::to_string(&ConvState::Idle).unwrap();

        conn.execute(
            "INSERT INTO conversations (id, slug, cwd, parent_conversation_id, user_initiated, state, state_updated_at, created_at, updated_at, archived, model)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?7, 0, ?8)",
            params![id, slug, cwd, parent_id, user_initiated, idle_state, now.to_rfc3339(), model],
        )?;

        Ok(Conversation {
            id: id.to_string(),
            slug: Some(slug.to_string()),
            cwd: cwd.to_string(),
            parent_conversation_id: parent_id.map(String::from),
            user_initiated,
            state: ConvState::Idle,
            state_updated_at: now,
            created_at: now,
            updated_at: now,
            archived: false,
            model: model.map(String::from),
            message_count: 0,
        })
    }

    /// Get conversation by ID
    pub fn get_conversation(&self, id: &str) -> DbResult<Conversation> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.slug, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c WHERE c.id = ?1"
        )?;

        stmt.query_row(params![id], |row| {
            let state_json: String = row.get(5)?;
            let state: ConvState = serde_json::from_str(&state_json).unwrap_or_default();
            Ok(Conversation {
                id: row.get(0)?,
                slug: row.get(1)?,
                cwd: row.get(2)?,
                parent_conversation_id: row.get(3)?,
                user_initiated: row.get(4)?,
                state,
                state_updated_at: parse_datetime(&row.get::<_, String>(6)?),
                created_at: parse_datetime(&row.get::<_, String>(7)?),
                updated_at: parse_datetime(&row.get::<_, String>(8)?),
                archived: row.get(9)?,
                model: row.get(10)?,
                message_count: row.get(11)?,
            })
        })
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => DbError::ConversationNotFound(id.to_string()),
            other => DbError::Sqlite(other),
        })
    }

    /// Get conversation by slug
    pub fn get_conversation_by_slug(&self, slug: &str) -> DbResult<Conversation> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.slug, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c WHERE c.slug = ?1"
        )?;

        stmt.query_row(params![slug], |row| {
            let state_json: String = row.get(5)?;
            let state: ConvState = serde_json::from_str(&state_json).unwrap_or_default();
            Ok(Conversation {
                id: row.get(0)?,
                slug: row.get(1)?,
                cwd: row.get(2)?,
                parent_conversation_id: row.get(3)?,
                user_initiated: row.get(4)?,
                state,
                state_updated_at: parse_datetime(&row.get::<_, String>(6)?),
                created_at: parse_datetime(&row.get::<_, String>(7)?),
                updated_at: parse_datetime(&row.get::<_, String>(8)?),
                archived: row.get(9)?,
                model: row.get(10)?,
                message_count: row.get(11)?,
            })
        })
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => DbError::ConversationNotFound(slug.to_string()),
            other => DbError::Sqlite(other),
        })
    }

    /// List active (non-archived) user-initiated conversations
    pub fn list_conversations(&self) -> DbResult<Vec<Conversation>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.slug, c.cwd, c.parent_conversation_id, c.user_initiated, c.state, 
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             WHERE c.archived = 0 AND c.user_initiated = 1
             ORDER BY c.updated_at DESC"
        )?;

        let rows = stmt.query_map([], |row| {
            let state_json: String = row.get(5)?;
            let state: ConvState = serde_json::from_str(&state_json).unwrap_or_default();
            Ok(Conversation {
                id: row.get(0)?,
                slug: row.get(1)?,
                cwd: row.get(2)?,
                parent_conversation_id: row.get(3)?,
                user_initiated: row.get(4)?,
                state,
                state_updated_at: parse_datetime(&row.get::<_, String>(6)?),
                created_at: parse_datetime(&row.get::<_, String>(7)?),
                updated_at: parse_datetime(&row.get::<_, String>(8)?),
                archived: row.get(9)?,
                model: row.get(10)?,
                message_count: row.get(11)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    /// List archived conversations
    pub fn list_archived_conversations(&self) -> DbResult<Vec<Conversation>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.slug, c.cwd, c.parent_conversation_id, c.user_initiated, c.state,
                    c.state_updated_at, c.created_at, c.updated_at, c.archived, c.model,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as message_count
             FROM conversations c
             WHERE c.archived = 1 AND c.user_initiated = 1
             ORDER BY c.updated_at DESC"
        )?;

        let rows = stmt.query_map([], |row| {
            let state_json: String = row.get(5)?;
            let state: ConvState = serde_json::from_str(&state_json).unwrap_or_default();
            Ok(Conversation {
                id: row.get(0)?,
                slug: row.get(1)?,
                cwd: row.get(2)?,
                parent_conversation_id: row.get(3)?,
                user_initiated: row.get(4)?,
                state,
                state_updated_at: parse_datetime(&row.get::<_, String>(6)?),
                created_at: parse_datetime(&row.get::<_, String>(7)?),
                updated_at: parse_datetime(&row.get::<_, String>(8)?),
                archived: row.get(9)?,
                model: row.get(10)?,
                message_count: row.get(11)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    /// Update conversation state
    pub fn update_conversation_state(
        &self,
        id: &str,
        state: &ConvState,
    ) -> DbResult<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now();
        let state_json = serde_json::to_string(state).unwrap();

        let updated = conn.execute(
            "UPDATE conversations SET state = ?1, state_updated_at = ?2, updated_at = ?2 WHERE id = ?3",
            params![state_json, now.to_rfc3339(), id],
        )?;

        if updated == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Archive a conversation
    pub fn archive_conversation(&self, id: &str) -> DbResult<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now();

        let updated = conn.execute(
            "UPDATE conversations SET archived = 1, updated_at = ?1 WHERE id = ?2",
            params![now.to_rfc3339(), id],
        )?;

        if updated == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Unarchive a conversation
    pub fn unarchive_conversation(&self, id: &str) -> DbResult<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now();

        let updated = conn.execute(
            "UPDATE conversations SET archived = 0, updated_at = ?1 WHERE id = ?2",
            params![now.to_rfc3339(), id],
        )?;

        if updated == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Delete a conversation and all its messages
    pub fn delete_conversation(&self, id: &str) -> DbResult<()> {
        let conn = self.conn.lock().unwrap();

        // Messages are deleted by CASCADE
        let deleted = conn.execute("DELETE FROM conversations WHERE id = ?1", params![id])?;

        if deleted == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Rename conversation (update slug)
    pub fn rename_conversation(&self, id: &str, new_slug: &str) -> DbResult<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now();

        // Check if slug already exists
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE slug = ?1 AND id != ?2)",
            params![new_slug, id],
            |row| row.get(0),
        )?;

        if exists {
            return Err(DbError::SlugExists(new_slug.to_string()));
        }

        let updated = conn.execute(
            "UPDATE conversations SET slug = ?1, updated_at = ?2 WHERE id = ?3",
            params![new_slug, now.to_rfc3339(), id],
        )?;

        if updated == 0 {
            return Err(DbError::ConversationNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Reset all conversations to idle on server restart
    pub fn reset_all_to_idle(&self) -> DbResult<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now();
        let idle_state = serde_json::to_string(&ConvState::Idle).unwrap();

        conn.execute(
            "UPDATE conversations SET state = ?1, state_updated_at = ?2, updated_at = ?2
             WHERE json_extract(state, '$.type') != 'idle'",
            params![idle_state, now.to_rfc3339()],
        )?;
        Ok(())
    }

    // ==================== Message Operations ====================

    /// Add a message to a conversation
    /// 
    /// If `local_id` is provided, it will be stored for idempotency checks.
    /// The unique constraint on (conversation_id, local_id) prevents duplicates.
    pub fn add_message(
        &self,
        id: &str,
        conversation_id: &str,
        content: &MessageContent,
        display_data: Option<&serde_json::Value>,
        usage_data: Option<&UsageData>,
        local_id: Option<&str>,
    ) -> DbResult<Message> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now();
        let msg_type = content.message_type();

        // Get next sequence ID
        let sequence_id: i64 = conn.query_row(
            "SELECT COALESCE(MAX(sequence_id), 0) + 1 FROM messages WHERE conversation_id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )?;

        let content_str = serde_json::to_string(&content.to_json()).unwrap();
        let display_str = display_data.map(|v| serde_json::to_string(v).unwrap());
        let usage_str = usage_data.map(|u| serde_json::to_string(u).unwrap());

        conn.execute(
            "INSERT INTO messages (id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at, local_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                conversation_id,
                sequence_id,
                msg_type.to_string(),
                content_str,
                display_str,
                usage_str,
                now.to_rfc3339(),
                local_id,
            ],
        )?;

        // Update conversation timestamp
        conn.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![now.to_rfc3339(), conversation_id],
        )?;

        Ok(Message {
            id: id.to_string(),
            conversation_id: conversation_id.to_string(),
            sequence_id,
            message_type: msg_type,
            content: content.clone(),
            display_data: display_data.cloned(),
            usage_data: usage_data.cloned(),
            created_at: now,
        })
    }

    /// Get messages for a conversation
    pub fn get_messages(&self, conversation_id: &str) -> DbResult<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY sequence_id ASC"
        )?;

        let rows = stmt.query_map(params![conversation_id], parse_message_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    /// Get messages after a sequence ID
    pub fn get_messages_after(
        &self,
        conversation_id: &str,
        after_sequence: i64,
    ) -> DbResult<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages WHERE conversation_id = ?1 AND sequence_id > ?2 ORDER BY sequence_id ASC"
        )?;

        let rows = stmt.query_map(params![conversation_id, after_sequence], parse_message_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    /// Check if a message with the given local_id already exists for this conversation
    /// Used for idempotent message sends - returns true if duplicate
    pub fn message_exists_by_local_id(
        &self,
        conversation_id: &str,
        local_id: &str,
    ) -> DbResult<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE conversation_id = ?1 AND local_id = ?2",
            params![conversation_id, local_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Get the last sequence ID for a conversation
    pub fn get_last_sequence_id(&self, conversation_id: &str) -> DbResult<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(sequence_id), 0) FROM messages WHERE conversation_id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )
        .map_err(DbError::from)
    }
}

/// Parse a message row from the database
fn parse_message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let msg_type = parse_message_type(&row.get::<_, String>(3)?);
    let content_str: String = row.get(4)?;
    let content_value: serde_json::Value = serde_json::from_str(&content_str).unwrap_or_default();
    
    // Parse content using the message type as discriminator
    let content = MessageContent::from_json(msg_type, content_value)
        .unwrap_or_else(|_| MessageContent::error(format!("Failed to parse {msg_type} message")));
    
    Ok(Message {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        sequence_id: row.get(2)?,
        message_type: msg_type,
        content,
        display_data: row
            .get::<_, Option<String>>(5)?
            .map(|s| serde_json::from_str(&s).unwrap_or_default()),
        usage_data: row
            .get::<_, Option<String>>(6)?
            .and_then(|s| serde_json::from_str(&s).ok()),
        created_at: parse_datetime(&row.get::<_, String>(7)?),
    })
}

fn parse_message_type(s: &str) -> MessageType {
    match s {
        "user" => MessageType::User,
        "agent" => MessageType::Agent,
        "tool" => MessageType::Tool,
        "error" => MessageType::Error,
        _ => MessageType::System,
    }
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get_conversation() {
        let db = Database::open_in_memory().unwrap();

        let conv = db
            .create_conversation("test-id", "test-slug", "/tmp/test", true, None, None)
            .unwrap();

        assert_eq!(conv.id, "test-id");
        assert_eq!(conv.slug, Some("test-slug".to_string()));
        assert_eq!(conv.cwd, "/tmp/test");
        assert!(matches!(conv.state, ConvState::Idle));

        let fetched = db.get_conversation("test-id").unwrap();
        assert_eq!(fetched.id, conv.id);
    }

    #[test]
    fn test_add_and_get_messages() {
        use crate::llm::ContentBlock;
        
        let db = Database::open_in_memory().unwrap();

        db.create_conversation("conv-1", "slug-1", "/tmp", true, None, None)
            .unwrap();

        let msg1 = db
            .add_message(
                "msg-1",
                "conv-1",
                &MessageContent::user("Hello"),
                None,
                None,
                Some("local-1"),
            )
            .unwrap();

        let msg2 = db
            .add_message(
                "msg-2",
                "conv-1",
                &MessageContent::agent(vec![ContentBlock::text("Hi there!")]),
                None,
                None,
                None, // Agent messages don't have local_id
            )
            .unwrap();

        assert_eq!(msg1.sequence_id, 1);
        assert_eq!(msg2.sequence_id, 2);
        assert_eq!(msg1.message_type, MessageType::User);
        assert_eq!(msg2.message_type, MessageType::Agent);

        let messages = db.get_messages("conv-1").unwrap();
        assert_eq!(messages.len(), 2);

        // Verify content is properly typed
        match &messages[0].content {
            MessageContent::User(u) => assert_eq!(u.text, "Hello"),
            _ => panic!("Expected User content"),
        }

        let after = db.get_messages_after("conv-1", 1).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, "msg-2");
    }
}
