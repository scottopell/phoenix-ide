use serde::Serialize;
use uuid::Uuid;

use super::{
    admission, apply_goal_tx, current_tx, history_entry, GoalActor, GoalAttribution, GoalChange,
    GoalDbError, GoalDisposition, GoalError, GoalHistoryEntry, GoalId, GoalMutation, GoalText,
    GoalTimestampUs, GoalTx, GoalUserCommandKind, GoalVersion, ProductConversationId, Result,
};
use crate::{workflow::LocalAuthorityResult, Database};

#[cfg(test)]
#[derive(Clone, Copy)]
enum UserCommitCut {
    None,
    LostAcknowledgement,
    RolledBack,
    LostClassification,
}
use sqlx::Row;

pub struct GoalUserCommand {
    pub owner: ProductConversationId,
    pub principal_id: Uuid,
    pub request_id: Uuid,
    pub expected_version: GoalVersion,
    pub change: GoalUserChange,
}

pub enum GoalUserChange {
    Create {
        objective: GoalText,
    },
    State {
        goal_id: GoalId,
        disposition: GoalDisposition,
    },
}

impl GoalUserChange {
    fn command(&self) -> (&'static str, GoalUserCommandKind) {
        match self {
            Self::Create { .. } => ("create", GoalUserCommandKind::Create),
            Self::State { .. } => ("state", GoalUserCommandKind::State),
        }
    }

    fn same_change(&self, retained: &GoalChange) -> bool {
        match (self, retained) {
            (
                Self::Create { objective },
                GoalChange::Create {
                    objective: previous,
                    ..
                },
            ) => objective == previous,
            (
                Self::State {
                    goal_id,
                    disposition,
                },
                GoalChange::SetDisposition {
                    goal_id: previous,
                    disposition: previous_state,
                },
            ) => goal_id == previous && disposition == previous_state,
            _ => false,
        }
    }

    fn rejected_fields(&self) -> (String, Option<&str>, &'static str, Option<&str>) {
        match self {
            Self::Create { objective } => (
                Uuid::new_v4().to_string(),
                Some(objective.as_str()),
                "underway",
                None,
            ),
            Self::State {
                goal_id,
                disposition,
            } => {
                let (state, reason) = match disposition {
                    GoalDisposition::Underway => ("underway", None),
                    GoalDisposition::Blocked(reason) => ("blocked", Some(reason.as_str())),
                    GoalDisposition::Suspended(reason) => ("suspended", Some(reason.as_str())),
                    GoalDisposition::Complete(reason) => ("complete", Some(reason.as_str())),
                    GoalDisposition::Exhausted(reason) => ("exhausted", Some(reason.as_str())),
                };
                (goal_id.as_str().to_owned(), None, state, reason)
            }
        }
    }

    fn mutation_change(&self) -> Result<GoalChange> {
        Ok(match self {
            Self::Create { objective } => GoalChange::Create {
                goal_id: GoalId::parse(Uuid::new_v4().to_string())?,
                objective: objective.clone(),
            },
            Self::State {
                goal_id,
                disposition,
            } => GoalChange::SetDisposition {
                goal_id: goal_id.clone(),
                disposition: disposition.clone(),
            },
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalUserRefusal {
    Version,
    Busy,
    Transition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum GoalUserResult {
    Accepted {
        entry: Box<GoalHistoryEntry>,
    },
    Rejected {
        reason: GoalUserRefusal,
        observed_version: GoalVersion,
        recorded_at: GoalTimestampUs,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoalUserReceipt {
    pub request_id: Uuid,
    pub result: GoalUserResult,
}

async fn retained_tx(
    tx: &mut GoalTx<'_>,
    input: &GoalUserCommand,
) -> Result<Option<GoalUserReceipt>> {
    let row = sqlx::query("SELECT command,expected_version,goal_id,outcome,result_version,rejected_objective,rejected_state,rejected_reason,recorded_at_us FROM conversation_goal_user_requests WHERE owner_id=? AND principal_id=? AND request_id=?")
        .bind(input.owner.as_str()).bind(input.principal_id.as_bytes().as_slice())
        .bind(input.request_id.as_bytes().as_slice()).fetch_optional(&mut **tx).await?;
    let Some(row) = row else { return Ok(None) };
    if row.try_get::<String, _>("command")? != input.change.command().0
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
            if !input.change.same_change(&entry.mutation().change) {
                return Err(GoalDbError::RetryConflict);
            }
            GoalUserResult::Accepted {
                entry: Box::new(entry),
            }
        }
        outcome @ ("rejected_version" | "rejected_busy" | "rejected_transition") => {
            let same = match &input.change {
                GoalUserChange::Create { objective } => {
                    row.try_get::<String, _>("rejected_objective")? == objective.as_str()
                }
                GoalUserChange::State {
                    goal_id,
                    disposition,
                } => {
                    let state = disposition.state();
                    let (kind, reason) = super::state_columns(&state);
                    row.try_get::<String, _>("goal_id")? == goal_id.as_str()
                        && row.try_get::<String, _>("rejected_state")? == kind
                        && row
                            .try_get::<Option<String>, _>("rejected_reason")?
                            .as_deref()
                            == reason
                }
            };
            if !same {
                return Err(GoalDbError::RetryConflict);
            }
            GoalUserResult::Rejected {
                reason: match outcome {
                    "rejected_version" => GoalUserRefusal::Version,
                    "rejected_busy" => GoalUserRefusal::Busy,
                    _ => GoalUserRefusal::Transition,
                },
                observed_version: version,
                recorded_at: GoalTimestampUs::parse(row.try_get("recorded_at_us")?)?,
            }
        }
        _ => return Err(GoalDbError::Integrity("goal user command receipt outcome")),
    };
    Ok(Some(GoalUserReceipt {
        request_id: input.request_id,
        result,
    }))
}

async fn reject_tx(
    tx: &mut GoalTx<'_>,
    input: &GoalUserCommand,
    reason: GoalUserRefusal,
    observed_version: GoalVersion,
    at: GoalTimestampUs,
) -> Result<GoalUserReceipt> {
    let outcome = match reason {
        GoalUserRefusal::Version => "rejected_version",
        GoalUserRefusal::Busy => "rejected_busy",
        GoalUserRefusal::Transition => "rejected_transition",
    };
    let (goal_id, objective, state, reason_text) = input.change.rejected_fields();
    sqlx::query("INSERT INTO conversation_goal_user_requests (owner_id,principal_id,request_id,command,expected_version,goal_id,rejected_objective,rejected_state,rejected_reason,outcome,result_version,recorded_at_us) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(input.owner.as_str()).bind(input.principal_id.as_bytes().as_slice())
        .bind(input.request_id.as_bytes().as_slice()).bind(input.change.command().0).bind(input.expected_version.get())
        .bind(goal_id).bind(objective).bind(state).bind(reason_text).bind(outcome).bind(observed_version.get()).bind(at.get())
        .execute(&mut **tx).await?;
    Ok(GoalUserReceipt {
        request_id: input.request_id,
        result: GoalUserResult::Rejected {
            reason,
            observed_version,
            recorded_at: at,
        },
    })
}

impl Database {
    /// Read an exact user goal-command receipt after revalidating the ordinary owner's live target.
    ///
    /// # Errors
    /// Rejects unavailable owners, changed request payloads and malformed or missing schema.
    pub async fn retained_user_goal_command(
        &self,
        input: &GoalUserCommand,
        expected_transcript: &str,
    ) -> Result<Option<GoalUserReceipt>> {
        let mut tx = self.pool().begin().await?;
        admission::require_target_tx(&mut tx, &input.owner, expected_transcript).await?;
        let retained = retained_tx(&mut tx, input).await?;
        tx.commit().await?;
        Ok(retained)
    }

    /// Apply a host-authenticated Create or State command while the caller holds both existing admission gates.
    ///
    /// # Errors
    /// Rejects changed/unavailable targets, conflicting request identity and storage failures.
    pub async fn apply_user_goal_command<F: FnOnce()>(
        &self,
        input: &GoalUserCommand,
        expected_transcript: &str,
        at: GoalTimestampUs,
        on_authority_lost: F,
    ) -> LocalAuthorityResult<Result<GoalUserReceipt>> {
        self.user_goal_command(
            input,
            expected_transcript,
            at,
            false,
            on_authority_lost,
            #[cfg(test)]
            UserCommitCut::None,
        )
        .await
    }

    /// Retain a busy refusal when the host could not acquire its existing admission gates.
    ///
    /// # Errors
    /// Rejects changed/unavailable targets, conflicting request identity and storage failures.
    pub async fn reject_busy_user_goal_command<F: FnOnce()>(
        &self,
        input: &GoalUserCommand,
        expected_transcript: &str,
        at: GoalTimestampUs,
        on_authority_lost: F,
    ) -> LocalAuthorityResult<Result<GoalUserReceipt>> {
        self.user_goal_command(
            input,
            expected_transcript,
            at,
            true,
            on_authority_lost,
            #[cfg(test)]
            UserCommitCut::None,
        )
        .await
    }

    async fn user_goal_command<F: FnOnce()>(
        &self,
        input: &GoalUserCommand,
        expected_transcript: &str,
        at: GoalTimestampUs,
        host_gate_busy: bool,
        on_authority_lost: F,
        #[cfg(test)] cut: UserCommitCut,
    ) -> LocalAuthorityResult<Result<GoalUserReceipt>> {
        let mut connection = match self.pool().acquire().await {
            Ok(connection) => connection,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let mut tx = match sqlx::Connection::begin_with(&mut *connection, "BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(error) => return LocalAuthorityResult::DurableFactEstablished(Err(error.into())),
        };
        let mut authority = super::control::ControlCommitAuthority::new(on_authority_lost);
        let result: Result<GoalUserReceipt> = async {
        admission::require_target_tx(&mut tx, &input.owner, expected_transcript).await?;
        if let Some(retained) = retained_tx(&mut tx, input).await? {
            return Ok(retained);
        }
        let current = current_tx(&mut tx, &input.owner).await?;
        let observed = current
            .as_ref()
            .map_or(GoalVersion::INITIAL, super::GoalSnapshot::version);
        if observed != input.expected_version || host_gate_busy {
            let reason = if observed == input.expected_version {
                GoalUserRefusal::Busy
            } else {
                GoalUserRefusal::Version
            };
            let receipt = reject_tx(&mut tx, input, reason, observed, at).await?;
            return Ok(receipt);
        }
        let actor = GoalActor::UserControl {
            principal_id: input.principal_id,
            request_id: input.request_id,
            command: input.change.command().1,
        };
        let mutation = GoalMutation {
            owner: input.owner.clone(),
            expected_version: input.expected_version,
            action_id: GoalText::parse(format!(
                "user-goal:{}:{}",
                input.principal_id, input.request_id
            ))?,
            attribution: GoalAttribution {
                requested_by: actor.clone(),
                applied_by: actor,
            },
            change: input.change.mutation_change()?,
        };
        match admission::require_settled_tx(&mut tx, &mutation, expected_transcript).await {
            Ok(()) => {}
            Err(GoalDbError::Busy) => {
                let receipt =
                    reject_tx(&mut tx, input, GoalUserRefusal::Busy, observed, at).await?;
                    return Ok(receipt);
            }
            Err(error) => return Err(error),
        }
        let next = match mutation.advance(current.as_ref()) {
            Ok(next) => next,
            Err(GoalError::Inapplicable | GoalError::Conflict)
                if matches!(&input.change, GoalUserChange::State { .. }) =>
            {
                let receipt =
                    reject_tx(&mut tx, input, GoalUserRefusal::Transition, observed, at).await?;
                    return Ok(receipt);
            }
            Err(error) => return Err(error.into()),
        };
        sqlx::query("INSERT INTO conversation_goal_user_requests (owner_id,principal_id,request_id,command,expected_version,goal_id,outcome,result_version,recorded_at_us) VALUES (?,?,?,?,?,?,'accepted',?,?)")
            .bind(input.owner.as_str()).bind(input.principal_id.as_bytes().as_slice())
            .bind(input.request_id.as_bytes().as_slice()).bind(input.change.command().0).bind(input.expected_version.get())
            .bind(next.goal_id().as_str()).bind(next.version().get()).bind(at.get())
            .execute(&mut *tx).await?;
        let entry = apply_goal_tx(
            &mut tx,
            &mutation,
            Some(expected_transcript),
            at,
            self.goal_write_policies(),
        )
        .await?;
        crate::coordinator_watches::goal_facts::settled_change_tx(
            &mut tx,
            self.goal_control_installation(),
            &entry,
            expected_transcript,
        )
        .await
        .map_err(GoalDbError::from)?;
        Ok(GoalUserReceipt {
            request_id: input.request_id,
            result: GoalUserResult::Accepted {
                entry: Box::new(entry),
            },
        })
        }.await;
        let attempted = match result {
            Ok(receipt) => {
                #[cfg(not(test))]
                let committed = tx.commit().await;
                #[cfg(test)]
                let committed = if matches!(cut, UserCommitCut::RolledBack) {
                    tx.rollback()
                        .await
                        .and(Err(sqlx::Error::Protocol("injected rollback".into())))
                } else {
                    tx.commit().await
                };
                #[cfg(test)]
                let committed = match cut {
                    UserCommitCut::LostAcknowledgement | UserCommitCut::LostClassification
                        if committed.is_ok() =>
                    {
                        Err(sqlx::Error::Protocol(
                            "injected lost acknowledgement".into(),
                        ))
                    }
                    _ => committed,
                };
                match committed {
                    Ok(()) => {
                        authority.classified();
                        return LocalAuthorityResult::DurableFactEstablished(Ok(receipt));
                    }
                    Err(error) => GoalDbError::Sqlx(error),
                }
            }
            Err(error) => {
                if tx.rollback().await.is_ok() {
                    authority.classified();
                    return LocalAuthorityResult::DurableFactEstablished(Err(error));
                }
                error
            }
        };
        let mut lookup =
            match sqlx::Connection::begin_with(&mut *connection, "BEGIN IMMEDIATE").await {
                Ok(tx) => tx,
                Err(_) => return LocalAuthorityResult::DurableFactUnclassified,
            };
        let retained = retained_tx(&mut lookup, input).await;
        #[cfg(test)]
        let retained = if matches!(cut, UserCommitCut::LostClassification) {
            Err(GoalDbError::Sqlx(sqlx::Error::Protocol(
                "injected receipt lookup failure".into(),
            )))
        } else {
            retained
        };
        let released = lookup.rollback().await;
        match (retained, released) {
            (Ok(Some(receipt)), Ok(())) => {
                authority.classified();
                LocalAuthorityResult::DurableFactEstablished(Ok(receipt))
            }
            (Ok(None), Ok(())) => {
                authority.classified();
                LocalAuthorityResult::DurableFactEstablished(Err(attempted))
            }
            _ => LocalAuthorityResult::DurableFactUnclassified,
        }
    }
}

#[cfg(test)]
mod tests;
