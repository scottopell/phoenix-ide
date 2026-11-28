//! Close orchestration shared by application entry points.

use crate::api::handlers::AppError;
use crate::api::{AppState, ConflictErrorResponse};
use crate::runtime::close_retirement::{
    GenericCloseIngress, GenericCloseIngressError, GenericCloseIngressOutcome,
};
use phoenix_core::domain::close::{
    CloseAttemptId, CloseObligation, ClosePhase, TranscriptConversationId,
};
use phoenix_core::domain::product_conversation::OrdinaryProductConversationLifecycle;

pub(crate) async fn require_close_settlement_without_retained_custody(
    state: &AppState,
    attempt_id: &phoenix_core::domain::close::CloseAttemptId,
) -> Result<(), AppError> {
    state
        .db
        .require_no_retained_legacy_creation_for_retirement(attempt_id)
        .await
        .map_err(|error| {
            let crate::db::DbError::CloseFoundationPrecondition(detail) = error else {
                return AppError::Internal(error.to_string());
            };
            AppError::Conflict(Box::new(ConflictErrorResponse::new(
                detail,
                "close_settlement_in_progress",
            )))
        })
}

fn close_settlement_conflict(detail: String) -> AppError {
    AppError::Conflict(Box::new(ConflictErrorResponse::new(
        detail,
        "close_settlement_in_progress",
    )))
}

pub(crate) fn project_completed_close(
    outcome: Option<phoenix_core::domain::close::CloseCompletionOutcome>,
    detail: Option<String>,
) -> Result<(), AppError> {
    use phoenix_core::domain::close::CloseCompletionOutcome;

    let (message, error_type) = match outcome {
        Some(
            CloseCompletionOutcome::Archived | CloseCompletionOutcome::ArchivedCleanupAttention,
        ) => {
            return Ok(());
        }
        Some(CloseCompletionOutcome::CloseIncomplete) => (
            "Close could not confirm that conversation and process shutdown completed",
            "close_incomplete",
        ),
        Some(CloseCompletionOutcome::Cancelled) => (
            "Close was cancelled before archival completed",
            "close_cancelled",
        ),
        None => {
            return Err(AppError::Internal(
                "completed Close attempt has no terminal outcome".to_string(),
            ));
        }
    };
    Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
        detail.unwrap_or_else(|| message.to_string()),
        error_type,
    ))))
}

pub(crate) async fn project_close_worker_failure(
    state: &AppState,
    attempt_id: &CloseAttemptId,
    detail: String,
) -> Result<(), AppError> {
    let phase = match state.db.get_close_obligation(attempt_id.as_str()).await {
        Ok(current) if current.phase() == ClosePhase::Completed => {
            return project_completed_close(current.close_outcome(), Some(detail));
        }
        Ok(current) => Some(current.phase()),
        Err(crate::db::DbError::CloseFoundationNotFound(_)) => None,
        Err(error) => return Err(AppError::Internal(error.to_string())),
    };
    if state.runtime.fatal_local_authority_is_latched() {
        return Err(AppError::TypedInternal {
            message: detail,
            error_type: "close_authority_unavailable".into(),
        });
    }
    let error_type = match phase {
        Some(ClosePhase::NeedsRepair | ClosePhase::RetirementRequested) => {
            "close_retirement_needs_repair"
        }
        Some(ClosePhase::AwaitingRetirementInspection) => "close_inspection_failed",
        _ => "close_settlement_in_progress",
    };
    Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
        detail, error_type,
    ))))
}

async fn observe_close_settlement(
    state: &AppState,
    attempt_id: &CloseAttemptId,
) -> Result<CloseObligation, AppError> {
    let advanced = state
        .db
        .get_close_obligation(attempt_id.as_str())
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?;
    if advanced.phase() == ClosePhase::SettlingActiveWork {
        require_close_settlement_without_retained_custody(state, advanced.attempt_id()).await?;
    }
    if matches!(
        advanced.phase(),
        ClosePhase::SettlingActiveWork | ClosePhase::CancelRequestedDuringSettlement
    ) {
        return Err(close_settlement_conflict(
            "Close settlement remains in progress".into(),
        ));
    }
    Ok(advanced)
}

#[cfg(test)]
struct CloseObservationPause {
    reached: tokio::sync::oneshot::Sender<Option<OrdinaryProductConversationLifecycle>>,
    resume: tokio::sync::oneshot::Receiver<()>,
}

#[cfg(test)]
tokio::task_local! {
    static CLOSE_OBSERVATION_PAUSE: std::cell::RefCell<Option<CloseObservationPause>>;
}

#[cfg(test)]
pub(crate) async fn pause_after_close_observation(
    lifecycle: Option<OrdinaryProductConversationLifecycle>,
) {
    let Ok(Some(pause)) = CLOSE_OBSERVATION_PAUSE.try_with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    pause.reached.send(lifecycle).unwrap();
    pause.resume.await.unwrap();
}

pub(crate) async fn run_legacy_close_compat(
    state: &AppState,
    id: &str,
    action: &str,
) -> Result<(), AppError> {
    let transcript = state
        .db
        .get_conversation(id)
        .await
        .map_err(|error| AppError::NotFound(error.to_string()))?;
    if action != "archive"
        && action != "archive chain"
        && transcript.conv_mode.worktree_path().is_none()
    {
        return Err(AppError::BadRequest(format!(
            "Conversation must own an allocated worktree to {action}"
        )));
    }
    let aggregate = state
        .db
        .get_ordinary_product_conversation(&transcript.product_conversation_id)
        .await
        .map_err(|error| AppError::NotFound(error.to_string()))?;
    if aggregate.latest_transcript_row_id != id {
        return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
            "Close is accepted only from the active aggregate transcript",
            "inactive_close_transcript",
        ))));
    }
    #[cfg(test)]
    pause_after_close_observation(aggregate.product_conversation.ordinary_lifecycle()).await;
    run_observed_close(state, &aggregate).await
}

#[allow(clippy::too_many_lines, clippy::single_match_else)]
pub(crate) async fn run_observed_close(
    state: &AppState,
    aggregate: &crate::db::ProductConversationAggregate,
) -> Result<(), AppError> {
    let product = aggregate.product_conversation.id();
    let expected_latest_transcript =
        TranscriptConversationId::parse(aggregate.latest_transcript_row_id.clone())
            .map_err(|error| AppError::Internal(error.to_string()))?;
    let mut obligation = match state
        .db
        .get_active_close_obligation_for_product(product)
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?
    {
        Some(obligation) => obligation,
        None => {
            if aggregate.product_conversation.ordinary_lifecycle()
                == Some(OrdinaryProductConversationLifecycle::History)
            {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    "ProductConversation is already in History",
                    "close_already_history",
                ))));
            }
            let attempt_id = CloseAttemptId::parse(uuid::Uuid::new_v4().to_string())
                .expect("UUID is a valid Close attempt id");
            let worker = state
                .runtime
                .start_generic_close_ingress(
                    attempt_id.clone(),
                    product.clone(),
                    expected_latest_transcript,
                    GenericCloseIngress::Initial,
                )
                .map_err(close_settlement_conflict)?;
            match worker.await {
                Ok(Ok(GenericCloseIngressOutcome::Continued)) => {
                    observe_close_settlement(state, &attempt_id).await?
                }
                Ok(Ok(GenericCloseIngressOutcome::Observed(obligation))) => *obligation,
                Ok(Err(GenericCloseIngressError::Admission(error))) => {
                    let error_type =
                        if matches!(error, crate::db::DbError::CloseFoundationStaleLatest { .. }) {
                            "stale_latest_close_transcript"
                        } else {
                            "close_start_failed"
                        };
                    return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                        error.to_string(),
                        error_type,
                    ))));
                }
                Ok(Err(GenericCloseIngressError::Worker(error))) => {
                    return project_close_worker_failure(state, &attempt_id, error).await;
                }
                Err(error) => {
                    return project_close_worker_failure(
                        state,
                        &attempt_id,
                        format!("Close settlement worker failed: {error}"),
                    )
                    .await;
                }
            }
        }
    };
    loop {
        obligation = match obligation.phase() {
            ClosePhase::AwaitingBlockerResolution => {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    "The retained Close attempt has no committed settlement to observe",
                    "close_start_failed",
                ))));
            }
            ClosePhase::AwaitingStopWorkConfirmation => {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    format!(
                        "Close attempt {} still requires explicit stop-work confirmation",
                        obligation.attempt_id()
                    ),
                    "close_stop_work_confirmation_required",
                ))));
            }
            ClosePhase::SettlingActiveWork | ClosePhase::CancelRequestedDuringSettlement => {
                state
                    .runtime
                    .wait_for_close_owner(obligation.attempt_id())
                    .await;
                observe_close_settlement(state, obligation.attempt_id()).await?
            }
            ClosePhase::AwaitingRetirementInspection | ClosePhase::RetirementRequested => {
                state
                    .runtime
                    .wait_for_close_owner(obligation.attempt_id())
                    .await;
                let current = state
                    .db
                    .get_close_obligation(obligation.attempt_id().as_str())
                    .await
                    .map_err(|error| AppError::Internal(error.to_string()))?;
                if matches!(
                    current.phase(),
                    ClosePhase::AwaitingRetirementInspection | ClosePhase::RetirementRequested
                ) {
                    let error_type = if current.phase() == ClosePhase::RetirementRequested {
                        "close_retirement_needs_repair"
                    } else {
                        "close_inspection_failed"
                    };
                    return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                        "Close continuation has not reached a durable outcome",
                        error_type,
                    ))));
                }
                current
            }
            ClosePhase::AwaitingLossConfirmation => {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    "Close requires confirmation of the exact persisted loss inventory",
                    "close_loss_confirmation_required",
                ))));
            }
            ClosePhase::NeedsRepair => {
                return Err(AppError::Conflict(Box::new(ConflictErrorResponse::new(
                    "Close retirement requires repair before it can continue",
                    "close_retirement_needs_repair",
                ))));
            }
            ClosePhase::Completed => {
                return project_completed_close(obligation.close_outcome(), None);
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt as _;

    async fn post_close(router: axum::Router, path: String) -> axum::response::Response {
        router
            .oneshot(Request::post(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn public_close_routes_keep_the_open_observation_across_winner_archival() {
        for legacy_alias in [false, true] {
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                assert_public_close_overlap(legacy_alias).await;
            })
            .await
            .expect("overlapping Close requests must finish without blocking the winner");
        }
    }

    async fn assert_public_close_overlap(legacy_alias: bool) {
        let state = crate::api::handlers::api_test_support::make_test_state().await;
        let id = "overlapping-public-close";
        let conversation = state
            .db
            .create_conversation(id, id, "/tmp", true, None, None)
            .await
            .unwrap();
        let router = crate::api::handlers::create_router(state.clone());
        let product_route = format!("/api/product-conversations/{id}/close");
        let losing_route = if legacy_alias {
            format!("/api/conversations/{id}/archive")
        } else {
            product_route.clone()
        };
        let (reached, arrived) = tokio::sync::oneshot::channel();
        let (release, resume) = tokio::sync::oneshot::channel();
        let losing_request = tokio::spawn(CLOSE_OBSERVATION_PAUSE.scope(
            std::cell::RefCell::new(Some(CloseObservationPause { reached, resume })),
            post_close(router.clone(), losing_route.clone()),
        ));
        assert_eq!(
            arrived.await.unwrap(),
            Some(OrdinaryProductConversationLifecycle::Open)
        );

        assert_eq!(
            post_close(router.clone(), product_route).await.status(),
            StatusCode::OK
        );
        let retained: (String, String) = sqlx::query_as(
            "SELECT attempt_id, close_outcome FROM close_obligations WHERE product_conversation_id = ?1",
        )
        .bind(conversation.product_conversation_id.as_str())
        .fetch_one(state.db.pool())
        .await
        .unwrap();
        assert_eq!(retained.1, "archived");
        assert!(state.db.get_conversation(id).await.unwrap().archived);

        release.send(()).unwrap();
        assert_eq!(losing_request.await.unwrap().status(), StatusCode::OK);
        let attempts: Vec<(String, String)> = sqlx::query_as(
            "SELECT attempt_id, close_outcome FROM close_obligations WHERE product_conversation_id = ?1",
        )
        .bind(conversation.product_conversation_id.as_str())
        .fetch_all(state.db.pool())
        .await
        .unwrap();
        assert_eq!(attempts, vec![retained]);
        let counts: (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM close_runs),
                    (SELECT count(*) FROM native_close_initial_receipts),
                    (SELECT count(*) FROM close_retirement_resource_dispatches)",
        )
        .fetch_one(state.db.pool())
        .await
        .unwrap();
        assert_eq!(counts, (1, 0, 0));

        let later_request = post_close(router, losing_route).await;
        assert_eq!(later_request.status(), StatusCode::CONFLICT);
        let body = to_bytes(later_request.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error_type"], "close_already_history");
    }

    #[tokio::test]
    async fn public_close_does_not_report_fenced_settlement_as_in_progress() {
        let state = crate::api::handlers::api_test_support::make_test_state().await;
        let id = "public-close-failed-settlement";
        state
            .db
            .create_conversation(id, id, "/tmp", true, None, None)
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TRIGGER fail_handoff BEFORE UPDATE OF phase ON close_obligations
             WHEN OLD.phase = 'settling_active_work' AND NEW.phase = 'awaiting_retirement_inspection'
             BEGIN SELECT RAISE(ABORT, 'injected handoff failure'); END;
             CREATE TRIGGER fail_terminalization BEFORE INSERT ON close_cleanup_failures
             BEGIN SELECT RAISE(ABORT, 'injected terminalization failure'); END;",
        )
        .execute(state.db.pool())
        .await
        .unwrap();

        let response = post_close(
            crate::api::handlers::create_router(state.clone()),
            format!("/api/product-conversations/{id}/close"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error_type"], "close_authority_unavailable");
        assert_eq!(
            state.runtime.observed_local_authority_failure_boundary(),
            Some("generic_close_settlement_unclassified")
        );
        assert!(!state.db.get_conversation(id).await.unwrap().archived);
    }

    #[test]
    fn public_close_projects_committed_cleanup_attention_as_success() {
        use phoenix_core::domain::close::CloseCompletionOutcome;

        assert!(project_completed_close(
            Some(CloseCompletionOutcome::ArchivedCleanupAttention),
            Some("worktree retained for cleanup".into()),
        )
        .is_ok());
    }

    #[tokio::test]
    async fn concurrent_initial_observes_winner_but_later_http_close_sees_history() {
        use phoenix_core::domain::close::CloseCompletionOutcome;

        let state = crate::api::handlers::api_test_support::make_test_state().await;
        let id = "concurrent-initial-close";
        state
            .db
            .create_conversation(id, id, "/tmp", true, None, None)
            .await
            .unwrap();
        let product = state
            .db
            .get_conversation(id)
            .await
            .unwrap()
            .product_conversation_id;
        let latest = TranscriptConversationId::parse(id).unwrap();
        let losing_attempt = CloseAttemptId::parse("losing-initial-close").unwrap();

        let open = state
            .db
            .get_ordinary_product_conversation(&product)
            .await
            .unwrap();
        assert_eq!(
            open.product_conversation.ordinary_lifecycle(),
            Some(OrdinaryProductConversationLifecycle::Open)
        );
        crate::api::product_conversations::close_product_conversation(
            axum::extract::State(state.clone()),
            axum::extract::Path(id.to_string()),
        )
        .await
        .unwrap();

        let worker = state
            .runtime
            .start_generic_close_ingress(
                losing_attempt.clone(),
                product.clone(),
                latest,
                GenericCloseIngress::Initial,
            )
            .unwrap();
        let Ok(GenericCloseIngressOutcome::Observed(retained)) = worker.await.unwrap() else {
            panic!("competing Initial must observe the committed winner");
        };
        assert_eq!(
            retained.close_outcome(),
            Some(CloseCompletionOutcome::Archived)
        );
        assert_ne!(retained.attempt_id(), &losing_attempt);
        run_observed_close(&state, &open).await.unwrap();
        let rows: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM close_obligations WHERE attempt_id = ?1)
                  + (SELECT count(*) FROM close_runs WHERE attempt_id = ?1)
                  + (SELECT count(*) FROM native_close_initial_receipts WHERE attempt_id = ?1)
                  + (SELECT count(*) FROM close_retirement_resource_dispatches WHERE attempt_id = ?1)",
        )
        .bind(losing_attempt.as_str())
        .fetch_one(state.db.pool())
        .await
        .unwrap();
        assert_eq!(rows, 0);
        let attempts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM close_obligations WHERE product_conversation_id = ?1",
        )
        .bind(product.as_str())
        .fetch_one(state.db.pool())
        .await
        .unwrap();
        assert_eq!(attempts, 1);

        let stale = state
            .db
            .begin_generic_close_settlement_classified(
                &product,
                &TranscriptConversationId::parse("stale-old-transcript").unwrap(),
                &losing_attempt,
            )
            .await
            .established()
            .expect("classified fixture transaction");
        assert!(matches!(
            stale,
            Err(crate::db::DbError::CloseFoundationStaleLatest { .. })
        ));
        let other = "different-close-target";
        let other_product = state
            .db
            .create_conversation(other, other, "/tmp", true, None, None)
            .await
            .unwrap()
            .product_conversation_id;
        state
            .db
            .begin_generic_close_settlement_classified(
                &other_product,
                &TranscriptConversationId::parse(other).unwrap(),
                &losing_attempt,
            )
            .await
            .established()
            .expect("classified fixture transaction")
            .unwrap();
        assert!(matches!(
            state
                .db
                .begin_generic_close_settlement_classified(
                    &product,
                    &TranscriptConversationId::parse(id).unwrap(),
                    &losing_attempt,
                )
                .await
                .established()
                .expect("classified fixture transaction"),
            Err(crate::db::DbError::CloseFoundationConflict(_))
        ));
        assert!(
            matches!(crate::api::product_conversations::close_product_conversation(
            axum::extract::State(state), axum::extract::Path(id.to_string()),
        ).await, Err(AppError::Conflict(conflict)) if conflict.error_type == "close_already_history")
        );
    }

    #[tokio::test]
    async fn public_close_observes_unprepared_attempt_without_advancing_it() {
        let state = crate::api::handlers::api_test_support::make_test_state().await;
        let id = "retained-unprepared-close";
        state
            .db
            .create_conversation(id, id, "/tmp", true, None, None)
            .await
            .unwrap();
        let transcript = state.db.get_conversation(id).await.unwrap();
        let latest = TranscriptConversationId::parse(id).unwrap();
        let before = state
            .db
            .begin_close_foundation(&transcript.product_conversation_id, &latest, id)
            .await
            .unwrap();
        assert_eq!(before.phase(), ClosePhase::AwaitingBlockerResolution);

        assert!(matches!(
            crate::api::product_conversations::close_product_conversation(
                axum::extract::State(state.clone()),
                axum::extract::Path(id.to_string()),
            )
            .await,
            Err(AppError::Conflict(conflict)) if conflict.error_type == "close_start_failed"
        ));
        assert_eq!(state.db.get_close_obligation(id).await.unwrap(), before);
    }
}
