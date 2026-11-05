use super::*;
use crate::workflow::LocalAuthorityResult;

impl Database {
    #[cfg(any(test, feature = "test-support"))]
    pub async fn persist_sub_agent_summary_with_lost_ack_for_test(
        &self,
        message: &Message,
    ) -> LocalAuthorityResult<DbResult<()>> {
        self.persist_sub_agent_summary_at_cut(message, TerminalEvidenceTransactionCut::AfterCommit)
            .await
    }

    pub async fn persist_sub_agent_summary(
        &self,
        message: &Message,
    ) -> LocalAuthorityResult<DbResult<()>> {
        self.persist_sub_agent_summary_at_cut(message, TerminalEvidenceTransactionCut::None)
            .await
    }

    async fn persist_sub_agent_summary_at_cut(
        &self,
        message: &Message,
        cut: TerminalEvidenceTransactionCut,
    ) -> LocalAuthorityResult<DbResult<()>> {
        if message.origin != schema::InputOrigin::SystemGenerated
            || message.message_type != MessageType::User
            || !matches!(&message.content, MessageContent::User(content)
                if content.is_meta && content.images.is_empty() && content.files.is_empty())
        {
            return LocalAuthorityResult::DurableFactEstablished(Err(DbError::Serialization(
                "sub-agent summary requires an attachment-free system-generated meta-user message"
                    .into(),
            )));
        }
        let command = async {
            let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
            let writes = async {
                insert_message_tx(&mut tx, message).await?;
                sqlx::query("UPDATE conversations SET updated_at = ?2 WHERE id = ?1")
                    .bind(&message.conversation_id)
                    .bind(message.created_at.to_rfc3339())
                    .execute(&mut *tx)
                    .await?;
                Ok::<(), DbError>(())
            }
            .await;
            if let Err(error) = writes {
                tx.rollback().await?;
                return Err(error);
            }
            cut.commit(tx).await
        }
        .await;
        match command {
            Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(())),
            Err(error) => {
                if cut.classifier_unavailable() {
                    return LocalAuthorityResult::DurableFactUnclassified;
                }
                match self.classify_sub_agent_summary(message).await {
                    Ok((false, _)) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                    Ok((true, true)) => LocalAuthorityResult::DurableFactEstablished(Ok(())),
                    Ok((true, false)) | Err(_) => LocalAuthorityResult::DurableFactUnclassified,
                }
            }
        }
    }

    async fn classify_sub_agent_summary(&self, message: &Message) -> DbResult<(bool, bool)> {
        let content = serde_json::to_string(&message.content.to_stored_json())
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let display = message
            .display_data
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let usage = message
            .usage_data
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE message_id = ?1),
                EXISTS(SELECT 1 FROM messages WHERE message_id = ?1
                  AND conversation_id = ?2 AND sequence_id = ?3 AND message_type = ?4
                  AND content = ?5 AND display_data IS ?6 AND usage_data IS ?7
                  AND created_at = ?8 AND origin_kind = 'system_generated'
                  AND origin_product_conversation_id IS NULL AND origin_transcript_id IS NULL
                  AND origin_subscription_event_id IS NULL AND origin_source_instance_id IS NULL
                  AND origin_source_message_id IS NULL AND origin_source_tool_use_id IS NULL
                  AND origin_slack_request_id IS NULL AND origin_reminder_id IS NULL
                  AND NOT EXISTS(SELECT 1 FROM message_files WHERE message_id = ?1)
                  AND NOT EXISTS(SELECT 1 FROM message_images WHERE message_id = ?1))",
        )
        .bind(&message.message_id)
        .bind(&message.conversation_id)
        .bind(message.sequence_id)
        .bind(message.message_type.to_string())
        .bind(content)
        .bind(display)
        .bind(usage)
        .bind(message.created_at.to_rfc3339())
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn standalone_summary_classifies_lost_ack_rollback_and_unavailable_evidence() {
        for cut in [
            TerminalEvidenceTransactionCut::AfterCommit,
            TerminalEvidenceTransactionCut::BeforeCommit,
            TerminalEvidenceTransactionCut::AfterCommitLookupUnavailable,
        ] {
            let db = Database::open_in_memory().await.unwrap();
            db.create_conversation("parent", "parent", "/tmp", true, None, None)
                .await
                .unwrap();
            let content =
                MessageContent::User(schema::UserContent::meta("authentic child results"));
            let message = Message {
                message_id: "exact-summary".into(),
                origin: schema::InputOrigin::SystemGenerated,
                conversation_id: "parent".into(),
                sequence_id: 1,
                message_type: content.message_type(),
                content,
                display_data: Some(serde_json::json!({"type": "subagent_summary", "results": []})),
                usage_data: None,
                created_at: Utc::now(),
            };
            let result = db.persist_sub_agent_summary_at_cut(&message, cut).await;
            match cut {
                TerminalEvidenceTransactionCut::AfterCommit => {
                    result.established().unwrap().unwrap();
                    db.persist_sub_agent_summary(&message)
                        .await
                        .established()
                        .unwrap()
                        .unwrap();
                    let stored = db.get_messages("parent").await.unwrap();
                    assert_eq!(stored.len(), 1);
                    assert_eq!(
                        serde_json::to_value(&stored[0]).unwrap(),
                        serde_json::to_value(&message).unwrap()
                    );
                    let mut conflicting = message.clone();
                    conflicting.created_at += chrono::Duration::seconds(1);
                    assert!(matches!(
                        db.persist_sub_agent_summary(&conflicting).await,
                        LocalAuthorityResult::DurableFactUnclassified
                    ));
                }
                TerminalEvidenceTransactionCut::BeforeCommit => {
                    assert!(result.established().unwrap().is_err());
                    assert!(db.get_messages("parent").await.unwrap().is_empty());
                    db.persist_sub_agent_summary(&message)
                        .await
                        .established()
                        .unwrap()
                        .unwrap();
                    assert_eq!(db.get_messages("parent").await.unwrap().len(), 1);
                }
                TerminalEvidenceTransactionCut::AfterCommitLookupUnavailable => {
                    assert!(matches!(
                        result,
                        LocalAuthorityResult::DurableFactUnclassified
                    ));
                    assert_eq!(db.get_messages("parent").await.unwrap().len(), 1);
                }
                TerminalEvidenceTransactionCut::None => unreachable!(),
            }
        }
    }
}
