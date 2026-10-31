use super::*;

#[tokio::test]
async fn native_settings_use_current_coordinator_source_and_exact_ordinary_target() {
    let db = Database::open_in_memory().await.unwrap();
    let ordinary = db
        .create_conversation("ordinary", "ordinary", "/tmp", true, None, None)
        .await
        .unwrap();
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let target = AutomaticContinuationTarget::Ordinary(&ordinary.product_conversation_id);

    let initial = db
        .read_native_automatic_continuation_settings(&coordinator.id, target)
        .await
        .unwrap();
    assert_eq!(
        initial.aggregate,
        ResolvedAutomaticContinuationAggregate::Ordinary(ordinary.product_conversation_id.clone())
    );
    assert_eq!(
        initial.preference,
        AutoContinueOnContextExhaustion::Disabled
    );

    let set = db
        .set_native_automatic_continuation_settings(
            &coordinator.id,
            target,
            AutoContinueOnContextExhaustion::Enabled,
        )
        .await
        .established()
        .expect("setting transaction is acknowledged")
        .unwrap();
    assert_eq!(set.preference, AutoContinueOnContextExhaustion::Enabled);
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&ordinary.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Enabled
    );

    let stale = db
        .read_native_automatic_continuation_settings("not-current-coordinator", target)
        .await;
    assert!(matches!(stale, Err(DbError::CurrentCoordinatorRequired)));
    let refused = db
        .set_native_automatic_continuation_settings(
            &ordinary.id,
            target,
            AutoContinueOnContextExhaustion::Disabled,
        )
        .await
        .established()
        .expect("validation rollback is acknowledged");
    assert!(matches!(refused, Err(DbError::CurrentCoordinatorRequired)));
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&ordinary.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Enabled
    );
}

#[tokio::test]
async fn native_coordinator_target_is_explicit_and_typed() {
    let db = Database::open_in_memory().await.unwrap();
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let settings = db
        .read_native_automatic_continuation_settings(
            &coordinator.id,
            AutomaticContinuationTarget::Coordinator,
        )
        .await
        .unwrap();
    assert_eq!(
        settings.aggregate,
        ResolvedAutomaticContinuationAggregate::Coordinator(
            coordinator.product_conversation_id.clone()
        )
    );
    assert_eq!(
        settings.aggregate.product_conversation_id(),
        &coordinator.product_conversation_id
    );
    assert_eq!(
        settings.preference,
        AutoContinueOnContextExhaustion::Disabled
    );
    let updated = db
        .set_native_automatic_continuation_settings(
            &coordinator.id,
            AutomaticContinuationTarget::Coordinator,
            AutoContinueOnContextExhaustion::Enabled,
        )
        .await
        .established()
        .expect("setting transaction is acknowledged")
        .unwrap();
    assert_eq!(
        updated.aggregate,
        ResolvedAutomaticContinuationAggregate::Coordinator(
            coordinator.product_conversation_id.clone()
        )
    );
    assert_eq!(updated.preference, AutoContinueOnContextExhaustion::Enabled);
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&coordinator.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Enabled
    );
}

#[tokio::test]
async fn lost_commit_acknowledgement_is_unclassified_after_preference_changes() {
    let db = Database::open_in_memory().await.unwrap();
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let target = AutomaticContinuationTarget::Coordinator;

    let result = db
        .set_native_automatic_continuation_settings_with_commit(
            &coordinator.id,
            target,
            AutoContinueOnContextExhaustion::Enabled,
            SettingsCommit::LoseAcknowledgement,
        )
        .await;

    assert!(matches!(
        result,
        LocalAuthorityResult::DurableFactUnclassified
    ));
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&coordinator.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Enabled
    );
}

#[tokio::test]
async fn coordinator_rotation_invalidates_old_source_for_read_and_set() {
    let db = Database::open_in_memory().await.unwrap();
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let target = AutomaticContinuationTarget::Coordinator;
    db.set_native_automatic_continuation_settings(
        &coordinator.id,
        target,
        AutoContinueOnContextExhaustion::Enabled,
    )
    .await
    .established()
    .expect("setting transaction is acknowledged")
    .unwrap();
    db.update_conversation_state(
        &coordinator.id,
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: "rotate coordinator".into(),
        },
    )
    .await
    .unwrap();
    let successor = match db.continue_conversation(&coordinator.id).await.unwrap() {
        crate::ContinueOutcome::Created(successor) => successor,
        other => panic!("expected a Coordinator successor, got {other:?}"),
    };

    assert!(matches!(
        db.read_native_automatic_continuation_settings(&coordinator.id, target)
            .await,
        Err(DbError::CurrentCoordinatorRequired)
    ));
    let rejected = db
        .set_native_automatic_continuation_settings(
            &coordinator.id,
            target,
            AutoContinueOnContextExhaustion::Disabled,
        )
        .await
        .established()
        .expect("validation rollback is acknowledged");
    assert!(matches!(rejected, Err(DbError::CurrentCoordinatorRequired)));
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&coordinator.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Enabled
    );
    assert_eq!(
        db.read_native_automatic_continuation_settings(&successor.id, target)
            .await
            .unwrap()
            .preference,
        AutoContinueOnContextExhaustion::Enabled
    );
}

#[tokio::test]
async fn ordinary_target_rejects_transcript_and_coordinator_id_targets() {
    let db = Database::open_in_memory().await.unwrap();
    let ordinary = db
        .create_conversation("ordinary", "ordinary", "/tmp", true, None, None)
        .await
        .unwrap();
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let source = &coordinator.id;

    let coordinator_product_id = coordinator.product_conversation_id.as_str().to_string();
    let missing_product_id =
        ProductConversationId::parse(uuid::Uuid::new_v4().to_string()).unwrap();
    for id in [&ordinary.id, &coordinator_product_id] {
        let parsed = ProductConversationId::parse(id.clone()).unwrap();
        let result = db
            .read_native_automatic_continuation_settings(
                source,
                AutomaticContinuationTarget::Ordinary(&parsed),
            )
            .await;
        assert!(result.is_err());
    }
    assert!(db
        .read_native_automatic_continuation_settings(
            source,
            AutomaticContinuationTarget::Ordinary(&missing_product_id),
        )
        .await
        .is_err());
}

#[tokio::test]
async fn failed_update_trigger_rolls_back_without_changing_preference() {
    let db = Database::open_in_memory().await.unwrap();
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_auto_continue_update BEFORE UPDATE OF auto_continue_on_context_exhaustion ON product_conversations BEGIN SELECT RAISE(ABORT, 'rejected'); END",
    )
    .execute(db.pool())
    .await
    .unwrap();

    let result = db
        .set_native_automatic_continuation_settings(
            &coordinator.id,
            AutomaticContinuationTarget::Coordinator,
            AutoContinueOnContextExhaustion::Enabled,
        )
        .await
        .established()
        .expect("trigger abort rollback is acknowledged");

    assert!(result.is_err());
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&coordinator.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Disabled
    );
}

#[tokio::test]
async fn native_setting_changes_are_prospective_at_summary_commit() {
    use chrono::Utc;
    use phoenix_core::domain::db_schema::{InputOrigin, MessageContent};
    use phoenix_core::domain::sm_state::{ContinuationSummaryRequest, ConvState};

    for initially_enabled in [false, true] {
        let db = Database::open_in_memory().await.unwrap();
        let coordinator = db
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let parent = db
            .create_conversation(
                "prospective-setting",
                "prospective-setting",
                "/tmp",
                true,
                None,
                None,
            )
            .await
            .unwrap();
        let target = AutomaticContinuationTarget::Ordinary(&parent.product_conversation_id);
        db.set_native_automatic_continuation_settings(
            &coordinator.id,
            target,
            AutoContinueOnContextExhaustion::from(initially_enabled),
        )
        .await
        .established()
        .expect("initial preference classified")
        .unwrap();
        db.update_conversation_state(
            &parent.id,
            &ConvState::AwaitingContinuation {
                request: ContinuationSummaryRequest {
                    operation_id: "preference-summary".into(),
                    rejected_tool_calls: Vec::new(),
                    attempt: 1,
                },
            },
        )
        .await
        .unwrap();
        let summary_text = "generated summary before preference flip";
        let content = MessageContent::continuation(summary_text);
        let summary = crate::Message {
            message_id: "prospective-summary".into(),
            conversation_id: parent.id.clone(),
            sequence_id: 1,
            message_type: content.message_type(),
            content,
            display_data: None,
            usage_data: None,
            created_at: Utc::now(),
            origin: InputOrigin::SystemGenerated,
        };
        let exhausted = ConvState::ContextExhausted {
            summary: summary_text.into(),
        };
        assert_eq!(
            db.commit_continuation(
                &parent.id,
                "preference-summary",
                &summary,
                &exhausted,
                summary.created_at,
            )
            .await
            .unwrap(),
            crate::ContinuationCommitOutcome::Applied,
        );
        let admission_before = db
            .automatic_continuation_admission(&parent.id)
            .await
            .unwrap();
        assert_eq!(admission_before.is_some(), initially_enabled);
        let intent_before = db.continuation_dispatch_intent(&parent.id).await.unwrap();
        let messages_before = db.get_messages(&parent.id).await.unwrap();

        let changed = db
            .set_native_automatic_continuation_settings(
                &coordinator.id,
                target,
                AutoContinueOnContextExhaustion::from(!initially_enabled),
            )
            .await
            .established()
            .expect("prospective preference flip classified")
            .unwrap();
        assert_eq!(bool::from(changed.preference), !initially_enabled);
        assert_eq!(
            db.automatic_continuation_admission(&parent.id)
                .await
                .unwrap(),
            admission_before,
        );
        assert_eq!(
            db.continuation_dispatch_intent(&parent.id).await.unwrap(),
            intent_before,
        );
        let after = db.get_conversation(&parent.id).await.unwrap();
        assert_eq!(after.state, exhausted);
        assert!(after.continued_in_conv_id.is_none());
        let messages_after = db.get_messages(&parent.id).await.unwrap();
        assert_eq!(messages_after.len(), messages_before.len());
        assert_eq!(messages_after[0].message_id, summary.message_id);
    }
}

#[tokio::test]
async fn native_settings_reject_subagent_sources_targets_and_unpublished_products() {
    let db = Database::open_in_memory().await.unwrap();
    let coordinator = db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let root = db
        .create_conversation(
            "settings-unpublished",
            "settings-unpublished",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let child = db
        .create_subagent_conversation(
            "settings-subagent",
            "settings-subagent",
            "/tmp",
            &root.id,
            "offline-model",
            &root.conv_mode,
            phoenix_core::llm_language::LlmLanguage::default(),
            root.attached_work_scope_id.as_ref(),
            crate::SubAgentExecution {
                connection: "mock",
                effort: None,
                persona: None,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        db.read_native_automatic_continuation_settings(
            &child.id,
            AutomaticContinuationTarget::Coordinator
        )
        .await,
        Err(DbError::CurrentCoordinatorRequired),
    ));
    assert!(matches!(
        db.set_native_automatic_continuation_settings(
            &child.id,
            AutomaticContinuationTarget::Coordinator,
            AutoContinueOnContextExhaustion::Enabled,
        )
        .await
        .established()
        .unwrap(),
        Err(DbError::CurrentCoordinatorRequired),
    ));
    let child_reference: phoenix_core::domain::product_conversation::ProductConversationId =
        child.id.parse().unwrap();
    let child_target = AutomaticContinuationTarget::Ordinary(&child_reference);
    assert!(db
        .read_native_automatic_continuation_settings(&coordinator.id, child_target)
        .await
        .is_err());
    assert!(db
        .set_native_automatic_continuation_settings(
            &coordinator.id,
            child_target,
            AutoContinueOnContextExhaustion::Enabled,
        )
        .await
        .established()
        .unwrap()
        .is_err());
    assert_eq!(child.product_conversation_id, root.product_conversation_id);
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&root.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Disabled
    );
    sqlx::query("INSERT INTO product_creation_jobs(request_id, product_conversation_id, cwd, objective, llm_language, status, accepted_at_unix_micros, updated_at_unix_micros, published_product_id, published_conversation_id, provisioning_stage, stage_updated_at_unix_micros, objective_origin) VALUES ('metadata-staged-job', ?1, '/tmp', '', 'phoenix-native', 'delivery_pending', 1, 1, ?1, ?2, 'bootstrap_initial_turn', 1, 'user_api')")
        .bind(root.product_conversation_id.as_str())
        .bind(&root.id)
        .execute(db.pool())
        .await
        .unwrap();
    let unpublished_target = AutomaticContinuationTarget::Ordinary(&root.product_conversation_id);
    assert!(db
        .read_native_automatic_continuation_settings(&coordinator.id, unpublished_target)
        .await
        .is_err());
    assert!(db
        .set_native_automatic_continuation_settings(
            &coordinator.id,
            unpublished_target,
            AutoContinueOnContextExhaustion::Enabled,
        )
        .await
        .established()
        .expect("invalid-target rollback acknowledged")
        .is_err());
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&root.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Disabled,
    );
}
