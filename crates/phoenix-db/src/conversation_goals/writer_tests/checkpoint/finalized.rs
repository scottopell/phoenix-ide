use super::*;

fn unprepared_report_cases() -> [(ToolResult, bool); 4] {
    [
        (
            ToolResult::success("checkpoint-call".into(), "Tool execution cancelled".into()),
            true,
        ),
        (
            ToolResult {
                tool_use_id: "checkpoint-call".into(),
                outcome: ToolOutcome::TrustedInstructions {
                    output: "Tool execution cancelled".into(),
                },
                duration_ms: None,
            },
            true,
        ),
        (
            ToolResult::error(
                "checkpoint-call".into(),
                accepted_report_result().output().into(),
            ),
            false,
        ),
        (
            ToolResult::cancelled("checkpoint-call".into(), accepted_report_result().output()),
            false,
        ),
    ]
}

fn scoped_evidence(mut evidence: serde_json::Value) -> serde_json::Value {
    let fields = evidence.as_object_mut().unwrap();
    fields.remove("facts");
    fields.remove("obligations");
    evidence
}

#[tokio::test]
async fn unprepared_report_guard_uses_outcome_not_output_across_db_entrypoints() {
    let db = fixture().await;
    let mut retained_cases = Vec::new();
    for entrypoint in ["plain", "state", "replay", "terminal", "pending"] {
        for (index, (mut result, rejected)) in unprepared_report_cases().into_iter().enumerate() {
            let case = format!("checkpoint-{entrypoint}-{index}");
            let (owner, mut round) = report_fixture_in(&db, &case).await;
            if let ToolOutcome::Error { output, .. } | ToolOutcome::Cancelled { message: output } =
                &mut result.outcome
            {
                *output = format!("Goal {case}-goal recorded complete at owner version 2.");
            }
            let finalized = vec![finalized_result(&round.assistant, &result)];
            round.results = vec![result.into()];
            let obligation = terminal::obligation(&db, &round).await;
            let before = evidence(&db, &owner, &round).await;
            let mut after_first = None;
            for _ in 0..2 {
                let outcome = match entrypoint {
                    "plain" => {
                        db.persist_tool_round(
                            &round.assistant.conversation_id,
                            &round.assistant,
                            &finalized,
                        )
                        .await
                    }
                    "state" => {
                        db.persist_tool_round_and_state(
                            &round.assistant.conversation_id,
                            &round.assistant,
                            &finalized,
                            &round.state,
                            round.timestamp,
                        )
                        .await
                    }
                    "replay" => {
                        db.persist_tool_round_state_and_replay(
                            &round.assistant.conversation_id,
                            &round.assistant,
                            &finalized,
                            &round.state,
                            round.timestamp,
                            &round.replay(),
                        )
                        .await
                    }
                    "terminal" => {
                        db.persist_tool_round_with_terminal_obligation(
                            &round.assistant.conversation_id,
                            &round.assistant,
                            &finalized,
                            &obligation,
                        )
                        .await
                    }
                    "pending" => round.commit(&db).await.map(|_| ()),
                    _ => unreachable!(),
                };
                let after = evidence(&db, &owner, &round).await;
                if rejected {
                    let error = outcome.unwrap_err();
                    assert!(
                        error.to_string().contains("goal report"),
                        "{entrypoint}: {error}"
                    );
                    assert_eq!(after, before, "{entrypoint}");
                } else {
                    outcome.unwrap();
                    for field in ["goal", "history", "facts"] {
                        assert_eq!(after[field], before[field], "{entrypoint}: {field}");
                    }
                    assert_eq!(
                        after["messages"].as_array().unwrap().len(),
                        before["messages"].as_array().unwrap().len() + 2,
                        "{entrypoint}"
                    );
                    if let Some(first) = &after_first {
                        assert_eq!(&after, first, "exact retry: {entrypoint}");
                    }
                    after_first = Some(after);
                }
            }
            retained_cases.push((
                case,
                owner,
                round,
                scoped_evidence(after_first.unwrap_or(before)),
            ));
        }
    }
    for (case, owner, round, expected) in retained_cases {
        assert_eq!(
            scoped_evidence(evidence(&db, &owner, &round).await),
            expected,
            "later cases changed {case}"
        );
    }
}

#[tokio::test]
async fn finalized_result_metadata_and_pairing_mismatches_reject_without_writes() {
    for mismatch in ["assistant", "conversation", "tool", "missing", "duplicate"] {
        let (db, owner, mut round) = report_fixture().await;
        terminal::ordinary_round(&mut round);
        let mut result = ordinary_result();
        if mismatch == "tool" {
            result.tool_use_id = "unpaired-call".into();
        }
        let mut finalized = vec![FinalizedToolResult::new(
            if mismatch == "assistant" {
                "other-assistant"
            } else {
                &round.assistant.message_id
            },
            if mismatch == "conversation" {
                "other-transcript"
            } else {
                &round.assistant.conversation_id
            },
            round.assistant.sequence_id + 1,
            round.assistant.created_at,
            &result,
        )];
        if mismatch == "missing" {
            finalized.clear();
        } else if mismatch == "duplicate" {
            let MessageContent::Agent(blocks) = &mut round.assistant.content else {
                unreachable!()
            };
            blocks.push(ContentBlock::ToolUse {
                id: "other-call".into(),
                name: "search".into(),
                input: serde_json::json!({}),
            });
            finalized.push(finalized[0].clone());
        }
        let before = evidence(&db, &owner, &round).await;
        assert!(
            db.persist_tool_round_state_and_replay(
                &round.assistant.conversation_id,
                &round.assistant,
                &finalized,
                &round.state,
                round.timestamp,
                &round.replay(),
            )
            .await
            .is_err(),
            "{mismatch}"
        );
        assert_eq!(evidence(&db, &owner, &round).await, before, "{mismatch}");
    }
}
