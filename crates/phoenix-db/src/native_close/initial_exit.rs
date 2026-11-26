//! Exact process-exit classification for a native initial Close admission.

use phoenix_core::domain::close::{CloseObligation, CloseRunOrdinal, CloseRunRef};
use sqlx::Row;

use super::{NativeCloseCommand, NativeCloseIntent, NativeCloseReceipt};
use crate::workflow::LocalAuthorityResult;
use crate::{Database, DbError, DbResult};

impl Database {
    /// Settles process interruption against the actor's retained initial receipt.
    /// The admitted owner must stop phase work first. This does not require an
    /// invented source-state transition, perform Close effects, or archive.
    pub async fn interrupt_native_close_initial(
        &self,
        command: &NativeCloseCommand,
        receipt: &NativeCloseReceipt,
    ) -> LocalAuthorityResult<DbResult<CloseObligation>> {
        let mut tx = match self.pool.begin_with("BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let result = async {
            let NativeCloseIntent::Initial { target } = command.intent() else {
                return Err(precondition(
                    "process exit requires an initial Close command",
                ));
            };
            if receipt.request_id != command.request_id()
                || receipt.admitted.target != *target
                || receipt.admitted.run_ordinal != CloseRunOrdinal::INITIAL
            {
                return Err(precondition(
                    "initial Close receipt does not match the command",
                ));
            }

            let retained = super::receipts::read_receipt_tx(&mut tx, command)
                .await?
                .ok_or_else(|| precondition("exact initial Close receipt is missing"))?;
            if retained != receipt.admitted {
                return Err(precondition(
                    "initial Close receipt does not bind the admitted run",
                ));
            }

            let row = sqlx::query(
                "SELECT obligation.native_managed, obligation.product_conversation_id,
                        run.run_ordinal, run.status,
                        (SELECT MAX(latest.run_ordinal) FROM close_runs latest
                         WHERE latest.attempt_id = obligation.attempt_id) AS latest_ordinal
                 FROM close_obligations obligation
                 JOIN close_runs run ON run.attempt_id = obligation.attempt_id
                                    AND run.run_ordinal = ?2
                 WHERE obligation.attempt_id = ?1",
            )
            .bind(retained.attempt_id.as_str())
            .bind(CloseRunOrdinal::INITIAL.get())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| precondition("admitted initial Close run is missing"))?;
            let native_managed: i64 = row.try_get("native_managed")?;
            let retained_target: String = row.try_get("product_conversation_id")?;
            let ordinal: i64 = row.try_get("run_ordinal")?;
            let status: String = row.try_get("status")?;
            let latest_ordinal: Option<i64> = row.try_get("latest_ordinal")?;
            if native_managed != 1
                || retained_target != target.as_str()
                || ordinal != CloseRunOrdinal::INITIAL.get()
            {
                return Err(precondition(
                    "receipt does not identify a native initial Close run",
                ));
            }

            let run = CloseRunRef {
                attempt_id: retained.attempt_id,
                ordinal: CloseRunOrdinal::INITIAL,
            };
            if status == "running" {
                if latest_ordinal != Some(CloseRunOrdinal::INITIAL.get()) {
                    return Err(precondition(
                        "initial Close run is no longer the latest run",
                    ));
                }
                let classified = super::interrupt_native_retry_tx(&mut tx, &run)
                    .await?
                    .ok_or_else(|| {
                        precondition("native initial Close interruption was not classified")
                    })?;
                Ok(classified)
            } else {
                crate::close_foundation::close_obligation_for_update(
                    &mut tx,
                    run.attempt_id.as_str(),
                )
                .await
            }
        }
        .await;

        match result {
            Ok(obligation) => match tx.commit().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Ok(obligation)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
            Err(error) => match tx.rollback().await {
                Ok(()) => LocalAuthorityResult::DurableFactEstablished(Err(error)),
                Err(_) => LocalAuthorityResult::DurableFactUnclassified,
            },
        }
    }
}

fn precondition(message: &str) -> DbError {
    DbError::CloseFoundationPrecondition(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_close::NativeCloseInitialResult;
    use phoenix_core::domain::db_schema::SourceToolCall;
    use phoenix_core::domain::llm_types::ContentBlock;
    use phoenix_core::domain::sm_state::{AssistantMessage, ConvState, ToolCall, ToolInput};
    use phoenix_core::llm_language::LlmLanguage;
    use serde_json::json;

    struct Fixture {
        db: Database,
        command: NativeCloseCommand,
        receipt: NativeCloseReceipt,
    }

    impl Fixture {
        async fn new() -> Self {
            let db = Database::open_in_memory().await.unwrap();
            let coordinator = db
                .get_or_create_coordinator(None, LlmLanguage::default())
                .await
                .unwrap();
            let target = db
                .create_conversation("exit-target", "exit-target", "/tmp", true, None, None)
                .await
                .unwrap()
                .product_conversation_id;
            let source = SourceToolCall {
                message_id: "exit-source-message".into(),
                tool_use_id: "exit-source-tool".into(),
            };
            let input = NativeCloseIntent::Initial {
                target: target.clone(),
            }
            .source_input();
            let state = ConvState::ToolExecuting {
                current_tool: ToolCall::new(
                    &source.tool_use_id,
                    ToolInput::Unknown {
                        name: "close_conversation".into(),
                        input: input.clone(),
                    },
                ),
                remaining_tools: vec![],
                completed_results: vec![],
                pending_sub_agents: vec![],
                assistant_message: AssistantMessage::new(
                    source.message_id.clone(),
                    vec![ContentBlock::ToolUse {
                        id: source.tool_use_id.clone(),
                        name: "close_conversation".into(),
                        input,
                    }],
                    None,
                    None,
                ),
            };
            db.update_conversation_state(&coordinator.id, &state)
                .await
                .unwrap();
            let command = NativeCloseCommand::new(
                coordinator.id,
                source,
                NativeCloseIntent::Initial { target },
            )
            .unwrap();
            let receipt = match db.apply_native_close_initial(&command).await {
                LocalAuthorityResult::DurableFactEstablished(Ok(
                    NativeCloseInitialResult::NewlyAdmitted(receipt)
                    | NativeCloseInitialResult::Replay(receipt),
                )) => receipt,
                other @ (LocalAuthorityResult::DurableFactEstablished(
                    Ok(NativeCloseInitialResult::ExistingOperationNeedsExactReference(_)) | Err(_),
                )
                | LocalAuthorityResult::DurableFactUnclassified) => {
                    panic!("initial admission failed: {other:?}");
                }
            };
            Self {
                db,
                command,
                receipt,
            }
        }
    }

    fn established(
        result: LocalAuthorityResult<DbResult<CloseObligation>>,
    ) -> DbResult<CloseObligation> {
        match result {
            LocalAuthorityResult::DurableFactEstablished(result) => result,
            LocalAuthorityResult::DurableFactUnclassified => {
                panic!("durable result was unclassified")
            }
        }
    }

    #[tokio::test]
    async fn exact_interruption_replay_retains_one_failure_and_outbox() {
        let fixture = Fixture::new().await;
        assert_eq!(run_status(&fixture).await, "running");
        assert!(matches!(
            source_state(&fixture).await,
            ConvState::ToolExecuting { .. }
        ));
        let first = established(
            fixture
                .db
                .interrupt_native_close_initial(&fixture.command, &fixture.receipt)
                .await,
        )
        .unwrap();
        let failures_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM close_cleanup_failures")
                .fetch_one(fixture.db.pool())
                .await
                .unwrap();
        let events_before: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM coordinator_watch_events WHERE route_kind = 'mandatory_close_failure'",
        )
        .fetch_one(fixture.db.pool())
        .await
        .unwrap();
        assert_eq!(failures_before, 1);
        assert_eq!(events_before, 1);
        let replay = established(
            fixture
                .db
                .interrupt_native_close_initial(&fixture.command, &fixture.receipt)
                .await,
        )
        .unwrap();
        assert_eq!(first, replay);
        assert_eq!(failures_before, 1);
        assert_eq!(events_before, 1);
        assert_eq!(run_status(&fixture).await, "stopped");
        assert_eq!(target_archived(&fixture).await, 0);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_cleanup_failures")
                .fetch_one(fixture.db.pool())
                .await
                .unwrap(),
            failures_before
        );
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM coordinator_watch_events WHERE route_kind = 'mandatory_close_failure'").fetch_one(fixture.db.pool()).await.unwrap(), events_before);
        assert!(matches!(
            source_state(&fixture).await,
            ConvState::ToolExecuting { .. }
        ));
    }

    #[tokio::test]
    async fn forged_receipt_or_source_is_refused_without_terminalization() {
        let fixture = Fixture::new().await;
        let mut forged = fixture.receipt.clone();
        forged.request_id = uuid::Uuid::new_v4();
        assert_eq!(run_status(&fixture).await, "running");
        assert_eq!(failure_count(&fixture).await, 0);
        assert!(established(
            fixture
                .db
                .interrupt_native_close_initial(&fixture.command, &forged)
                .await,
        )
        .is_err());

        let wrong_source = NativeCloseCommand::new(
            fixture.command.source_transcript_id().to_owned(),
            SourceToolCall {
                message_id: "forged-message".into(),
                tool_use_id: "forged-tool".into(),
            },
            fixture.command.intent().clone(),
        )
        .unwrap();
        assert!(established(
            fixture
                .db
                .interrupt_native_close_initial(&wrong_source, &fixture.receipt)
                .await,
        )
        .is_err());
        let status: String = sqlx::query_scalar(
            "SELECT status FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = 1",
        )
        .bind(fixture.receipt.admitted.attempt_id.as_str())
        .fetch_one(fixture.db.pool())
        .await
        .unwrap();
        assert_eq!(status, "running");
        assert_eq!(failure_count(&fixture).await, 0);
        assert!(matches!(
            source_state(&fixture).await,
            ConvState::ToolExecuting { .. }
        ));
    }

    #[tokio::test]
    async fn mandatory_outbox_failure_rolls_back_interruption_classification() {
        let fixture = Fixture::new().await;
        sqlx::query(
            "CREATE TRIGGER reject_exit_outbox BEFORE INSERT ON coordinator_watch_events
             WHEN NEW.route_kind = 'mandatory_close_failure'
             BEGIN SELECT RAISE(ABORT, 'injected mandatory outbox failure'); END",
        )
        .execute(fixture.db.pool())
        .await
        .unwrap();
        let error = established(
            fixture
                .db
                .interrupt_native_close_initial(&fixture.command, &fixture.receipt)
                .await,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("injected mandatory outbox failure"));
        let status: String = sqlx::query_scalar(
            "SELECT status FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = 1",
        )
        .bind(fixture.receipt.admitted.attempt_id.as_str())
        .fetch_one(fixture.db.pool())
        .await
        .unwrap();
        assert_eq!(status, "running");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM close_cleanup_failures")
                .fetch_one(fixture.db.pool())
                .await
                .unwrap(),
            0
        );
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM coordinator_watch_events WHERE route_kind = 'mandatory_close_failure'").fetch_one(fixture.db.pool()).await.unwrap(), 0);
        assert!(matches!(
            source_state(&fixture).await,
            ConvState::ToolExecuting { .. }
        ));
        assert_eq!(target_archived(&fixture).await, 0);
    }

    async fn run_status(fixture: &Fixture) -> String {
        sqlx::query_scalar(
            "SELECT status FROM close_runs WHERE attempt_id = ?1 AND run_ordinal = 1",
        )
        .bind(fixture.receipt.admitted.attempt_id.as_str())
        .fetch_one(fixture.db.pool())
        .await
        .unwrap()
    }

    async fn failure_count(fixture: &Fixture) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM close_cleanup_failures")
            .fetch_one(fixture.db.pool())
            .await
            .unwrap()
    }

    async fn source_state(fixture: &Fixture) -> ConvState {
        let state: String = sqlx::query_scalar("SELECT state FROM conversations WHERE id = ?1")
            .bind(fixture.command.source_transcript_id())
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
        serde_json::from_str(&state).unwrap()
    }

    async fn target_archived(fixture: &Fixture) -> i64 {
        sqlx::query_scalar("SELECT archived FROM conversations WHERE id = 'exit-target'")
            .fetch_one(fixture.db.pool())
            .await
            .unwrap()
    }
}
