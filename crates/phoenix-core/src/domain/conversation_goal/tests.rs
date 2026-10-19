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
    let execution = GoalExecution {
        workflow_id: 1,
        turn_id: 2,
    };
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

#[test]
fn objective_edit_preserves_status_and_fences_admitted_report() {
    let first = initial();
    let mut stale = report(&first);
    let edit = mutation(
        1,
        GoalChange::EditObjective {
            goal_id: first.goal_id.clone(),
            objective: text("prepare and verify"),
        },
    );
    let second = edit.advance(Some(&first)).unwrap();
    assert_eq!(second.version.get(), 2);
    assert_eq!(second.objective_revision.get(), 2);
    assert_eq!(second.state, GoalState::Active);
    assert_eq!(stale.advance(Some(&second)), Err(GoalError::Conflict));
    // Updating a report's CAS field is not enough to rebind its captured authority.
    stale.expected_version = second.version;
    assert_eq!(stale.advance(Some(&second)), Err(GoalError::Conflict));
}

#[test]
fn later_suspend_wins_even_with_unchanged_objective() {
    let first = initial();
    let mut late = report(&first);
    let suspended = mutation(
        1,
        GoalChange::SetDisposition {
            goal_id: first.goal_id.clone(),
            disposition: GoalDisposition::Suspended(text("user Stop")),
        },
    )
    .advance(Some(&first))
    .unwrap();
    assert_eq!(suspended.objective_revision, first.objective_revision);
    late.expected_version = suspended.version;
    assert_eq!(late.advance(Some(&suspended)), Err(GoalError::Conflict));
}

#[test]
fn completion_cannot_be_reopened_by_an_ordinary_edit() {
    let first = initial();
    let complete = report(&first).advance(Some(&first)).unwrap();
    let edit = mutation(
        2,
        GoalChange::EditObjective {
            goal_id: first.goal_id.clone(),
            objective: text("new words"),
        },
    );
    let edited = edit.advance(Some(&complete)).unwrap();
    assert!(matches!(edited.state, GoalState::Complete(_)));
    assert_eq!(
        mutation(
            2,
            GoalChange::Create {
                goal_id: GoalId::parse("replacement").unwrap(),
                objective: text("new commission"),
            }
        )
        .advance(Some(&complete)),
        Err(GoalError::Inapplicable)
    );
}

#[test]
fn cleared_goal_retains_owner_fence_across_replacement() {
    let first = initial();
    let cleared = mutation(
        1,
        GoalChange::SetDisposition {
            goal_id: first.goal_id.clone(),
            disposition: GoalDisposition::Cleared(text("archive unfinished")),
        },
    )
    .advance(Some(&first))
    .unwrap();
    let create = GoalChange::Create {
        goal_id: GoalId::parse("replacement").unwrap(),
        objective: text("new commission"),
    };
    assert_eq!(
        mutation(0, create.clone()).advance(Some(&cleared)),
        Err(GoalError::Conflict)
    );
    let replacement = mutation(2, create).advance(Some(&cleared)).unwrap();
    assert_eq!(replacement.version.get(), 3);
    assert_eq!(replacement.objective_revision.get(), 1);
    assert_ne!(replacement.goal_id, first.goal_id);
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
        source.execution.turn_id += 1;
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
fn blank_and_out_of_range_values_are_not_constructed() {
    assert_eq!(GoalText::parse(" \n\t"), Err(GoalError::Blank));
    assert_eq!(GoalVersion::parse(-1), Err(GoalError::Range));
    assert_eq!(GoalTimestampUs::parse(-1), Err(GoalError::Range));
    let mut first = initial();
    first.version = GoalVersion::parse(i64::MAX).unwrap();
    let overflow = mutation(
        i64::MAX,
        GoalChange::EditObjective {
            goal_id: first.goal_id.clone(),
            objective: text("edit"),
        },
    );
    assert_eq!(overflow.advance(Some(&first)), Err(GoalError::Range));
}

#[test]
fn fabricated_fresh_report_cannot_release_suspended_or_completed_state() {
    let first = initial();
    for disposition in [
        GoalDisposition::Suspended(text("user Stop")),
        GoalDisposition::Blocked(text("input needed")),
        GoalDisposition::Complete(text("done")),
    ] {
        let inactive = mutation(
            1,
            GoalChange::SetDisposition {
                goal_id: first.goal_id.clone(),
                disposition,
            },
        )
        .advance(Some(&first))
        .unwrap();
        let forged = report(&inactive);
        assert_eq!(
            forged.advance(Some(&inactive)),
            Err(GoalError::Inapplicable)
        );
    }
}
