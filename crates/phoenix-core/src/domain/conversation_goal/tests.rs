use super::*;

fn text(value: &str) -> GoalText {
    GoalText::parse(value).unwrap()
}

fn user() -> GoalActor {
    GoalActor::UserApi {
        transcript_id: text("transcript"),
        message_id: text("instruction"),
    }
}

fn mutation(expected: i64, change: GoalChange) -> GoalMutation {
    GoalMutation {
        owner: ProductConversationId::parse("owner").unwrap(),
        expected_version: GoalVersion::parse(expected).unwrap(),
        action_id: text(&format!("action-{expected}")),
        attribution: GoalAttribution {
            requested_by: user(),
            applied_by: user(),
        },
        change,
    }
}

fn initial() -> GoalSnapshot {
    mutation(
        0,
        GoalChange::Create {
            goal_id: GoalId::parse("goal").unwrap(),
            objective: text("prepare only"),
        },
    )
    .advance(None)
    .unwrap()
}

fn report(current: &GoalSnapshot) -> GoalMutation {
    let execution = GoalExecution::parse(1, 2).unwrap();
    let actor = GoalActor::OwningAgent(GoalAgentSource {
        product_conversation_id: current.owner.clone(),
        transcript_id: text("transcript"),
        execution,
        message_id: text("tool-message"),
        tool_use_id: text("report-call"),
    });
    let mut report = mutation(
        current.version.get(),
        GoalChange::Report {
            binding: GoalExecutionBinding {
                owner: current.owner.clone(),
                execution,
                observation: GoalAdmissionObservation::Goal(current.fence()),
            },
            disposition: GoalDisposition::Complete(text("prepared, not deployed")),
        },
    );
    report.attribution = GoalAttribution {
        requested_by: actor.clone(),
        applied_by: actor,
    };
    report
}

fn set(current: &GoalSnapshot, disposition: GoalDisposition) -> GoalMutation {
    mutation(
        current.version.get(),
        GoalChange::SetDisposition {
            goal_id: current.goal_id.clone(),
            disposition,
        },
    )
}

#[test]
fn saved_goal_fence_rejects_zero_through_constructor_and_nested_binding_wire() {
    let owner = ProductConversationId::parse("owner").unwrap();
    let goal_id = GoalId::parse("saved").unwrap();
    assert_eq!(
        GoalFence::new(owner.clone(), goal_id.clone(), GoalVersion::INITIAL),
        Err(GoalError::Range)
    );
    for version in [1, 2, i64::MAX] {
        let version = GoalVersion::parse(version).unwrap();
        let fence = GoalFence::new(owner.clone(), goal_id.clone(), version).unwrap();
        assert_eq!(fence.owner(), &owner);
        assert_eq!(fence.goal_id(), &goal_id);
        assert_eq!(fence.version(), version);
        let wire = serde_json::to_value(&fence).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"owner": owner, "goal_id": "saved", "version": version.get()})
        );
        assert_eq!(
            serde_json::from_value::<GoalFence>(wire.clone()).unwrap(),
            fence
        );
        let mut zero = wire;
        zero["version"] = serde_json::json!(0);
        assert!(serde_json::from_value::<GoalFence>(zero).is_err());

        let binding = GoalExecutionBinding {
            owner: owner.clone(),
            execution: GoalExecution::parse(1, 2).unwrap(),
            observation: GoalAdmissionObservation::Goal(fence),
        };
        let mut wire = serde_json::to_value(&binding).unwrap();
        assert_eq!(
            serde_json::from_value::<GoalExecutionBinding>(wire.clone()).unwrap(),
            binding
        );
        wire["observation"]["Goal"]["version"] = serde_json::json!(0);
        assert!(serde_json::from_value::<GoalExecutionBinding>(wire).is_err());
    }
}

#[test]
fn no_goal_binding_preserves_initial_zero_observation() {
    let owner = ProductConversationId::parse("owner").unwrap();
    let binding = GoalExecutionBinding {
        owner,
        execution: GoalExecution::parse(1, 2).unwrap(),
        observation: GoalAdmissionObservation::NoGoal {
            observed_version: GoalVersion::INITIAL,
        },
    };
    let wire = serde_json::to_value(&binding).unwrap();
    assert_eq!(
        wire["observation"],
        serde_json::json!({"NoGoal": {"observed_version": 0}})
    );
    assert_eq!(
        serde_json::from_value::<GoalExecutionBinding>(wire).unwrap(),
        binding
    );
}

#[test]
fn saved_snapshot_requires_committed_version_in_constructor_and_wire() {
    let saved = initial();
    let mut encoded = serde_json::to_value(&saved).unwrap();
    assert_eq!(
        serde_json::from_value::<GoalSnapshot>(encoded.clone()).unwrap(),
        saved
    );
    assert_eq!(saved.version().get(), 1);
    assert_eq!(saved.owner().as_str(), "owner");
    assert_eq!(saved.goal_id().as_str(), "goal");
    assert_eq!(saved.objective().as_str(), "prepare only");
    assert_eq!(saved.state(), &GoalState::Underway);
    assert_eq!(
        GoalSnapshot::new(
            saved.owner().clone(),
            saved.goal_id().clone(),
            GoalVersion::INITIAL,
            saved.objective().clone(),
            saved.state().clone()
        ),
        Err(GoalError::Range)
    );
    encoded["version"] = serde_json::json!(0);
    assert!(serde_json::from_value::<GoalSnapshot>(encoded).is_err());
}

#[test]
fn user_control_requires_exact_pair_and_applicable_command() {
    let first = initial();
    let principal = uuid::Uuid::new_v4();
    let request = uuid::Uuid::new_v4();
    let control = GoalActor::UserControl {
        principal_id: principal,
        request_id: request,
        command: GoalUserCommandKind::State,
    };
    let change = GoalChange::SetDisposition {
        goal_id: first.goal_id().clone(),
        disposition: GoalDisposition::Blocked(text("waiting")),
    };
    let mut command = mutation(1, change);
    command.attribution = GoalAttribution {
        requested_by: control.clone(),
        applied_by: control.clone(),
    };
    assert_eq!(
        command.advance(Some(&first)).unwrap().state(),
        &GoalState::Blocked(text("waiting"))
    );
    for mismatch in [
        GoalActor::UserControl {
            principal_id: uuid::Uuid::new_v4(),
            request_id: request,
            command: GoalUserCommandKind::State,
        },
        GoalActor::UserControl {
            principal_id: principal,
            request_id: uuid::Uuid::new_v4(),
            command: GoalUserCommandKind::State,
        },
        GoalActor::UserControl {
            principal_id: principal,
            request_id: request,
            command: GoalUserCommandKind::Create,
        },
        user(),
    ] {
        command.attribution.applied_by = mismatch.clone();
        assert_eq!(command.advance(Some(&first)), Err(GoalError::Inapplicable));
        command.attribution = GoalAttribution {
            requested_by: mismatch,
            applied_by: control.clone(),
        };
        assert_eq!(command.advance(Some(&first)), Err(GoalError::Inapplicable));
        command.attribution.requested_by = control.clone();
    }
    command.attribution.applied_by = control;
    command.change = GoalChange::Create {
        goal_id: GoalId::parse("other").unwrap(),
        objective: text("new"),
    };
    assert_eq!(command.advance(Some(&first)), Err(GoalError::Inapplicable));
}

#[test]
fn history_reconstruction_checks_local_transition_without_live_authority() {
    let create = mutation(
        0,
        GoalChange::Create {
            goal_id: GoalId::parse("goal").unwrap(),
            objective: text("prepare only"),
        },
    );
    let initial = create.advance(None).unwrap();
    let timestamp = GoalTimestampUs::parse(123).unwrap();
    let receipt = GoalHistoryEntry::new(initial.clone(), create.clone(), timestamp).unwrap();
    assert_eq!(receipt.snapshot(), &initial);
    assert_eq!(receipt.mutation(), &create);
    assert_eq!(receipt.recorded_at_us(), timestamp);
    let encoded = serde_json::to_value(&receipt).unwrap();
    assert_eq!(
        serde_json::from_value::<GoalHistoryEntry>(encoded.clone()).unwrap(),
        receipt
    );
    // A retained receipt is locally consistent even when the current goal has since changed.
    let retired = set(&initial, GoalDisposition::Complete(text("finished")))
        .advance(Some(&initial))
        .unwrap();
    assert_eq!(
        GoalHistoryEntry::new(initial.clone(), create.clone(), timestamp).unwrap(),
        receipt
    );
    assert_ne!(retired, initial);
    for (path, value) in [
        ("owner", serde_json::json!("other")),
        ("version", serde_json::json!(2)),
        ("goal_id", serde_json::json!("wrong")),
        ("objective", serde_json::json!("changed")),
        ("state", serde_json::json!({"Blocked":"waiting"})),
    ] {
        let mut invalid = encoded.clone();
        invalid["snapshot"][path] = value;
        assert!(
            serde_json::from_value::<GoalHistoryEntry>(invalid).is_err(),
            "{path}"
        );
    }
    let mut invalid = encoded;
    invalid["mutation"]["expected_version"] = serde_json::json!(1);
    assert!(serde_json::from_value::<GoalHistoryEntry>(invalid).is_err());
    let state_command = set(&initial, GoalDisposition::Blocked(text("waiting")));
    let blocked = state_command.advance(Some(&initial)).unwrap();
    let state_receipt = GoalHistoryEntry::new(blocked.clone(), state_command, timestamp).unwrap();
    assert_eq!(
        serde_json::from_value::<GoalHistoryEntry>(serde_json::to_value(&state_receipt).unwrap())
            .unwrap(),
        state_receipt
    );
    let reported = report(&initial);
    let result = reported.advance(Some(&initial)).unwrap();
    let report_receipt = GoalHistoryEntry::new(result, reported, timestamp).unwrap();
    assert_eq!(
        serde_json::from_value::<GoalHistoryEntry>(serde_json::to_value(&report_receipt).unwrap())
            .unwrap(),
        report_receipt
    );
}

#[test]
fn history_reconstruction_rejects_impossible_local_attribution() {
    fn rejects(snapshot: &GoalSnapshot, mutation: &GoalMutation) {
        let recorded_at_us = GoalTimestampUs::parse(123).unwrap();
        assert!(GoalHistoryEntry::new(snapshot.clone(), mutation.clone(), recorded_at_us).is_err());
        let wire = serde_json::json!({
            "snapshot": snapshot,
            "mutation": mutation,
            "recorded_at_us": recorded_at_us,
        });
        assert!(serde_json::from_value::<GoalHistoryEntry>(wire).is_err());
    }

    let snapshot = initial();
    let mut create = mutation(
        0,
        GoalChange::Create {
            goal_id: snapshot.goal_id().clone(),
            objective: snapshot.objective().clone(),
        },
    );
    let principal_id = uuid::Uuid::new_v4();
    let request_id = uuid::Uuid::new_v4();
    let control = GoalActor::UserControl {
        principal_id,
        request_id,
        command: GoalUserCommandKind::Create,
    };
    create.attribution = GoalAttribution {
        requested_by: control.clone(),
        applied_by: control.clone(),
    };
    let timestamp = GoalTimestampUs::parse(123).unwrap();
    assert!(GoalHistoryEntry::new(snapshot.clone(), create.clone(), timestamp).is_ok());
    for applied in [
        GoalActor::UserControl {
            principal_id: uuid::Uuid::new_v4(),
            request_id,
            command: GoalUserCommandKind::Create,
        },
        GoalActor::UserControl {
            principal_id,
            request_id: uuid::Uuid::new_v4(),
            command: GoalUserCommandKind::Create,
        },
        user(),
    ] {
        let mut invalid = create.clone();
        invalid.attribution.applied_by = applied;
        rejects(&snapshot, &invalid);
    }
    let mut wrong_command = create;
    let state_control = GoalActor::UserControl {
        principal_id,
        request_id,
        command: GoalUserCommandKind::State,
    };
    wrong_command.attribution = GoalAttribution {
        requested_by: state_control.clone(),
        applied_by: state_control,
    };
    rejects(&snapshot, &wrong_command);

    let report = report(&snapshot);
    let result = report.advance(Some(&snapshot)).unwrap();
    let mut wrong_actor = report.clone();
    wrong_actor.attribution.requested_by = user();
    rejects(&result, &wrong_actor);
    let mut wrong_execution = report.clone();
    if let GoalActor::OwningAgent(source) = &mut wrong_execution.attribution.requested_by {
        source.execution = GoalExecution::parse(1, 3).unwrap();
    }
    rejects(&result, &wrong_execution);
    let mut wrong_owner = report.clone();
    if let GoalActor::OwningAgent(source) = &mut wrong_owner.attribution.requested_by {
        source.product_conversation_id = ProductConversationId::parse("other").unwrap();
    }
    rejects(&result, &wrong_owner);
    let mut unbound = set(&snapshot, GoalDisposition::Complete(text("done")));
    unbound.attribution.requested_by = report.attribution.requested_by;
    rejects(&result, &unbound);
}

#[test]
fn replacement_changes_identity_and_fences_admitted_report_even_with_same_text() {
    let first = initial();
    let mut stale = report(&first);
    let second = mutation(
        1,
        GoalChange::Create {
            goal_id: GoalId::parse("replacement").unwrap(),
            objective: first.objective.clone(),
        },
    )
    .advance(Some(&first))
    .unwrap();
    assert_eq!(second.version.get(), 2);
    assert_eq!(second.state, GoalState::Underway);
    assert_ne!(second.goal_id, first.goal_id);
    assert_eq!(stale.advance(Some(&second)), Err(GoalError::Conflict));
    stale.expected_version = second.version;
    assert_eq!(stale.advance(Some(&second)), Err(GoalError::Conflict));
}

#[test]
fn later_suspend_fences_old_report_without_claiming_runtime_stop_protection() {
    let first = initial();
    let mut late = report(&first);
    let suspended = set(&first, GoalDisposition::Suspended(text("recorded stop")))
        .advance(Some(&first))
        .unwrap();
    late.expected_version = suspended.version;
    assert_eq!(late.advance(Some(&suspended)), Err(GoalError::Inapplicable));
    assert_eq!(
        set(&suspended, GoalDisposition::Underway).advance(Some(&suspended)),
        Err(GoalError::Inapplicable)
    );
}

#[test]
fn complete_and_exhausted_are_distinct_terminal_outcomes_and_cannot_reopen() {
    let first = initial();
    let complete = report(&first).advance(Some(&first)).unwrap();
    let exhausted = set(
        &first,
        GoalDisposition::Exhausted(text("limit reached; not success")),
    )
    .advance(Some(&first))
    .unwrap();
    assert_ne!(complete.state, exhausted.state);
    for terminal in [complete, exhausted] {
        for disposition in [
            GoalDisposition::Underway,
            GoalDisposition::Blocked(text("retry")),
            GoalDisposition::Complete(text("reworded")),
            GoalDisposition::Exhausted(text("reworded")),
        ] {
            assert_eq!(
                set(&terminal, disposition).advance(Some(&terminal)),
                Err(GoalError::Inapplicable)
            );
        }
        let next = mutation(
            terminal.version.get(),
            GoalChange::Create {
                goal_id: GoalId::parse("new-goal").unwrap(),
                objective: text("new deliberate objective"),
            },
        )
        .advance(Some(&terminal))
        .unwrap();
        assert_eq!(next.version.get(), 3);
        assert_ne!(next.goal_id, terminal.goal_id);
    }
}

#[test]
fn objective_cannot_change_under_current_identity_and_supersession_needs_observed_version() {
    let first = initial();
    let reuse = GoalChange::Create {
        goal_id: first.goal_id.clone(),
        objective: text("different objective"),
    };
    assert_eq!(
        mutation(1, reuse).advance(Some(&first)),
        Err(GoalError::Inapplicable)
    );
    let create = GoalChange::Create {
        goal_id: GoalId::parse("replacement").unwrap(),
        objective: text("new commission"),
    };
    assert_eq!(
        mutation(0, create.clone()).advance(Some(&first)),
        Err(GoalError::Conflict)
    );
    assert_eq!(
        mutation(1, create)
            .advance(Some(&first))
            .unwrap()
            .version
            .get(),
        2
    );
}

#[test]
fn blocked_can_return_underway_with_same_identity_and_objective() {
    let first = initial();
    let blocked = set(
        &first,
        GoalDisposition::Blocked(text("question awaiting answer")),
    )
    .advance(Some(&first))
    .unwrap();
    let underway = set(&blocked, GoalDisposition::Underway)
        .advance(Some(&blocked))
        .unwrap();
    assert_eq!(underway.goal_id, first.goal_id);
    assert_eq!(underway.objective, first.objective);
    assert_eq!(underway.version.get(), 3);
    assert_eq!(underway.state, GoalState::Underway);
    assert_eq!(
        set(&first, GoalDisposition::Underway).advance(Some(&first)),
        Err(GoalError::Inapplicable)
    );
}

#[test]
fn agent_cannot_bypass_report_binding_with_state_command() {
    let first = initial();
    let mut bypass = report(&first);
    bypass.change = GoalChange::SetDisposition {
        goal_id: first.goal_id.clone(),
        disposition: GoalDisposition::Complete(text("done")),
    };
    assert_eq!(
        bypass.advance(Some(&first)),
        Err(GoalError::ReportBindingRequired)
    );
}

#[test]
fn wrong_owner_or_execution_cannot_report() {
    let first = initial();
    let mut wrong = report(&first);
    if let GoalActor::OwningAgent(source) = &mut wrong.attribution.requested_by {
        source.execution = GoalExecution::parse(1, 3).unwrap();
    }
    assert_eq!(
        wrong.advance(Some(&first)),
        Err(GoalError::ReportBindingRequired)
    );
    let mut wrong = report(&first);
    wrong.owner = ProductConversationId::parse("other").unwrap();
    assert_eq!(wrong.advance(Some(&first)), Err(GoalError::Conflict));
}

#[test]
fn blank_out_of_range_and_version_overflow_are_rejected() {
    assert_eq!(GoalText::parse(" \n\t"), Err(GoalError::Blank));
    assert_eq!(GoalVersion::parse(-1), Err(GoalError::Range));
    assert_eq!(GoalTimestampUs::parse(-1), Err(GoalError::Range));
    assert_eq!(GoalExecution::parse(0, 1), Err(GoalError::Range));
    assert_eq!(GoalExecution::parse(1, -1), Err(GoalError::Range));
    let mut first = initial();
    first.version = GoalVersion::parse(i64::MAX).unwrap();
    assert_eq!(
        set(&first, GoalDisposition::Blocked(text("input needed"))).advance(Some(&first)),
        Err(GoalError::Range)
    );
    assert_eq!(
        mutation(
            i64::MAX,
            GoalChange::Create {
                goal_id: GoalId::parse("new-goal").unwrap(),
                objective: text("new objective"),
            }
        )
        .advance(Some(&first)),
        Err(GoalError::Range)
    );
}

#[test]
fn fabricated_fresh_report_cannot_release_suspended_blocked_or_terminal_state() {
    let first = initial();
    for disposition in [
        GoalDisposition::Suspended(text("recorded Stop")),
        GoalDisposition::Blocked(text("input needed")),
        GoalDisposition::Complete(text("done")),
        GoalDisposition::Exhausted(text("limit reached")),
    ] {
        let inactive = set(&first, disposition).advance(Some(&first)).unwrap();
        assert_eq!(
            report(&inactive).advance(Some(&inactive)),
            Err(GoalError::Inapplicable)
        );
    }
}

#[test]
fn checkpoint_payload_roundtrips_exact_mutation_actor_and_fence() {
    let snapshot = initial();
    let mut commands = vec![
        mutation(
            0,
            GoalChange::Create {
                goal_id: GoalId::parse("next-goal").unwrap(),
                objective: text("  exact objective — no deployment  "),
            },
        ),
        report(&snapshot),
    ];
    for disposition in [
        GoalDisposition::Underway,
        GoalDisposition::Blocked(text("restore access")),
        GoalDisposition::Suspended(text("stopped")),
        GoalDisposition::Complete(text("done")),
        GoalDisposition::Exhausted(text("limit")),
    ] {
        commands.push(set(&snapshot, disposition));
    }
    for command in commands {
        let bytes = serde_json::to_vec(&command).unwrap();
        let restored: GoalMutation = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored, command);
        assert_eq!(serde_json::to_vec(&restored).unwrap(), bytes);
    }
    let inactive = GoalExecutionBinding {
        owner: snapshot.owner.clone(),
        execution: GoalExecution::parse(41, 57).unwrap(),
        observation: GoalAdmissionObservation::NoGoal {
            observed_version: snapshot.version,
        },
    };
    let restored: GoalExecutionBinding =
        serde_json::from_value(serde_json::to_value(&inactive).unwrap()).unwrap();
    assert_eq!(restored, inactive);
}

#[test]
fn checkpoint_deserialization_preserves_checked_scalar_invariants_and_requires_fields() {
    for value in ["", " ", "\n\t"] {
        let encoded = serde_json::to_string(value).unwrap();
        assert!(serde_json::from_str::<GoalText>(&encoded).is_err());
        assert!(serde_json::from_str::<GoalId>(&encoded).is_err());
    }
    for value in [-1, i64::MIN] {
        assert!(serde_json::from_value::<GoalVersion>(serde_json::json!(value)).is_err());
        assert!(serde_json::from_value::<GoalTimestampUs>(serde_json::json!(value)).is_err());
    }
    for (workflow, turn) in [(0, 1), (1, 0), (-1, 1), (1, -1)] {
        assert!(serde_json::from_value::<GoalExecution>(
            serde_json::json!({"workflow_id":workflow,"turn_id":turn})
        )
        .is_err());
    }
    let serialized = serde_json::to_value(report(&initial())).unwrap();
    for field in [
        "owner",
        "expected_version",
        "action_id",
        "attribution",
        "change",
    ] {
        let mut missing = serialized.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<GoalMutation>(missing).is_err(),
            "missing {field} must not fabricate a pending command"
        );
    }
}
