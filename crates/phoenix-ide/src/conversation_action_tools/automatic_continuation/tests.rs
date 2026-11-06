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
    for instance in [
        "local",
        "remote-name",
        "00000000-0000-0000-0000-000000000001",
    ] {
        assert!(serde_json::from_value::<ReadRequest>(
            json!({"target":coordinator, "instance":instance})
        )
        .is_ok());
        assert!(serde_json::from_value::<SetRequest>(json!({"target":coordinator, "instance":instance, "auto_continue_on_context_exhaustion":false})).is_ok());
    }
    assert!(
        serde_json::from_value::<ReadRequest>(json!({"target":coordinator, "instance":42}))
            .is_err()
    );
    assert!(serde_json::from_value::<SetRequest>(
        json!({"target":coordinator, "instance":true, "auto_continue_on_context_exhaustion":true})
    )
    .is_err());
    assert!(
        serde_json::from_value::<ReadRequest>(json!({"target":coordinator, "instance":null}))
            .is_err()
    );
    assert!(serde_json::from_value::<SetRequest>(
        json!({"target":coordinator, "instance":null, "auto_continue_on_context_exhaustion":true})
    )
    .is_err());
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
    assert_eq!(
        read.input_schema()["properties"]["instance"]["type"],
        "string"
    );
    assert_eq!(
        set.input_schema()["properties"]["instance"]["type"],
        "string"
    );
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
async fn remote_selector_errors_never_write_local_preference() {
    let (runtime, db, source) = native_runtime().await;
    let tool = AutomaticContinuationTool::set(runtime);
    let ordinary = db
        .create_conversation(
            "remote-selector",
            "remote-selector",
            "/tmp",
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let target =
        json!({"kind":"ordinary", "product_conversation_id":ordinary.product_conversation_id});
    for instance in ["unknown-peer", "LOCAL"] {
        assert!(tool.run_inner(json!({"instance":instance, "target":target, "auto_continue_on_context_exhaustion":true}), native_context(&source)).await.is_err());
        assert_eq!(
            db.auto_continue_on_context_exhaustion(&ordinary.product_conversation_id)
                .await
                .unwrap(),
            AutoContinueOnContextExhaustion::Disabled
        );
    }
    let result = tool.run_inner(json!({"instance":"local", "target":target, "auto_continue_on_context_exhaustion":true}), native_context(&source)).await.unwrap();
    assert_eq!(result["auto_continue_on_context_exhaustion"], true);
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "TLS peer fixture and admission ordering are exercised together"
)]
async fn cancelled_remote_preference_request_keeps_source_admission_and_authority_until_peer_settles(
) {
    use phoenix_core::domain::instance_identity::{
        InstanceId, PeerBearerCredential, PeerCaCertificatePem, PeerTlsTrust,
    };
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};

    let (runtime, db, source) = native_runtime().await;
    let source_product = db
        .get_conversation(&source)
        .await
        .unwrap()
        .product_conversation_id;
    let destination = InstanceId::new();
    let caller = db.instance_id().await.unwrap();
    let bearer = PeerBearerCredential::parse(format!("phx_peer_{}", "c".repeat(43))).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut tls = crate::tls::load_config(&crate::tls::ConfigSource::Auto {
        dir: temp.path().to_path_buf(),
        hosts: vec!["localhost".to_string()],
    })
    .unwrap();
    tls.server.alpn_protocols = vec![b"http/1.1".to_vec()];
    let trust = PeerTlsTrust::PrivateCa {
        certificate_pem: PeerCaCertificatePem::parse(
            std::fs::read_to_string(tls.ca_cert_path.unwrap()).unwrap(),
        )
        .unwrap(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    db.save_federation_peer_connection(
        destination,
        &"saved-peer".parse().unwrap(),
        &format!(
            "https://localhost:{}",
            listener.local_addr().unwrap().port()
        )
        .parse()
        .unwrap(),
        &bearer,
        &trust,
    )
    .await
    .unwrap();

    let (received_tx, received_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let stream = tokio_rustls::TlsAcceptor::from(Arc::new(tls.server))
            .accept(stream)
            .await
            .unwrap();
        let mut stream = tokio::io::BufReader::new(stream);
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            assert_ne!(stream.read_until(b'\n', &mut headers).await.unwrap(), 0);
        }
        let headers = String::from_utf8(headers).unwrap();
        assert!(
            headers.starts_with("POST /api/federation/peer/read-automatic-continuation HTTP/1.1")
        );
        assert!(headers
            .to_ascii_lowercase()
            .contains("authorization: bearer phx_peer_"));
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .and_then(|value| value.trim().parse().ok())
            })
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap();
        let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(request["destination_instance_id"], destination.to_string());
        assert_eq!(request["caller_instance_id"], caller.to_string());
        assert_eq!(request["target"]["kind"], "coordinator");
        received_tx.send(()).unwrap();
        release_rx.await.unwrap();
        stream
            .get_mut()
            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });

    let tool = AutomaticContinuationTool::read(runtime.clone());
    let requester = tokio::spawn(async move {
        tool.run_inner(
            json!({"instance":"saved-peer", "target":{"kind":"coordinator"}}),
            native_context(&source),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), received_rx)
        .await
        .expect("peer must receive the bounded request")
        .unwrap();
    requester.abort();
    assert!(requester.await.unwrap_err().is_cancelled());

    let closing_authority = runtime.acquire_local_authority_pass().unwrap();
    closing_authority.close("remote_preference_cancellation_test");
    assert_eq!(runtime.fatal_authority_owners_at_first_close(), Some(3));
    drop(closing_authority);

    let admission = runtime.lock_product_message_admission(source_product.as_str());
    tokio::pin!(admission);
    assert!(matches!(
        futures::poll!(admission.as_mut()),
        std::task::Poll::Pending
    ));

    release_tx.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), server)
        .await
        .expect("peer must finish responding")
        .unwrap();
    let _guard = tokio::time::timeout(std::time::Duration::from_secs(10), admission)
        .await
        .expect("source admission must resume after peer settlement");
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
async fn preclosed_remote_source_authority_refuses_before_peer_resolution() {
    let (runtime, _, coordinator) = native_runtime().await;
    let closing_authority = runtime.acquire_local_authority_pass().unwrap();
    closing_authority.close("preclosed_remote_source_test");
    drop(closing_authority);

    let error = remote_from_coordinator(
        runtime,
        coordinator,
        "unconfigured-peer".into(),
        TargetInput::Coordinator {},
        native::Operation::Read,
    )
    .await
    .unwrap_err();

    assert_eq!(error, "Local mutation authority is closed");
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
