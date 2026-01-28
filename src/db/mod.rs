//! Database module for Phoenix IDE
//! 
//! Provides persistence for conversations and messages.

mod schema;

pub use schema::*;

use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use chrono::{DateTime, Utc};

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
    ) -> DbResult<Conversation> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now();
        
        conn.execute(
            "INSERT INTO conversations (id, slug, cwd, parent_conversation_id, user_initiated, state, state_updated_at, created_at, updated_at, archived)
             VALUES (?1, ?2, ?3, ?4, ?5, 'idle', ?6, ?6, ?6, 0)",
            params![id, slug, cwd, parent_id, user_initiated, now.to_rfc3339()],
        )?;
        
        Ok(Conversation {
            id: id.to_string(),
            slug: Some(slug.to_string()),
            cwd: cwd.to_string(),
            parent_conversation_id: parent_id.map(String::from),
            user_initiated,
            state: ConversationState::Idle,
            state_data: None,
            state_updated_at: now,
            created_at: now,
            updated_at: now,
            archived: false,
        })
    }

    /// Get conversation by ID
    pub fn get_conversation(&self, id: &str) -> DbResult<Conversation> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, slug, cwd, parent_conversation_id, user_initiated, state, state_data, state_updated_at, created_at, updated_at, archived
             FROM conversations WHERE id = ?1"
        )?;
        
        stmt.query_row(params![id], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                slug: row.get(1)?,
                cwd: row.get(2)?,
                parent_conversation_id: row.get(3)?,
                user_initiated: row.get(4)?,
                state: parse_state(row.get::<_, String>(5)?.as_str()),
                state_data: row.get::<_, Option<String>>(6)?.map(|s| serde_json::from_str(&s).unwrap_or_default()),
                state_updated_at: parse_datetime(&row.get::<_, String>(7)?),
                created_at: parse_datetime(&row.get::<_, String>(8)?),
                updated_at: parse_datetime(&row.get::<_, String>(9)?),
                archived: row.get(10)?,
            })
        }).map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => DbError::ConversationNotFound(id.to_string()),
            other => DbError::Sqlite(other),
        })
    }

    /// Get conversation by slug
    pub fn get_conversation_by_slug(&self, slug: &str) -> DbResult<Conversation> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, slug, cwd, parent_conversation_id, user_initiated, state, state_data, state_updated_at, created_at, updated_at, archived
             FROM conversations WHERE slug = ?1"
        )?;
        
        stmt.query_row(params![slug], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                slug: row.get(1)?,
                cwd: row.get(2)?,
                parent_conversation_id: row.get(3)?,
                user_initiated: row.get(4)?,
                state: parse_state(row.get::<_, String>(5)?.as_str()),
                state_data: row.get::<_, Option<String>>(6)?.map(|s| serde_json::from_str(&s).unwrap_or_default()),
                state_updated_at: parse_datetime(&row.get::<_, String>(7)?),
                created_at: parse_datetime(&row.get::<_, String>(8)?),
                updated_at: parse_datetime(&row.get::<_, String>(9)?),
                archived: row.get(10)?,
            })
        }).map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => DbError::ConversationNotFound(slug.to_string()),
            other => DbError::Sqlite(other),
        })
    }

    /// List active (non-archived) user-initiated conversations
    pub fn list_conversations(&self) -> DbResult<Vec<Conversation>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, slug, cwd, parent_conversation_id, user_initiated, state, state_data, state_updated_at, created_at, updated_at, archived
             FROM conversations 
             WHERE archived = 0 AND user_initiated = 1
             ORDER BY updated_at DESC"
        )?;
        
        let rows = stmt.query_map([], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                slug: row.get(1)?,
                cwd: row.get(2)?,
                parent_conversation_id: row.get(3)?,
                user_initiated: row.get(4)?,
                state: parse_state(row.get::<_, String>(5)?.as_str()),
                state_data: row.get::<_, Option<String>>(6)?.map(|s| serde_json::from_str(&s).unwrap_or_default()),
                state_updated_at: parse_datetime(&row.get::<_, String>(7)?),
                created_at: parse_datetime(&row.get::<_, String>(8)?),
                updated_at: parse_datetime(&row.get::<_, String>(9)?),
                archived: row.get(10)?,
            })
        })?;
        
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    /// List archived conversations
    pub fn list_archived_conversations(&self) -> DbResult<Vec<Conversation>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, slug, cwd, parent_conversation_id, user_initiated, state, state_data, state_updated_at, created_at, updated_at, archived
             FROM conversations 
             WHERE archived = 1 AND user_initiated = 1
             ORDER BY updated_at DESC"
        )?;
        
        let rows = stmt.query_map([], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                slug: row.get(1)?,
                cwd: row.get(2)?,
                parent_conversation_id: row.get(3)?,
                user_initiated: row.get(4)?,
                state: parse_state(row.get::<_, String>(5)?.as_str()),
                state_data: row.get::<_, Option<String>>(6)?.map(|s| serde_json::from_str(&s).unwrap_or_default()),
                state_updated_at: parse_datetime(&row.get::<_, String>(7)?),
                created_at: parse_datetime(&row.get::<_, String>(8)?),
                updated_at: parse_datetime(&row.get::<_, String>(9)?),
                archived: row.get(10)?,
            })
        })?;
        
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    /// Update conversation state
    pub fn update_conversation_state(
        &self,
        id: &str,
        state: &ConversationState,
        state_data: Option<&serde_json::Value>,
    ) -> DbResult<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now();
        let state_str = state.to_string();
        let state_data_str = state_data.map(|v| serde_json::to_string(v).unwrap());
        
        let updated = conn.execute(
            "UPDATE conversations SET state = ?1, state_data = ?2, state_updated_at = ?3, updated_at = ?3 WHERE id = ?4",
            params![state_str, state_data_str, now.to_rfc3339(), id],
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
        let deleted = conn.execute(
            "DELETE FROM conversations WHERE id = ?1",
            params![id],
        )?;
        
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
        
        conn.execute(
            "UPDATE conversations SET state = 'idle', state_data = NULL, state_updated_at = ?1, updated_at = ?1
             WHERE state != 'idle'",
            params![now.to_rfc3339()],
        )?;
        Ok(())
    }

    // ==================== Message Operations ====================

    /// Add a message to a conversation
    pub fn add_message(
        &self,
        id: &str,
        conversation_id: &str,
        msg_type: MessageType,
        content: &serde_json::Value,
        display_data: Option<&serde_json::Value>,
        usage_data: Option<&UsageData>,
    ) -> DbResult<Message> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now();
        
        // Get next sequence ID
        let sequence_id: i64 = conn.query_row(
            "SELECT COALESCE(MAX(sequence_id), 0) + 1 FROM messages WHERE conversation_id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )?;
        
        let content_str = serde_json::to_string(content).unwrap();
        let display_str = display_data.map(|v| serde_json::to_string(v).unwrap());
        let usage_str = usage_data.map(|u| serde_json::to_string(u).unwrap());
        
        conn.execute(
            "INSERT INTO messages (id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                conversation_id,
                sequence_id,
                msg_type.to_string(),
                content_str,
                display_str,
                usage_str,
                now.to_rfc3339(),
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
        
        let rows = stmt.query_map(params![conversation_id], |row| {
            Ok(Message {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                sequence_id: row.get(2)?,
                message_type: parse_message_type(&row.get::<_, String>(3)?),
                content: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
                display_data: row.get::<_, Option<String>>(5)?.map(|s| serde_json::from_str(&s).unwrap_or_default()),
                usage_data: row.get::<_, Option<String>>(6)?.and_then(|s| serde_json::from_str(&s).ok()),
                created_at: parse_datetime(&row.get::<_, String>(7)?),
            })
        })?;
        
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    /// Get messages after a sequence ID
    pub fn get_messages_after(&self, conversation_id: &str, after_sequence: i64) -> DbResult<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, sequence_id, message_type, content, display_data, usage_data, created_at
             FROM messages WHERE conversation_id = ?1 AND sequence_id > ?2 ORDER BY sequence_id ASC"
        )?;
        
        let rows = stmt.query_map(params![conversation_id, after_sequence], |row| {
            Ok(Message {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                sequence_id: row.get(2)?,
                message_type: parse_message_type(&row.get::<_, String>(3)?),
                content: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
                display_data: row.get::<_, Option<String>>(5)?.map(|s| serde_json::from_str(&s).unwrap_or_default()),
                usage_data: row.get::<_, Option<String>>(6)?.and_then(|s| serde_json::from_str(&s).ok()),
                created_at: parse_datetime(&row.get::<_, String>(7)?),
            })
        })?;
        
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    /// Get the last sequence ID for a conversation
    pub fn get_last_sequence_id(&self, conversation_id: &str) -> DbResult<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(sequence_id), 0) FROM messages WHERE conversation_id = ?1",
            params![conversation_id],
            |row| row.get(0),
        ).map_err(DbError::from)
    }
}

fn parse_state(s: &str) -> ConversationState {
    match s {
        "idle" => ConversationState::Idle,
        "awaiting_llm" => ConversationState::AwaitingLlm,
        "llm_requesting" => ConversationState::LlmRequesting { attempt: 1 },
        "tool_executing" => ConversationState::ToolExecuting {
            current_tool_id: String::new(),
            remaining_tool_ids: vec![],
            completed_results: vec![],
        },
        "cancelling" => ConversationState::Cancelling { pending_tool_id: None },
        "awaiting_sub_agents" => ConversationState::AwaitingSubAgents {
            pending_ids: vec![],
            completed_results: vec![],
        },
        "error" => ConversationState::Error {
            message: String::new(),
            error_kind: ErrorKind::Unknown,
        },
        _ => ConversationState::Idle,
    }
}

fn parse_message_type(s: &str) -> MessageType {
    match s {
        "user" => MessageType::User,
        "agent" => MessageType::Agent,
        "tool" => MessageType::Tool,
        "system" => MessageType::System,
        "error" => MessageType::Error,
        _ => MessageType::System,
    }
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get_conversation() {
        let db = Database::open_in_memory().unwrap();
        
        let conv = db.create_conversation(
            "test-id",
            "test-slug",
            "/tmp/test",
            true,
            None,
        ).unwrap();
        
        assert_eq!(conv.id, "test-id");
        assert_eq!(conv.slug, Some("test-slug".to_string()));
        assert_eq!(conv.cwd, "/tmp/test");
        assert!(matches!(conv.state, ConversationState::Idle));
        
        let fetched = db.get_conversation("test-id").unwrap();
        assert_eq!(fetched.id, conv.id);
    }

    #[test]
    fn test_add_and_get_messages() {
        let db = Database::open_in_memory().unwrap();
        
        db.create_conversation("conv-1", "slug-1", "/tmp", true, None).unwrap();
        
        let msg1 = db.add_message(
            "msg-1",
            "conv-1",
            MessageType::User,
            &serde_json::json!({"text": "Hello"}),
            None,
            None,
        ).unwrap();
        
        let msg2 = db.add_message(
            "msg-2",
            "conv-1",
            MessageType::Agent,
            &serde_json::json!([{"type": "text", "text": "Hi there!"}]),
            None,
            None,
        ).unwrap();
        
        assert_eq!(msg1.sequence_id, 1);
        assert_eq!(msg2.sequence_id, 2);
        
        let messages = db.get_messages("conv-1").unwrap();
        assert_eq!(messages.len(), 2);
        
        let after = db.get_messages_after("conv-1", 1).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, "msg-2");
    }
}
