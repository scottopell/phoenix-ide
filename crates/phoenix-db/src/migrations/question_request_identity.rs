use crate::{DbError, DbResult};
use sqlx::{Row, Sqlite, Transaction};

pub(super) async fn run(tx: &mut Transaction<'_, Sqlite>) -> DbResult<()> {
    let dismissed = sqlx::query("SELECT c.id, m.content FROM conversations c JOIN messages m ON m.conversation_id = c.id WHERE c.state_kind = 'idle' AND m.message_type = 'system' AND m.sequence_id = (SELECT MAX(latest.sequence_id) FROM messages latest WHERE latest.conversation_id = c.id) AND NOT EXISTS (SELECT 1 FROM steering_messages queued WHERE queued.conversation_id = c.id)").fetch_all(&mut **tx).await?;
    for row in dismissed {
        let content: crate::SystemContent =
            serde_json::from_str(row.try_get::<&str, _>("content")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
        if content.text == "[ask-user-question-dismissed]" {
            sqlx::query("INSERT INTO question_dismissal_pauses (conversation_id) VALUES (?1)")
                .bind(row.try_get::<String, _>("id")?)
                .execute(&mut **tx)
                .await?;
        }
    }
    let rows = sqlx::query(
        "SELECT id, state FROM conversations WHERE state_kind = 'awaiting_user_response'",
    )
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let id: String = row.try_get("id")?;
        let mut state: serde_json::Value =
            serde_json::from_str(row.try_get::<&str, _>("state")?)
                .map_err(|error| DbError::Serialization(error.to_string()))?;
        let object = state.as_object_mut().ok_or_else(|| {
            DbError::Serialization("pending question state must be an object".into())
        })?;
        match object.get("request_id") {
            None => {
                object.insert("request_id".into(), uuid::Uuid::new_v4().to_string().into());
            }
            Some(serde_json::Value::String(value)) if !value.is_empty() => {}
            Some(_) => {
                return Err(DbError::Serialization(
                    "pending question request identity is invalid".into(),
                ))
            }
        }
        // Deserialize the complete aggregate before replacing the persisted value.
        let _: crate::ConvState = serde_json::from_value(state.clone())
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        sqlx::query("UPDATE conversations SET state = ?1 WHERE id = ?2")
            .bind(state.to_string())
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn migration_preserves_legacy_queued_resumption_on_both_sides_of_dismissal() {
        let db = crate::Database::open_in_memory().await.unwrap();
        sqlx::query("DROP TABLE question_dismissal_pauses")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM _migrations WHERE version = 97")
            .execute(db.pool())
            .await
            .unwrap();
        for (id, queued_before) in [("before-dismissal", true), ("after-dismissal", false)] {
            db.create_conversation(id, id, "/tmp", true, None, None)
                .await
                .unwrap();
            for ordinal in 0..2 {
                if ordinal == 0 && !queued_before {
                    db.add_message_with_seq(
                        &format!("{id}-dismiss"),
                        id,
                        1,
                        &crate::MessageContent::system("[ask-user-question-dismissed]"),
                        Some(&serde_json::json!({"hidden":true})),
                        None,
                    )
                    .await
                    .unwrap();
                }
                sqlx::query("INSERT INTO steering_messages (message_id, conversation_id, ordinal, text) VALUES (?1, ?2, ?3, ?4)")
                    .bind(format!("{id}-{ordinal}")).bind(id).bind(ordinal).bind(format!("input {ordinal}"))
                    .execute(db.pool()).await.unwrap();
                sqlx::query("INSERT INTO steering_acceptance_receipts (conversation_id, message_id, request_fingerprint) VALUES (?1, ?2, ?3)")
                    .bind(id).bind(format!("{id}-{ordinal}")).bind(format!("fingerprint-{ordinal}"))
                    .execute(db.pool()).await.unwrap();
                sqlx::query("UPDATE conversations SET updated_at = ?1 WHERE id = ?2")
                    .bind(chrono::Utc::now().to_rfc3339())
                    .bind(id)
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
            if queued_before {
                db.add_message_with_seq(
                    &format!("{id}-dismiss"),
                    id,
                    1,
                    &crate::MessageContent::system("[ask-user-question-dismissed]"),
                    Some(&serde_json::json!({"hidden":true})),
                    None,
                )
                .await
                .unwrap();
            }
        }

        assert_eq!(crate::run_pending_migrations(db.pool()).await.unwrap(), 1);
        for id in ["before-dismissal", "after-dismissal"] {
            assert!(!db.question_dismissal_paused(id).await.unwrap());
            let queue = db.get_steering_queue(id).await.unwrap();
            assert_eq!(
                queue
                    .iter()
                    .map(|entry| entry.text.as_str())
                    .collect::<Vec<_>>(),
                ["input 0", "input 1"]
            );
            assert!(db
                .get_steering_acceptance_fingerprint(id, &format!("{id}-1"))
                .await
                .unwrap()
                .is_some());
        }
    }

    #[tokio::test]
    async fn migration_backfills_only_latest_idle_question_dismissal_pause() {
        let db = crate::Database::open_in_memory().await.unwrap();
        for id in ["paused", "resumed", "working"] {
            db.create_conversation(id, id, "/tmp", true, None, None)
                .await
                .unwrap();
            let content = crate::MessageContent::system("[ask-user-question-dismissed]");
            db.add_message_with_seq(
                &format!("{id}-dismiss"),
                id,
                1,
                &content,
                Some(&serde_json::json!({"hidden":true})),
                None,
            )
            .await
            .unwrap();
        }
        db.add_message_with_seq(
            "explicit",
            "resumed",
            2,
            &crate::MessageContent::user("Continue"),
            None,
            None,
        )
        .await
        .unwrap();
        db.update_conversation_state("working", &crate::ConvState::LlmRequesting { attempt: 1 })
            .await
            .unwrap();
        sqlx::query("DROP TABLE question_dismissal_pauses")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM _migrations WHERE version = 97")
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(crate::run_pending_migrations(db.pool()).await.unwrap(), 1);
        assert!(db.question_dismissal_paused("paused").await.unwrap());
        assert!(!db.question_dismissal_paused("resumed").await.unwrap());
        assert!(!db.question_dismissal_paused("working").await.unwrap());
        let entry = |id: &str| phoenix_core::domain::sm_event::SteerEntry {
            text: id.into(),
            llm_text: None,
            images: vec![],
            files: vec![],
            message_id: id.into(),
            user_agent: None,
            skill_invocation: None,
        };
        db.append_steering_entry(
            "paused",
            &entry("background"),
            "background",
            crate::SteeringAdmissionSource::DeferredObjective,
        )
        .await
        .unwrap();
        assert!(db.question_dismissal_paused("paused").await.unwrap());
        db.append_steering_entry(
            "paused",
            &entry("explicit-after-dismiss"),
            "explicit",
            crate::SteeringAdmissionSource::ExplicitUserMessage,
        )
        .await
        .unwrap();
        assert!(!db.question_dismissal_paused("paused").await.unwrap());
        let queue = db.get_steering_queue("paused").await.unwrap();
        assert_eq!(
            queue
                .iter()
                .map(|e| e.message_id.as_str())
                .collect::<Vec<_>>(),
            ["background", "explicit-after-dismiss"]
        );
    }

    #[tokio::test]
    async fn migration_preserves_pending_answers_and_assigns_distinct_durable_incarnations() {
        let db = crate::Database::open_in_memory().await.unwrap();
        let legacy = serde_json::json!({"type":"awaiting_user_response", "tool_use_id":"reused-provider-id",
            "questions":[{"question":"Choice?", "header":"Choice", "options":[{"label":"A"},{"label":"B"}], "multiSelect":false}]});
        for id in ["first", "second"] {
            db.create_conversation(id, id, "/tmp", true, None, None)
                .await
                .unwrap();
            sqlx::query("UPDATE conversations SET state = ?1, state_kind = 'awaiting_user_response' WHERE id = ?2")
                .bind(legacy.to_string()).bind(id).execute(db.pool()).await.unwrap();
        }
        sqlx::query("DROP TABLE question_dismissal_pauses")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM _migrations WHERE version = 97")
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(crate::run_pending_migrations(db.pool()).await.unwrap(), 1);
        let mut identities = Vec::new();
        for id in ["first", "second"] {
            let state = db.get_conversation(id).await.unwrap().state;
            let crate::ConvState::AwaitingUserResponse {
                request_id,
                tool_use_id,
                questions,
            } = state
            else {
                panic!("pending questions preserved")
            };
            assert_eq!(tool_use_id, "reused-provider-id");
            assert_eq!(questions[0].question, "Choice?");
            uuid::Uuid::parse_str(&request_id).unwrap();
            identities.push(request_id);
        }
        assert_ne!(identities[0], identities[1]);
        assert_eq!(crate::run_pending_migrations(db.pool()).await.unwrap(), 0);
        let crate::ConvState::AwaitingUserResponse { request_id, .. } =
            db.get_conversation("first").await.unwrap().state
        else {
            panic!("pending")
        };
        assert_eq!(request_id, identities[0]);
    }
}
