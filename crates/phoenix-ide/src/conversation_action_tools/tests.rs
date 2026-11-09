use super::*;

fn create_input(request: Value) -> Value {
    Value::Object(serde_json::Map::from_iter([("request".into(), request)]))
}

fn submit() -> Value {
    json!({
        "op": "create", "request_id": "01b22123-04de-4b01-88dd-83e74eb2ed51",
        "cwd": "/tmp/project", "objective": "/skill this stays literal",
        "start": "submit_objective"
    })
}

#[tokio::test]
async fn create_schema_closes_the_envelope_and_each_operation_branch() {
    let (runtime, _, gc) = native_runtime().await;
    let registry = tools(runtime, &gc).await.unwrap();
    let schema = registry
        .iter()
        .find(|tool| tool.name() == "create_conversation")
        .unwrap()
        .input_schema();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"], json!(["request"]));
    let branches = schema["properties"]["request"]["oneOf"].as_array().unwrap();
    assert_eq!(branches.len(), 3);
    for branch in branches {
        assert_eq!(branch["additionalProperties"], false);
        assert_eq!(branch["type"], "object");
    }
    assert_eq!(branches[0]["properties"]["op"]["const"], "options");
    assert_eq!(branches[1]["properties"]["op"]["const"], "read");
    assert_eq!(branches[2]["properties"]["op"]["const"], "create");
    assert_eq!(
        branches[2]["properties"]["start"]["const"],
        "submit_objective"
    );
}

#[tokio::test]
async fn malformed_create_calls_are_non_durable_and_errors_are_operation_guided() {
    let (runtime, db, gc) = native_runtime().await;
    let tool = CreateConversation(runtime);
    let request_id = uuid::Uuid::new_v4().to_string();
    let malformed = [
        json!({"op":"options", "request_id":request_id}),
        json!({"op":"read", "cwd":"/secret/path"}),
        json!({"op":"create", "request_id":request_id, "cwd":"/secret/path", "objective":"private objective", "start":"submit_objective", "reference":"wrong operation"}),
        json!({"op":"options", "cwd":null}),
        json!({"op":"create", "request_id":request_id, "cwd":"/secret/path", "objective":"private objective", "start":"submit_objective", "model":null}),
        json!({"op":"create", "request_id":request_id, "cwd":"/secret/path", "objective":"private objective", "start":"submit_objective", "effort":null}),
        json!({"op":"create", "request_id":request_id, "cwd":"/secret/path", "objective":"private objective", "start":"submit_objective", "llm_language":null}),
    ];
    let expected = [
        "options", "read", "create", "options", "create", "create", "create",
    ];
    for (request, operation) in malformed.into_iter().zip(expected) {
        let output = tool.run(create_input(request), native_context(&gc)).await;
        let error: Value = serde_json::from_str(output.output()).unwrap();
        assert_eq!(error["status"], "error");
        let message = error["error"].as_str().unwrap();
        assert!(message.contains(operation), "{message}");
        assert!(!message.contains("/secret/path"), "{message}");
        assert!(!message.contains("private objective"), "{message}");
    }
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM product_creation_jobs WHERE request_id=?1")
            .bind(&request_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn create_registration_is_single_tool() {
    let (runtime, _, gc) = native_runtime().await;
    let registry = tools(runtime, &gc).await.unwrap();
    assert_eq!(
        registry
            .iter()
            .filter(|tool| tool.name() == "create_conversation")
            .count(),
        1
    );
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

pub(super) fn native_context(transcript: &str) -> ToolContext {
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
        phoenix_core::work_scope::ResourceScopeKey::Coordinator,
        phoenix_core::work_scope::ResourceAuthority::Work,
    )
    .with_svg_assistant_message_id("native-message")
    .with_tool_use_id("native-call")
}

pub(crate) async fn native_runtime() -> (Arc<RuntimeManager>, crate::db::Database, String) {
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

pub(super) fn native_context_without_source_identity(transcript: &str) -> ToolContext {
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
        phoenix_core::work_scope::ResourceScopeKey::Coordinator,
        phoenix_core::work_scope::ResourceAuthority::Work,
    )
}

fn native_continue_input(product: &ProductConversationId, predecessor: &str) -> Value {
    json!({
        "product_conversation_id": product,
        "predecessor_transcript_id": predecessor,
        "summary_message_id": "native-summary"
    })
}

async fn native_continue_predecessor(
    db: &crate::db::Database,
    id: &str,
    summary: &str,
) -> ProductConversationId {
    use phoenix_core::domain::db_schema::{InputOrigin, MessageContent};
    db.update_conversation_state(
        id,
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: summary.into(),
        },
    )
    .await
    .unwrap();
    db.add_message_with_seq_and_origin(
        "native-summary",
        id,
        1,
        &MessageContent::continuation(summary),
        None,
        None,
        &InputOrigin::SystemGenerated,
    )
    .await
    .unwrap();
    db.get_conversation(id)
        .await
        .unwrap()
        .product_conversation_id
}

fn assert_immutable_creation_conflict(changed: &ToolOutput) {
    assert!(matches!(changed, ToolOutput::Error { .. }));
    assert_eq!(
        serde_json::from_str::<Value>(changed.output()).unwrap(),
        json!({
            "status": "error",
            "error": "request_id conflicts with immutable creation intent",
        })
    );
}

#[tokio::test]
async fn native_creation_read_and_submit_receipt_require_current_snapshot_authority() {
    let (runtime, db, gc) = native_runtime().await;
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let request_id = uuid::Uuid::new_v4().to_string();
    let registry = tools(runtime.clone(), &gc).await.unwrap();
    let create = registry
        .iter()
        .find(|tool| tool.name() == "create_conversation")
        .unwrap();
    let accepted = create
        .run(
            create_input(json!({
                "op":"create", "request_id":request_id,
                "cwd":cwd.to_string_lossy(), "objective":"snapshot objective",
                "start":"submit_objective", "model":"gpt-5.6-sol"
            })),
            native_context(&gc),
        )
        .await;
    assert_eq!(
        serde_json::from_str::<Value>(accepted.output()).unwrap()["status"],
        "accepted"
    );
    let replay = create
        .run(
            create_input(json!({
                "op":"create", "request_id":request_id,
                "cwd":cwd.to_string_lossy(), "objective":"snapshot objective",
                "start":"submit_objective", "model":"gpt-5.6-sol"
            })),
            native_context(&gc),
        )
        .await;
    let replay: Value = serde_json::from_str(replay.output()).unwrap();
    assert_eq!(replay["status"], "accepted");
    assert_eq!(replay["request_id"], request_id);
    let changed = create
        .run(
            create_input(json!({
                "op":"create", "request_id":request_id,
                "cwd":cwd.to_string_lossy(), "objective":"changed objective",
                "start":"submit_objective", "model":"gpt-5.6-sol"
            })),
            native_context(&gc),
        )
        .await;
    assert_immutable_creation_conflict(&changed);
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
async fn rename_unclassified_result_closes_fence_before_releasing_owner() {
    let (runtime, _, _) = native_runtime().await;
    let authority = RenameAuthorityPass::new(runtime.acquire_local_authority_pass().unwrap());
    let result =
        authority.complete(phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified);
    assert!(result.is_err());
    assert!(runtime.local_authority_is_closed());
    assert_eq!(runtime.fatal_authority_owners_at_first_close(), Some(1));
    assert!(runtime.acquire_local_authority_pass().is_err());
}

#[tokio::test]
async fn rename_established_rejection_and_stale_result_keep_admission_open() {
    let (runtime, _, _) = native_runtime().await;
    let authority = RenameAuthorityPass::new(runtime.acquire_local_authority_pass().unwrap());
    assert!(authority
        .complete(
            phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Err(
                crate::db::DbError::CurrentCoordinatorRequired
            ),)
        )
        .is_err());
    assert!(!runtime.local_authority_is_closed());
    let authority = RenameAuthorityPass::new(runtime.acquire_local_authority_pass().unwrap());
    assert_eq!(
        authority
            .complete(
                phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(Ok(
                    ProductConversationRenameResult::Stale
                ),)
            )
            .unwrap(),
        ProductConversationRenameResult::Stale
    );
    assert!(!runtime.local_authority_is_closed());
    assert_eq!(runtime.fatal_authority_owners_at_first_close(), None);
}

#[tokio::test]
async fn cancelled_rename_authority_closes_fence_before_releasing_owner() {
    let (runtime, _, _) = native_runtime().await;
    let owner = runtime.acquire_local_authority_pass().unwrap();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let authority = RenameAuthorityPass::new(owner);
        entered_tx.send(()).unwrap();
        std::future::pending::<()>().await;
        authority.complete(phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified)
    });
    entered_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(runtime.local_authority_is_closed());
    assert_eq!(runtime.fatal_authority_owners_at_first_close(), Some(1));
}

#[tokio::test]
async fn panicked_rename_authority_closes_fence_before_releasing_owner() {
    let (runtime, _, _) = native_runtime().await;
    let owner = runtime.acquire_local_authority_pass().unwrap();
    let task = tokio::spawn(async move {
        let _authority = RenameAuthorityPass::new(owner);
        panic!("injected rename boundary panic");
    });
    assert!(task.await.unwrap_err().is_panic());
    assert!(runtime.local_authority_is_closed());
    assert_eq!(runtime.fatal_authority_owners_at_first_close(), Some(1));
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
        .run(
            create_input(json!({"op":"options", "cwd":cwd})),
            native_context(&gc),
        )
        .await;
    let options: Value = serde_json::from_str(options.output()).unwrap();
    assert_eq!(options["status"], "options", "{options}");
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut request = json!({"op":"create", "request_id":request_id,
        "cwd":options["cwd"], "objective":"/literal @not-a-user-file 東京", "start":"submit_objective", "model":"gpt-5.6-sol"});
    let accepted = create
        .run(create_input(request.clone()), native_context(&gc))
        .await;
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
            create_input(request.clone()),
            native_context(&gc).with_tool_use_id("retry-call"),
        )
        .await;
    let replay: Value = serde_json::from_str(replay.output()).unwrap();
    assert_eq!(replay, accepted);
    request["objective"] = json!("different intent");
    let conflict = create.run(create_input(request), native_context(&gc)).await;
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
            create_input(json!({"op":"read", "request_id":request_id})),
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
            create_input(json!({"op":"options"})),
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
            create_input(json!({"op":"options", "cwd":dir.path()})),
            native_context(&gc),
        )
        .await;
    let options: Value = serde_json::from_str(options.output()).unwrap();
    assert_eq!(options["status"], "options", "{options}");
    let request_id = uuid::Uuid::new_v4().to_string();
    let request = json!({"op":"create", "request_id":request_id,
        "cwd":options["cwd"], "objective":"historical native objective",
        "start":"submit_objective", "model":"gpt-5.6-sol"});
    let accepted = create
        .run(create_input(request.clone()), native_context(&gc))
        .await;
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
            claim: claimed.claim.clone(),
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
    let staged = create
        .run(create_input(read_request.clone()), native_context(&gc))
        .await;
    let staged: Value = serde_json::from_str(staged.output()).unwrap();
    assert_eq!(staged["status"], "delivery_pending", "{staged}");
    assert!(staged["publication"].is_null());
    let delivery = crate::db::ClaimedProductCreationJob {
        claim: claimed.claim,
        job: db
            .get_product_creation_job(&request_id)
            .await
            .unwrap()
            .unwrap(),
    };
    let fingerprint = format!("product-create:{request_id}");
    let entry = phoenix_core::domain::sm_event::SteerEntry {
        text: delivery.job.intent.objective.clone(),
        origin: delivery.job.intent.origin.input_origin(),
        message_id: request_id.clone(),
        llm_text: None,
        images: Vec::new(),
        files: Vec::new(),
        user_agent: None,
        skill_invocation: None,
    };
    db.append_product_creation_steering_entry(&delivery, &entry, &fingerprint)
        .await
        .unwrap();
    assert!(db
        .complete_product_creation_delivery(
            &request_id,
            &delivery.claim,
            &crate::db::SteeringAcceptanceFingerprint::Exact(fingerprint),
            &delivery.job.intent.live_intent().unwrap(),
        )
        .await
        .unwrap());
    let content = phoenix_core::domain::db_schema::MessageContent::User(
        phoenix_core::domain::db_schema::UserContent::new(delivery.job.intent.objective.clone()),
    );
    db.commit_steering_drain(
        &conversation_id,
        &[phoenix_core::domain::db_schema::Message {
            message_id: request_id.clone(),
            conversation_id: conversation_id.clone(),
            sequence_id: 1,
            origin: delivery.job.intent.origin.input_origin(),
            message_type: content.message_type(),
            content,
            display_data: None,
            usage_data: None,
            created_at: chrono::Utc::now(),
        }],
        &phoenix_core::domain::sm_state::ConvState::LlmRequesting { attempt: 0 },
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    db.update_conversation_state(
        &conversation_id,
        &phoenix_core::domain::sm_state::ConvState::Error {
            message: "native initial objective failure".into(),
            error_kind: phoenix_core::domain::db_schema::ErrorKind::InvalidRequest,
            resets_at: None,
        },
    )
    .await
    .unwrap();
    let failed = create
        .run(create_input(read_request.clone()), native_context(&gc))
        .await;
    let failed: Value = serde_json::from_str(failed.output()).unwrap();
    assert_eq!(failed["status"], "published", "{failed}");
    assert_eq!(failed["error"], Value::Null, "{failed}");
    assert_eq!(
        failed["initial_objective_execution"],
        json!({"status":"failed", "reason":"native initial objective failure"}),
        "{failed}"
    );
    assert_eq!(failed["publication"]["kind"], "available", "{failed}");
    assert_eq!(failed["publication"]["conversation_id"], conversation_id);
    assert!(failed["publication"]["canonical_route"].as_str().is_some());

    db.delete_conversation(&conversation_id).await.unwrap();
    let historical = create
        .run(create_input(read_request.clone()), native_context(&gc))
        .await;
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
    assert_eq!(
        historical["initial_objective_execution"],
        json!({"status":"failed", "reason":"native initial objective failure"})
    );
    assert_eq!(historical["error"], Value::Null, "{historical}");
    let replay = create
        .run(
            create_input(request.clone()),
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
    let conflict = create.run(create_input(changed), native_context(&gc)).await;
    assert_immutable_creation_conflict(&conflict);
    sqlx::query("UPDATE conversations SET coordinator_head=0 WHERE id=?1")
        .bind(&gc)
        .execute(db.pool())
        .await
        .unwrap();
    let revoked = create
        .run(create_input(read_request), native_context(&gc))
        .await;
    assert!(matches!(revoked, ToolOutput::Error { .. }));
    let revoked_replay = create.run(create_input(request), native_context(&gc)).await;
    assert!(matches!(revoked_replay, ToolOutput::Error { .. }));
}

#[tokio::test]
async fn native_continue_requires_tool_call_identity_and_refuses_non_coordinator_sources() {
    let (runtime, db, gc) = native_runtime().await;
    let predecessor = db
        .create_conversation(
            "native-continue",
            "native-continue",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let product = native_continue_predecessor(&db, &predecessor.id, "retained summary").await;
    let tool = continuation::ContinueConversation(runtime.clone());
    let input = native_continue_input(&product, &predecessor.id);
    let missing_identity = tool
        .run(input.clone(), native_context_without_source_identity(&gc))
        .await;
    assert!(matches!(missing_identity, ToolOutput::Error { .. }));
    assert!(db
        .get_conversation(&predecessor.id)
        .await
        .unwrap()
        .continued_in_conv_id
        .is_none());

    let ordinary = db
        .create_conversation(
            "ordinary-source",
            "ordinary-source",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let refused = tool.run(input, native_context(&ordinary.id)).await;
    assert!(matches!(refused, ToolOutput::Error { .. }));
    assert!(db
        .get_conversation(&predecessor.id)
        .await
        .unwrap()
        .continued_in_conv_id
        .is_none());
    assert!(!runtime.local_authority_is_closed());
}

#[tokio::test]
async fn native_continue_adapter_is_a_pending_sink_after_local_authority_closes() {
    use futures::FutureExt;

    let (runtime, _, gc) = native_runtime().await;
    runtime.fence_fatal_local_authority().await;
    let tool = continuation::ContinueConversation(runtime);
    let future = tool.run(json!({"unexpected":"input"}), native_context(&gc));
    assert!(future.now_or_never().is_none());
}

#[tokio::test]
async fn native_continue_replays_completed_opening_without_replacing_its_message_identity() {
    use phoenix_core::domain::db_schema::{InputOrigin, MessageContent};
    use phoenix_db::{CoordinatorContinuationOpening, CurrentCoordinatorContinuationRequest};
    use phoenix_workflow::ClientTurnKey;

    let (runtime, db, gc) = native_runtime().await;
    let predecessor = db
        .create_conversation("native-replay", "native-replay", "/tmp", true, None, None)
        .await
        .unwrap();
    let product = native_continue_predecessor(&db, &predecessor.id, "retained summary").await;
    let opening_key = ClientTurnKey::try_from("already-opened-message").unwrap();
    let (outcome, opening) = db
        .continue_from_current_coordinator(CurrentCoordinatorContinuationRequest {
            source_transcript_id: &gc,
            product_conversation_id: &product,
            predecessor_transcript_id: &predecessor.id,
            summary_message_id: "native-summary",
            first_message_id: opening_key,
        })
        .await
        .established()
        .expect("reservation classified")
        .unwrap();
    let phoenix_db::ContinueOutcome::Created(successor) = outcome else {
        panic!("expected first continuation reservation");
    };
    let CoordinatorContinuationOpening::Pending(intent) = opening else {
        panic!("expected a new opening reservation");
    };
    assert_eq!(intent.message_id.as_str(), "already-opened-message");
    db.add_message_with_seq_and_origin(
        "already-opened-message",
        &successor.id,
        1,
        &MessageContent::continuation("retained summary"),
        None,
        None,
        &InputOrigin::SystemGenerated,
    )
    .await
    .unwrap();
    assert!(db
        .continuation_dispatch_intent(&predecessor.id)
        .await
        .unwrap()
        .is_none());

    let tool = continuation::ContinueConversation(runtime.clone());
    let replay = tool
        .run(
            native_continue_input(&product, &predecessor.id),
            native_context(&gc),
        )
        .await;
    assert!(
        !matches!(replay, ToolOutput::Error { .. }),
        "{}",
        replay.output()
    );
    let replay: Value = serde_json::from_str(replay.output()).unwrap();
    assert_eq!(replay["conversation_id"], successor.id);
    assert_eq!(replay["status"], "already_exists");
    let messages = db.get_messages(&successor.id).await.unwrap();
    let openings: Vec<_> = messages
        .iter()
        .filter(|message| {
            matches!(
                message.content,
                phoenix_core::domain::db_schema::MessageContent::Continuation { .. }
            )
        })
        .collect();
    assert_eq!(openings.len(), 1);
    assert_eq!(openings[0].message_id, "already-opened-message");
    assert!(matches!(openings[0].origin, InputOrigin::SystemGenerated));

    db.update_conversation_state(
        &gc,
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: "Coordinator rotation".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        db.continue_conversation(&gc).await.unwrap(),
        phoenix_db::ContinueOutcome::Created(_)
    ));
    let stale_replay = tool
        .run(
            native_continue_input(&product, &predecessor.id),
            native_context(&gc),
        )
        .await;
    assert!(matches!(stale_replay, ToolOutput::Error { .. }));
    assert_eq!(db.get_messages(&successor.id).await.unwrap().len(), 1);
    assert!(!runtime.local_authority_is_closed());
}

async fn insert_project_option(db: &crate::db::Database, id: &str, path: &std::path::Path) {
    sqlx::query("INSERT INTO projects (id, canonical_path, created_at) VALUES (?1, ?2, ?3)")
        .bind(id)
        .bind(path.to_str().unwrap())
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn native_options_skips_stale_prefix_and_canonicalizes_usable_projects() {
    let (runtime, db, gc) = native_runtime().await;
    let tool = CreateConversation(runtime);
    let temp = tempfile::tempdir().unwrap();
    for index in 0..70 {
        insert_project_option(
            &db,
            &format!("a-{index:03}"),
            &temp.path().join(format!("missing-{index}")),
        )
        .await;
    }
    let file = temp.path().join("file");
    std::fs::write(&file, "fixture").unwrap();
    insert_project_option(&db, "b-file", &file).await;
    insert_project_option(&db, "b-root", std::path::Path::new("/")).await;
    insert_project_option(&db, "z-valid", &temp.path().join(".")).await;
    let output = tool
        .run(create_input(json!({"op":"options"})), native_context(&gc))
        .await;
    let options: Value = serde_json::from_str(output.output()).unwrap();
    assert_eq!(options["projects_truncated"], false);
    assert_eq!(
        options["projects"],
        json!([{"project_id":"z-valid", "cwd":temp.path().canonicalize().unwrap()}])
    );
    let accepted = tool.run(create_input(json!({"op":"create", "request_id":uuid::Uuid::new_v4().to_string(),
        "cwd":options["projects"][0]["cwd"], "objective":"/literal @original", "start":"submit_objective", "model":"gpt-5.6-sol"})), native_context(&gc)).await;
    assert!(
        !matches!(accepted, ToolOutput::Error { .. }),
        "{}",
        accepted.output()
    );
}

#[tokio::test]
async fn native_options_truncates_only_after_fifty_usable_projects() {
    let (runtime, db, gc) = native_runtime().await;
    let tool = CreateConversation(runtime);
    let temp = tempfile::tempdir().unwrap();
    for index in 0..70 {
        insert_project_option(
            &db,
            &format!("a-{index:03}"),
            &temp.path().join(format!("missing-{index}")),
        )
        .await;
    }
    for index in 0..50 {
        let path = temp.path().join(format!("valid-{index:03}"));
        std::fs::create_dir(&path).unwrap();
        insert_project_option(&db, &format!("z-{index:03}"), &path).await;
    }
    let output = tool
        .run(create_input(json!({"op":"options"})), native_context(&gc))
        .await;
    let options: Value = serde_json::from_str(output.output()).unwrap();
    assert_eq!(options["projects"].as_array().unwrap().len(), 50);
    assert_eq!(options["projects_truncated"], false);
    let path = temp.path().join("last-valid");
    std::fs::create_dir(&path).unwrap();
    insert_project_option(&db, "z-050", &path).await;
    let output = tool
        .run(create_input(json!({"op":"options"})), native_context(&gc))
        .await;
    let options: Value = serde_json::from_str(output.output()).unwrap();
    assert_eq!(options["projects"].as_array().unwrap().len(), 50);
    assert_eq!(options["projects_truncated"], true);
}

#[tokio::test]
async fn native_rename_rechecks_archive_but_retains_authorized_replay() {
    let (runtime, db, gc) = native_runtime().await;
    let root = db
        .create_conversation("archive-native", "archive-native", "/tmp", true, None, None)
        .await
        .unwrap();
    let tool = RenameConversation(runtime);
    let detail = tool
        .run(
            json!({"op":"detail", "reference":root.product_conversation_id}),
            native_context(&gc),
        )
        .await;
    let detail: Value = serde_json::from_str(detail.output()).unwrap();
    let mut request = json!({"op":"rename", "operation_id":uuid::Uuid::new_v4().to_string(),
        "product_conversation_id":detail["detail"]["product_conversation_id"], "expected_root_transcript_id":detail["detail"]["root_transcript_id"],
        "expected_name_revision":detail["detail"]["name_revision"], "title":"Archive fenced"});
    let applied = tool.run(request.clone(), native_context(&gc)).await;
    let applied: Value = serde_json::from_str(applied.output()).unwrap();
    assert_eq!(applied["status"], "applied");
    sqlx::query("UPDATE conversations SET archived=1 WHERE id=?1")
        .bind(&root.id)
        .execute(db.pool())
        .await
        .unwrap();
    let replay = tool.run(request.clone(), native_context(&gc)).await;
    let replay: Value = serde_json::from_str(replay.output()).unwrap();
    assert_eq!(replay["status"], "replayed");
    assert_eq!(replay["receipt"], applied["receipt"]);
    request["operation_id"] = json!(uuid::Uuid::new_v4().to_string());
    request["expected_name_revision"] = json!(1);
    let refused = tool.run(request.clone(), native_context(&gc)).await;
    let refused: Value = serde_json::from_str(refused.output()).unwrap();
    assert_eq!(refused["status"], "unavailable");
    assert!(db
        .read_product_conversation_rename_receipt(&gc, request["operation_id"].as_str().unwrap())
        .await
        .unwrap()
        .is_none());
    sqlx::query("UPDATE conversations SET coordinator_head=0 WHERE id=?1")
        .bind(&gc)
        .execute(db.pool())
        .await
        .unwrap();
    request["operation_id"] = applied["receipt"]["operation_id"].clone();
    request["expected_name_revision"] = json!(0);
    let revoked = tool.run(request, native_context(&gc)).await;
    assert!(matches!(revoked, ToolOutput::Error { .. }));
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
            .run(
                create_input(json!({"op":"options", "cwd":path})),
                native_context(&gc),
            )
            .await;
        assert!(
            matches!(output, ToolOutput::Error { .. }),
            "{}",
            output.output()
        );
    }
    let options = tool
        .run(
            create_input(json!({"op":"options", "cwd":temp.path()})),
            native_context(&gc),
        )
        .await;
    let options: Value = serde_json::from_str(options.output()).unwrap();
    let request_id = uuid::Uuid::new_v4().to_string();
    let accepted = tool
        .run(
            create_input(json!({"op":"create", "request_id":request_id,
        "cwd":options["cwd"], "objective":"offline acceptance", "start":"submit_objective",
        "model":"gpt-5.6-sol"})),
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
            create_input(json!({"op":"read", "request_id":request_id})),
            native_context(&gc),
        )
        .await;
    let read: Value = serde_json::from_str(read.output()).unwrap();
    assert_eq!(read["allowed_actions"], json!(["read"]));
    assert_eq!(
        read["initial_objective_execution"],
        json!({"status":"unavailable"})
    );
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
