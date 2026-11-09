use super::*;

fn envelope() -> serde_json::Value {
    serde_json::json!({
        "caller_instance_id": InstanceId::new(),
        "destination_instance_id": InstanceId::new(),
    })
}

#[test]
fn create_options_requires_exact_variant_shape() {
    let mut request = envelope();
    request["kind"] = "without_cwd".into();
    request["correlation_id"] = uuid::Uuid::new_v4().to_string().into();
    assert!(serde_json::from_value::<NativeCreateOptionsRequest>(request.clone()).is_ok());
    request["cwd"] = "/tmp".into();
    assert!(serde_json::from_value::<NativeCreateOptionsRequest>(request.clone()).is_err());
    request["kind"] = "with_cwd".into();
    assert!(serde_json::from_value::<NativeCreateOptionsRequest>(request.clone()).is_ok());
    request.as_object_mut().unwrap().remove("cwd");
    assert!(serde_json::from_value::<NativeCreateOptionsRequest>(request.clone()).is_err());
    request["cwd"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<NativeCreateOptionsRequest>(request).is_err());
}

#[test]
fn create_request_requires_explicit_nullables_and_source() {
    let mut request = envelope();
    request.as_object_mut().unwrap().extend(
        serde_json::json!({
            "request_id": uuid::Uuid::new_v4().to_string(), "cwd": "/tmp", "objective": "test",
            "model": null, "effort": null, "language": null, "start": "submit_objective",
            "source": {"source_product_conversation_id": ProductConversationId::new(),
                "source_transcript_id": uuid::Uuid::new_v4().to_string(),
                "source_assistant_message_id": uuid::Uuid::new_v4().to_string(),
                "source_tool_use_id": "tool"}
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    assert!(serde_json::from_value::<NativeCreateRequest>(request.clone()).is_ok());
    request.as_object_mut().unwrap().remove("model");
    assert!(serde_json::from_value::<NativeCreateRequest>(request.clone()).is_err());
    request["model"] = serde_json::Value::Null;
    request["start"] = "skip_objective".into();
    assert!(serde_json::from_value::<NativeCreateRequest>(request.clone()).is_err());
    request["start"] = "submit_objective".into();
    request["source"]["extra"] = true.into();
    assert!(serde_json::from_value::<NativeCreateRequest>(request).is_err());
}

#[test]
fn rename_and_status_requests_reject_unknown_fields() {
    let mut status = envelope();
    status["request_id"] = uuid::Uuid::new_v4().to_string().into();
    assert!(serde_json::from_value::<NativeCreateStatusRequest>(status.clone()).is_ok());
    status["reference"] = "unexpected".into();
    assert!(serde_json::from_value::<NativeCreateStatusRequest>(status).is_err());

    let mut rename = envelope();
    rename.as_object_mut().unwrap().extend(
        serde_json::json!({
            "operation_id": uuid::Uuid::new_v4().to_string(), "product_conversation_id": ProductConversationId::new(),
            "expected_root_transcript_id": uuid::Uuid::new_v4().to_string(), "expected_name_revision": 0,
            "title": "title", "source": {
                "source_product_conversation_id": ProductConversationId::new(),
                "source_transcript_id": uuid::Uuid::new_v4().to_string(),
                "source_assistant_message_id": uuid::Uuid::new_v4().to_string(),
                "source_tool_use_id": "tool"
            }
        })
        .as_object()
        .unwrap()
        .clone(),
    );
    assert!(serde_json::from_value::<NativeRenameRequest>(rename.clone()).is_ok());
    rename["expected_name_revision"] = "0".into();
    assert!(serde_json::from_value::<NativeRenameRequest>(rename).is_err());
}

#[test]
fn rename_detail_targets_normalize_destination_lookup_identities() {
    assert_eq!(
        conversation_lookup_identity("@conv:opaque-product").unwrap(),
        "opaque-product"
    );
    assert_eq!(
        conversation_lookup_identity("@transcript:opaque-transcript#message-opaque-message")
            .unwrap(),
        "opaque-transcript"
    );
    assert_eq!(
        conversation_lookup_identity("opaque-product-or-slug").unwrap(),
        "opaque-product-or-slug"
    );
    assert!(conversation_lookup_identity(" @conv:opaque-product").is_err());
    assert!(conversation_lookup_identity("@conv:opaque-product ").is_err());
}

#[test]
fn responses_reject_payloads_beyond_route_bound() {
    let failure = native_error(format!("secret detail: {}", "x".repeat(100_000)));
    let body = serde_json::to_vec(&failure.body).unwrap();
    assert!(body.len() < 256);
    assert!(!String::from_utf8_lossy(&body).contains("secret detail"));
    assert_eq!(
        native_json(&"large payload", 4).unwrap_err().status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert!(native_json(
        &serde_json::json!({"kind":"unknown"}),
        NATIVE_CREATE_STATUS_RESPONSE_BYTES
    )
    .is_ok());
    assert_eq!(NATIVE_CREATE_OPTIONS_REQUEST_BYTES, 4096);
    assert_eq!(NATIVE_RENAME_RESPONSE_BYTES, 16_384);
}

fn source_call() -> NativeActionSourceCall {
    NativeActionSourceCall {
        product_conversation: ProductConversationId::new(),
        source_transcript_id: uuid::Uuid::new_v4().to_string(),
        source_assistant_message_id: uuid::Uuid::new_v4().to_string(),
        source_tool_use_id: "call_provider_123".into(),
    }
}

fn native_rename_request() -> NativeRenameRequest {
    NativeRenameRequest {
        caller_instance_id: InstanceId::new(),
        destination_instance_id: InstanceId::new(),
        operation_id: uuid::Uuid::new_v4().to_string(),
        product_conversation_id: ProductConversationId::new(),
        expected_root_transcript_id: uuid::Uuid::new_v4().to_string(),
        expected_name_revision: 0,
        title: "new title".into(),
        source: source_call(),
    }
}

#[test]
fn protocol_witnesses_are_canonical_but_persisted_identities_are_opaque() {
    let valid = serde_json::json!({
        "caller_instance_id": InstanceId::new(),
        "destination_instance_id": InstanceId::new(),
        "request_id": "abcdef01-2345-4678-9abc-def012345678"
    });
    assert!(serde_json::from_value::<NativeCreateStatusRequest>(valid.clone()).is_ok());
    for field in [
        "caller_instance_id",
        "destination_instance_id",
        "request_id",
    ] {
        for invalid in [
            "ABCDEF01-2345-4678-9ABC-DEF012345678",
            "abcdef01234546789abcdef012345678",
            "not-a-uuid",
        ] {
            let mut request = valid.clone();
            request[field] = invalid.into();
            assert!(
                serde_json::from_value::<NativeCreateStatusRequest>(request).is_err(),
                "{field}: {invalid}"
            );
        }
    }
    let source = source_call();
    assert!(serde_json::from_value::<NativeActionSourceCall>(
        serde_json::to_value(&source).unwrap()
    )
    .is_ok());
    assert!(native_source_valid(&source));
    for invalid in ["", "   ", "call\n123"] {
        let mut source = source.clone();
        source.source_tool_use_id = invalid.into();
        assert!(!native_source_valid(&source));
    }
    let mut invalid_source = serde_json::to_value(source).unwrap();
    invalid_source["source_transcript_id"] = "transcript:root".into();
    assert!(serde_json::from_value::<NativeActionSourceCall>(invalid_source.clone()).is_ok());
    invalid_source["source_transcript_id"] = "   ".into();
    let source: NativeActionSourceCall = serde_json::from_value(invalid_source).unwrap();
    assert!(!native_source_valid(&source));
}

#[test]
fn constructed_native_intents_are_validated_before_transport() {
    let mut source = source_call();
    source.source_transcript_id = "transcript:root".into();
    source.source_assistant_message_id = "message:assistant".into();
    assert!(native_source_valid(&source));
    source.source_assistant_message_id = "x".repeat(NATIVE_DOMAIN_ID_MAX_BYTES + 1);
    assert!(!native_source_valid(&source));
    let mut create = NativeCreateRequest {
        caller_instance_id: InstanceId::new(),
        destination_instance_id: InstanceId::new(),
        request_id: uuid::Uuid::new_v4().to_string(),
        cwd: "/destination/project".into(),
        objective: "Implement the requested change".into(),
        model: None,
        effort: None,
        language: None,
        start: NativeCreateStart::SubmitObjective,
        source: source_call(),
    };
    assert!(native_create_intent_valid(&create));
    create.objective = " \n ".into();
    assert!(!native_create_intent_valid(&create));
    create.objective = "objective".into();
    create.model = Some(String::new());
    assert!(!native_create_intent_valid(&create));
    create.model = None;
    create.source.source_assistant_message_id = "bad-message".into();
    assert!(native_create_intent_valid(&create));

    let mut rename = native_rename_request();
    assert!(native_rename_intent_valid(&rename));
    for revision in [-1, i64::MAX] {
        rename.expected_name_revision = revision;
        assert!(!native_rename_intent_valid(&rename));
    }
    rename.expected_name_revision = 0;
    rename.title = "  ".into();
    assert!(!native_rename_intent_valid(&rename));
}

#[test]
fn native_rename_receipt_preflight_covers_both_variants_at_field_bounds() {
    let mut rename = native_rename_request();
    rename.product_conversation_id =
        ProductConversationId::parse("p".repeat(NATIVE_DOMAIN_ID_MAX_BYTES)).unwrap();
    rename.expected_root_transcript_id = "r".repeat(NATIVE_DOMAIN_ID_MAX_BYTES);
    rename.title = "İ".repeat(200);
    rename.source.product_conversation =
        ProductConversationId::parse("p".repeat(NATIVE_DOMAIN_ID_MAX_BYTES)).unwrap();
    rename.source.source_transcript_id = "t".repeat(NATIVE_DOMAIN_ID_MAX_BYTES);
    rename.source.source_assistant_message_id = "m".repeat(NATIVE_DOMAIN_ID_MAX_BYTES);
    rename.source.source_tool_use_id = "u".repeat(NATIVE_DOMAIN_ID_MAX_BYTES);

    assert!(native_rename_intent_valid(&rename));
    assert!(native_rename_receipt_responses_fit(&rename));
}

#[test]
fn create_options_verifies_the_servers_fifty_project_limit_even_with_cwd() {
    let caller = InstanceId::new();
    let destination = InstanceId::new();
    let correlation = uuid::Uuid::new_v4().to_string();
    let request = NativeCreateOptionsRequest::WithCwd {
        caller_instance_id: caller,
        destination_instance_id: destination,
        correlation_id: correlation.clone(),
        cwd: "~/project".into(),
    };
    let mut response = NativeCreateOptionsResponse {
        caller_instance_id: caller,
        destination_instance_id: destination,
        correlation_id: correlation,
        cwd: Some("/destination/project".into()),
        projects: (0..50)
            .map(|i| NativeCreateProjectOption {
                project_id: uuid::Uuid::new_v4().to_string(),
                cwd: format!("/destination/project-{i}"),
            })
            .collect(),
        projects_truncated: true,
        models: vec![NativeCreateModelOption {
            model: "destination-model".into(),
            supported_efforts: vec![],
        }],
        default_model: "destination-model".into(),
        default_llm_language: "phoenix-native".into(),
    };
    assert!(native_verify_create_options(&request, &response).is_ok());
    response.projects.push(NativeCreateProjectOption {
        project_id: uuid::Uuid::new_v4().to_string(),
        cwd: "/destination/extra".into(),
    });
    assert!(native_verify_create_options(&request, &response).is_err());
    response.projects.truncate(49);
    assert!(native_verify_create_options(&request, &response).is_err());
    response.projects_truncated = false;
    assert!(native_verify_create_options(&request, &response).is_ok());
    response.projects[1].project_id = response.projects[0].project_id.clone();
    assert!(native_verify_create_options(&request, &response).is_err());
    response.projects.clear();
    response.correlation_id = uuid::Uuid::new_v4().to_string();
    assert!(native_verify_create_options(&request, &response).is_err());
}

#[test]
fn create_options_accepts_opaque_project_ids_and_rejects_malformed_ids() {
    let caller = InstanceId::new();
    let destination = InstanceId::new();
    let correlation = uuid::Uuid::new_v4().to_string();
    let request = NativeCreateOptionsRequest::WithoutCwd {
        caller_instance_id: caller,
        destination_instance_id: destination,
        correlation_id: correlation.clone(),
    };
    let mut response = NativeCreateOptionsResponse {
        caller_instance_id: caller,
        destination_instance_id: destination,
        correlation_id: correlation,
        cwd: None,
        projects: vec![NativeCreateProjectOption {
            project_id: "project-000".into(),
            cwd: "/destination/project".into(),
        }],
        projects_truncated: false,
        models: vec![NativeCreateModelOption {
            model: "destination-model".into(),
            supported_efforts: vec![],
        }],
        default_model: "destination-model".into(),
        default_llm_language: "phoenix-native".into(),
    };
    assert!(native_verify_create_options(&request, &response).is_ok());
    for invalid in ["", "   ", "project\n000"] {
        response.projects[0].project_id = invalid.into();
        assert!(
            native_verify_create_options(&request, &response).is_err(),
            "{invalid:?}"
        );
    }
    response.projects[0].project_id = "x".repeat(NATIVE_DOMAIN_ID_MAX_BYTES + 1);
    assert!(native_verify_create_options(&request, &response).is_err());
}

#[test]
fn rename_options_binds_correlation_not_raw_destination_lookup_reference() {
    let request = NativeRenameOptionsRequest {
        caller_instance_id: InstanceId::new(),
        destination_instance_id: InstanceId::new(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        reference: uuid::Uuid::new_v4().to_string(), // A continuation transcript, not the root.
    };
    let mut response = NativeRenameOptionsResponse {
        caller_instance_id: request.caller_instance_id,
        destination_instance_id: request.destination_instance_id,
        correlation_id: request.correlation_id.clone(),
        canonical_route: "/c/destination-slug".into(),
        detail: NativeRenameDetail {
            can_rename: true,
            product_conversation_id: ProductConversationId::new(),
            root_transcript_id: uuid::Uuid::new_v4().to_string(),
            name_revision: 2,
            title: Some("Existing  title".into()),
            slug: Some("destination-slug".into()),
        },
        allowed_actions: vec![
            NativeRenameDetailAction::Detail,
            NativeRenameDetailAction::Rename,
        ],
    };
    assert!(native_verify_rename_options(&request, &response).is_ok());
    response.canonical_route = "/c/another-product".into();
    assert!(native_verify_rename_options(&request, &response).is_err());
    response.canonical_route = "/c/destination-slug".into();
    response
        .allowed_actions
        .insert(1, NativeRenameDetailAction::Read);
    assert!(native_verify_rename_options(&request, &response).is_err());
    response.allowed_actions = vec![NativeRenameDetailAction::Detail];
    assert!(native_verify_rename_options(&request, &response).is_err());
    response.detail.can_rename = false;
    assert!(native_verify_rename_options(&request, &response).is_ok());
    response.correlation_id = uuid::Uuid::new_v4().to_string();
    assert!(native_verify_rename_options(&request, &response).is_err());
}

#[test]
fn rename_options_closes_direct_product_reference_and_validates_target_ids() {
    let product = ProductConversationId::parse("opaque-product").unwrap();
    let mut request = NativeRenameOptionsRequest {
        caller_instance_id: InstanceId::new(),
        destination_instance_id: InstanceId::new(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        reference: "@conv:opaque-product".into(),
    };
    let mut response = NativeRenameOptionsResponse {
        caller_instance_id: request.caller_instance_id,
        destination_instance_id: request.destination_instance_id,
        correlation_id: request.correlation_id.clone(),
        canonical_route: "/c/destination-slug".into(),
        detail: NativeRenameDetail {
            can_rename: true,
            product_conversation_id: product,
            root_transcript_id: "opaque-root".into(),
            name_revision: 1,
            title: Some("Title".into()),
            slug: Some("destination-slug".into()),
        },
        allowed_actions: vec![
            NativeRenameDetailAction::Detail,
            NativeRenameDetailAction::Rename,
        ],
    };
    assert!(native_verify_rename_options(&request, &response).is_ok());
    request.reference = "@conv:other-product".into();
    assert!(native_verify_rename_options(&request, &response).is_err());
    for reference in [
        "@transcript:opaque-root",
        "opaque-product",
        "destination-slug",
    ] {
        request.reference = reference.into();
        assert!(
            native_verify_rename_options(&request, &response).is_ok(),
            "{reference}"
        );
    }
    for invalid in ["", " ", "root\ninvalid"] {
        response.detail.root_transcript_id = invalid.into();
        assert!(
            native_verify_rename_options(&request, &response).is_err(),
            "{invalid:?}"
        );
    }
    response.detail.root_transcript_id = "r".repeat(NATIVE_DOMAIN_ID_MAX_BYTES + 1);
    assert!(native_verify_rename_options(&request, &response).is_err());
    response.detail.root_transcript_id = "opaque-root".into();
    let invalid = "product\ninvalid";
    response.detail.product_conversation_id = ProductConversationId::parse(invalid).unwrap();
    assert!(
        native_verify_rename_options(&request, &response).is_err(),
        "{invalid:?}"
    );
    response.detail.product_conversation_id =
        ProductConversationId::parse("p".repeat(NATIVE_DOMAIN_ID_MAX_BYTES + 1)).unwrap();
    assert!(native_verify_rename_options(&request, &response).is_err());
}

fn status_fixture() -> (NativeCreateStatusRequest, serde_json::Value) {
    let request = NativeCreateStatusRequest {
        caller_instance_id: InstanceId::new(),
        destination_instance_id: InstanceId::new(),
        request: uuid::Uuid::new_v4().to_string(),
    };
    let response = serde_json::json!({
        "kind": "known",
        "caller_instance_id": request.caller_instance_id,
        "destination_instance_id": request.destination_instance_id,
        "request_id": request.request,
        "product_conversation_id": ProductConversationId::new(),
        "creation_status": "accepted",
        "provisioning_stage": "validate_intent",
        "provisioning_stage_updated_at_unix_micros": 1,
        "retry_at_unix_micros": null,
        "error": null,
        "publication": {"kind": "none"},
        "initial_execution": {"kind": "unavailable"}
    });
    (request, response)
}

#[test]
fn create_status_dtos_are_closed_at_every_tagged_boundary() {
    let (_, valid) = status_fixture();
    assert!(serde_json::from_value::<NativeCreateStatusResponse>(valid.clone()).is_ok());
    for pointer in ["", "/publication", "/initial_execution"] {
        let mut value = valid.clone();
        value.pointer_mut(pointer).unwrap()["unexpected"] = true.into();
        assert!(
            serde_json::from_value::<NativeCreateStatusResponse>(value).is_err(),
            "{pointer}"
        );
    }
    for field in [
        "retry_at_unix_micros",
        "error",
        "publication",
        "initial_execution",
    ] {
        let mut value = valid.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<NativeCreateStatusResponse>(value).is_err(),
            "{field}"
        );
    }
    let mut value = valid;
    value["creation_status"] = "invented".into();
    assert!(serde_json::from_value::<NativeCreateStatusResponse>(value).is_err());
}

#[test]
fn create_status_verification_accepts_delivery_and_cancellation_evidence() {
    let (request, mut response) = status_fixture();
    for state in ["accepted", "claimed", "failed", "deletion_pending"] {
        response["creation_status"] = state.into();
        let typed = serde_json::from_value(response.clone()).unwrap();
        assert!(native_verify_status(&request, &typed).is_ok(), "{state}");
    }
    response["provisioning_stage"] = "bootstrap_initial_turn".into();
    for state in ["delivery_pending", "delivery_failed"] {
        response["creation_status"] = state.into();
        response["initial_execution"] = serde_json::json!({"kind":"running"});
        let typed = serde_json::from_value(response.clone()).unwrap();
        assert!(native_verify_status(&request, &typed).is_ok(), "{state}");
    }
    response["retry_at_unix_micros"] = 2.into();
    response["initial_execution"] = serde_json::json!({"kind":"unavailable"});
    response["publication"] = serde_json::json!({"kind":"historical", "conversation_id":uuid::Uuid::new_v4().to_string()});
    response["creation_status"] = "delivery_pending".into();
    let typed = serde_json::from_value(response.clone()).unwrap();
    assert!(native_verify_status(&request, &typed).is_ok());
    response["publication"] = serde_json::json!({"kind":"none"});
    for state in ["cancelling", "cancelled", "cleanup_ambiguous"] {
        response["creation_status"] = state.into();
        let typed = serde_json::from_value(response.clone()).unwrap();
        assert!(native_verify_status(&request, &typed).is_ok(), "{state}");
    }
    response["creation_status"] = "published".into();
    response["provisioning_stage"] = "finalize_publication".into();
    response["retry_at_unix_micros"] = serde_json::Value::Null;
    response["publication"] = serde_json::json!({"kind":"historical", "conversation_id":uuid::Uuid::new_v4().to_string()});
    let typed = serde_json::from_value(response.clone()).unwrap();
    assert!(native_verify_status(&request, &typed).is_ok());
    response["publication"] = serde_json::json!({"kind":"available", "conversation_id":uuid::Uuid::new_v4().to_string(), "canonical_route":"/c/published-slug"});
    response["initial_execution"] =
        serde_json::json!({"kind":"failed", "reason":"provider failed"});
    let typed = serde_json::from_value(response).unwrap();
    assert!(native_verify_status(&request, &typed).is_ok());
}

#[test]
fn create_status_verification_rejects_false_publication_and_mismatched_witnesses() {
    let (request, valid) = status_fixture();
    for (field, value) in [
        ("creation_status", serde_json::json!("published")),
        ("creation_status", serde_json::json!("retry_scheduled")),
        (
            "publication",
            serde_json::json!({"kind":"available", "conversation_id":uuid::Uuid::new_v4().to_string(), "canonical_route":"/c/slug"}),
        ),
        ("initial_execution", serde_json::json!({"kind":"completed"})),
        (
            "request_id",
            serde_json::json!(uuid::Uuid::new_v4().to_string()),
        ),
        (
            "destination_instance_id",
            serde_json::json!(InstanceId::new()),
        ),
    ] {
        let mut value_with_error = valid.clone();
        value_with_error[field] = value;
        let typed = serde_json::from_value(value_with_error).unwrap();
        assert!(native_verify_status(&request, &typed).is_err(), "{field}");
    }
    let mut retry = valid;
    retry["creation_status"] = "retry_scheduled".into();
    retry["retry_at_unix_micros"] = 2.into();
    let typed = serde_json::from_value(retry).unwrap();
    assert!(native_verify_status(&request, &typed).is_ok());
}

#[test]
fn create_status_validates_timestamps_stages_and_independent_execution_evidence() {
    let (request, valid) = status_fixture();
    for timestamp in [-1, i64::MAX] {
        let mut invalid = valid.clone();
        invalid["provisioning_stage_updated_at_unix_micros"] = timestamp.into();
        let response = serde_json::from_value(invalid).unwrap();
        assert!(native_verify_status(&request, &response).is_err());

        let mut invalid_retry = valid.clone();
        invalid_retry["creation_status"] = "retry_scheduled".into();
        invalid_retry["retry_at_unix_micros"] = timestamp.into();
        let response = serde_json::from_value(invalid_retry).unwrap();
        assert!(native_verify_status(&request, &response).is_err());
    }
    for state in ["accepted", "claimed", "retry_scheduled", "failed"] {
        for stage in ["bootstrap_initial_turn", "finalize_publication"] {
            let mut invalid = valid.clone();
            invalid["creation_status"] = state.into();
            invalid["provisioning_stage"] = stage.into();
            if state == "retry_scheduled" {
                invalid["retry_at_unix_micros"] = 2.into();
            }
            let response = serde_json::from_value(invalid).unwrap();
            assert!(
                native_verify_status(&request, &response).is_err(),
                "{state}/{stage}"
            );
        }
    }
    let mut published = valid;
    published["creation_status"] = "published".into();
    published["provisioning_stage"] = "finalize_publication".into();
    published["publication"] = serde_json::json!({
        "kind": "available", "conversation_id": uuid::Uuid::new_v4().to_string(),
        "canonical_route": "/c/published-slug"
    });
    for execution in [
        "queued",
        "running",
        "completed",
        "cancelled",
        "awaiting_user_response",
        "awaiting_task_approval",
        "unavailable",
    ] {
        published["initial_execution"] = serde_json::json!({"kind": execution});
        let response = serde_json::from_value(published.clone()).unwrap();
        assert!(
            native_verify_status(&request, &response).is_ok(),
            "{execution}"
        );
    }
    for reason in ["", "  \n"] {
        published["initial_execution"] = serde_json::json!({"kind": "failed", "reason": reason});
        let response = serde_json::from_value(published.clone()).unwrap();
        assert!(native_verify_status(&request, &response).is_err());
    }
    published["initial_execution"] = serde_json::json!({"kind": "completed"});
    published["creation_status"] = "deletion_pending".into();
    published["publication"] = serde_json::json!({"kind": "none"});
    let response = serde_json::from_value(published).unwrap();
    assert!(native_verify_status(&request, &response).is_ok());
}

#[test]
fn create_status_rejects_malformed_available_and_historical_publication_ids() {
    let (request, mut status) = status_fixture();
    status["creation_status"] = "published".into();
    status["provisioning_stage"] = "finalize_publication".into();
    for publication in ["available", "historical"] {
        for invalid in ["", "  ", "root\ninvalid"] {
            status["publication"] = if publication == "available" {
                serde_json::json!({"kind": publication, "conversation_id": invalid, "canonical_route": "/c/slug"})
            } else {
                serde_json::json!({"kind": publication, "conversation_id": invalid})
            };
            let typed = serde_json::from_value(status.clone()).unwrap();
            assert!(
                native_verify_status(&request, &typed).is_err(),
                "{publication}/{invalid:?}"
            );
        }
        status["publication"]["conversation_id"] =
            "x".repeat(NATIVE_DOMAIN_ID_MAX_BYTES + 1).into();
        let typed = serde_json::from_value(status.clone()).unwrap();
        assert!(
            native_verify_status(&request, &typed).is_err(),
            "{publication}/oversized"
        );
        status["publication"]["conversation_id"] = "opaque-conversation".into();
        let typed = serde_json::from_value(status.clone()).unwrap();
        assert!(
            native_verify_status(&request, &typed).is_ok(),
            "{publication}/opaque"
        );
    }
}

#[test]
fn create_status_bounds_retained_diagnostics_without_losing_snapshot() {
    let (request, mut status) = status_fixture();
    status["creation_status"] = "published".into();
    status["provisioning_stage"] = "finalize_publication".into();
    status["publication"] = serde_json::json!({
        "kind": "available", "conversation_id": "opaque-conversation", "canonical_route": "/c/slug"
    });
    status["initial_execution"] = serde_json::json!({"kind":"failed", "reason":"short reason"});
    status["error"] = "short error".into();
    let mut known: NativeCreateStatusResponse = serde_json::from_value(status.clone()).unwrap();
    native_bound_status_diagnostics(&mut known).unwrap();
    assert_eq!(serde_json::to_value(&known).unwrap(), status);

    for (error_size, reason_size) in [(100_000, 12), (12, 100_000), (100_000, 100_000)] {
        status["error"] = "e".repeat(error_size).into();
        status["initial_execution"]["reason"] = "r".repeat(reason_size).into();
        let mut known: NativeCreateStatusResponse = serde_json::from_value(status.clone()).unwrap();
        native_bound_status_diagnostics(&mut known).unwrap();
        let encoded = serde_json::to_vec(&known).unwrap();
        assert!(encoded.len() <= NATIVE_CREATE_STATUS_RESPONSE_BYTES);
        assert!(native_json(&known, NATIVE_CREATE_STATUS_RESPONSE_BYTES).is_ok());
        assert!(native_verify_status(&request, &known).is_ok());
        let value = serde_json::to_value(&known).unwrap();
        assert_eq!(value["publication"], status["publication"]);
        assert_eq!(value["creation_status"], "published");
        assert_eq!(
            value["product_conversation_id"],
            status["product_conversation_id"]
        );
        let expected_error = if error_size == 100_000 {
            "diagnostic omitted: response size limit".to_owned()
        } else {
            "e".repeat(error_size)
        };
        assert_eq!(value["error"].as_str(), Some(expected_error.as_str()));
        let expected_reason = if reason_size == 100_000 {
            "diagnostic omitted: response size limit".to_owned()
        } else {
            "r".repeat(reason_size)
        };
        assert_eq!(
            value["initial_execution"]["reason"].as_str(),
            Some(expected_reason.as_str())
        );
    }
}

#[test]
fn rename_replay_preserves_historical_tool_locator_but_not_foreign_provenance() {
    let mut too_long = NativeRenameRequest {
        caller_instance_id: InstanceId::new(),
        destination_instance_id: InstanceId::new(),
        operation_id: uuid::Uuid::new_v4().to_string(),
        product_conversation_id: ProductConversationId::new(),
        expected_root_transcript_id: "root:opaque".into(),
        expected_name_revision: 0,
        title: "x".repeat(NATIVE_DOMAIN_ID_MAX_BYTES + 1),
        source: source_call(),
    };
    assert!(!native_rename_intent_valid(&too_long));
    too_long.title = "bounded".into();
    too_long.expected_root_transcript_id = String::new();
    assert!(!native_rename_intent_valid(&too_long));
    let request = NativeRenameRequest {
        caller_instance_id: InstanceId::new(),
        destination_instance_id: InstanceId::new(),
        operation_id: uuid::Uuid::new_v4().to_string(),
        product_conversation_id: ProductConversationId::new(),
        expected_root_transcript_id: uuid::Uuid::new_v4().to_string(),
        expected_name_revision: 2,
        title: "new title".into(),
        source: source_call(),
    };
    let mut receipt = serde_json::json!({
        "status": "applied",
        "caller_instance_id": request.caller_instance_id,
        "destination_instance_id": request.destination_instance_id,
        "operation_id": request.operation_id,
        "product_conversation_id": request.product_conversation_id,
        "root_transcript_id": request.expected_root_transcript_id,
        "receipt_title": request.title,
        "receipt_slug_at_commit": "fixed-slug",
        "receipt_name_revision_before": request.expected_name_revision,
        "receipt_committed_at_unix_micros": 1,
        "receipt_source_instance_id": request.caller_instance_id,
        "receipt_source": request.source
    });
    let applied = serde_json::from_value(receipt.clone()).unwrap();
    assert!(native_verify_rename(&request, &applied).is_ok());
    receipt["receipt_source"]["source_assistant_message_id"] =
        uuid::Uuid::new_v4().to_string().into();
    receipt["receipt_source"]["source_tool_use_id"] = "historical-tool-call".into();
    let applied = serde_json::from_value(receipt.clone()).unwrap();
    assert!(native_verify_rename(&request, &applied).is_err());
    receipt["status"] = "replayed".into();
    let replayed = serde_json::from_value(receipt.clone()).unwrap();
    assert!(native_verify_rename(&request, &replayed).is_ok());
    for (pointer, value) in [
        (
            "/receipt_source_instance_id",
            serde_json::json!(InstanceId::new()),
        ),
        (
            "/receipt_source/source_product_conversation_id",
            serde_json::json!(ProductConversationId::new()),
        ),
        (
            "/receipt_source/source_transcript_id",
            serde_json::json!(uuid::Uuid::new_v4().to_string()),
        ),
        ("/receipt_source/source_tool_use_id", serde_json::json!("")),
        (
            "/receipt_source/source_tool_use_id",
            serde_json::json!("tool\ncall"),
        ),
    ] {
        let mut invalid = receipt.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        let response = serde_json::from_value(invalid).unwrap();
        assert!(
            native_verify_rename(&request, &response).is_err(),
            "{pointer}"
        );
    }
    receipt["receipt_source"]["source_assistant_message_id"] = "message:opaque".into();
    assert!(serde_json::from_value::<NativeRenameResponse>(receipt.clone()).is_ok());
    receipt["receipt_source"]["source_assistant_message_id"] = String::new().into();
    let response: NativeRenameResponse = serde_json::from_value(receipt).unwrap();
    assert!(native_verify_rename(&request, &response).is_err());
}

struct NativeCreateCaseResponses {
    expect_status_post: bool,
    first_response: Vec<u8>,
    known_body: Vec<u8>,
}

struct NativeCreateServerCase {
    case: &'static str,
    expected_create: serde_json::Value,
    expected_status: serde_json::Value,
    responses: NativeCreateCaseResponses,
    coordinator_rotation: Option<(Database, String)>,
}

fn native_create_status_case_body(
    case: &str,
    caller: InstanceId,
    destination: InstanceId,
    request: &NativeCreateRequest,
) -> Vec<u8> {
    let (_, mut known) = status_fixture();
    known["caller_instance_id"] = serde_json::json!(caller);
    known["destination_instance_id"] = serde_json::json!(destination);
    known["request_id"] = request.request_id.clone().into();
    if case == "status_witness_mismatch" {
        known["destination_instance_id"] = serde_json::json!(InstanceId::new());
    }
    if case == "status_invalid_product" {
        known["product_conversation_id"] = "   ".into();
    }
    if case == "status_malformed" {
        b"not json".to_vec()
    } else {
        serde_json::to_vec(&known).unwrap()
    }
}

fn native_create_case_responses(
    case: &str,
    caller: InstanceId,
    destination: InstanceId,
    request: &NativeCreateRequest,
) -> NativeCreateCaseResponses {
    let expect_status_post = matches!(
        case,
        "disconnect"
            | "remote_failure"
            | "status_malformed"
            | "status_witness_mismatch"
            | "status_invalid_product"
    );
    let mut receipt = serde_json::json!({
        "caller_instance_id": caller, "destination_instance_id": destination,
        "request_id": request.request_id, "product_conversation_id": ProductConversationId::new(),
        "status": "accepted"
    });
    let mut status = "200 OK";
    let body = match case {
        "malformed" => b"not json".to_vec(),
        "oversized" | "oversized_stream" => vec![b' '; NATIVE_CREATE_RESPONSE_BYTES + 1],
        "wrong_caller" => {
            receipt["caller_instance_id"] = serde_json::json!(InstanceId::new());
            serde_json::to_vec(&receipt).unwrap()
        }
        "wrong_destination" => {
            receipt["destination_instance_id"] = serde_json::json!(InstanceId::new());
            serde_json::to_vec(&receipt).unwrap()
        }
        "wrong_request" => {
            receipt["request_id"] = uuid::Uuid::new_v4().to_string().into();
            serde_json::to_vec(&receipt).unwrap()
        }
        "missing_receipt" => {
            receipt
                .as_object_mut()
                .unwrap()
                .remove("product_conversation_id");
            serde_json::to_vec(&receipt).unwrap()
        }
        "invalid_product" => {
            receipt["product_conversation_id"] = "   ".into();
            serde_json::to_vec(&receipt).unwrap()
        }
        "domain_rejection" => {
            status = "409 Conflict";
            br#"{"error_type":"conflict","error":"conflict"}"#.to_vec()
        }
        "remote_failure" => {
            status = "500 Internal Server Error";
            br#"{"error_type":"internal","error":"response outcome unavailable"}"#.to_vec()
        }
        "unsupported" => {
            status = "404 Not Found";
            Vec::new()
        }
        "disconnect"
        | "disconnect_after_rotation"
        | "status_malformed"
        | "status_witness_mismatch"
        | "status_invalid_product" => Vec::new(),
        _ => unreachable!(),
    };
    let create_response_dropped = matches!(
        case,
        "disconnect"
            | "disconnect_after_rotation"
            | "status_malformed"
            | "status_witness_mismatch"
            | "status_invalid_product"
    );
    let first_response = if create_response_dropped {
        Vec::new()
    } else if case == "oversized_stream" {
        format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            body.len(),
            String::from_utf8(body).unwrap()
        )
        .into_bytes()
    } else {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes()
        .into_iter()
        .chain(body)
        .collect()
    };
    let known_body = native_create_status_case_body(case, caller, destination, request);
    NativeCreateCaseResponses {
        expect_status_post,
        first_response,
        known_body,
    }
}

async fn rotate_coordinator(db: &Database, transcript_id: &str) {
    db.update_conversation_state(
        transcript_id,
        &phoenix_core::domain::sm_state::ConvState::ContextExhausted {
            summary: "rotate coordinator before reconciliation".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        db.continue_conversation(transcript_id).await.unwrap(),
        phoenix_db::ContinueOutcome::Created(_)
    ));
}

async fn serve_native_create_case(
    listener: tokio::net::TcpListener,
    tls: tokio_rustls::TlsAcceptor,
    server_case: NativeCreateServerCase,
) -> tokio::net::TcpListener {
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};

    let NativeCreateServerCase {
        case,
        expected_create,
        expected_status,
        responses,
        coordinator_rotation,
    } = server_case;
    for step in 0..=usize::from(responses.expect_status_post) {
        let (socket, _) = listener.accept().await.unwrap();
        let socket = tls.accept(socket).await.unwrap();
        let mut socket = tokio::io::BufReader::new(socket);
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            assert_ne!(socket.read_until(b'\n', &mut headers).await.unwrap(), 0);
        }
        let headers = String::from_utf8(headers).unwrap().to_ascii_lowercase();
        let route = if step == 0 {
            "native-create"
        } else {
            "native-create-status"
        };
        assert!(headers.starts_with(&format!("post /api/federation/peer/{route} http/1.1\r\n")));
        assert!(headers.contains("authorization: bearer phx_peer_"));
        let length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("content-length: "))
            .unwrap()
            .parse()
            .unwrap();
        let mut body = vec![0; length];
        socket.read_exact(&mut body).await.unwrap();
        let actual: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let expected = if step == 0 {
            &expected_create
        } else {
            &expected_status
        };
        assert_eq!(&actual, expected);
        if step == 0 {
            if case == "disconnect_after_rotation" {
                let (db, transcript_id) = coordinator_rotation.as_ref().unwrap();
                rotate_coordinator(db, transcript_id).await;
                drop(socket);
                continue;
            }
            if case != "disconnect" {
                socket.write_all(&responses.first_response).await.unwrap();
                socket.flush().await.unwrap();
            }
        } else {
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                responses.known_body.len()
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
            socket.write_all(&responses.known_body).await.unwrap();
            socket.flush().await.unwrap();
        }
    }
    listener
}

fn assert_native_create_case_outcome(
    case: &str,
    outcome: &Result<NativeCreateOutcome, FederationClientError>,
    request: &NativeCreateRequest,
) {
    match case {
        "disconnect" | "remote_failure" => assert!(matches!(
            outcome,
            Ok(NativeCreateOutcome::Reconciled {
                status: NativeCreateStatusResponse::Known {
                    request_id,
                    destination_instance_id,
                    ..
                }
            }) if request_id == &request.request_id
                && *destination_instance_id == request.destination_instance_id
        )),
        "status_malformed" | "status_witness_mismatch" | "status_invalid_product" => {
            assert!(matches!(
            outcome,
            Err(FederationClientError::NativeCreateUncertain {
                destination_instance_id,
                request_id,
                ..
            }) if *destination_instance_id == request.destination_instance_id
                && request_id == &request.request_id
            ));
        }
        "disconnect_after_rotation" => assert!(matches!(
            outcome,
            Err(FederationClientError::NativeCreateUncertain {
                destination_instance_id,
                request_id,
                message,
            }) if *destination_instance_id == request.destination_instance_id
                && request_id == &request.request_id
                && message.contains("status lookup not sent")
        )),
        "oversized" | "oversized_stream" => assert!(matches!(
            outcome,
            Err(FederationClientError::ResponseTooLarge(
                NATIVE_CREATE_RESPONSE_BYTES
            ))
        )),
        "domain_rejection" => assert!(matches!(
            outcome,
            Err(FederationClientError::NativeDomainRejected(_))
        )),
        "unsupported" => assert!(matches!(
            outcome,
            Err(FederationClientError::NativeEndpointUnsupported)
        )),
        _ => assert!(matches!(
            outcome,
            Err(FederationClientError::NativeProtocolRejected(_))
        )),
    }
}

struct NativeCreateTestFixture {
    state: crate::api::AppState,
    caller: InstanceId,
    coordinator_id: String,
    acceptor: tokio_rustls::TlsAcceptor,
    trust: PeerTlsTrust,
}

async fn native_create_test_fixture() -> NativeCreateTestFixture {
    let state = crate::api::handlers::hard_delete_cascade_tests::make_test_state().await;
    let caller = state.db.instance_id().await.unwrap();
    let coordinator = state
        .db
        .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
        .await
        .unwrap();
    let cert_dir = tempfile::tempdir().unwrap();
    let mut config = crate::tls::load_config(&crate::tls::ConfigSource::Auto {
        dir: cert_dir.path().into(),
        hosts: vec!["localhost".into()],
    })
    .unwrap();
    config.server.alpn_protocols = vec![b"http/1.1".to_vec()];
    let trust = PeerTlsTrust::PrivateCa {
        certificate_pem: phoenix_core::domain::instance_identity::PeerCaCertificatePem::parse(
            std::fs::read_to_string(config.ca_cert_path.unwrap()).unwrap(),
        )
        .unwrap(),
    };
    let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config.server));
    NativeCreateTestFixture {
        state,
        caller,
        coordinator_id: coordinator.id,
        acceptor,
        trust,
    }
}

async fn run_native_create_case(fixture: &NativeCreateTestFixture, case: &'static str) {
    let destination = InstanceId::new();
    let request = NativeCreateRequest {
        caller_instance_id: fixture.caller,
        destination_instance_id: destination,
        request_id: uuid::Uuid::new_v4().to_string(),
        cwd: "/destination/project".into(),
        objective: "Implement the change".into(),
        model: None,
        effort: None,
        language: None,
        start: NativeCreateStart::SubmitObjective,
        source: NativeActionSourceCall {
            source_transcript_id: fixture.coordinator_id.clone(),
            ..source_call()
        },
    };
    let responses = native_create_case_responses(case, fixture.caller, destination, &request);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let peer_name = format!("native-{case}");
    let address = format!(
        "https://localhost:{}",
        listener.local_addr().unwrap().port()
    );
    let credential = phoenix_core::domain::instance_identity::PeerBearerCredential::parse(format!(
        "phx_peer_{}{}",
        uuid::Uuid::new_v4().simple(),
        "n".repeat(11)
    ))
    .unwrap();
    fixture
        .state
        .db
        .save_federation_peer_connection(
            destination,
            &peer_name.parse().unwrap(),
            &address.parse().unwrap(),
            &credential,
            &fixture.trust,
        )
        .await
        .unwrap();
    let peer = resolve_saved_peer_for_test(&fixture.state.db, &peer_name)
        .await
        .unwrap();
    let expected_create = serde_json::to_value(&request).unwrap();
    let expected_status = serde_json::json!({
        "caller_instance_id": fixture.caller, "destination_instance_id": destination,
        "request_id": request.request_id
    });
    let coordinator_rotation = (case == "disconnect_after_rotation")
        .then(|| (fixture.state.db.clone(), fixture.coordinator_id.clone()));
    let server = tokio::spawn(serve_native_create_case(
        listener,
        fixture.acceptor.clone(),
        NativeCreateServerCase {
            case,
            expected_create,
            expected_status,
            responses,
            coordinator_rotation,
        },
    ));
    let mut invalid = request.clone();
    invalid.objective.clear();
    assert!(matches!(
        remote_native_create(&fixture.state.db, &peer, &invalid).await,
        Err(FederationClientError::NativeProtocolRejected(_))
    ));
    invalid.objective = "x".repeat(NATIVE_CREATE_REQUEST_BYTES);
    assert!(matches!(
        remote_native_create(&fixture.state.db, &peer, &invalid).await,
        Err(FederationClientError::NativeRequestTooLarge(
            NATIVE_CREATE_REQUEST_BYTES
        ))
    ));
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        remote_native_create(&fixture.state.db, &peer, &request),
    )
    .await
    .unwrap();
    assert_native_create_case_outcome(case, &outcome, &request);
    let listener = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    let listener = listener.into_std().unwrap();
    listener.set_nonblocking(true).unwrap();
    assert!(
        matches!(
            listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ),
        "unexpected extra connection or listener error for {case}"
    );
}

#[tokio::test]
async fn native_create_reconciles_only_uncertain_dispatch_failures_without_retrying_create() {
    let fixture = native_create_test_fixture().await;
    for case in [
        "malformed",
        "oversized",
        "oversized_stream",
        "status_malformed",
        "status_witness_mismatch",
        "status_invalid_product",
        "wrong_caller",
        "wrong_destination",
        "wrong_request",
        "missing_receipt",
        "invalid_product",
        "domain_rejection",
        "remote_failure",
        "unsupported",
        "disconnect",
        "disconnect_after_rotation",
    ] {
        run_native_create_case(&fixture, case).await;
    }
}

#[test]
fn normalized_native_cwd_trims_before_expansion_and_resolution() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let expected = project
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        normalized_native_cwd("~/project", home.path()).unwrap(),
        expected
    );
    assert_eq!(
        normalized_native_cwd("  ~/project  ", home.path()).unwrap(),
        expected
    );
    assert_eq!(
        normalized_native_cwd(&format!("  {}  ", project.to_string_lossy()), home.path()).unwrap(),
        expected
    );
    assert!(normalized_native_cwd("  \t\n ", home.path()).is_err());
}

#[test]
fn native_error_status_and_domain_classification_remain_distinct() {
    for (status, code) in [
        (
            StatusCode::BAD_REQUEST,
            NativeActionErrorCode::InvalidRequest,
        ),
        (
            StatusCode::UNAUTHORIZED,
            NativeActionErrorCode::Unauthorized,
        ),
        (StatusCode::FORBIDDEN, NativeActionErrorCode::Forbidden),
        (StatusCode::NOT_FOUND, NativeActionErrorCode::NotFound),
        (StatusCode::CONFLICT, NativeActionErrorCode::Conflict),
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            NativeActionErrorCode::TooLarge,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            NativeActionErrorCode::Unavailable,
        ),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            NativeActionErrorCode::Internal,
        ),
    ] {
        let failure = NativeActionFailure::from(status);
        assert_eq!(failure.body.error_type, code);
        assert!(native_error_status_matches(status, code));
        assert!(!native_error_status_matches(StatusCode::OK, code));
        let body = serde_json::to_vec(&failure.body).unwrap();
        let error = native_response_error(status, &body);
        if matches!(&error, FederationClientError::NativeIdentityRejected(_)) {
            assert!(matches!(
                code,
                NativeActionErrorCode::Unauthorized | NativeActionErrorCode::Forbidden
            ));
        } else if matches!(&error, FederationClientError::NativeRemoteFailure(_)) {
            assert!(status.is_server_error());
        } else if matches!(&error, FederationClientError::NativeDomainRejected(_)) {
            assert!(
                status.is_client_error()
                    && !matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
            );
        } else {
            panic!("unexpected classification: {error}");
        }
    }
    let body = br#"{"error_type":"not_found","error":"missing"}"#;
    assert!(matches!(
        native_response_error(StatusCode::NOT_FOUND, body),
        FederationClientError::NativeDomainRejected(_)
    ));
    assert!(matches!(
        native_response_error(StatusCode::CONFLICT, body),
        FederationClientError::NativeProtocolRejected(_)
    ));
    assert!(matches!(
        native_response_error(StatusCode::NOT_FOUND, b""),
        FederationClientError::NativeEndpointUnsupported
    ));
    assert!(matches!(
        native_response_error(StatusCode::UNAUTHORIZED, b"unauthorized"),
        FederationClientError::NativeIdentityRejected(_)
    ));
    assert!(matches!(
        native_response_error(StatusCode::SERVICE_UNAVAILABLE, b"unavailable"),
        FederationClientError::NativeRemoteFailure(_)
    ));
    for body in [
        br#"{"error_type":"conflict","error":"conflict","extra":true}"#.as_slice(),
        br#"{"error_type":"invented","error":"conflict"}"#.as_slice(),
        br#"{"error_type":"conflict"}"#.as_slice(),
        br#"{"error_type":"conflict","error":" "}"#.as_slice(),
    ] {
        assert!(matches!(
            native_response_error(StatusCode::CONFLICT, body),
            FederationClientError::NativeProtocolRejected(_)
        ));
    }
}

#[test]
fn native_streamed_response_limit_counts_every_chunk() {
    let mut body = Vec::new();
    append_bounded_response_chunk(&mut body, b"1234", 8).unwrap();
    append_bounded_response_chunk(&mut body, b"5678", 8).unwrap();
    assert!(matches!(
        append_bounded_response_chunk(&mut body, b"9", 8),
        Err(FederationClientError::ResponseTooLarge(8))
    ));
    assert_eq!(body, b"12345678");
}
