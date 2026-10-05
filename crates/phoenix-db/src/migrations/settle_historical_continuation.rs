use phoenix_core::domain::db_schema::{MessageContent, MessageType};
use sqlx::{Row, Sqlite, Transaction};

use crate::DbResult;

pub(super) async fn run(tx: &mut Transaction<'_, Sqlite>) -> DbResult<()> {
    let intents = sqlx::query(
        "SELECT intent.parent_conversation_id, intent.successor_conversation_id,
                intent.message_id, intent.handoff
         FROM continuation_dispatch_intents intent
         JOIN conversations predecessor ON predecessor.id = intent.parent_conversation_id
         JOIN conversations successor ON successor.id = intent.successor_conversation_id
         WHERE intent.opening_authority = 'user_authorized_instruction'
           AND predecessor.continued_in_conv_id = successor.id
           AND predecessor.product_conversation_id = successor.product_conversation_id
           AND predecessor.parent_conversation_id IS NULL
           AND successor.parent_conversation_id IS NULL
           AND predecessor.runtime_role IN ('user', 'coordinator')
           AND successor.runtime_role = predecessor.runtime_role
           AND NOT EXISTS (
               SELECT 1 FROM completed_continuation_handoffs completed
               WHERE completed.predecessor_conversation_id = predecessor.id
                  OR completed.successor_conversation_id = successor.id
           )",
    )
    .fetch_all(&mut **tx)
    .await?;
    for intent in intents {
        let predecessor: String = intent.try_get("parent_conversation_id")?;
        let successor: String = intent.try_get("successor_conversation_id")?;
        let client_key: String = intent.try_get("message_id")?;
        let handoff: String = intent.try_get("handoff")?;
        let openings = sqlx::query(
            "SELECT message_id, message_type, content FROM messages
             WHERE conversation_id = ?1
               AND (message_id = ?2 OR message_id = ?1 || ':' || ?2)",
        )
        .bind(&successor)
        .bind(&client_key)
        .fetch_all(&mut **tx)
        .await?;
        let summaries = sqlx::query(
            "SELECT message_id, content FROM messages
             WHERE conversation_id = ?1 AND message_type = 'continuation'",
        )
        .bind(&predecessor)
        .fetch_all(&mut **tx)
        .await?;
        let ([opening], [summary]) = (openings.as_slice(), summaries.as_slice()) else {
            tracing::warn!(%predecessor, %successor, "historical continuation opening lacks unambiguous settlement evidence; retaining intent");
            continue;
        };
        let opening_json: String = opening.try_get("content")?;
        let summary_json: String = summary.try_get("content")?;
        let opening_content = serde_json::from_str(&opening_json)
            .ok()
            .and_then(|value| MessageContent::from_stored_json(MessageType::User, value).ok());
        let summary_content = serde_json::from_str(&summary_json).ok().and_then(|value| {
            MessageContent::from_stored_json(MessageType::Continuation, value).ok()
        });
        let opening_kind: String = opening.try_get("message_type")?;
        if opening_kind != "user"
            || !matches!(opening_content, Some(MessageContent::User(ref user))
                if user.text == handoff && !user.is_meta && user.llm_text.is_none())
            || !matches!(summary_content, Some(MessageContent::Continuation(_)))
        {
            tracing::warn!(%predecessor, %successor, "historical continuation opening payload does not prove settlement; retaining intent");
            continue;
        }
        let opening_id: String = opening.try_get("message_id")?;
        let summary_id: String = summary.try_get("message_id")?;
        sqlx::query(
            "INSERT INTO completed_continuation_handoffs (
                 predecessor_conversation_id, successor_conversation_id,
                 continuation_message_id, accepted_successor_message_id, opening_authority
             ) VALUES (?1, ?2, ?3, ?4, 'user_authorized_instruction')",
        )
        .bind(&predecessor)
        .bind(&successor)
        .bind(&summary_id)
        .bind(&opening_id)
        .execute(&mut **tx)
        .await?;
        sqlx::query("DELETE FROM continuation_dispatch_intents WHERE parent_conversation_id = ?1")
            .bind(&predecessor)
            .execute(&mut **tx)
            .await?;
        tracing::info!(%predecessor, %successor, %opening_id, "settled historical continuation opening from existing accepted message");
    }
    Ok(())
}
