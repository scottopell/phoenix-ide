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
