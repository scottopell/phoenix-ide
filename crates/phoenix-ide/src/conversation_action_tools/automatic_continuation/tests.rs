use super::*;
use crate::conversation_action_tools::tests::{
    native_context, native_context_without_source_identity, native_runtime,
};
use phoenix_core::domain::product_conversation::AutoContinueOnContextExhaustion;

#[test]
fn settings_parsers_are_closed_and_boolean_is_required_for_set_only() {
    let ordinary = json!({"kind":"ordinary", "product_conversation_id":"product-id"});
    let coordinator = json!({"kind":"coordinator"});
    assert!(serde_json::from_value::<ReadRequest>(json!({"target":ordinary})).is_ok());
    assert!(serde_json::from_value::<ReadRequest>(json!({"target":coordinator})).is_ok());
    for enabled in [false, true] {
        assert!(serde_json::from_value::<SetRequest>(json!({
            "target":{"kind":"coordinator"},
            "auto_continue_on_context_exhaustion":enabled
        }))
        .is_ok());
    }
    for input in [
        json!({"target":{"kind":"coordinator"}}),
        json!({"target":{"kind":"coordinator"}, "auto_continue_on_context_exhaustion":"false"}),
        json!({"target":{"kind":"coordinator"}, "auto_continue_on_context_exhaustion":0}),
        json!({"target":{"kind":"coordinator"}, "auto_continue_on_context_exhaustion":null}),
        json!({"target":{"kind":"unknown"}, "auto_continue_on_context_exhaustion":false}),
        json!({"product_conversation_id":"legacy", "auto_continue_on_context_exhaustion":false}),
        json!({"target":{"kind":"ordinary", "product_conversation_id":"  "}, "auto_continue_on_context_exhaustion":true}),
        json!({"target":{"kind":"ordinary"}, "auto_continue_on_context_exhaustion":true}),
        json!({"target":{"kind":"coordinator", "product_conversation_id":"spoof"}, "auto_continue_on_context_exhaustion":true}),
        json!({"target":{"kind":"coordinator"}, "auto_continue_on_context_exhaustion":true, "source_transcript_id":"spoof"}),
        json!({"target":{"kind":"coordinator"}, "auto_continue_on_context_exhaustion":true, "retry":true}),
    ] {
        assert!(
            serde_json::from_value::<SetRequest>(input.clone()).is_err(),
            "{input}"
        );
    }
    assert!(serde_json::from_value::<ReadRequest>(json!({
        "target":{"kind":"coordinator"}, "auto_continue_on_context_exhaustion":false
    }))
    .is_err());
}

#[tokio::test]
async fn settings_schema_and_real_preference_round_trip() {
    let (runtime, db, source) = native_runtime().await;
    let read = AutomaticContinuationTool::read(runtime.clone());
    let set = AutomaticContinuationTool::set(runtime);
    assert_eq!(read.name(), "read_automatic_continuation");
    assert_eq!(set.name(), "set_automatic_continuation");
    assert_eq!(read.input_schema()["additionalProperties"], false);
    assert_eq!(read.input_schema()["required"], json!(["target"]));
    assert_eq!(
        set.input_schema()["required"],
        json!(["target", "auto_continue_on_context_exhaustion"])
    );
    assert_eq!(
        set.input_schema()["properties"]["auto_continue_on_context_exhaustion"]["type"],
        "boolean"
    );
    let ordinary = db
        .create_conversation(
            "native-settings",
            "native-settings",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    for (target, id, kind) in [
        (
            json!({"kind":"ordinary", "product_conversation_id":ordinary.product_conversation_id}),
            ordinary.product_conversation_id.clone(),
            "ordinary",
        ),
        (
            json!({"kind":"coordinator"}),
            db.get_conversation(&source)
                .await
                .unwrap()
                .product_conversation_id,
            "coordinator",
        ),
    ] {
        let initial = read
            .run_inner(json!({"target":target}), native_context(&source))
            .await
            .unwrap();
        assert_eq!(initial["auto_continue_on_context_exhaustion"], false);
        assert_eq!(initial["aggregate"]["kind"], kind);
        assert_eq!(initial["aggregate"]["product_conversation_id"], id.as_str());
        assert!(initial.get("latest_admission").is_none());
        for enabled in [true, false] {
            let result = set
                .run_inner(
                    json!({
                        "target":target, "auto_continue_on_context_exhaustion":enabled
                    }),
                    native_context(&source),
                )
                .await
                .unwrap();
            assert_eq!(result["auto_continue_on_context_exhaustion"], enabled);
            assert_eq!(
                db.auto_continue_on_context_exhaustion(&id).await.unwrap(),
                enabled.into()
            );
        }
    }
    assert!(db.get_messages(&ordinary.id).await.unwrap().is_empty());
    assert!(db
        .automatic_continuation_admission(&ordinary.id)
        .await
        .unwrap()
        .is_none());
    assert!(db
        .continuation_dispatch_intent(&ordinary.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn settings_require_trusted_source_and_refuse_ordinary_sources_without_writing() {
    let (runtime, db, source) = native_runtime().await;
    let set = AutomaticContinuationTool::set(runtime.clone());
    let ordinary = db
        .create_conversation(
            "settings-source",
            "settings-source",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let input = json!({
        "target":{"kind":"ordinary", "product_conversation_id":ordinary.product_conversation_id},
        "auto_continue_on_context_exhaustion":true
    });
    let read = AutomaticContinuationTool::read(runtime.clone());
    assert!(read
        .run_inner(
            json!({"target":{"kind":"coordinator"}}),
            native_context_without_source_identity(&source),
        )
        .await
        .is_err());
    assert!(read
        .run_inner(
            json!({"target":{"kind":"coordinator"}}),
            native_context(&ordinary.id),
        )
        .await
        .is_err());
    assert!(set
        .run_inner(
            input.clone(),
            native_context_without_source_identity(&source)
        )
        .await
        .is_err());
    assert!(set
        .run_inner(input, native_context(&ordinary.id))
        .await
        .is_err());
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&ordinary.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Disabled,
    );
    assert!(!runtime.local_authority_is_closed());
}

#[tokio::test]
async fn preclosed_setting_adapter_produces_no_ordinary_tool_output() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    let (runtime, _, source) = native_runtime().await;
    let owner = runtime.acquire_local_authority_pass().unwrap();
    owner.close("settings_adapter_preclosed_fixture");
    let tool = AutomaticContinuationTool::set(runtime);
    let future = tool.run(
        json!({
            "target":{"kind":"coordinator"}, "auto_continue_on_context_exhaustion":true
        }),
        native_context(&source),
    );
    tokio::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "preserve the sequential source-registry integration test and all source assertions"
)]
async fn registry_excludes_ordinary_subagent_and_rotated_coordinator_sources() {
    let (runtime, db, source) = native_runtime().await;
    let ordinary = db
        .create_conversation(
            "settings-registry",
            "settings-registry",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let child = db
        .create_subagent_conversation(
            "settings-child",
            "settings-child",
            "/tmp",
            &ordinary.id,
            "offline-model",
            &ordinary.conv_mode,
            phoenix_core::llm_language::LlmLanguage::default(),
            ordinary.attached_work_scope_id.as_ref(),
            phoenix_db::SubAgentExecution {
                connection: "mock",
                effort: None,
                persona: None,
            },
        )
        .await
        .unwrap();
    for excluded in [&ordinary.id, &child.id] {
        assert!(
            crate::conversation_action_tools::tools(runtime.clone(), excluded)
                .await
                .unwrap()
                .is_empty()
        );
    }
    let registry = crate::conversation_action_tools::tools(runtime.clone(), &source)
        .await
        .unwrap();
    let read = registry
        .iter()
        .find(|tool| tool.name() == "read_automatic_continuation")
        .unwrap();
    let set = registry
        .iter()
        .find(|tool| tool.name() == "set_automatic_continuation")
        .unwrap();
    for tool in [read, set] {
        let schema = tool.input_schema();
        let alternatives = schema["properties"]["target"]["anyOf"].as_array().unwrap();
        assert_eq!(alternatives.len(), 2);
        for alternative in alternatives {
            assert_eq!(alternative["additionalProperties"], false);
        }
    }
    db.update_conversation_state(
        &source,
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: "settings registry rotation".into(),
        },
    )
    .await
    .unwrap();
    let successor = match db.continue_conversation(&source).await.unwrap() {
        phoenix_db::ContinueOutcome::Created(successor) => successor,
        other @ (phoenix_db::ContinueOutcome::AlreadyContinued(_)
        | phoenix_db::ContinueOutcome::ParentNotContextExhausted { .. }) => {
            panic!("expected Coordinator rotation, got {other:?}")
        }
    };
    assert!(
        crate::conversation_action_tools::tools(runtime.clone(), &source)
            .await
            .unwrap()
            .is_empty()
    );
    let refused = set
        .run(
            json!({
                "target":{"kind":"coordinator"}, "auto_continue_on_context_exhaustion":true,
            }),
            native_context(&source),
        )
        .await;
    assert!(matches!(refused, ToolOutput::Error { .. }));
    let refused_read = read
        .run(
            json!({"target":{"kind":"coordinator"}}),
            native_context(&source),
        )
        .await;
    assert!(matches!(refused_read, ToolOutput::Error { .. }));
    assert_eq!(
        db.auto_continue_on_context_exhaustion(&successor.product_conversation_id)
            .await
            .unwrap(),
        AutoContinueOnContextExhaustion::Disabled,
    );
    let new_registry = crate::conversation_action_tools::tools(runtime.clone(), &successor.id)
        .await
        .unwrap();
    assert!(new_registry
        .iter()
        .any(|tool| tool.name() == "set_automatic_continuation"));
    assert!(!runtime.local_authority_is_closed());
}
