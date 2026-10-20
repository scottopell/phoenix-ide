use serde::Serialize;
use uuid::Uuid;

use super::{
    admission, apply_goal_tx, current_tx, history_entry, GoalActor, GoalAttribution, GoalChange,
    GoalDbError, GoalHistoryEntry, GoalId, GoalMutation, GoalText, GoalTimestampUs, GoalTx,
    GoalUserCommandKind, GoalVersion, ProductConversationId, Result,
};
use crate::Database;
use sqlx::Row;

pub struct GoalCreateCommand {
    pub owner: ProductConversationId,
    pub principal_id: Uuid,
    pub request_id: Uuid,
    pub expected_version: GoalVersion,
    pub objective: GoalText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalCreateRefusal {
    Version,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum GoalCreateResult {
    Accepted {
        entry: Box<GoalHistoryEntry>,
    },
    Rejected {
        reason: GoalCreateRefusal,
        observed_version: GoalVersion,
        recorded_at: GoalTimestampUs,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoalCreateReceipt {
    pub request_id: Uuid,
    pub result: GoalCreateResult,
}

async fn retained_tx(
    tx: &mut GoalTx<'_>,
    input: &GoalCreateCommand,
) -> Result<Option<GoalCreateReceipt>> {
    let row = sqlx::query("SELECT command,expected_version,outcome,result_version,rejected_objective,recorded_at_us FROM conversation_goal_user_requests WHERE owner_id=? AND principal_id=? AND request_id=?")
        .bind(input.owner.as_str()).bind(input.principal_id.as_bytes().as_slice())
        .bind(input.request_id.as_bytes().as_slice()).fetch_optional(&mut **tx).await?;
    let Some(row) = row else { return Ok(None) };
    if row.try_get::<String, _>("command")? != "create"
        || row.try_get::<i64, _>("expected_version")? != input.expected_version.get()
    {
        return Err(GoalDbError::RetryConflict);
    }
    let version = GoalVersion::parse(row.try_get("result_version")?)?;
    let result = match row.try_get::<String, _>("outcome")?.as_str() {
        "accepted" => {
            let history = sqlx::query("SELECT h.*,g.objective FROM conversation_goal_history h JOIN conversation_goal_ids g USING (owner_id,goal_id) WHERE owner_id=? AND version=?")
                .bind(input.owner.as_str()).bind(version.get()).fetch_one(&mut **tx).await?;
            let entry = history_entry(tx, &history).await?;
            if entry.snapshot.objective != input.objective {
                return Err(GoalDbError::RetryConflict);
            }
            GoalCreateResult::Accepted {
                entry: Box::new(entry),
            }
        }
        outcome @ ("rejected_version" | "rejected_busy") => {
            if row.try_get::<String, _>("rejected_objective")? != input.objective.as_str() {
                return Err(GoalDbError::RetryConflict);
            }
            GoalCreateResult::Rejected {
                reason: if outcome == "rejected_version" {
                    GoalCreateRefusal::Version
                } else {
                    GoalCreateRefusal::Busy
                },
                observed_version: version,
                recorded_at: GoalTimestampUs::parse(row.try_get("recorded_at_us")?)?,
            }
        }
        _ => return Err(GoalDbError::Integrity("goal Create receipt outcome")),
    };
    Ok(Some(GoalCreateReceipt {
        request_id: input.request_id,
        result,
    }))
}

async fn reject_tx(
    tx: &mut GoalTx<'_>,
    input: &GoalCreateCommand,
    reason: GoalCreateRefusal,
    observed_version: GoalVersion,
    at: GoalTimestampUs,
) -> Result<GoalCreateReceipt> {
    let outcome = match reason {
        GoalCreateRefusal::Version => "rejected_version",
        GoalCreateRefusal::Busy => "rejected_busy",
    };
    sqlx::query("INSERT INTO conversation_goal_user_requests (owner_id,principal_id,request_id,command,expected_version,goal_id,rejected_objective,rejected_state,outcome,result_version,recorded_at_us) VALUES (?,?,?,'create',?,?,?,'underway',?,?,?)")
        .bind(input.owner.as_str()).bind(input.principal_id.as_bytes().as_slice())
        .bind(input.request_id.as_bytes().as_slice()).bind(input.expected_version.get())
        .bind(Uuid::new_v4().to_string()).bind(input.objective.as_str()).bind(outcome).bind(observed_version.get()).bind(at.get())
        .execute(&mut **tx).await?;
    Ok(GoalCreateReceipt {
        request_id: input.request_id,
        result: GoalCreateResult::Rejected {
            reason,
            observed_version,
            recorded_at: at,
        },
    })
}

impl Database {
    /// Read an exact Create receipt after revalidating the ordinary owner's live target.
    ///
    /// # Errors
    /// Rejects unavailable owners, changed request payloads and malformed or missing schema.
    pub async fn retained_user_goal_create(
        &self,
        input: &GoalCreateCommand,
        expected_transcript: &str,
    ) -> Result<Option<GoalCreateReceipt>> {
        let mut tx = self.pool().begin().await?;
        admission::require_target_tx(&mut tx, &input.owner, expected_transcript).await?;
        let retained = retained_tx(&mut tx, input).await?;
        tx.commit().await?;
        Ok(retained)
    }

    /// Apply a host-authenticated Create while the caller holds both existing admission gates.
    ///
    /// # Errors
    /// Rejects changed/unavailable targets, conflicting request identity and storage failures.
    pub async fn apply_user_goal_create(
        &self,
        input: &GoalCreateCommand,
        expected_transcript: &str,
        at: GoalTimestampUs,
    ) -> Result<GoalCreateReceipt> {
        self.user_goal_create(input, expected_transcript, at, false)
            .await
    }

    /// Retain a busy refusal when the host could not acquire its existing admission gates.
    ///
    /// # Errors
    /// Rejects changed/unavailable targets, conflicting request identity and storage failures.
    pub async fn reject_busy_user_goal_create(
        &self,
        input: &GoalCreateCommand,
        expected_transcript: &str,
        at: GoalTimestampUs,
    ) -> Result<GoalCreateReceipt> {
        self.user_goal_create(input, expected_transcript, at, true)
            .await
    }

    async fn user_goal_create(
        &self,
        input: &GoalCreateCommand,
        expected_transcript: &str,
        at: GoalTimestampUs,
        host_gate_busy: bool,
    ) -> Result<GoalCreateReceipt> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        admission::require_target_tx(&mut tx, &input.owner, expected_transcript).await?;
        if let Some(retained) = retained_tx(&mut tx, input).await? {
            tx.commit().await?;
            return Ok(retained);
        }
        let current = current_tx(&mut tx, &input.owner).await?;
        let observed = current
            .as_ref()
            .map_or(GoalVersion::INITIAL, |goal| goal.version);
        if observed != input.expected_version || host_gate_busy {
            let reason = if observed != input.expected_version {
                GoalCreateRefusal::Version
            } else {
                GoalCreateRefusal::Busy
            };
            let receipt = reject_tx(&mut tx, input, reason, observed, at).await?;
            tx.commit().await?;
            return Ok(receipt);
        }
        let actor = GoalActor::UserControl {
            principal_id: input.principal_id,
            request_id: input.request_id,
            command: GoalUserCommandKind::Create,
        };
        let mutation = GoalMutation {
            owner: input.owner.clone(),
            expected_version: input.expected_version,
            action_id: GoalText::parse(format!(
                "user-create:{}:{}",
                input.principal_id, input.request_id
            ))?,
            attribution: GoalAttribution {
                requested_by: actor.clone(),
                applied_by: actor,
            },
            change: GoalChange::Create {
                goal_id: GoalId::parse(Uuid::new_v4().to_string())?,
                objective: input.objective.clone(),
            },
        };
        match admission::require_settled_tx(&mut tx, &mutation, expected_transcript).await {
            Ok(()) => {}
            Err(GoalDbError::Busy) => {
                let receipt =
                    reject_tx(&mut tx, input, GoalCreateRefusal::Busy, observed, at).await?;
                tx.commit().await?;
                return Ok(receipt);
            }
            Err(error) => return Err(error),
        }
        let next = mutation.advance(current.as_ref())?;
        sqlx::query("INSERT INTO conversation_goal_user_requests (owner_id,principal_id,request_id,command,expected_version,goal_id,outcome,result_version,recorded_at_us) VALUES (?,?,?,'create',?,?,'accepted',?,?)")
            .bind(input.owner.as_str()).bind(input.principal_id.as_bytes().as_slice())
            .bind(input.request_id.as_bytes().as_slice()).bind(input.expected_version.get())
            .bind(next.goal_id.as_str()).bind(next.version.get()).bind(at.get())
            .execute(&mut *tx).await?;
        let entry = apply_goal_tx(
            &mut tx,
            &mutation,
            Some(expected_transcript),
            at,
            self.goal_control_installation(),
        )
        .await?;
        tx.commit().await?;
        Ok(GoalCreateReceipt {
            request_id: input.request_id,
            result: GoalCreateResult::Accepted {
                entry: Box::new(entry),
            },
        })
    }
}
