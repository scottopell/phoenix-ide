use std::collections::HashMap;

use phoenix_core::domain::product_conversation::ProductConversationId;
use sqlx::Row;

use crate::{Database, DbError, DbResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductConversationKind {
    Ordinary,
    Coordinator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductConversationUnreadReason {
    /// The user explicitly marked the conversation unread to revisit it.
    Marked,
    /// Non-user output landed after the user's read cursor.
    NewActivity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductConversationReadState {
    Read,
    Unread(ProductConversationUnreadReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductConversationReadAdvance {
    /// The user opened the conversation or chose "mark read".
    Acknowledge,
    /// Output arrived while the conversation was on screen.
    Observe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductConversationReadStateEntry {
    pub product_conversation_id: ProductConversationId,
    pub kind: ProductConversationKind,
    pub read_state: ProductConversationReadState,
}

/// The latest message of a product conversation's live head transcript row,
/// selected for the product bound to `?1`.
macro_rules! latest_head_message_id {
    () => {
        "SELECT message.message_id
     FROM conversations head
     JOIN messages message ON message.conversation_id = head.id
     WHERE head.product_conversation_id = ?1
       AND head.parent_conversation_id IS NULL
       AND head.runtime_role IN ('user', 'coordinator')
       AND head.continued_in_conv_id IS NULL
     ORDER BY message.sequence_id DESC
     LIMIT 1"
    };
}

fn derive_read_state(
    marked_unread: bool,
    read_through_message_id: Option<&str>,
    latest_message_id: Option<&str>,
    latest_message_type: Option<&str>,
) -> ProductConversationReadState {
    if marked_unread {
        return ProductConversationReadState::Unread(ProductConversationUnreadReason::Marked);
    }
    match (latest_message_id, latest_message_type) {
        (Some(latest), Some(message_type))
            if message_type != "user" && read_through_message_id != Some(latest) =>
        {
            ProductConversationReadState::Unread(ProductConversationUnreadReason::NewActivity)
        }
        _ => ProductConversationReadState::Read,
    }
}

impl Database {
    /// Read state for every product conversation, ordinary and Coordinator.
    ///
    /// # Errors
    ///
    /// Returns a [`DbError`] if the query fails or a row is malformed.
    pub async fn list_product_conversation_read_states(
        &self,
    ) -> DbResult<Vec<ProductConversationReadStateEntry>> {
        let rows = sqlx::query(
            "WITH head AS (
                 SELECT id, product_conversation_id
                 FROM conversations
                 WHERE product_conversation_id IS NOT NULL
                   AND parent_conversation_id IS NULL
                   AND runtime_role IN ('user', 'coordinator')
                   AND continued_in_conv_id IS NULL
             ), latest AS (
                 SELECT head.product_conversation_id, (
                     SELECT message.message_id FROM messages message
                     WHERE message.conversation_id = head.id
                     ORDER BY message.sequence_id DESC
                     LIMIT 1
                 ) AS message_id
                 FROM head
             )
             SELECT product.id AS product_conversation_id, product.kind,
                    state.read_through_message_id,
                    COALESCE(state.marked_unread, 0) AS marked_unread,
                    latest.message_id AS latest_message_id,
                    message.message_type AS latest_message_type
             FROM product_conversations product
             LEFT JOIN latest ON latest.product_conversation_id = product.id
             LEFT JOIN messages message ON message.message_id = latest.message_id
             LEFT JOIN product_conversation_read_state state
               ON state.product_conversation_id = product.id
             ORDER BY product.id, latest.message_id IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut entries: Vec<ProductConversationReadStateEntry> = Vec::with_capacity(rows.len());
        let mut index: HashMap<String, usize> = HashMap::new();
        for row in rows {
            let id: String = row.try_get("product_conversation_id")?;
            let kind = match row.try_get::<String, _>("kind")?.as_str() {
                "ordinary" => ProductConversationKind::Ordinary,
                "coordinator" => ProductConversationKind::Coordinator,
                other => {
                    return Err(DbError::Serialization(format!(
                        "unknown product conversation kind {other}"
                    )))
                }
            };
            let read_through: Option<String> = row.try_get("read_through_message_id")?;
            let marked_unread: i64 = row.try_get("marked_unread")?;
            let latest_id: Option<String> = row.try_get("latest_message_id")?;
            let latest_type: Option<String> = row.try_get("latest_message_type")?;
            let read_state = derive_read_state(
                marked_unread != 0,
                read_through.as_deref(),
                latest_id.as_deref(),
                latest_type.as_deref(),
            );
            // A product has one live head; should several rows appear, any
            // unread head makes the product unread.
            if let Some(&existing) = index.get(&id) {
                if entries[existing].read_state == ProductConversationReadState::Read {
                    entries[existing].read_state = read_state;
                }
                continue;
            }
            let product_conversation_id = ProductConversationId::parse(id.clone())
                .map_err(|error| DbError::Serialization(error.to_string()))?;
            index.insert(id, entries.len());
            entries.push(ProductConversationReadStateEntry {
                product_conversation_id,
                kind,
                read_state,
            });
        }
        Ok(entries)
    }

    /// Advance the read cursor to the product's current latest message.
    /// Only [`ProductConversationReadAdvance::Acknowledge`] clears an explicit
    /// unread mark.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::ConversationNotFound`] for an unknown product, or a
    /// [`DbError`] if the write fails.
    pub async fn mark_product_conversation_read(
        &self,
        product_conversation_id: &ProductConversationId,
        advance: ProductConversationReadAdvance,
    ) -> DbResult<()> {
        let clears_mark = i64::from(advance == ProductConversationReadAdvance::Acknowledge);
        let result = sqlx::query(concat!(
            "INSERT INTO product_conversation_read_state(
                 product_conversation_id, read_through_message_id, marked_unread
             )
             SELECT product.id, (",
            latest_head_message_id!(),
            "), 0
             FROM product_conversations product WHERE product.id = ?1
             ON CONFLICT(product_conversation_id) DO UPDATE SET
                 read_through_message_id = excluded.read_through_message_id,
                 marked_unread = CASE WHEN ?2 = 1 THEN 0 ELSE marked_unread END"
        ))
        .bind(product_conversation_id.as_str())
        .bind(clears_mark)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(
                product_conversation_id.as_str().to_owned(),
            ));
        }
        Ok(())
    }

    /// Explicitly mark the product unread, keeping its read cursor.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::ConversationNotFound`] for an unknown product, or a
    /// [`DbError`] if the write fails.
    pub async fn mark_product_conversation_unread(
        &self,
        product_conversation_id: &ProductConversationId,
    ) -> DbResult<()> {
        let result = sqlx::query(
            "INSERT INTO product_conversation_read_state(
                 product_conversation_id, read_through_message_id, marked_unread
             )
             SELECT product.id, NULL, 1
             FROM product_conversations product WHERE product.id = ?1
             ON CONFLICT(product_conversation_id) DO UPDATE SET marked_unread = 1",
        )
        .bind(product_conversation_id.as_str())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::ConversationNotFound(
                product_conversation_id.as_str().to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageContent;
    use phoenix_core::domain::llm_types::ContentBlock;

    async fn state_of(db: &Database, id: &ProductConversationId) -> ProductConversationReadState {
        db.list_product_conversation_read_states()
            .await
            .unwrap()
            .into_iter()
            .find(|entry| &entry.product_conversation_id == id)
            .unwrap()
            .read_state
    }

    #[tokio::test]
    async fn read_cursor_and_explicit_mark_round_trip() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("read-state", "read-state", "/tmp", true, None, None)
            .await
            .unwrap();
        let product = conversation.product_conversation_id.clone();
        assert_eq!(
            state_of(&db, &product).await,
            ProductConversationReadState::Read
        );

        db.add_message(
            "u1",
            &conversation.id,
            &MessageContent::user("hi"),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            state_of(&db, &product).await,
            ProductConversationReadState::Read
        );

        db.add_message(
            "a1",
            &conversation.id,
            &MessageContent::agent(vec![ContentBlock::text("hello")]),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            state_of(&db, &product).await,
            ProductConversationReadState::Unread(ProductConversationUnreadReason::NewActivity)
        );

        db.mark_product_conversation_read(&product, ProductConversationReadAdvance::Acknowledge)
            .await
            .unwrap();
        assert_eq!(
            state_of(&db, &product).await,
            ProductConversationReadState::Read
        );

        db.mark_product_conversation_unread(&product).await.unwrap();
        assert_eq!(
            state_of(&db, &product).await,
            ProductConversationReadState::Unread(ProductConversationUnreadReason::Marked)
        );

        db.mark_product_conversation_read(&product, ProductConversationReadAdvance::Acknowledge)
            .await
            .unwrap();
        db.add_message(
            "a2",
            &conversation.id,
            &MessageContent::agent(vec![ContentBlock::text("more")]),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            state_of(&db, &product).await,
            ProductConversationReadState::Unread(ProductConversationUnreadReason::NewActivity)
        );
    }

    #[tokio::test]
    async fn passive_observation_keeps_another_clients_unread_mark() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("observe", "observe", "/tmp", true, None, None)
            .await
            .unwrap();
        let product = conversation.product_conversation_id.clone();
        db.add_message(
            "a1",
            &conversation.id,
            &MessageContent::agent(vec![ContentBlock::text("hello")]),
            None,
            None,
        )
        .await
        .unwrap();
        db.mark_product_conversation_unread(&product).await.unwrap();
        db.mark_product_conversation_read(&product, ProductConversationReadAdvance::Observe)
            .await
            .unwrap();
        assert_eq!(
            state_of(&db, &product).await,
            ProductConversationReadState::Unread(ProductConversationUnreadReason::Marked)
        );
        db.mark_product_conversation_read(&product, ProductConversationReadAdvance::Acknowledge)
            .await
            .unwrap();
        assert_eq!(
            state_of(&db, &product).await,
            ProductConversationReadState::Read
        );
    }

    #[tokio::test]
    async fn observation_advances_cursor_without_a_mark() {
        let db = Database::open_in_memory().await.unwrap();
        let conversation = db
            .create_conversation("observe-cursor", "observe-cursor", "/tmp", true, None, None)
            .await
            .unwrap();
        let product = conversation.product_conversation_id.clone();
        db.add_message(
            "a1",
            &conversation.id,
            &MessageContent::agent(vec![ContentBlock::text("hello")]),
            None,
            None,
        )
        .await
        .unwrap();
        db.mark_product_conversation_read(&product, ProductConversationReadAdvance::Observe)
            .await
            .unwrap();
        assert_eq!(
            state_of(&db, &product).await,
            ProductConversationReadState::Read
        );
    }

    #[tokio::test]
    async fn marking_unknown_product_is_not_found() {
        let db = Database::open_in_memory().await.unwrap();
        let missing = ProductConversationId::parse("missing-product").unwrap();
        assert!(matches!(
            db.mark_product_conversation_read(
                &missing,
                ProductConversationReadAdvance::Acknowledge
            )
            .await,
            Err(DbError::ConversationNotFound(_))
        ));
        assert!(matches!(
            db.mark_product_conversation_unread(&missing).await,
            Err(DbError::ConversationNotFound(_))
        ));
    }

    #[test]
    fn explicit_mark_wins_over_cursor() {
        assert_eq!(
            derive_read_state(true, Some("m1"), Some("m1"), Some("agent")),
            ProductConversationReadState::Unread(ProductConversationUnreadReason::Marked)
        );
    }

    #[test]
    fn non_user_message_after_cursor_is_new_activity() {
        assert_eq!(
            derive_read_state(false, Some("m1"), Some("m2"), Some("agent")),
            ProductConversationReadState::Unread(ProductConversationUnreadReason::NewActivity)
        );
        assert_eq!(
            derive_read_state(false, None, Some("m2"), Some("error")),
            ProductConversationReadState::Unread(ProductConversationUnreadReason::NewActivity)
        );
    }

    #[test]
    fn caught_up_user_authored_or_empty_is_read() {
        assert_eq!(
            derive_read_state(false, Some("m2"), Some("m2"), Some("agent")),
            ProductConversationReadState::Read
        );
        assert_eq!(
            derive_read_state(false, Some("m1"), Some("m2"), Some("user")),
            ProductConversationReadState::Read
        );
        assert_eq!(
            derive_read_state(false, None, None, None),
            ProductConversationReadState::Read
        );
    }
}
