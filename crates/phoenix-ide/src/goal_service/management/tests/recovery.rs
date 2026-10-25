use super::*;
use crate::runtime::traits::{
    terminal_obligation, ActiveDirectTurnSettlement, ActiveDirectTurnTerminal,
};
use phoenix_core::domain::{
    db_schema::{ForkProposal, ForkProposalStatus, ToolResult},
    provider_replay::{
        AnthropicPrivateBlock, AnthropicResponseIdentity, AnthropicResponseSet, ContentIndex,
        ProviderReplayUpdate,
    },
    responses_replay::ResponsesResponseSet,
};

#[derive(Clone, Copy, Debug)]
enum Provider {
    Anthropic,
    Responses,
}

#[derive(Clone, Copy, Debug)]
enum Owner {
    Runtime,
    Fork,
    Terminal,
}

fn replay(provider: Provider, assistant: &Message) -> ProviderReplayUpdate {
    let MessageContent::Agent(blocks) = &assistant.content else {
        panic!("canonical assistant")
    };
    match provider {
        Provider::Anthropic => ProviderReplayUpdate::Anthropic(
            AnthropicResponseSet::with_public_content(
                AnthropicResponseIdentity {
                    response_id: "management-recovery-response".into(),
                    model: "claude-opus-5-5".into(),
                },
                blocks.clone(),
                vec![AnthropicPrivateBlock::Thinking {
                    index: ContentIndex(0),
                    thinking: "private checkpoint reasoning".into(),
                    signature: "opaque-recovery-signature".into(),
                }],
            )
            .unwrap()
            .with_owner_message_id(assistant.message_id.clone()),
        ),
        Provider::Responses => {
            let mut output_items = vec![serde_json::json!({
                "type":"reasoning", "id":"management-reasoning", "summary":[],
                "encrypted_content":"opaque-management-reasoning"
            })];
            output_items.extend(blocks.iter().filter_map(|block| {
                let ContentBlock::ToolUse { id, name, input } = block else {
                    return None;
                };
                Some(serde_json::json!({
                    "type":"function_call", "id":format!("fc-{id}"), "call_id":id,
                    "name":name, "arguments":serde_json::to_string(input).unwrap()
                }))
            }));
            ProviderReplayUpdate::Responses(ResponsesResponseSet {
                response_id: "management-recovery-response".into(),
                model: "gpt-6-sol".into(),
                owner_message_id: assistant.message_id.clone(),
                public_content: blocks.clone(),
                output_items,
            })
        }
    }
}

fn proposal(assistant: &mut Message, results: &mut Vec<ToolRoundResult>) -> ForkProposal {
    let MessageContent::Agent(blocks) = &mut assistant.content else {
        panic!("canonical assistant")
    };
    blocks.push(ContentBlock::ToolUse {
        id: "management-proposal-call".into(),
        name: "propose_task".into(),
        input: serde_json::json!({"task_file":"tasks/management-follow-up.md"}),
    });
    results.push(
        ToolResult::success("management-proposal-call".into(), "Task proposed".into()).into(),
    );
    ForkProposal {
        id: "management-proposal".into(),
        origin_conversation_id: assistant.conversation_id.clone(),
        task_file: "tasks/management-follow-up.md".into(),
        title: "Management follow-up".into(),
        priority: "p2".into(),
        body: "Retained proposal body".into(),
        status: ForkProposalStatus::Pending,
        fork_conversation_id: None,
        refinement_conversation_id: None,
        created_at: assistant.created_at,
        resolved_at: None,
    }
}

async fn snapshot(fixture: &Fixture) -> serde_json::Value {
    let db = fixture.runtime.db();
    let source = db.get_conversation(&fixture.source).await.unwrap();
    let turn: (i64, Option<String>, i64) = sqlx::query_as(
        "SELECT generation,terminal_kind,owns_conversation FROM durable_turns WHERE turn_id=?",
    )
    .bind(i64::try_from(fixture.active.turn_id.0).unwrap())
    .fetch_one(db.pool())
    .await
    .unwrap();
    serde_json::json!({
        "state":source.state, "state_updated_at":source.state_updated_at, "turn":turn,
        "messages":db.get_messages(&fixture.source).await.unwrap(),
        "history":db.conversation_goal_history(&fixture.target, GoalVersion::INITIAL).await.unwrap(),
        "proposals":db.list_fork_proposals_for_origin(&fixture.source).await.unwrap(),
        "anthropic":db.load_provider_replay_state(&fixture.source).await.unwrap(),
        "responses":db.load_responses_replay_state(&fixture.source).await.unwrap(),
    })
}

async fn assert_replay(fixture: &Fixture, expected: &ProviderReplayUpdate, terminal: bool) {
    let db = fixture.runtime.db();
    let anthropic = db
        .load_provider_replay_state(&fixture.source)
        .await
        .unwrap();
    let responses = db
        .load_responses_replay_state(&fixture.source)
        .await
        .unwrap();
    if terminal {
        assert!(anthropic.is_none());
        assert!(responses.is_empty());
        return;
    }
    match expected {
        ProviderReplayUpdate::Anthropic(set) => {
            assert_eq!(anthropic.unwrap().response_sets, vec![set.clone()]);
            assert!(responses.is_empty());
        }
        ProviderReplayUpdate::Responses(set) => {
            assert!(anthropic.is_none());
            assert_eq!(responses, vec![set.clone()]);
        }
        ProviderReplayUpdate::Clear => panic!("fixture must exercise a real provider replay"),
    }
}

fn lost_response() -> phoenix_db::DbError {
    phoenix_db::DbError::Serialization("injected lost checkpoint response".into())
}

async fn recover(
    fixture: &Fixture,
    checkpoint: &GoalRoundCheckpoint<'_>,
    kind: GoalManagementRoundKind<'_>,
    replay: &ProviderReplayUpdate,
) -> TerminalMutationEstablishment<CommittedGoalRound> {
    GoalManagementService::reconcile_checkpoint(
        fixture.runtime.db(),
        checkpoint,
        kind,
        Some(replay),
        &lost_response(),
    )
    .await
}

#[tokio::test]
async fn conversation_goal_management_response_loss_recovers_each_owner_and_provider_without_reapplying(
) {
    for owner in [Owner::Runtime, Owner::Fork, Owner::Terminal] {
        for provider in [Provider::Anthropic, Provider::Responses] {
            for refused in [false, true] {
                recover_committed(owner, provider, refused).await;
            }
        }
    }
}

async fn recover_committed(owner: Owner, provider: Provider, refused: bool) {
    let fixture = fixture(true).await;
    let (mut assistant, mut results) = round(&fixture).await;
    let proposal = matches!(owner, Owner::Fork).then(|| proposal(&mut assistant, &mut results));
    let state = if matches!(owner, Owner::Terminal) {
        ConvState::Idle
    } else {
        ConvState::LlmRequesting { attempt: 2 }
    };
    let checkpoint = round_checkpoint(&assistant, &results, &state);
    let obligation = terminal_obligation(
        &ActiveDirectTurnSettlement {
            turn: fixture.active.clone(),
            conversation_id: fixture.source.clone(),
            terminal: ActiveDirectTurnTerminal::Cancelled,
            state: state.clone(),
            state_updated_at: checkpoint.state_updated_at,
            execution_occurrence_message_id: None,
        },
        None,
    );
    let kind = match owner {
        Owner::Runtime => GoalManagementRoundKind::Runtime,
        Owner::Fork => GoalManagementRoundKind::Fork(proposal.as_ref().unwrap()),
        Owner::Terminal => GoalManagementRoundKind::Terminal(&obligation),
    };
    let update = replay(provider, &assistant);
    let initial = snapshot(&fixture).await;
    assert!(matches!(
        recover(&fixture, &checkpoint, kind, &update).await,
        TerminalMutationEstablishment::KnownNotCommitted(_)
    ));
    assert_eq!(snapshot(&fixture).await, initial);
    let guard = if refused {
        Some(
            fixture
                .runtime
                .lock_product_message_admission(fixture.target.as_str())
                .await,
        )
    } else {
        None
    };
    let first = established(
        fixture
            .service
            .checkpoint(&checkpoint, kind, Some(&update))
            .await,
    );
    assert!(!first.replayed);
    let MessageContent::Tool(output) = &first.results[0].content else {
        panic!("canonical management result")
    };
    assert_eq!(output.is_error, refused);
    assert_eq!(first.results.len(), results.len());
    assert_replay(&fixture, &update, matches!(owner, Owner::Terminal)).await;
    assert_committed_owner(&fixture, owner, refused, proposal.as_ref()).await;
    drop(guard);
    if !refused {
        close_target(&fixture, true).await;
    }
    let before_recovery = snapshot(&fixture).await;
    let _product = fixture
        .runtime
        .lock_product_message_admission(fixture.target.as_str())
        .await;
    let _transcript = fixture
        .runtime
        .lock_message_acceptance("management-target")
        .await;
    let recovered = established(recover(&fixture, &checkpoint, kind, &update).await);
    assert!(recovered.replayed);
    assert_eq!(
        serde_json::to_value(&recovered.results).unwrap(),
        serde_json::to_value(&first.results).unwrap()
    );
    assert_eq!(snapshot(&fixture).await, before_recovery);
    assert_eq!(recovered.state, state);
    assert_eq!(recovered.state_updated_at, checkpoint.state_updated_at);
}

async fn assert_committed_owner(
    fixture: &Fixture,
    owner: Owner,
    refused: bool,
    proposal: Option<&ForkProposal>,
) {
    let db = fixture.runtime.db();
    let history = db
        .conversation_goal_history(&fixture.target, GoalVersion::INITIAL)
        .await
        .unwrap();
    assert_eq!(history.len(), usize::from(!refused));
    assert_eq!(
        db.list_fork_proposals_for_origin(&fixture.source)
            .await
            .unwrap(),
        proposal.cloned().into_iter().collect::<Vec<_>>()
    );
    let ownership: (Option<String>, i64) =
        sqlx::query_as("SELECT terminal_kind,owns_conversation FROM durable_turns WHERE turn_id=?")
            .bind(i64::try_from(fixture.active.turn_id.0).unwrap())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        ownership,
        if matches!(owner, Owner::Terminal) {
            (Some("Cancelled".into()), 0)
        } else {
            (None, 1)
        }
    );
}

#[tokio::test]
async fn conversation_goal_management_partial_retained_pair_is_unclassifiable_not_success_or_retry()
{
    for refused in [false, true] {
        let fixture = fixture(true).await;
        let (assistant, results) = round(&fixture).await;
        let state = ConvState::LlmRequesting { attempt: 2 };
        let checkpoint = round_checkpoint(&assistant, &results, &state);
        let update = replay(Provider::Anthropic, &assistant);
        let guard = if refused {
            Some(
                fixture
                    .runtime
                    .lock_product_message_admission(fixture.target.as_str())
                    .await,
            )
        } else {
            None
        };
        let first = established(
            fixture
                .service
                .checkpoint(&checkpoint, GoalManagementRoundKind::Runtime, Some(&update))
                .await,
        );
        drop(guard);
        sqlx::query("DELETE FROM messages WHERE message_id=?")
            .bind(&first.results[0].message_id)
            .execute(fixture.runtime.db().pool())
            .await
            .unwrap();
        let partial = snapshot(&fixture).await;
        assert!(matches!(
            recover(
                &fixture,
                &checkpoint,
                GoalManagementRoundKind::Runtime,
                &update
            )
            .await,
            TerminalMutationEstablishment::Unclassifiable(_)
        ));
        assert_eq!(snapshot(&fixture).await, partial);
    }
}

#[derive(Clone, Copy, Debug)]
enum Failure {
    Proposal,
    State,
    Replay,
}

#[tokio::test]
async fn conversation_goal_management_fork_rolls_back_all_side_effects_at_each_real_owner_write() {
    for provider in [Provider::Anthropic, Provider::Responses] {
        for failure in [Failure::Proposal, Failure::State, Failure::Replay] {
            fork_rollback(provider, failure).await;
        }
    }
}

async fn fork_rollback(provider: Provider, failure: Failure) {
    let fixture = fixture(true).await;
    let (mut assistant, mut results) = round(&fixture).await;
    let proposal = proposal(&mut assistant, &mut results);
    let state = ConvState::LlmRequesting { attempt: 2 };
    let checkpoint = round_checkpoint(&assistant, &results, &state);
    let update = replay(provider, &assistant);
    let db = fixture.runtime.db();
    let before = snapshot(&fixture).await;
    let trigger = match (failure, provider) {
        (Failure::Proposal, _) => "CREATE TRIGGER fail_management_fork BEFORE INSERT ON fork_proposals BEGIN SELECT RAISE(ABORT,'management fork cut'); END",
        (Failure::State, _) => "CREATE TRIGGER fail_management_fork BEFORE UPDATE OF state ON conversations BEGIN SELECT RAISE(ABORT,'management fork cut'); END",
        (Failure::Replay, Provider::Anthropic) => "CREATE TRIGGER fail_management_fork BEFORE INSERT ON active_provider_replay_state BEGIN SELECT RAISE(ABORT,'management fork cut'); END",
        (Failure::Replay, Provider::Responses) => "CREATE TRIGGER fail_management_fork BEFORE INSERT ON active_responses_replay_items BEGIN SELECT RAISE(ABORT,'management fork cut'); END",
    };
    sqlx::query(trigger).execute(db.pool()).await.unwrap();
    let failed = fixture
        .service
        .checkpoint(
            &checkpoint,
            GoalManagementRoundKind::Fork(&proposal),
            Some(&update),
        )
        .await;
    let TerminalMutationEstablishment::KnownNotCommitted(error) = failed else {
        panic!("{provider:?}/{failure:?}: rollback must be known")
    };
    assert!(error.contains("management fork cut"), "{error}");
    assert_eq!(snapshot(&fixture).await, before);
    sqlx::query("DROP TRIGGER fail_management_fork")
        .execute(db.pool())
        .await
        .unwrap();
    let accepted = established(
        fixture
            .service
            .checkpoint(
                &checkpoint,
                GoalManagementRoundKind::Fork(&proposal),
                Some(&update),
            )
            .await,
    );
    assert!(!accepted.replayed);
    assert_eq!(accepted.results.len(), 2);
    let MessageContent::Tool(management) = &accepted.results[0].content else {
        panic!("management result")
    };
    assert!(!management.is_error);
    assert_eq!(
        accepted.results[1].content,
        MessageContent::Tool(results[1].finalized().unwrap().persisted_tool_content())
    );
    assert_eq!(
        db.list_fork_proposals_for_origin(&fixture.source)
            .await
            .unwrap(),
        vec![proposal.clone()]
    );
    assert_replay(&fixture, &update, false).await;
    assert_fork_recovery(&fixture, &checkpoint, &proposal, &update, &accepted).await;
}

async fn assert_fork_recovery(
    fixture: &Fixture,
    checkpoint: &GoalRoundCheckpoint<'_>,
    proposal: &ForkProposal,
    update: &ProviderReplayUpdate,
    accepted: &CommittedGoalRound,
) {
    let before_recovery = snapshot(fixture).await;
    let mut changed = proposal.clone();
    changed.body.push_str(" changed");
    assert!(matches!(
        recover(
            fixture,
            checkpoint,
            GoalManagementRoundKind::Fork(&changed),
            update
        )
        .await,
        TerminalMutationEstablishment::Unclassifiable(_)
    ));
    assert_eq!(snapshot(fixture).await, before_recovery);
    let recovered = established(
        recover(
            fixture,
            checkpoint,
            GoalManagementRoundKind::Fork(proposal),
            update,
        )
        .await,
    );
    assert!(recovered.replayed);
    assert_eq!(
        serde_json::to_value(recovered.results).unwrap(),
        serde_json::to_value(&accepted.results).unwrap()
    );
    assert_eq!(snapshot(fixture).await, before_recovery);
}
