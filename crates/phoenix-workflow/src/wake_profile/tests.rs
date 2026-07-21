use crate::wake_profile::*;
use crate::*;

#[test]
fn registration_plan_declares_reclaimable_observation_effect() {
    let intent = ObserveHandleIntent {
        contract_id: "contract".into(),
        resource: WakeResourceIdentity::Subagent(SubagentResourceIdentity {
            child_conversation_id: "child".into(),
        }),
        expires_at: Timestamp(5),
    };
    let snapshot = WakeRegistrationSnapshot {
        contract_id: "contract".into(),
        resource: intent.resource.clone(),
        registered: false,
        terminal: None,
        runtime_availability: RuntimeAvailability::Idle,
    };
    let plan = registration_plan(
        snapshot.clone(),
        WakeRegistrationEvent::Registered,
        intent.clone(),
    );
    assert_eq!(plan.snapshot, snapshot);
    assert_eq!(plan.effects.len(), 1);
    assert_eq!(
        plan.effects[0].capability,
        ExecutionCapability::ReclaimableObservation
    );
    assert_eq!(plan.effects[0].intent, intent);
}

#[test]
fn acceptance_profile_exposes_wake_codec_support() {
    let acceptance = acceptance_profile();
    assert!(acceptance.supported_codecs.supports(&snapshot_codec()));
    assert!(acceptance.supported_codecs.supports(&event_codec()));
    assert!(acceptance.supported_codecs.supports(&terminal_codec()));
    assert!(acceptance.runtime_acceptance_enabled());
    assert!(!acceptance.external_acceptance_enabled());
}

#[test]
fn barrier_event_and_manual_choices_round_trip_helpers() {
    let receipt = WakeRegistrationReceipt {
        contract_id: "contract".into(),
        resource: WakeResourceIdentity::Subagent(SubagentResourceIdentity {
            child_conversation_id: "child".into(),
        }),
        expires_at: Timestamp(9),
        registering_tool_use_id: "tool-use".into(),
    };
    let events = barrier_events(receipt.clone());
    assert_eq!(
        events[&REGISTRATION_BARRIER_ID],
        registration_barrier_event(receipt.clone())
    );

    let terminal = WakeTerminalPayload::Expired {
        contract_id: receipt.contract_id.clone(),
        resource: receipt.resource.clone(),
        resolved_at: Timestamp(10),
    };
    let choices = manual_choices(terminal.clone());
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].kind, ManualChoiceKind::AcceptAsTerminal);
    assert_eq!(choices[0].receipt, terminal);
}

#[test]
fn cancellation_request_invalidates_registration_effect() {
    let snapshot = WakeRegistrationSnapshot {
        contract_id: "contract".into(),
        resource: WakeResourceIdentity::Subagent(SubagentResourceIdentity {
            child_conversation_id: "child".into(),
        }),
        registered: true,
        terminal: None,
        runtime_availability: RuntimeAvailability::Pending,
    };
    let request = cancellation_request(
        Version(3),
        snapshot.clone(),
        registration_plan(
            snapshot,
            WakeRegistrationEvent::CancelRequested,
            ObserveHandleIntent {
                contract_id: "contract".into(),
                resource: WakeResourceIdentity::Subagent(SubagentResourceIdentity {
                    child_conversation_id: "child".into(),
                }),
                expires_at: Timestamp(12),
            },
        ),
    );
    assert_eq!(request.expected_workflow_version, Version(3));
    assert_eq!(
        request.invalidations,
        vec![EffectInvalidationDecl {
            effect_id: REGISTRATION_EFFECT_ID
        }]
    );
}

#[test]
fn work_scope_identity_decodes_legacy_object_payload() {
    let identity: WorkScopeIdentity =
        serde_json::from_str(r#"{"kind":"Worktree","stable_key":"worktree:/tmp/project"}"#)
            .expect("legacy identity");
    assert_eq!(identity.as_str(), "worktree:/tmp/project");
}

#[test]
fn tmux_identity_without_work_scope_remains_decodable() {
    let identity: TmuxResourceIdentity = serde_json::from_str(
        r#"{"server_token":"server","window_id":"@1","completion_policy":"KeepOpen"}"#,
    )
    .expect("legacy tmux identity");
    assert_eq!(identity.work_scope.as_str(), "legacy-unscoped");
}
