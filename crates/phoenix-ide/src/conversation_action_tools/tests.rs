use super::*;

fn submit() -> Value {
    json!({
        "op": "create", "request_id": "01b22123-04de-4b01-88dd-83e74eb2ed51",
        "cwd": "/tmp/project", "objective": "/skill this stays literal",
        "start": "submit_objective"
    })
}

#[test]
fn create_requires_explicit_submit_and_rejects_uncommissioned_options() {
    assert!(serde_json::from_value::<CreateRequest>(submit()).is_ok());
    let mut missing = submit();
    missing.as_object_mut().unwrap().remove("start");
    assert!(serde_json::from_value::<CreateRequest>(missing).is_err());
    for start in ["dormant", "create_only", ""] {
        let mut value = submit();
        value["start"] = json!(start);
        assert!(serde_json::from_value::<CreateRequest>(value).is_err());
    }
    for key in [
        "images",
        "attachments",
        "work_scope_id",
        "artifact_ref",
        "mode",
    ] {
        let mut value = submit();
        value[key] = json!([]);
        assert!(
            serde_json::from_value::<CreateRequest>(value).is_err(),
            "{key}"
        );
    }
    for op in ["cancel", "retry_delivery"] {
        assert!(serde_json::from_value::<CreateRequest>(json!({"op": op})).is_err());
    }
}

#[test]
fn create_decodes_explicit_start_and_omitted_defaults() {
    let CreateRequest::Create {
        start: CreateStart::SubmitObjective,
        model,
        effort,
        llm_language,
        ..
    } = serde_json::from_value(submit()).unwrap()
    else {
        panic!()
    };
    assert!(model.is_none());
    assert!(effort.is_none());
    assert!(llm_language.is_none());
}

#[test]
fn rename_decoder_requires_cas_and_rejects_retargeting_extra_fields() {
    let valid = json!({
        "op": "rename", "operation_id": "01b22123-04de-4b01-88dd-83e74eb2ed51",
        "product_conversation_id": "product", "expected_root_transcript_id": "root",
        "expected_name_revision": 2, "title": "new title"
    });
    assert!(serde_json::from_value::<RenameRequest>(valid.clone()).is_ok());
    for field in [
        "expected_name_revision",
        "expected_root_transcript_id",
        "operation_id",
    ] {
        let mut value = valid.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<RenameRequest>(value).is_err());
    }
    let mut value = valid;
    value["slug"] = json!("guessed-current-slug");
    assert!(serde_json::from_value::<RenameRequest>(value).is_err());
}

#[test]
fn rename_detail_uses_shared_encoded_canonical_route_with_stable_id_fallback() {
    let mut detail = crate::db::ProductConversationRenameDetail {
        can_rename: true,
        product_conversation_id: "pc_test".parse().unwrap(),
        root_transcript_id: "root".into(),
        name_revision: 7,
        title: Some("東京 / name".into()),
        slug: Some("東京 / name".into()),
    };
    let response = rename_detail_response(&detail);
    assert_eq!(
        response["canonical_route"],
        "/c/%E6%9D%B1%E4%BA%AC%20%2F%20name"
    );
    assert_eq!(response["detail"]["name_revision"], 7);
    assert_eq!(
        response["allowed_actions"],
        json!(["detail", "read", "rename"])
    );
    detail.can_rename = false;
    assert_eq!(
        rename_detail_response(&detail)["allowed_actions"],
        json!(["detail", "read"])
    );
    detail.slug = None;
    assert_eq!(
        rename_detail_response(&detail)["canonical_route"],
        "/c/pc_test"
    );
}

fn native_context(transcript: &str) -> ToolContext {
    struct Offline;
    impl phoenix_core::llm_service::LlmSelector for Offline {
        fn get(&self, _: &str) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
            None
        }
        fn default_service(&self) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
            None
        }
    }
    ToolContext::new_without_filesystem(
        tokio_util::sync::CancellationToken::new(),
        transcript.into(),
        Arc::new(phoenix_tools::BrowserSessionManager::default()),
        Arc::new(phoenix_tools::BashHandleRegistry::new()),
        Arc::new(Offline),
        phoenix_terminal::ActiveTerminals::new(),
        Arc::new(phoenix_tools::TmuxRegistry::new()),
    )
    .with_svg_assistant_message_id("native-message")
    .with_tool_use_id("native-call")
}

async fn native_runtime() -> (Arc<RuntimeManager>, crate::db::Database, String) {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    let coordinator = db
        .get_or_create_coordinator(None, LlmLanguage::default())
        .await
        .unwrap();
    let registry = phoenix_llm::ModelRegistry::new(&phoenix_llm::LlmConfig {
        openai_api_key: Some("fixture-not-dispatched".into()),
        ..phoenix_llm::LlmConfig::default()
    });
    let runtime = Arc::new(RuntimeManager::new(
        db.clone(),
        Arc::new(registry),
        crate::platform::PlatformCapability::None {
            details: "offline tools fixture".into(),
        },
        Arc::new(crate::tools::mcp::McpClientManager::new()),
        None,
    ));
    (runtime, db, coordinator.id)
}

#[tokio::test]
async fn native_creation_read_and_submit_receipt_require_current_snapshot_authority() {
    let (runtime, db, gc) = native_runtime().await;
    let dir = tempfile::tempdir().unwrap();
    let request_id = uuid::Uuid::new_v4().to_string();
    let registry = tools(runtime.clone(), &gc).await.unwrap();
    let create = registry
        .iter()
        .find(|tool| tool.name() == "create_conversation")
        .unwrap();
    let accepted = create
        .run(
            json!({
                "op":"create", "request_id":request_id,
                "cwd":dir.path().to_string_lossy(), "objective":"snapshot objective",
                "start":"submit_objective", "model":"gpt-5.6-sol"
            }),
            native_context(&gc),
        )
        .await;
    assert_eq!(
        serde_json::from_str::<Value>(accepted.output()).unwrap()["status"],
        "accepted"
    );
    let replay = create
        .run(
            json!({
                "op":"create", "request_id":request_id,
                "cwd":dir.path().to_string_lossy(), "objective":"snapshot objective",
                "start":"submit_objective", "model":"gpt-5.6-sol"
            }),
            native_context(&gc),
        )
        .await;
    let replay: Value = serde_json::from_str(replay.output()).unwrap();
    assert_eq!(replay["status"], "accepted");
    assert_eq!(replay["request_id"], request_id);
    let changed = create
        .run(
            json!({
                "op":"create", "request_id":request_id,
                "cwd":dir.path().to_string_lossy(), "objective":"changed objective",
                "start":"submit_objective", "model":"gpt-5.6-sol"
            }),
            native_context(&gc),
        )
        .await;
    let changed: Value = serde_json::from_str(changed.output()).unwrap();
    assert_eq!(changed["status"], "conflict", "{changed}");
    assert!(read_native_creation_status(&runtime, &gc, &request_id)
        .await
        .is_ok());
    assert!(
        native_creation_acceptance_receipt(&runtime, &gc, &request_id)
            .await
            .is_ok()
    );
    db.update_conversation_state(
        &gc,
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: "snapshot successor".into(),
        },
    )
    .await
    .unwrap();
    let crate::db::ContinueOutcome::Created(successor) =
        db.continue_conversation(&gc).await.unwrap()
    else {
        panic!("expected Coordinator successor");
    };
    assert!(read_native_creation_status(&runtime, &gc, &request_id)
        .await
        .is_err());
    assert!(
        native_creation_acceptance_receipt(&runtime, &gc, &request_id)
            .await
            .is_err()
    );
    assert!(
        read_native_creation_status(&runtime, &successor.id, &request_id)
            .await
            .is_ok()
    );
    sqlx::query("UPDATE conversations SET coordinator_head=0 WHERE id=?1")
        .bind(&successor.id)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        read_native_creation_status(&runtime, &successor.id, &request_id)
            .await
            .is_err()
    );
    assert!(
        native_creation_acceptance_receipt(&runtime, &successor.id, &request_id)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn native_registered_rename_is_guarded_replayable_and_revision_checked() {
    let (runtime, db, gc) = native_runtime().await;
    let root = db
        .create_conversation("native-root", "native-root", "/tmp", true, None, None)
        .await
        .unwrap();
    assert!(tools(runtime.clone(), &root.id).await.unwrap().is_empty());
    let registry = tools(runtime, &gc).await.unwrap();
    let rename = registry
        .iter()
        .find(|tool| tool.name() == "rename_conversation")
        .unwrap();
    let detail = rename
        .run(
            json!({"op":"detail", "reference":format!("@conv:{}", root.product_conversation_id)}),
            native_context(&gc),
        )
        .await;
    let detail: Value = serde_json::from_str(detail.output()).unwrap();
    assert_eq!(detail["status"], "detail");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut request = json!({"op":"rename", "operation_id":operation,
        "product_conversation_id":detail["detail"]["product_conversation_id"],
        "expected_root_transcript_id":detail["detail"]["root_transcript_id"],
        "expected_name_revision":detail["detail"]["name_revision"], "title":"  Native 東京  "});
    let applied = rename.run(request.clone(), native_context(&gc)).await;
    let applied: Value = serde_json::from_str(applied.output()).unwrap();
    assert_eq!(applied["status"], "applied", "{applied}");
    let replay = rename.run(request.clone(), native_context(&gc)).await;
    let replay: Value = serde_json::from_str(replay.output()).unwrap();
    assert_eq!(replay["status"], "replayed");
    assert_eq!(replay["receipt"], applied["receipt"]);
    request["operation_id"] = json!(uuid::Uuid::new_v4().to_string());
    let stale = rename.run(request.clone(), native_context(&gc)).await;
    assert_eq!(
        serde_json::from_str::<Value>(stale.output()).unwrap()["status"],
        "stale"
    );
    let denied = rename.run(request, native_context(&root.id)).await;
    assert!(matches!(denied, ToolOutput::Error { .. }));
    let renamed = db.get_conversation(&root.id).await.unwrap();
    assert_eq!(renamed.title.as_deref(), Some("Native 東京"));
    db.delete_conversation(&root.id).await.unwrap();
    let historical = rename
        .run(
            json!({"op":"read", "operation_id":operation}),
            native_context(&gc),
        )
        .await;
    let historical: Value = serde_json::from_str(historical.output()).unwrap();
    assert_eq!(historical["status"], "applied");
    assert_eq!(historical["historical"], true);
    assert_eq!(historical["receipt"], applied["receipt"]);
    assert!(historical.get("detail").is_none());
    sqlx::query("UPDATE conversations SET coordinator_head=0 WHERE id=?1")
        .bind(&gc)
        .execute(db.pool())
        .await
        .unwrap();
    let revoked = rename
        .run(
            json!({"op":"read", "operation_id":operation}),
            native_context(&gc),
        )
        .await;
    assert!(matches!(revoked, ToolOutput::Error { .. }));
}

#[tokio::test]
async fn native_registered_create_freezes_literal_intent_and_reconciles_same_request() {
    let (runtime, db, gc) = native_runtime().await;
    let registry = tools(runtime, &gc).await.unwrap();
    let create = registry
        .iter()
        .find(|tool| tool.name() == "create_conversation")
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path())
        .unwrap()
        .to_string_lossy()
        .to_string();
    let options = create
        .run(json!({"op":"options", "cwd":cwd}), native_context(&gc))
        .await;
    let options: Value = serde_json::from_str(options.output()).unwrap();
    assert_eq!(options["status"], "options", "{options}");
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut request = json!({"op":"create", "request_id":request_id,
        "cwd":options["cwd"], "objective":"/literal @not-a-user-file 東京", "start":"submit_objective", "model":"gpt-5.6-sol"});
    let accepted = create.run(request.clone(), native_context(&gc)).await;
    let accepted: Value = serde_json::from_str(accepted.output()).unwrap();
    assert_eq!(accepted["status"], "accepted", "{accepted}");
    let job = db
        .get_product_creation_job(&request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.intent.objective, "/literal @not-a-user-file 東京");
    assert!(matches!(
        job.intent.origin,
        crate::db::ProductCreationOrigin::InternalConversation { .. }
    ));
    let replay = create
        .run(
            request.clone(),
            native_context(&gc).with_tool_use_id("retry-call"),
        )
        .await;
    let replay: Value = serde_json::from_str(replay.output()).unwrap();
    assert_eq!(replay, accepted);
    request["objective"] = json!("different intent");
    let conflict = create.run(request, native_context(&gc)).await;
    assert!(matches!(conflict, ToolOutput::Error { .. }));
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM product_creation_jobs WHERE request_id=?1")
            .bind(&request_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(count, 1);
    let read = create
        .run(
            json!({"op":"read", "request_id":request_id}),
            native_context(&gc),
        )
        .await;
    assert!(
        !matches!(read, ToolOutput::Error { .. }),
        "{}",
        read.output()
    );
    assert_eq!(
        db.get_product_creation_job(&request_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "accepted"
    );
    let denied = create
        .run(
            json!({"op":"options"}),
            native_context("ordinary-or-missing"),
        )
        .await;
    assert!(matches!(denied, ToolOutput::Error { .. }));
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "Exercises registered tool authorization, publication, deletion and identity replay end to end"
)]
async fn native_registered_creation_read_replays_historical_publication_after_target_deletion() {
    use crate::db::ProductCreationProvisioningStage as Stage;
    use phoenix_core::domain::db_schema::{ConvMode, Conversation};
    use phoenix_core::domain::llm_types::ServiceTier;
    use phoenix_core::domain::sm_state::ConvState;
    use phoenix_core::work_scope::{AuthorityKind, EnvironmentContext, RuntimeRole, WorkScopeId};

    let (runtime, db, gc) = native_runtime().await;
    let registry = tools(runtime, &gc).await.unwrap();
    let create = registry
        .iter()
        .find(|tool| tool.name() == "create_conversation")
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let options = create
        .run(
            json!({"op":"options", "cwd":dir.path()}),
            native_context(&gc),
        )
        .await;
    let options: Value = serde_json::from_str(options.output()).unwrap();
    assert_eq!(options["status"], "options", "{options}");
    let request_id = uuid::Uuid::new_v4().to_string();
    let request = json!({"op":"create", "request_id":request_id,
        "cwd":options["cwd"], "objective":"historical native objective",
        "start":"submit_objective", "model":"gpt-5.6-sol"});
    let accepted = create.run(request.clone(), native_context(&gc)).await;
    let accepted: Value = serde_json::from_str(accepted.output()).unwrap();
    assert_eq!(accepted["status"], "accepted", "{accepted}");
    assert_eq!(accepted["request_id"], request_id);
    let job = db
        .get_product_creation_job(&request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        accepted["product_conversation_id"],
        json!(job.product_conversation_id)
    );
    assert_eq!(job.status, "accepted");
    let claimed = db
        .claim_product_creation(
            &request_id,
            "native-history-fixture",
            "native-history-claim",
            chrono::Utc::now(),
            chrono::Duration::minutes(5),
        )
        .await
        .unwrap()
        .unwrap();
    for stage in [
        Stage::ResolveRepository,
        Stage::ResolveStartingPin,
        Stage::MaterializeWorktree,
        Stage::FinalizeAttachments,
        Stage::CommitMetadata,
    ] {
        assert!(db
            .advance_product_creation_provisioning_stage(&request_id, &claimed.claim, stage)
            .await
            .unwrap());
    }
    let conversation_id = format!("native-history-{request_id}");
    let now = chrono::Utc::now();
    let scope = WorkScopeId::new();
    assert!(db
        .publish_product_creation_atomically(&crate::db::ProductCreationPublishInput {
            request_id: request_id.clone(),
            claim: claimed.claim,
            conversation: Conversation {
                id: conversation_id.clone(),
                product_conversation_id: job.product_conversation_id.clone(),
                slug: Some(conversation_id.clone()),
                title: Some("historical native objective".into()),
                cwd: job.intent.cwd.clone(),
                parent_conversation_id: None,
                user_initiated: true,
                state: ConvState::Idle,
                state_updated_at: now,
                created_at: now,
                updated_at: now,
                archived: false,
                model: job.intent.model.clone(),
                effort: job.intent.effort,
                service_tier: ServiceTier::Standard,
                project_id: None,
                conv_mode: ConvMode::Direct,
                runtime_role: RuntimeRole::User,
                attached_work_scope_id: Some(scope),
                desired_base_branch: None,
                message_count: 0,
                transcript_generation: 1,
                seed_parent_id: None,
                seed_label: None,
                continued_in_conv_id: None,
                llm_language: job.intent.llm_language,
                spawned_from_conversation_id: None,
            },
            authority_kind: AuthorityKind::Work,
            environment: EnvironmentContext::UnownedCwd {
                cwd: job.intent.cwd.clone(),
            },
            git_publication: None,
        })
        .await
        .unwrap());
    let read_request = json!({"op":"read", "request_id":request_id});
    let live = create.run(read_request.clone(), native_context(&gc)).await;
    let live: Value = serde_json::from_str(live.output()).unwrap();
    assert_eq!(live["publication"]["kind"], "available", "{live}");
    assert_eq!(live["publication"]["conversation_id"], conversation_id);
    assert!(live["publication"]["canonical_route"].as_str().is_some());

    db.delete_conversation(&conversation_id).await.unwrap();
    let historical = create.run(read_request.clone(), native_context(&gc)).await;
    let historical: Value = serde_json::from_str(historical.output()).unwrap();
    assert_eq!(historical["request_id"], request_id);
    assert_eq!(
        historical["product_conversation_id"],
        accepted["product_conversation_id"]
    );
    assert_eq!(
        historical["publication"]["kind"], "historical",
        "{historical}"
    );
    assert_eq!(
        historical["publication"]["conversation_id"],
        conversation_id
    );
    assert!(historical["publication"].get("canonical_route").is_none());
    assert_eq!(historical["initial_objective_execution"], "unavailable");
    let replay = create
        .run(
            request.clone(),
            native_context(&gc).with_tool_use_id("native-history-replay"),
        )
        .await;
    let replay: Value = serde_json::from_str(replay.output()).unwrap();
    assert_eq!(replay, accepted);
    let retained = db
        .get_product_creation_job(&request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retained.product_conversation_id,
        job.product_conversation_id
    );
    assert_eq!(retained.intent.origin, job.intent.origin);
    assert_eq!(
        retained.published_conversation_id.as_deref(),
        Some(conversation_id.as_str())
    );
    let job_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM product_creation_jobs WHERE request_id=?1")
            .bind(&request_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(job_count, 1);
    let mut changed = request.clone();
    changed["objective"] = json!("changed historical intent");
    let conflict = create.run(changed, native_context(&gc)).await;
    assert_eq!(
        serde_json::from_str::<Value>(conflict.output()).unwrap()["status"],
        "conflict"
    );
    sqlx::query("UPDATE conversations SET coordinator_head=0 WHERE id=?1")
        .bind(&gc)
        .execute(db.pool())
        .await
        .unwrap();
    let revoked = create.run(read_request, native_context(&gc)).await;
    assert!(matches!(revoked, ToolOutput::Error { .. }));
    let revoked_replay = create.run(request, native_context(&gc)).await;
    assert!(matches!(revoked_replay, ToolOutput::Error { .. }));
}

#[tokio::test]
async fn native_options_rejects_missing_and_file_cwd_and_returns_usable_directory() {
    let (runtime, db, gc) = native_runtime().await;
    let tool = CreateConversation(runtime.clone());
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("not-a-directory");
    std::fs::write(&file, "fixture").unwrap();
    for path in [temp.path().join("missing"), file] {
        let output = tool
            .run(json!({"op":"options", "cwd":path}), native_context(&gc))
            .await;
        assert!(
            matches!(output, ToolOutput::Error { .. }),
            "{}",
            output.output()
        );
    }
    let options = tool
        .run(
            json!({"op":"options", "cwd":temp.path()}),
            native_context(&gc),
        )
        .await;
    let options: Value = serde_json::from_str(options.output()).unwrap();
    let request_id = uuid::Uuid::new_v4().to_string();
    let accepted = tool
        .run(
            json!({"op":"create", "request_id":request_id,
        "cwd":options["cwd"], "objective":"offline acceptance", "start":"submit_objective",
        "model":"gpt-5.6-sol"}),
            native_context(&gc),
        )
        .await;
    assert!(
        !matches!(accepted, ToolOutput::Error { .. }),
        "{}",
        accepted.output()
    );
    let read = tool
        .run(
            json!({"op":"read", "request_id":request_id}),
            native_context(&gc),
        )
        .await;
    let read: Value = serde_json::from_str(read.output()).unwrap();
    assert_eq!(read["allowed_actions"], json!(["read"]));
    assert_eq!(read["initial_objective_execution"], "unavailable");
    assert!(read["execution_interpretation"]
        .as_str()
        .unwrap()
        .contains("not objective completed"));
    assert_eq!(
        db.get_product_creation_job(&request_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "accepted"
    );
}

#[test]
fn native_creation_publication_serialization_never_routes_historical_receipts() {
    let historical = serde_json::to_value(NativeCreationPublication::Historical {
        conversation_id: "original-transcript".into(),
    })
    .unwrap();
    assert_eq!(historical["kind"], "historical");
    assert_eq!(historical["conversation_id"], "original-transcript");
    assert!(historical.get("canonical_route").is_none());

    let available = serde_json::to_value(NativeCreationPublication::Available {
        conversation_id: "live-transcript".into(),
        canonical_route: "/c/live-product".into(),
    })
    .unwrap();
    assert_eq!(available["kind"], "available");
    assert_eq!(available["canonical_route"], "/c/live-product");
}

#[tokio::test]
async fn native_archived_detail_does_not_advertise_rename() {
    let (runtime, db, gc) = native_runtime().await;
    let root = db
        .create_conversation("archive-detail", "archive-detail", "/tmp", true, None, None)
        .await
        .unwrap();
    let tool = RenameConversation(runtime);
    let input = json!({"op":"detail", "reference":root.product_conversation_id});
    let open = tool.run(input.clone(), native_context(&gc)).await;
    let open: Value = serde_json::from_str(open.output()).unwrap();
    assert_eq!(open["allowed_actions"], json!(["detail", "read", "rename"]));
    sqlx::query("UPDATE product_conversations SET ordinary_lifecycle = 'history' WHERE id = ?1")
        .bind(root.product_conversation_id.as_str())
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE conversations SET archived = 1 WHERE id = ?1")
        .bind(&root.id)
        .execute(db.pool())
        .await
        .unwrap();
    let closed = tool.run(input, native_context(&gc)).await;
    let closed: Value = serde_json::from_str(closed.output()).unwrap();
    assert_eq!(closed["allowed_actions"], json!(["detail", "read"]));
}
