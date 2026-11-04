use super::*;
use crate::conversation_action_tools::tests::native_runtime;

fn context(transcript: &str, with_source: bool) -> ToolContext {
    struct Offline;
    impl phoenix_core::llm_service::LlmSelector for Offline {
        fn get(&self, _: &str) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
            None
        }
        fn default_service(&self) -> Option<Arc<dyn phoenix_core::llm_service::CompletionService>> {
            None
        }
    }
    let ctx = ToolContext::new_without_filesystem(
        tokio_util::sync::CancellationToken::new(),
        transcript.into(),
        Arc::new(phoenix_tools::BrowserSessionManager::default()),
        Arc::new(phoenix_tools::BashHandleRegistry::new()),
        Arc::new(Offline),
        phoenix_terminal::ActiveTerminals::new(),
        Arc::new(phoenix_tools::TmuxRegistry::new()),
    );
    if with_source {
        ctx.with_svg_assistant_message_id("authority-message")
            .with_tool_use_id("authority-call")
    } else {
        ctx
    }
}

#[test]
fn authority_requests_are_closed_and_size_is_byte_bounded() {
    assert!(serde_json::from_value::<ReadInput>(json!({})).is_ok());
    assert!(serde_json::from_value::<SetInput>(json!({"instructions":""})).is_ok());
    for invalid in [
        json!(null),
        json!({"instance":"local"}),
        json!({"instructions":"grant"}),
    ] {
        assert!(serde_json::from_value::<ReadInput>(invalid).is_err());
    }
    for invalid in [
        json!({}),
        json!({"instructions":null}),
        json!({"instructions":false}),
        json!({"instructions":"grant", "human_approved":true}),
        json!({"instructions":"grant", "source_transcript_id":"spoof"}),
        json!({"instructions":"grant", "target":"another"}),
        json!({"instructions":"grant", "instance":"local"}),
        json!({"instructions":"🐦".repeat(MAX_AUTHORITY_BYTES / 4 + 1)}),
    ] {
        assert!(serde_json::from_value::<SetInput>(invalid).is_err());
    }
}

#[tokio::test]
async fn authority_registered_only_on_coordinator_and_tools_round_trip() {
    let (runtime, db, source) = native_runtime().await;
    let registry = runtime
        .coordinator_tool_registry(&source, None)
        .await
        .unwrap();
    let definitions = registry.definitions();
    for name in ["read_authority", "set_authority"] {
        assert!(definitions.iter().any(|tool| tool.name == name));
        for other in [
            phoenix_tools::ToolRegistry::direct(Vec::new()),
            phoenix_tools::ToolRegistry::for_subagent_work(),
            phoenix_tools::ToolRegistry::for_subagent_explore_no_sandbox(),
        ] {
            assert!(!other.definitions().iter().any(|tool| tool.name == name));
        }
    }
    let read = AuthorityTool {
        runtime: runtime.clone(),
        mode: Mode::Read,
    };
    let set = AuthorityTool {
        runtime,
        mode: Mode::Set,
    };
    assert_eq!(read.input_schema()["additionalProperties"], false);
    assert_eq!(set.input_schema()["required"], json!(["instructions"]));
    assert_eq!(
        read.run_inner(json!({}), context(&source, true))
            .await
            .unwrap(),
        json!({"instructions":""})
    );
    for text in ["  Preserve source.\nAllow unused caches. 🐦\n", ""] {
        assert_eq!(
            set.run_inner(json!({"instructions":text}), context(&source, true))
                .await
                .unwrap(),
            json!({"instructions":text})
        );
        assert_eq!(db.coordinator_authority().await.unwrap().as_str(), text);
    }
    assert!(db.get_messages(&source).await.unwrap().is_empty());
}

#[tokio::test]
async fn authority_rejects_missing_source_identity_ineligible_sources_and_malformed_set() {
    let (runtime, db, source) = native_runtime().await;
    let read = AuthorityTool {
        runtime: runtime.clone(),
        mode: Mode::Read,
    };
    let set = AuthorityTool {
        runtime,
        mode: Mode::Set,
    };
    for tool in [&read, &set] {
        let input = if matches!(tool.mode, Mode::Set) {
            json!({"instructions":"grant"})
        } else {
            json!({})
        };
        assert!(tool
            .run_inner(input.clone(), context(&source, false))
            .await
            .is_err());
        assert!(tool
            .run_inner(input, context("not-current-coordinator", true))
            .await
            .is_err());
        assert!(tool
            .run_inner(json!([]), context(&source, true))
            .await
            .is_err());
        assert!(tool
            .run_inner(json!(["grant"]), context(&source, true))
            .await
            .is_err());
    }
    set.run_inner(json!({"instructions":"retain"}), context(&source, true))
        .await
        .unwrap();
    assert!(set
        .run_inner(
            json!({"instructions":"a".repeat(MAX_AUTHORITY_BYTES + 1)}),
            context(&source, true)
        )
        .await
        .is_err());
    assert_eq!(db.coordinator_authority().await.unwrap().as_str(), "retain");
}

#[tokio::test]
async fn authority_retained_tools_refuse_predecessor_after_coordinator_rotation() {
    let (runtime, db, source) = native_runtime().await;
    let read = AuthorityTool {
        runtime: runtime.clone(),
        mode: Mode::Read,
    };
    let set = AuthorityTool {
        runtime,
        mode: Mode::Set,
    };
    set.run_inner(json!({"instructions":"retained"}), context(&source, true))
        .await
        .unwrap();
    db.update_conversation_state(
        &source,
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: "rotate".into(),
        },
    )
    .await
    .unwrap();
    let successor = match db.continue_conversation(&source).await.unwrap() {
        crate::db::ContinueOutcome::Created(successor) => successor,
        other @ (crate::db::ContinueOutcome::AlreadyContinued(_)
        | crate::db::ContinueOutcome::ParentNotContextExhausted { .. }) => {
            panic!("expected successor: {other:?}")
        }
    };
    assert!(read
        .run_inner(json!({}), context(&source, true))
        .await
        .is_err());
    assert!(set
        .run_inner(json!({"instructions":""}), context(&source, true))
        .await
        .is_err());
    assert_eq!(
        read.run_inner(json!({}), context(&successor.id, true))
            .await
            .unwrap(),
        json!({"instructions":"retained"})
    );
}

#[tokio::test]
async fn authority_write_outlives_requester_cancellation() {
    let (runtime, db, source) = native_runtime().await;
    let product = db
        .get_conversation(&source)
        .await
        .unwrap()
        .product_conversation_id;
    let gate = runtime
        .lock_product_message_admission(product.as_str())
        .await;
    let tool = AuthorityTool {
        runtime,
        mode: Mode::Set,
    };
    let (at_gate, waiting) = tokio::sync::oneshot::channel();
    let (completed, finished) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        tool.run_inner_observed(
            json!({"instructions":"retained after cancellation"}),
            context(&source, true),
            Some(BoundaryObservation {
                source_gate_wait: at_gate,
                worker_completed: completed,
            }),
        )
        .await
    });
    waiting.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    drop(gate);
    finished.await.unwrap();
    assert_eq!(
        db.coordinator_authority().await.unwrap().as_str(),
        "retained after cancellation"
    );
}

#[tokio::test]
async fn authority_unclassified_write_closes_admission_before_owner_release() {
    let (runtime, _, _) = native_runtime().await;
    let owner = runtime.acquire_local_authority_pass().unwrap();
    let result = AuthorityWritePass { owner: Some(owner) }
        .classify(LocalAuthorityResult::<()>::DurableFactUnclassified);
    assert!(result.is_err());
    assert!(runtime.local_authority_is_closed());
    assert!(runtime.acquire_local_authority_pass().is_err());
}
