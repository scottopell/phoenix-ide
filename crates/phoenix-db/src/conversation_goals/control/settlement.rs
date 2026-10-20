use phoenix_core::domain::{provider_replay::ProviderReplayUpdate, sm_state::ConvState};
use phoenix_workflow::{TurnCommand, TurnTerminal};

use crate::{
    conversation_goals::{
        control::GoalControlInstallation,
        round::{write_round_tx, ToolRoundWrite},
    },
    workflow::{DirectTurnTerminalObligationInput, TerminalizeAuthoritativeTurnInput},
    Database, DbError, DbResult, Message,
};

impl Database {
    /// Commit the cancelled tool pair, target projection, and terminal ownership together.
    ///
    /// # Errors
    /// Rejects non-revoked control bindings and stale or inconsistent terminal authority.
    pub async fn persist_revoked_goal_tool_round(
        &self,
        conversation_id: &str,
        assistant: &Message,
        results: &[Message],
        obligation: &DirectTurnTerminalObligationInput,
        update: Option<&ProviderReplayUpdate>,
    ) -> DbResult<()> {
        if self.goal_control_installation != GoalControlInstallation::Installed
            || obligation.terminal != TurnTerminal::Cancelled
            || obligation.projection.state != ConvState::Idle
        {
            return Err(DbError::Serialization(
                "invalid revoked-goal settlement".into(),
            ));
        }
        let repo = self.workflow_repository();
        let mut tx = repo.begin_immediate_tx().await?;
        let turn_id = i64::try_from(obligation.turn_id.0)
            .map_err(|error| DbError::Serialization(error.to_string()))?;
        let revoked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM durable_turns t
             JOIN conversation_goal_control_bindings b ON b.turn_id=t.turn_id AND b.workflow_id=t.workflow_id
             JOIN conversation_goal_controls g ON g.owner_id=b.owner_id
             JOIN conversations c ON c.id=t.conversation_id AND c.product_conversation_id=b.owner_id
             JOIN product_conversations p ON p.id=b.owner_id AND p.kind='ordinary'
             WHERE t.turn_id=? AND t.conversation_id=? AND b.control_version<g.control_version)",
        )
        .bind(turn_id)
        .bind(conversation_id)
        .fetch_one(&mut *tx.tx)
        .await?;
        if !revoked {
            return Err(DbError::Serialization(
                "tool round has no revoked goal-control binding".into(),
            ));
        }
        let replayed = retained_pair_tx(&mut tx.tx, assistant, results).await?;
        write_round_tx(
            &mut tx.tx,
            conversation_id,
            assistant,
            ToolRoundWrite::Finalized(results),
            &obligation.projection.state,
            obligation.projection.state_updated_at,
            self.goal_control_installation,
        )
        .await?;
        if !replayed {
            if let Some(update) = update {
                crate::provider_replay::apply_replay_tx(&mut tx.tx, conversation_id, update)
                    .await?;
            }
        }
        let terminal = repo
            .terminalize_authoritative_turn_in_tx(
                &mut tx,
                &TerminalizeAuthoritativeTurnInput {
                    command: TurnCommand::Cancel {
                        turn_id: obligation.turn_id,
                        expected_generation: obligation.expected_generation,
                    },
                    projection: Some(obligation.projection.clone()),
                },
            )
            .await?;
        if !replayed
            && matches!(
                terminal,
                crate::workflow::TerminalizeAuthoritativeTurnOutcome::AlreadyReconciled(_)
            )
        {
            return Err(DbError::Serialization(
                "settled revoked tool round lost its exact pair".into(),
            ));
        }
        tx.tx.commit().await?;
        Ok(())
    }
}

async fn retained_pair_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    assistant: &Message,
    results: &[Message],
) -> DbResult<bool> {
    let mut retained = 0;
    for message in std::iter::once(assistant).chain(results.iter()) {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM messages WHERE message_id=?)")
                .bind(&message.message_id)
                .fetch_one(&mut **tx)
                .await?;
        retained += usize::from(exists);
    }
    let replayed = retained == results.len() + 1;
    if retained != 0 && !replayed {
        return Err(DbError::Serialization(
            "revoked tool round has partial retained evidence".into(),
        ));
    }
    Ok(replayed)
}
