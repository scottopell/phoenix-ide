use std::{collections::BTreeMap, sync::Weak};

use phoenix_core::domain::conversation_goal::{
    GoalAgentSource, GoalPendingManagement, GoalText, ToolRoundResult,
};
use phoenix_db::conversation_goals::{
    management::{GoalManagementRoundKind, GoalManagementTarget},
    round::{CommittedGoalRound, GoalRoundCheckpoint},
};

use crate::runtime::{traits::TerminalMutationEstablishment, RuntimeManager};

/// Internal checkpoint owner; no public tool registration installs this service.
#[allow(
    dead_code,
    reason = "Coordinator management activation is not installed"
)]
pub(crate) struct GoalManagementService {
    runtime: Weak<RuntimeManager>,
}

impl GoalManagementService {
    #[allow(
        dead_code,
        reason = "Coordinator management activation is not installed"
    )]
    pub(crate) fn new(runtime: Weak<RuntimeManager>) -> Self {
        Self { runtime }
    }

    pub(crate) fn definition() -> phoenix_core::domain::llm_types::ToolDefinition {
        phoenix_core::domain::llm_types::ToolDefinition {
            name: "goal_manage".into(),
            description: "Manage an ordinary conversation's goal through a version-checked Coordinator checkpoint. No work is started.".into(),
            defer_loading: false,
            input_schema: serde_json::json!({
                "type":"object",
                "oneOf":[
                    {"type":"object","properties":{
                        "op":{"type":"string","enum":["create"]},
                        "owner":{"type":"string","minLength":1},
                        "expected_version":{"type":"integer","minimum":0},
                        "objective":{"type":"string","minLength":1}
                    },"required":["op","owner","expected_version","objective"],"additionalProperties":false},
                    {"type":"object","properties":{
                        "op":{"type":"string","enum":["state"]},
                        "owner":{"type":"string","minLength":1},
                        "expected_version":{"type":"integer","minimum":1},
                        "goal_id":{"type":"string","minLength":1},
                        "disposition":{"type":"object","oneOf":[
                            {"type":"object","properties":{"state":{"type":"string","enum":["underway"]}},"required":["state"],"additionalProperties":false},
                            {"type":"object","properties":{
                                "state":{"type":"string","enum":["blocked","complete","exhausted"]},
                                "reason":{"type":"string","minLength":1}
                            },"required":["state","reason"],"additionalProperties":false}
                        ]}
                    },"required":["op","owner","expected_version","goal_id","disposition"],"additionalProperties":false}
                ]
            }),
        }
    }

    pub(crate) async fn prepare(
        &self,
        transcript: &str,
        turn: &crate::runtime::traits::ActiveDirectTurn,
        message: &str,
        tool_use: &str,
        request: phoenix_core::domain::conversation_goal::GoalManagementRequest,
    ) -> Result<GoalPendingManagement, String> {
        let runtime = self.runtime.upgrade().ok_or("management runtime closed")?;
        let turn_id = i64::try_from(turn.turn_id.0).map_err(|error| error.to_string())?;
        let (owner, execution) = runtime
            .db()
            .goal_management_execution(transcript, turn_id, turn.generation)
            .await
            .map_err(|error| error.to_string())?;
        Self::pending_management(
            (owner, execution, turn.generation),
            transcript,
            message,
            tool_use,
            request,
        )
    }

    fn pending_management(
        authority: (
            phoenix_core::domain::product_conversation::ProductConversationId,
            phoenix_core::domain::conversation_goal::GoalExecution,
            u64,
        ),
        transcript: &str,
        message: &str,
        tool_use: &str,
        request: phoenix_core::domain::conversation_goal::GoalManagementRequest,
    ) -> Result<GoalPendingManagement, String> {
        let (owner, execution, generation) = authority;
        let source = GoalAgentSource {
            product_conversation_id: owner,
            execution,
            transcript_id: GoalText::parse(transcript).map_err(|error| error.to_string())?,
            message_id: GoalText::parse(message).map_err(|error| error.to_string())?,
            tool_use_id: GoalText::parse(tool_use).map_err(|error| error.to_string())?,
        };
        Ok(GoalPendingManagement::new(source, generation, request))
    }

    pub(crate) async fn checkpoint(
        &self,
        checkpoint: &GoalRoundCheckpoint<'_>,
        kind: GoalManagementRoundKind<'_>,
        replay: Option<&phoenix_core::domain::provider_replay::ProviderReplayUpdate>,
    ) -> TerminalMutationEstablishment<CommittedGoalRound> {
        let Some(runtime) = self.runtime.upgrade() else {
            return TerminalMutationEstablishment::KnownNotCommitted(
                "management runtime closed".into(),
            );
        };
        let retained = match runtime
            .db()
            .retained_goal_management_round(checkpoint)
            .await
        {
            Ok(retained) => retained,
            Err(error) => return TerminalMutationEstablishment::Unclassifiable(error.to_string()),
        };
        let mut targets = Vec::new();
        let mut product_guards = Vec::new();
        let mut transcript_guards = Vec::new();
        if retained.is_none() {
            let owners: BTreeMap<_, _> = checkpoint
                .results
                .iter()
                .filter_map(|result| match result {
                    ToolRoundResult::PendingGoalManagement { management, .. } => {
                        let owner = management.request().owner().clone();
                        Some((owner.as_str().to_owned(), owner))
                    }
                    ToolRoundResult::Finalized(_) | ToolRoundResult::PendingGoalReport { .. } => {
                        None
                    }
                })
                .collect();
            for owner in owners.into_values() {
                let Some(product) = runtime
                    .try_lock_product_message_admission(owner.as_str())
                    .await
                else {
                    targets.push(GoalManagementTarget::Refused { owner });
                    continue;
                };
                let latest = match runtime.db().get_ordinary_product_conversation(&owner).await {
                    Ok(latest) => latest,
                    Err(phoenix_db::DbError::ConversationNotFound(_)) => {
                        targets.push(GoalManagementTarget::Refused { owner });
                        continue;
                    }
                    Err(error) => {
                        return TerminalMutationEstablishment::KnownNotCommitted(error.to_string())
                    }
                };
                let Some(transcript) = runtime
                    .try_lock_message_acceptance(&latest.latest_transcript_row_id)
                    .await
                else {
                    targets.push(GoalManagementTarget::Refused { owner });
                    continue;
                };
                targets.push(GoalManagementTarget::Admitted {
                    owner,
                    transcript: latest.latest_transcript_row_id,
                });
                product_guards.push(product);
                transcript_guards.push(transcript);
            }
        }
        let result = runtime
            .db()
            .persist_goal_management_round(checkpoint, &targets, kind, replay)
            .await;
        drop(transcript_guards);
        drop(product_guards);
        match result {
            Ok(value) => TerminalMutationEstablishment::Established {
                transcript_generation: None,
                value,
            },
            Err(error) => {
                Self::reconcile_checkpoint(runtime.db(), checkpoint, kind, replay, &error).await
            }
        }
    }

    async fn reconcile_checkpoint(
        db: &phoenix_db::Database,
        checkpoint: &GoalRoundCheckpoint<'_>,
        kind: GoalManagementRoundKind<'_>,
        replay: Option<&phoenix_core::domain::provider_replay::ProviderReplayUpdate>,
        error: &phoenix_db::DbError,
    ) -> TerminalMutationEstablishment<CommittedGoalRound> {
        match db.retained_goal_management_round(checkpoint).await {
            Ok(None) => TerminalMutationEstablishment::KnownNotCommitted(error.to_string()),
            Ok(Some(_)) => match db
                .persist_goal_management_round(checkpoint, &[], kind, replay)
                .await
            {
                Ok(value) => TerminalMutationEstablishment::Established {
                    transcript_generation: None,
                    value,
                },
                Err(replay_error) => TerminalMutationEstablishment::Unclassifiable(format!(
                    "management commit {error}; exact replay {replay_error}"
                )),
            },
            Err(probe) => TerminalMutationEstablishment::Unclassifiable(format!(
                "management commit {error}; exact probe {probe}"
            )),
        }
    }
}

#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests;
