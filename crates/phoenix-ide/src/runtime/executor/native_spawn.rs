use super::*;
use crate::runtime::{
    NativeSpawnClosureReceipt, NativeSpawnTerminalEvidence, SubAgentAdmissionReceipt,
    SubAgentAdmissionResponse,
};

pub(super) enum NativeSpawnPhase {
    Admission(oneshot::Receiver<SubAgentAdmissionReceipt>),
    Admitted,
    Closing,
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
            NativeSpawnPhase::Closing => NativeSpawnObservation::Closure((&mut self.closure).await),
            NativeSpawnPhase::Admitted => std::future::pending().await,
        }
    }
}

impl<S, L, T> ConversationRuntime<S, L, T>
where
    S: Storage + Clone + 'static,
    L: LlmClient + 'static,
    T: ToolExecutor + 'static,
{
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

    async fn fail_native_spawn(&mut self, error: String) -> Result<(), String> {
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
        self.native_spawn_handoff = None;
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
