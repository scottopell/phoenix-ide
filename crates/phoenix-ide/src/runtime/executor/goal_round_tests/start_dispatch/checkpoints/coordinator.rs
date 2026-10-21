use super::*;
use phoenix_core::domain::conversation_goal::{GoalManagementRequest, GoalVersion};

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn conversation_goal_actual_coordinator_start_management_response_uses_adopted_request_source_once(
) {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::open(
        directory
            .path()
            .join("coordinator-checkpoint.db")
            .to_str()
            .unwrap(),
    )
    .await
    .unwrap();
    db.initialize_coordinator_work_scope(directory.path().to_str().unwrap())
        .await
        .unwrap();
    let source = db
        .get_or_create_coordinator(
            Some("claude-opus-5-5"),
            phoenix_core::llm_language::LlmLanguage::default(),
        )
        .await
        .unwrap();
    let target = db
        .create_conversation(
            "start-management-target",
            "target",
            directory.path().to_str().unwrap(),
            true,
            None,
            None,
        )
        .await
        .unwrap();
    let db = install(db).await;
    let owner = source.product_conversation_id.clone();
    let actor = GoalActor::GeneratedHost {
        action_id: text("coordinator-checkpoint-create"),
    };
    let created = db
        .apply_conversation_goal(
            &GoalMutation {
                owner: owner.clone(),
                expected_version: GoalVersion::INITIAL,
                action_id: text("coordinator-checkpoint-create"),
                attribution: GoalAttribution {
                    requested_by: actor.clone(),
                    applied_by: actor,
                },
                change: GoalChange::Create {
                    goal_id: GoalId::parse("coordinator-start-source-goal").unwrap(),
                    objective: text("manage through an actual Start request"),
                },
            },
            &source.id,
            GoalTimestampUs::parse(1).unwrap(),
        )
        .await
        .unwrap();
    change_control(&db, &owner, ControlCommand::Release, 0).await;
    let request = GoalStartInput::new(
        uuid::Uuid::new_v4(),
        GoalStartRequester::Owner {
            principal_id: db.instance_id().await.unwrap().to_string().parse().unwrap(),
        },
        created.snapshot.fence(),
    )
    .unwrap();
    let GoalStartAcceptance::Accepted(receipt) = db
        .accept_owner_goal_start(&request, &source.id, GoalTimestampUs::parse(3).unwrap())
        .await
        .unwrap()
    else {
        panic!("canonical Coordinator Start must be accepted");
    };
    let now = u64::try_from(Utc::now().timestamp()).unwrap();
    let authority = db
        .workflow_repository()
        .claim_authoritative_turn(&ClaimAuthoritativeTurnInput {
            turn_id: TurnAuthorityId(u64::try_from(receipt.execution().turn_id()).unwrap()),
            workflow_id: WorkflowId(u64::try_from(receipt.execution().workflow_id()).unwrap()),
            process_incarnation: ProcessIncarnation(1),
            now: Timestamp(now),
            lease_until: LeaseExpiry(now + 3600),
        })
        .await
        .unwrap()
        .authority
        .unwrap();
    let event_authority = phoenix_core::domain::sm_event::DirectTurnAttemptAuthority::new(
        authority.workflow_id.0,
        u64::try_from(receipt.execution().turn_id()).unwrap(),
        authority.effect_id.0,
        authority.attempt_id.0,
        authority.declared_workflow_version.0,
        authority.generation.0,
        authority.process_incarnation.0,
    );
    let management = GoalManagementRequest::Create {
        owner: target.product_conversation_id.clone(),
        expected_version: GoalVersion::INITIAL,
        objective: text("created by the actual admitted Coordinator response"),
    };
    let client = Arc::new(RenderingAnthropicClient::scripted(vec![provider_response(
        vec![ContentBlock::tool_use(
            "actual-coordinator-management",
            "goal_manage",
            serde_json::to_value(&management).unwrap(),
        )],
    )]));
    let mut context = ConvContext::new(
        source.id.clone(),
        directory.path().to_path_buf(),
        "claude-opus-5-5",
        200_000,
    );
    context.is_coordinator = true;
    let manager = Arc::new(crate::runtime::RuntimeManager::new(
        db.clone(),
        Arc::new(ModelRegistry::new_empty()),
        phoenix_core::platform::PlatformCapability::None {
            details: "admitted Coordinator checkpoint fixture".into(),
        },
        Arc::new(crate::tools::mcp::McpClientManager::new()),
        None,
    ));
    let mut rt = runtime_for_context(
        db.clone(),
        context,
        client.clone(),
        Arc::new(DeferredMcpCatalog::new(false)),
    );
    let stored = db.get_conversation(&source.id).await.unwrap();
    rt.state = stored.state.clone();
    rt.state_updated_at = stored.state_updated_at;
    let authority = event_authority;
    rt.goal_management = Some(Arc::new(
        crate::goal_service::management::GoalManagementService::new(Arc::downgrade(&manager)),
    ));
    let mut result = transition(
        &rt.state,
        &rt.context,
        Event::AuthoritativeGoalStart {
            receipt: receipt.clone(),
            authority,
        },
    )
    .unwrap();
    assert!(result
        .effects
        .iter()
        .any(|effect| matches!(effect, Effect::RequestLlm)));
    // Stop at the real executor's adoption boundary so tests can interpose a committed race.
    result
        .effects
        .retain(|effect| !matches!(effect, Effect::RequestLlm));
    rt.apply_transition_result(result).await.unwrap();
    let active = rt.active_direct_turn.as_deref().unwrap().clone();
    assert!(
        db.goal_management_execution(&source.id, receipt.execution().turn_id(), active.generation)
            .await
            .is_err(),
        "ordinary canonical-user resolver is not weakened for Start"
    );
    let completion = dispatch_response(&mut rt).await;
    let request_id = completion.admitted.intent().id.as_str().to_owned();
    assert!(db
        .goal_management_admitted_execution(
            &source.id,
            &request_id,
            &completion.admitted,
            (&rt.state, rt.state_updated_at),
        )
        .await
        .is_ok());
    assert!(db
        .goal_management_admitted_execution(
            &source.id,
            "wrong-actual-request",
            &completion.admitted,
            (&rt.state, rt.state_updated_at),
        )
        .await
        .is_err());
    assert!(
        db.goal_management_admitted_execution(
            &source.id,
            &request_id,
            &completion.admitted,
            (
                &rt.state,
                rt.state_updated_at + chrono::Duration::microseconds(1)
            ),
        )
        .await
        .is_err(),
        "matching persisted request stamp cannot substitute for the supplied exact slot"
    );
    assert!(
        db.goal_management_admitted_execution(
            &source.id,
            &request_id,
            &completion.admitted,
            (
                &ConvState::LlmRequesting { attempt: 2 },
                rt.state_updated_at
            ),
        )
        .await
        .is_err(),
        "same timestamp with a different supplied source state is not the dispatch slot"
    );
    let before = canonical_counts(&db).await;
    rt.process_goal_request_completion(completion)
        .await
        .unwrap();
    let after = canonical_counts(&db).await;
    assert_eq!(after.0, before.0 + 2);
    assert_eq!(after.2, before.2 + 1);
    assert_eq!(after.3, before.3);
    let goal = db
        .conversation_goal(&target.product_conversation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        goal.objective.as_str(),
        "created by the actual admitted Coordinator response"
    );
    assert_eq!(goal.version.get(), 1);
    let canonical_user: Option<String> =
        sqlx::query_scalar("SELECT canonical_message_id FROM durable_turns WHERE turn_id=?")
            .bind(receipt.execution().turn_id())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(
        canonical_user.is_none(),
        "management must not manufacture a user turn"
    );
    let rows = db.get_messages(&source.id).await.unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row.message_id == request_id)
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row.content, MessageContent::Tool(_)))
            .count(),
        1
    );
    assert!(rt.pending_goal_request_checkpoint.is_none());
    assert_eq!(rt.active_direct_turn.as_deref(), Some(&active));
    rt.llm_task_handle.take().unwrap().await.unwrap();
    assert_eq!(client.requests.lock().unwrap().len(), 2);
    manager.begin_process_shutdown();
    manager.drain_process_shutdown().await;
}
