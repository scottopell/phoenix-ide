use super::*;
use crate::runtime::{
    NativeSpawnClosureReceipt, NativeSpawnTerminalEvidence, SubAgentAdmissionReceipt,
    SubAgentAdmissionResponse,
};

pub(super) enum NativeSpawnPhase {
    Admission(oneshot::Receiver<SubAgentAdmissionReceipt>),
    Admitted,
    ActivationOwnerLost,
    Closing,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::testing::{MockLlmClient, MockToolExecutor};
    use crate::runtime::traits::DatabaseStorage;
    use crate::runtime::RuntimeManager;
    use phoenix_core::domain::sm_state::{
        AssistantMessage, ConversationExecutionEnvironment, SpawnAgentsInput, SubAgentTask,
    };

    type TestRuntime =
        ConversationRuntime<DatabaseStorage, Arc<MockLlmClient>, Arc<MockToolExecutor>>;

    async fn pending_native_spawn() -> (Arc<RuntimeManager>, TestRuntime, SubAgentSpawnRequest) {
        let manager = Arc::new(crate::runtime::scope_liveness_tests::test_manager().await);
        let parent = manager
            .db()
            .get_or_create_coordinator(None, phoenix_core::llm_language::LlmLanguage::default())
            .await
            .unwrap();
        let call = ToolCall::new(
            "spawn",
            ToolInput::SpawnAgents(SpawnAgentsInput {
                tasks: (0..2)
                    .map(|index| SubAgentTask {
                        task: format!("task {index}"),
                        cwd: None,
                        mode: Some(SubAgentMode::Explore),
                        execution: None,
                        max_turns: Some(1),
                        agent_type: None,
                    })
                    .collect(),
            }),
        );
        let state = ConvState::ToolExecuting {
            current_tool: call.clone(),
            remaining_tools: vec![],
            completed_results: vec![],
            pending_sub_agents: vec![],
            assistant_message: AssistantMessage::new(
                "assistant".into(),
                vec![ContentBlock::tool_use(
                    "spawn",
                    "spawn_agents",
                    call.input.to_value(),
                )],
                None,
                None,
            ),
        };
        manager
            .db()
            .update_conversation_state(&parent.id, &state)
            .await
            .unwrap();
        let mut context =
            ConvContext::new(&parent.id, "/tmp".into(), "gpt-5.6-sol", 200_000, 16_384);
        context.is_coordinator = true;
        context.execution_environment = ConversationExecutionEnvironment::NoFilesystem;
        context.resource_scope = phoenix_core::work_scope::ResourceScopeKey::Coordinator;
        let (event_tx, event_rx) = mpsc::channel(32);
        let (spawn_tx, mut spawn_rx) = mpsc::channel(1);
        let (cancel_tx, _) = mpsc::channel(1);
        let mut runtime = ConversationRuntime::new(
            context,
            state,
            DatabaseStorage::new(manager.db().clone()),
            Arc::new(MockLlmClient::new("gpt-5.6-sol")),
            Arc::new(MockToolExecutor::new()),
            Arc::new(BrowserSessionManager::default()),
            Arc::new(crate::tools::BashHandleRegistry::new()),
            Arc::new(crate::tools::TmuxRegistry::new()),
            Arc::new(ModelRegistry::new(&phoenix_llm::LlmConfig {
                openai_api_key: Some("test-key".into()),
                ..Default::default()
            })),
            crate::terminal::ActiveTerminals::new(),
            event_rx,
            event_tx,
            SseBroadcaster::new(128, 0),
        )
        .with_spawn_channels(spawn_tx, cancel_tx)
        .with_fatal_local_authority_fence(Arc::clone(&manager.fatal_local_authority_fence));
        assert!(runtime
            .start_spawn_agents_tool(call)
            .await
            .unwrap()
            .is_none());
        let request = spawn_rx
            .try_recv()
            .expect("native admission request was submitted");
        (manager, runtime, request)
    }

    async fn cancel(runtime: &mut TestRuntime) {
        runtime
            .process_event(Event::UserCancel {
                reason: None,
                cause: crate::state_machine::event::CancelCause::UserRequested,
            })
            .await
            .unwrap();
        assert!(matches!(runtime.state, ConvState::CancellingTool { .. }));
    }

    #[tokio::test]
    async fn native_cancel_and_late_admission_join_one_closure_then_atomic_fan_in() {
        let (manager, mut runtime, request) = pending_native_spawn().await;
        let origin = request.invocation.clone();
        let children: Vec<_> = request
            .specs
            .iter()
            .map(|spec| spec.agent_id.clone())
            .collect();
        sqlx::query("CREATE TABLE closure_counts(child TEXT PRIMARY KEY, n INTEGER NOT NULL)")
            .execute(manager.db().pool())
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER count_live_closure AFTER UPDATE OF cancellation_requested_at_unix_micros ON sub_agent_runs BEGIN INSERT INTO closure_counts VALUES (NEW.child_conversation_id, 1) ON CONFLICT(child) DO UPDATE SET n = n + 1; END").execute(manager.db().pool()).await.unwrap();
        cancel(&mut runtime).await;
        runtime
            .native_spawn_handoff
            .as_mut()
            .unwrap()
            .request_cancel();
        manager.handle_spawn_request(request).await;
        let observation = runtime.observe_native_spawn().await;
        let event = runtime
            .native_spawn_observed(observation)
            .await
            .unwrap()
            .expect("one reconciled cancellation receipt");
        assert!(
            matches!(&event, Event::ToolAborted { tool_use_id } if *tool_use_id == origin.tool_use_id)
        );
        let persisted = manager
            .db()
            .get_conversation(&origin.parent_conversation_id)
            .await
            .unwrap()
            .state;
        assert!(
            matches!(persisted, ConvState::CancellingTool { ref pending_sub_agents, .. } if pending_sub_agents.iter().map(|member| &member.agent_id).eq(children.iter()))
        );
        for child in &children {
            assert!(!manager
                .db()
                .sub_agent_terminal_is_accepted(child)
                .await
                .unwrap());
            let claims: Option<i64> = sqlx::query_scalar("SELECT initial_dispatch_claimed_at_unix_micros FROM sub_agent_runs WHERE child_conversation_id = ?1").bind(child).fetch_one(manager.db().pool()).await.unwrap();
            assert_eq!(claims, None);
            let count: i64 = sqlx::query_scalar("SELECT n FROM closure_counts WHERE child = ?1")
                .bind(child)
                .fetch_one(manager.db().pool())
                .await
                .unwrap();
            assert_eq!(
                count, 1,
                "cancellation and dropped late response joined one transaction"
            );
        }
        runtime.process_event(event).await.unwrap();
        assert!(matches!(runtime.state, ConvState::Idle));
        assert!(runtime.native_spawn_handoff.is_none());
        assert!(runtime.llm_client.recorded_requests().is_empty());
        for child in children {
            assert!(manager
                .db()
                .sub_agent_terminal_is_accepted(&child)
                .await
                .unwrap());
            let before = manager
                .db()
                .get_messages(&origin.parent_conversation_id)
                .await
                .unwrap();
            runtime
                .process_event(Event::SubAgentResult {
                    agent_id: child,
                    outcome: SubAgentOutcome::Failure {
                        error: "duplicate".into(),
                        error_kind: crate::db::ErrorKind::Cancelled,
                    },
                })
                .await
                .unwrap();
            assert_eq!(
                manager
                    .db()
                    .get_messages(&origin.parent_conversation_id)
                    .await
                    .unwrap()
                    .len(),
                before.len()
            );
        }
    }

    #[tokio::test]
    async fn native_lost_admission_response_reconciles_without_administrative_acceptance() {
        let (manager, mut runtime, mut request) = pending_native_spawn().await;
        let origin = request.invocation.clone();
        let (replacement, dropped) = oneshot::channel();
        drop(dropped);
        drop(std::mem::replace(&mut request.response_tx, replacement));
        let response_loss = runtime.observe_native_spawn().await;
        assert!(runtime
            .native_spawn_observed(response_loss)
            .await
            .unwrap()
            .is_none());
        manager.handle_spawn_request(request).await;
        let closure = runtime.observe_native_spawn().await;
        let completion = runtime
            .native_spawn_observed(closure)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(completion, Event::ToolComplete { ref result, .. } if result.is_error()));
        assert_eq!(
            manager
                .db()
                .owed_sub_agent_terminals(&origin.parent_conversation_id)
                .await
                .unwrap()
                .len(),
            2
        );
        runtime.process_event(completion).await.unwrap();
        assert!(manager
            .db()
            .owed_sub_agent_terminals(&origin.parent_conversation_id)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn native_activation_loss_is_observed_by_retained_admitted_owner() {
        let (manager, mut runtime, request) = pending_native_spawn().await;
        let manager_task = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.handle_spawn_request(request).await }
        });
        let response = runtime.observe_native_spawn().await;
        assert!(matches!(
            runtime.native_spawn_observed(response).await.unwrap(),
            Some(Event::SpawnAgentsComplete { .. })
        ));
        drop(runtime.pending_sub_agent_activation.take());
        manager_task.await.unwrap();
        let closure = runtime.observe_native_spawn().await;
        let completion = runtime
            .native_spawn_observed(closure)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(completion, Event::ToolComplete { ref result, .. } if result.is_error()));
        assert!(runtime.native_spawn_handoff.is_none());
        runtime.process_event(completion).await.unwrap();
        assert!(manager
            .db()
            .owed_sub_agent_terminals(&runtime.context.conversation_id)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn native_definite_closure_failure_is_non_resumable_and_keeps_admission() {
        let (manager, mut runtime, request) = pending_native_spawn().await;
        sqlx::query("CREATE TRIGGER reject_live_closure BEFORE UPDATE OF terminal_cause ON sub_agent_runs BEGIN SELECT RAISE(ABORT, 'known live closure failure'); END").execute(manager.db().pool()).await.unwrap();
        cancel(&mut runtime).await;
        manager.handle_spawn_request(request).await;
        let closure = runtime.observe_native_spawn().await;
        assert!(runtime
            .native_spawn_observed(closure)
            .await
            .unwrap()
            .is_none());
        assert!(matches!(
            runtime.state,
            ConvState::Error {
                error_kind: crate::db::ErrorKind::SubAgentError,
                ..
            }
        ));
        assert!(!manager.fatal_local_authority_fence.is_closed());
        let admitted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs WHERE parent_accepted_at_unix_micros IS NULL AND terminal_at_unix_micros IS NULL").fetch_one(manager.db().pool()).await.unwrap();
        assert_eq!(admitted, 2);
        assert!(manager
            .db()
            .get_messages(&runtime.context.conversation_id)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn native_unresolved_backstop_cannot_acknowledge_or_replace_origin() {
        let (manager, mut runtime, request) = pending_native_spawn().await;
        let origin = request.invocation.clone();
        cancel(&mut runtime).await;
        runtime.handle_cancelling_tool_timeout().await;
        assert!(runtime
            .native_spawn_handoff
            .as_ref()
            .is_some_and(|owner| owner.invocation == origin));
        assert!(matches!(runtime.state, ConvState::CancellingTool { .. }));
        assert!(manager.fatal_local_authority_fence.is_closed());
        assert!(manager
            .db()
            .get_messages(&origin.parent_conversation_id)
            .await
            .unwrap()
            .is_empty());
        drop(request);
    }

    #[tokio::test]
    async fn native_membership_write_failure_cannot_publish_reconciled_or_accept_children() {
        let (manager, mut runtime, request) = pending_native_spawn().await;
        cancel(&mut runtime).await;
        manager.handle_spawn_request(request).await;
        sqlx::query("CREATE TRIGGER reject_native_membership BEFORE UPDATE OF state ON conversations WHEN json_array_length(json_extract(NEW.state, '$.pending_sub_agents')) > 0 BEGIN SELECT RAISE(ABORT, 'known native membership failure'); END").execute(manager.db().pool()).await.unwrap();
        let closure = runtime.observe_native_spawn().await;
        assert!(runtime
            .native_spawn_observed(closure)
            .await
            .unwrap()
            .is_none());
        assert!(matches!(
            runtime.state,
            ConvState::Error {
                error_kind: crate::db::ErrorKind::SubAgentError,
                ..
            }
        ));
        assert!(!manager.fatal_local_authority_fence.is_closed());
        assert_eq!(
            manager
                .db()
                .owed_sub_agent_terminals(&runtime.context.conversation_id)
                .await
                .unwrap()
                .len(),
            2
        );
        assert!(runtime.sub_agent_result_buffer.is_empty());
        assert!(runtime.llm_client.recorded_requests().is_empty());
        assert!(manager
            .db()
            .get_messages(&runtime.context.conversation_id)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn native_unclassified_receipt_closes_shared_fence_even_after_origin_replacement() {
        let (manager, mut runtime, request) = pending_native_spawn().await;
        let origin = request.invocation.clone();
        runtime.state = ConvState::Idle;
        let result = runtime
            .native_spawn_observed(NativeSpawnObservation::Closure(Ok(
                NativeSpawnClosureReceipt {
                    invocation: origin,
                    evidence: NativeSpawnTerminalEvidence::Unclassified,
                },
            )))
            .await;
        assert!(result.is_err());
        assert!(manager.fatal_local_authority_fence.is_closed());
        assert!(runtime.llm_client.recorded_requests().is_empty());
        drop(request);
    }

    #[tokio::test]
    async fn native_missing_classified_owner_reports_failure_without_inventing_sqlite_unknown() {
        let (manager, mut runtime, request) = pending_native_spawn().await;
        cancel(&mut runtime).await;
        drop(request);
        let observation = runtime.observe_native_spawn().await;
        assert!(runtime
            .native_spawn_observed(observation)
            .await
            .unwrap()
            .is_none());
        assert!(matches!(
            runtime.state,
            ConvState::Error {
                error_kind: crate::db::ErrorKind::SubAgentError,
                ..
            }
        ));
        assert!(!manager.fatal_local_authority_fence.is_closed());
        assert!(manager
            .db()
            .get_messages(&runtime.context.conversation_id)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn native_postcommit_activation_receiver_loss_stops_effect_tail_without_fencing() {
        let (manager, mut runtime, request) = pending_native_spawn().await;
        manager
            .admit_sub_agent_batch(
                &request.invocation.batch_id,
                &request.specs,
                &request.invocation.parent_conversation_id,
                request.parent_scope,
                request.parallel_work_qualified,
            )
            .await
            .unwrap();
        let next = ToolCall::new(
            "later",
            ToolInput::from(crate::tools::BashToolInput::run("true")),
        );
        if let ConvState::ToolExecuting {
            remaining_tools,
            assistant_message,
            ..
        } = &mut runtime.state
        {
            remaining_tools.push(next.clone());
            assistant_message.content.push(ContentBlock::tool_use(
                "later",
                next.name(),
                next.input.to_value(),
            ));
        }
        manager
            .db()
            .update_conversation_state(&runtime.context.conversation_id, &runtime.state)
            .await
            .unwrap();
        request
            .response_tx
            .send(SubAgentAdmissionReceipt {
                invocation: request.invocation.clone(),
                outcome: SubAgentAdmissionResponse::Admitted,
            })
            .unwrap();
        drop(request.activation_rx);
        let response = runtime.observe_native_spawn().await;
        let event = runtime
            .native_spawn_observed(response)
            .await
            .unwrap()
            .unwrap();
        runtime.process_event(event).await.unwrap();
        assert!(matches!(
            runtime.state,
            ConvState::Error {
                error_kind: crate::db::ErrorKind::SubAgentError,
                ..
            }
        ));
        assert!(!manager.fatal_local_authority_fence.is_closed());
        assert!(runtime.tool_executor.recorded_executions().is_empty());
        assert!(runtime.llm_client.recorded_requests().is_empty());
        assert!(runtime
            .native_spawn_handoff
            .as_ref()
            .is_some_and(|handoff| handoff.invocation == request.invocation));
        let accepted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs WHERE parent_accepted_at_unix_micros IS NOT NULL OR initial_dispatch_claimed_at_unix_micros IS NOT NULL").fetch_one(manager.db().pool()).await.unwrap();
        assert_eq!(accepted, 0);
        drop(request.closure_tx);
        let missing_receipt = runtime.observe_native_spawn().await;
        assert!(runtime
            .native_spawn_observed(missing_receipt)
            .await
            .unwrap()
            .is_none());
        assert!(!manager.fatal_local_authority_fence.is_closed());
    }

    #[tokio::test]
    async fn native_abnormal_replaced_origin_withholds_activation_without_fencing() {
        let (manager, mut runtime, request) = pending_native_spawn().await;
        let manager_task = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.handle_spawn_request(request).await }
        });
        let response = runtime.observe_native_spawn().await;
        if let ConvState::ToolExecuting {
            assistant_message, ..
        } = &mut runtime.state
        {
            assistant_message.message_id = "successor-assistant".into();
        }
        let successor = runtime.state.clone();
        assert!(runtime
            .native_spawn_observed(response)
            .await
            .unwrap()
            .is_none());
        manager_task.await.unwrap();
        assert_eq!(runtime.state, successor);
        assert!(!manager.fatal_local_authority_fence.is_closed());
        let claims: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sub_agent_runs WHERE initial_dispatch_claimed_at_unix_micros IS NOT NULL OR parent_accepted_at_unix_micros IS NOT NULL").fetch_one(manager.db().pool()).await.unwrap();
        assert_eq!(claims, 0);
    }
}

pub(super) struct NativeSpawnHandoff {
    pub(super) invocation: phoenix_db::SubAgentSpawnInvocation,
    pub(super) members: Vec<crate::state_machine::state::PendingSubAgent>,
    pub(super) activation: Option<oneshot::Sender<()>>,
    pub(super) cancellation: CancellationToken,
    pub(super) phase: NativeSpawnPhase,
    pub(super) closure: oneshot::Receiver<NativeSpawnClosureReceipt>,
}

pub(super) enum NativeSpawnObservation {
    Admission(Result<SubAgentAdmissionReceipt, oneshot::error::RecvError>),
    Closure(Result<NativeSpawnClosureReceipt, oneshot::error::RecvError>),
}

impl NativeSpawnHandoff {
    pub(super) fn request_cancel(&mut self) {
        self.cancellation.cancel();
        self.activation = None;
        self.phase = NativeSpawnPhase::Closing;
    }

    pub(super) async fn observe(&mut self) -> NativeSpawnObservation {
        match &mut self.phase {
            NativeSpawnPhase::Admission(receiver) => {
                NativeSpawnObservation::Admission(receiver.await)
            }
            NativeSpawnPhase::Closing
            | NativeSpawnPhase::Admitted
            | NativeSpawnPhase::ActivationOwnerLost => {
                NativeSpawnObservation::Closure((&mut self.closure).await)
            }
        }
    }
}

impl<S, L, T> ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
    #[cfg(test)]
    pub(super) async fn observe_native_spawn(&mut self) -> NativeSpawnObservation {
        match self.native_spawn_handoff.as_mut() {
            Some(handoff) => handoff.observe().await,
            None => std::future::pending().await,
        }
    }

    pub(super) async fn native_spawn_observed(
        &mut self,
        observation: NativeSpawnObservation,
    ) -> Result<Option<Event>, String> {
        if matches!(
            &observation,
            NativeSpawnObservation::Admission(Ok(SubAgentAdmissionReceipt {
                outcome: SubAgentAdmissionResponse::Unclassified,
                ..
            })) | NativeSpawnObservation::Closure(Ok(NativeSpawnClosureReceipt {
                evidence: NativeSpawnTerminalEvidence::Unclassified,
                ..
            }))
        ) {
            self.fatal_local_authority_fence
                .close("native_spawn_authority");
            return Err("FATAL_LOCAL_AUTHORITY_UNCLASSIFIED: native spawn operation".into());
        }
        let Some(handoff) = self.native_spawn_handoff.as_ref() else {
            tracing::error!("native spawn observation lost its originating owner");
            return Ok(None);
        };
        let origin = handoff.invocation.clone();
        if origin.parent_conversation_id != self.context.conversation_id
            || !origin.owns_state(&self.state)
        {
            tracing::error!(?origin, state = self.state.variant_name(), "native spawn origin was abnormally replaced; activation withheld and durable evidence retained");
            self.native_spawn_handoff = None;
            return Ok(None);
        }
        match observation {
            NativeSpawnObservation::Admission(Ok(receipt)) if receipt.invocation != origin => {
                self.fail_native_spawn(
                    "native spawn admission carried a different invocation".into(),
                )
                .await?;
                Ok(None)
            }
            NativeSpawnObservation::Admission(Ok(receipt)) => match receipt.outcome {
                SubAgentAdmissionResponse::Admitted => {
                    let handoff = self.native_spawn_handoff.as_mut().expect("origin retained");
                    if matches!(self.state, ConvState::CancellingTool { .. }) {
                        handoff.request_cancel();
                        return Ok(None);
                    }
                    handoff.phase = NativeSpawnPhase::Admitted;
                    self.pending_sub_agent_activation = Some(PendingSubAgentActivation {
                        child_ids: handoff
                            .members
                            .iter()
                            .map(|member| member.agent_id.clone())
                            .collect(),
                        sender: handoff
                            .activation
                            .take()
                            .ok_or("native spawn activation capability lost")?,
                    });
                    Ok(Some(Event::SpawnAgentsComplete {
                        tool_use_id: origin.tool_use_id.clone(),
                        result: ToolResult::success(
                            origin.tool_use_id,
                            format!(
                                "Spawning {} sub-agent(s): {}",
                                handoff.members.len(),
                                handoff
                                    .members
                                    .iter()
                                    .map(|member| member.agent_id.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        ),
                        spawned: handoff.members.clone(),
                    }))
                }
                SubAgentAdmissionResponse::Rejected(error) => {
                    self.native_spawn_handoff = None;
                    Ok(Some(Event::ToolComplete {
                        tool_use_id: origin.tool_use_id.clone(),
                        result: ToolResult::error(origin.tool_use_id, error),
                    }))
                }
                SubAgentAdmissionResponse::Unclassified => {
                    self.fatal_local_authority_fence
                        .close("native_spawn_admission");
                    Err("FATAL_LOCAL_AUTHORITY_UNCLASSIFIED: native spawn admission".into())
                }
            },
            NativeSpawnObservation::Admission(Err(_)) => {
                self.native_spawn_handoff
                    .as_mut()
                    .expect("origin retained")
                    .request_cancel();
                Ok(None)
            }
            NativeSpawnObservation::Closure(Err(error)) => {
                self.fail_native_spawn(format!(
                    "native spawn owner disappeared without a classified closure: {error}"
                ))
                .await?;
                Ok(None)
            }
            NativeSpawnObservation::Closure(Ok(receipt)) => {
                if receipt.invocation != origin {
                    self.fail_native_spawn(
                        "native spawn closure carried a different invocation".into(),
                    )
                    .await?;
                    return Ok(None);
                }
                match receipt.evidence {
                    NativeSpawnTerminalEvidence::Unclassified => {
                        self.fatal_local_authority_fence
                            .close("native_spawn_closure");
                        return Err(
                            "FATAL_LOCAL_AUTHORITY_UNCLASSIFIED: native spawn closure".into()
                        );
                    }
                    NativeSpawnTerminalEvidence::Failed(error) => {
                        self.fail_native_spawn(error).await?;
                        return Ok(None);
                    }
                    NativeSpawnTerminalEvidence::NotAdmitted => {}
                    NativeSpawnTerminalEvidence::Terminal(terminals) => {
                        let members = &self
                            .native_spawn_handoff
                            .as_ref()
                            .expect("origin retained")
                            .members;
                        if !terminals
                            .iter()
                            .map(|terminal| &terminal.child_conversation_id)
                            .eq(members.iter().map(|member| &member.agent_id))
                        {
                            self.fail_native_spawn(
                                "native spawn closure omitted exact admitted membership".into(),
                            )
                            .await?;
                            return Ok(None);
                        }
                        let membership = phoenix_db::UnactivatedSubAgentMembership {
                            invocation: origin.clone(),
                            previous_state: self.state.clone(),
                            members: members.clone(),
                        };
                        let updated_at = Utc::now();
                        let owner = self.admit_authoritative_effect()?;
                        let mut authority = super::super::native_spawn::NativeSpawnWriteAuthority {
                            owner: Some(owner),
                        };
                        let established = self
                            .storage
                            .establish_unactivated_sub_agent_membership(&membership, updated_at)
                            .await;
                        authority.owner = None;
                        match established {
                            phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(
                                Ok(state),
                            ) => {
                                self.state = state;
                                self.state_updated_at = updated_at;
                            }
                            phoenix_db::workflow::LocalAuthorityResult::DurableFactEstablished(
                                Err(error),
                            ) => {
                                self.fail_native_spawn(error).await?;
                                return Ok(None);
                            }
                            phoenix_db::workflow::LocalAuthorityResult::DurableFactUnclassified => {
                                self.fatal_local_authority_fence
                                    .close("native_spawn_parent_membership");
                                return Err("FATAL_LOCAL_AUTHORITY_UNCLASSIFIED: native spawn parent membership".into());
                            }
                        }
                        self.sub_agent_result_buffer
                            .extend(terminals.into_iter().map(|terminal| Event::SubAgentResult {
                                agent_id: terminal.child_conversation_id,
                                outcome: terminal.outcome,
                            }));
                    }
                }
                self.native_spawn_handoff = None;
                if matches!(self.state, ConvState::CancellingTool { .. }) {
                    Ok(Some(Event::ToolAborted {
                        tool_use_id: origin.tool_use_id,
                    }))
                } else {
                    Ok(Some(Event::ToolComplete {
                        tool_use_id: origin.tool_use_id.clone(),
                        result: ToolResult::error(
                            origin.tool_use_id,
                            "Sub-agent activation cancelled before parent adoption".into(),
                        ),
                    }))
                }
            }
        }
    }

    pub(super) async fn fail_native_spawn(&mut self, error: String) -> Result<(), String> {
        let handoff = self.native_spawn_handoff.take();
        tracing::error!(invocation = ?handoff.as_ref().map(|owner| &owner.invocation), %error, "native spawn cannot complete its live handoff");
        let new_state = ConvState::Error {
            message: error,
            error_kind: crate::db::ErrorKind::SubAgentError,
            resets_at: None,
        };
        self.classify_active_direct_turn_state_terminal(&new_state);
        let transition = crate::state_machine::transition::TransitionResult {
            new_state,
            effects: vec![Effect::PersistState, Effect::notify_state_change()],
        };
        let result = Box::pin(self.apply_transition_result(transition)).await;
        if result.is_err() {
            self.fatal_local_authority_fence
                .close("native_spawn_failure_settlement");
        }
        result.map(|_| ())
    }

    #[cfg(test)]
    pub(super) async fn prepare_spawn_and_wait(
        &mut self,
        tool: ToolCall,
    ) -> Result<Option<Event>, String> {
        let event = self.start_spawn_agents_tool(tool).await?;
        if event.is_some() {
            return Ok(event);
        }
        let observation = self.observe_native_spawn().await;
        self.native_spawn_observed(observation).await
    }
}
