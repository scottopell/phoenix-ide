use phoenix_core::domain::conversation_goal::GoalAgentSource;
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
    LostRollbackAcknowledgement,
}
use sqlx::Row;

pub struct GoalUserCommand {
    pub owner: ProductConversationId,
    pub principal_id: Uuid,
    pub request_id: Uuid,
    pub expected_version: GoalVersion,
    pub change: GoalUserChange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy)]
pub(super) enum GoalReceiptStore {
    Owner,
    Coordinator,
}

impl GoalControlAuthority<'_> {
    fn receipts(&self) -> GoalReceiptStore {
        match self {
            Self::Owner => GoalReceiptStore::Owner,
            Self::Coordinator { .. } | Self::RemoteCoordinator { .. } => {
                GoalReceiptStore::Coordinator
            }
        }
    }
}

pub(super) async fn retained_tx(
    tx: &mut GoalTx<'_>,
    input: &GoalUserCommand,
    store: GoalReceiptStore,
) -> Result<Option<GoalUserReceipt>> {
    let row = match store {
        GoalReceiptStore::Owner => sqlx::query("SELECT command,expected_version,goal_id,outcome,result_version,rejected_objective,rejected_state,rejected_reason,recorded_at_us FROM conversation_goal_user_requests WHERE owner_id=? AND principal_id=? AND request_id=?")
            .bind(input.owner.as_str()).bind(input.principal_id.as_bytes().as_slice())
            .bind(input.request_id.as_bytes().as_slice()).fetch_optional(&mut **tx).await?,
        GoalReceiptStore::Coordinator => sqlx::query("SELECT CASE p.operation WHEN 'state' THEN 'state' ELSE 'create' END AS command,p.expected_version,p.observed_goal_id AS goal_id,r.outcome,r.result_version,p.objective AS rejected_objective,COALESCE(p.disposition,'underway') AS rejected_state,p.reason AS rejected_reason,r.recorded_at_us FROM conversation_goal_coordinator_receipts r JOIN conversation_goal_preparations p ON p.reference=r.preparation_reference WHERE r.owner_id=? AND r.preparation_reference=?")
            .bind(input.owner.as_str()).bind(input.request_id.as_bytes().as_slice())
            .fetch_optional(&mut **tx).await?,
    };
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
    authority: &GoalControlAuthority<'_>,
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
    match authority {
        GoalControlAuthority::Owner => sqlx::query("INSERT INTO conversation_goal_user_requests (owner_id,principal_id,request_id,command,expected_version,goal_id,rejected_objective,rejected_state,rejected_reason,outcome,result_version,recorded_at_us) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(input.owner.as_str()).bind(input.principal_id.as_bytes().as_slice())
            .bind(input.request_id.as_bytes().as_slice()).bind(input.change.command().0).bind(input.expected_version.get())
            .bind(goal_id).bind(objective).bind(state).bind(reason_text).bind(outcome)
            .bind(observed_version.get()).bind(at.get()).execute(&mut **tx).await?,
        GoalControlAuthority::Coordinator { .. } | GoalControlAuthority::RemoteCoordinator { .. } => sqlx::query("INSERT INTO conversation_goal_coordinator_receipts (preparation_reference,owner_id,outcome,result_version,recorded_at_us) VALUES (?,?,?,?,?)")
            .bind(input.request_id.as_bytes().as_slice()).bind(input.owner.as_str()).bind(outcome)
            .bind(observed_version.get()).bind(at.get()).execute(&mut **tx).await?,
    };
    Ok(GoalUserReceipt {
        request_id: input.request_id,
        result: GoalUserResult::Rejected {
            reason,
            observed_version,
            recorded_at: at,
        },
    })
}

enum GoalControlAuthority<'a> {
    Owner,
    Coordinator {
        caller: &'a str,
        source: &'a GoalAgentSource,
    },
    RemoteCoordinator {
        enrollment_id: Uuid,
        source: phoenix_core::domain::conversation_goal::RemoteGoalAgentSource,
    },
}

impl GoalControlAuthority<'_> {
    async fn require(&self, tx: &mut GoalTx<'_>) -> Result<()> {
        match self {
            Self::Coordinator { caller, .. } => {
                super::prepared::require_coordinator(tx, caller).await?;
            }
            Self::RemoteCoordinator { .. } | Self::Owner => {}
        }
        Ok(())
    }

    fn actor(&self, input: &GoalUserCommand) -> GoalActor {
        match self {
            Self::Owner => GoalActor::UserControl {
                principal_id: input.principal_id,
                request_id: input.request_id,
                command: input.change.command().1,
            },
            Self::Coordinator { source, .. } => GoalActor::Coordinator((*source).clone()),
            Self::RemoteCoordinator {
                enrollment_id,
                source,
            } => GoalActor::RemoteCoordinator(
                phoenix_core::domain::conversation_goal::EnrolledRemoteGoalAgentSource::new(
                    *enrollment_id,
                    source.source.clone(),
                ),
            ),
        }
    }
}

struct GoalUserCommandOutcome {
    receipt: GoalUserReceipt,
    committed_goal: Option<super::GoalVersionCommittedNotification>,
}

impl Database {
    /// Read an exact Coordinator receipt for a retained preparation.
    ///
    /// # Errors
    /// Rejects a non-current Coordinator, a missing or changed preparation,
    /// conflicting receipt identity, malformed data, or a storage failure.
    pub async fn retained_coordinator_goal_command(
        &self,
        caller: &str,
        input: &GoalUserCommand,
    ) -> Result<Option<GoalUserReceipt>> {
        let mut tx = self.pool().begin().await?;
        super::prepared::require_coordinator(&mut tx, caller).await?;
        super::prepared::require_retained_preparation(&mut tx, caller, input).await?;
        let result = retained_tx(&mut tx, input, GoalReceiptStore::Coordinator).await?;
        tx.rollback().await?;
        Ok(result)
    }

    /// Read an immutable remote receipt, scoped to the peer that prepared the reference.
    ///
    /// # Errors
    /// Rejects a non-current peer, a missing or changed preparation, conflicting
    /// receipt identity, malformed retained data, or a storage failure.
    pub async fn retained_remote_coordinator_goal_command(
        &self,
        authority: &crate::federation_enrollment::FederationSendAuthority,
        reference: Uuid,
    ) -> Result<Option<GoalUserReceipt>> {
        let mut tx = self.pool().begin().await?;
        let prepared =
            super::prepared::remote_preparation_tx(&mut tx, authority, reference).await?;
        let input = prepared.command(Uuid::nil())?;
        let receipt = retained_tx(&mut tx, &input, GoalReceiptStore::Coordinator).await?;
        tx.rollback().await?;
        Ok(receipt)
    }

    /// Apply only the action retained under an authenticated peer and reference.
    pub async fn apply_remote_coordinator_goal_command<F: FnOnce()>(
        &self,
        authority: &crate::federation_enrollment::FederationSendAuthority,
        reference: Uuid,
        host_gate_busy: bool,
        clock: impl FnOnce() -> Result<GoalTimestampUs>,
        on_authority_lost: F,
    ) -> LocalAuthorityResult<Result<GoalUserReceipt>> {
        self.apply_remote_coordinator_goal_command_with_claim_commit(
            authority,
            reference,
            host_gate_busy,
            clock,
            on_authority_lost,
            #[cfg(test)]
            super::prepared::RemoteGoalClaimCommitCut::None,
            #[cfg(test)]
            None,
        )
        .await
    }

    #[cfg_attr(test, allow(clippy::too_many_arguments))]
    pub(super) async fn apply_remote_coordinator_goal_command_with_claim_commit<F: FnOnce()>(
        &self,
        authority: &crate::federation_enrollment::FederationSendAuthority,
        reference: Uuid,
        host_gate_busy: bool,
        clock: impl FnOnce() -> Result<GoalTimestampUs>,
        on_authority_lost: F,
        #[cfg(test)] claim_commit: super::prepared::RemoteGoalClaimCommitCut,
        #[cfg(test)] claim_pause: Option<super::prepared::RemoteGoalClaimPause>,
    ) -> LocalAuthorityResult<Result<GoalUserReceipt>> {
        let mut claim_authority = super::control::ControlCommitAuthority::new(on_authority_lost);
        match self
            .claim_remote_coordinator_goal_preparation(
                authority,
                reference,
                #[cfg(test)]
                claim_commit,
            )
            .await
        {
            Ok(super::prepared::RemoteCoordinatorGoalClaim::Claimed(action)) => {
                #[cfg(feature = "test-support")]
                if let Some(pause) = super::prepared::take_remote_goal_claim_pause(reference) {
                    pause.after_claim().await;
                }
                #[cfg(test)]
                if let Some(pause) = claim_pause {
                    pause.after_claim().await;
                }
                let super::prepared::RemoteClaimedGoalAction {
                    command,
                    enrollment_id,
                    source,
                } = *action;
                let result = self
                    .control_goal_command_with_notification(
                        &command,
                        "",
                        GoalControlAuthority::RemoteCoordinator {
                            enrollment_id,
                            source,
                        },
                        clock,
                        host_gate_busy,
                        || {},
                        #[cfg(test)]
                        UserCommitCut::None,
                    )
                    .await;
                if matches!(&result, LocalAuthorityResult::DurableFactEstablished(_)) {
                    claim_authority.classified();
                }
                self.notify_goal_command_outcome(&result);
                Self::without_goal_notification(result)
            }
            Ok(super::prepared::RemoteCoordinatorGoalClaim::Replay(receipt)) => {
                claim_authority.classified();
                LocalAuthorityResult::DurableFactEstablished(Ok(receipt))
            }
            Ok(super::prepared::RemoteCoordinatorGoalClaim::Unclassified) => {
                LocalAuthorityResult::DurableFactUnclassified
            }
            Err(error) => {
                claim_authority.classified();
                LocalAuthorityResult::DurableFactEstablished(Err(error))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn apply_coordinator_goal_command<F: FnOnce()>(
        &self,
        caller: &str,
        source: &GoalAgentSource,
        input: &GoalUserCommand,
        expected_transcript: &str,
        host_gate_busy: bool,
        clock: impl FnOnce() -> Result<GoalTimestampUs>,
        on_authority_lost: F,
    ) -> LocalAuthorityResult<Result<GoalUserReceipt>> {
        let result = self
            .control_goal_command_with_notification(
                input,
                expected_transcript,
                GoalControlAuthority::Coordinator { caller, source },
                clock,
                host_gate_busy,
                on_authority_lost,
                #[cfg(test)]
                UserCommitCut::None,
            )
            .await;
        self.notify_goal_command_outcome(&result);
        Self::without_goal_notification(result)
    }

    /// Read an exact receipt for an ordinary owner before checking admission for a new write.
    ///
    /// The transcript argument is a prior caller sample; this read resolves the live target itself.
    ///
    /// # Errors
    /// Rejects unavailable owners, changed request payloads and malformed or missing schema.
    pub async fn retained_user_goal_command(
        &self,
        input: &GoalUserCommand,
        _expected_transcript: &str,
    ) -> Result<Option<GoalUserReceipt>> {
        let mut tx = self.pool().begin().await?;
        let live = Self::get_ordinary_product_conversation_on(&mut tx, &input.owner).await?;
        let retained = retained_tx(&mut tx, input, GoalReceiptStore::Owner).await?;
        if retained.is_none() {
            admission::require_target_tx(&mut tx, &input.owner, &live.latest_transcript_row_id)
                .await?;
        }
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
        clock: impl FnOnce() -> Result<GoalTimestampUs>,
        on_authority_lost: F,
    ) -> LocalAuthorityResult<Result<GoalUserReceipt>> {
        let result = self
            .control_goal_command_with_notification(
                input,
                expected_transcript,
                GoalControlAuthority::Owner,
                clock,
                false,
                on_authority_lost,
                #[cfg(test)]
                UserCommitCut::None,
            )
            .await;
        self.notify_goal_command_outcome(&result);
        Self::without_goal_notification(result)
    }

    /// Retain a busy refusal when the host could not acquire its existing admission gates.
    ///
    /// # Errors
    /// Rejects changed/unavailable targets, conflicting request identity and storage failures.
    pub async fn reject_busy_user_goal_command<F: FnOnce()>(
        &self,
        input: &GoalUserCommand,
        expected_transcript: &str,
        clock: impl FnOnce() -> Result<GoalTimestampUs>,
        on_authority_lost: F,
    ) -> LocalAuthorityResult<Result<GoalUserReceipt>> {
        self.user_goal_command(
            input,
            expected_transcript,
            clock,
            true,
            on_authority_lost,
            #[cfg(test)]
            UserCommitCut::None,
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn user_goal_command_tx(
        &self,
        tx: &mut GoalTx<'_>,
        input: &GoalUserCommand,
        expected_transcript: &str,
        requester: &GoalControlAuthority<'_>,
        clock: impl FnOnce() -> Result<GoalTimestampUs>,
        host_gate_busy: bool,
    ) -> Result<GoalUserCommandOutcome> {
        requester.require(tx).await?;
        let live = Self::get_ordinary_product_conversation_on(tx, &input.owner).await?;
        if let GoalControlAuthority::Coordinator { caller, source } = requester {
            super::prepared::require_exact_preparation(tx, caller, source, input).await?;
        }
        if let Some(retained) = retained_tx(tx, input, requester.receipts()).await? {
            return Ok(GoalUserCommandOutcome {
                receipt: retained,
                committed_goal: None,
            });
        }
        if matches!(requester, GoalControlAuthority::RemoteCoordinator { .. }) {
            let apply_state: String = sqlx::query_scalar(
                "SELECT apply_state FROM conversation_goal_preparations WHERE reference=?",
            )
            .bind(input.request_id.as_bytes().as_slice())
            .fetch_one(&mut **tx)
            .await?;
            if apply_state != "claimed" {
                return Err(GoalDbError::TargetChanged);
            }
        }

        let target = if host_gate_busy
            || matches!(requester, GoalControlAuthority::RemoteCoordinator { .. })
        {
            live.latest_transcript_row_id.as_str()
        } else {
            expected_transcript
        };
        admission::require_target_tx(tx, &input.owner, target).await?;
        let at = clock()?;
        let current = current_tx(tx, &input.owner).await?;
        let observed = current
            .as_ref()
            .map_or(GoalVersion::INITIAL, super::GoalSnapshot::version);
        if observed != input.expected_version || host_gate_busy {
            let reason = if observed == input.expected_version {
                GoalUserRefusal::Busy
            } else {
                GoalUserRefusal::Version
            };
            let receipt = reject_tx(tx, input, requester, reason, observed, at).await?;
            return Ok(GoalUserCommandOutcome {
                receipt,
                committed_goal: None,
            });
        }
        let actor = requester.actor(input);
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
        match admission::require_settled_tx(tx, &mutation, target).await {
            Ok(()) => {}
            Err(GoalDbError::Busy) => {
                let receipt =
                    reject_tx(tx, input, requester, GoalUserRefusal::Busy, observed, at).await?;
                return Ok(GoalUserCommandOutcome {
                    receipt,
                    committed_goal: None,
                });
            }
            Err(error) => return Err(error),
        }
        let next = match mutation.advance(current.as_ref()) {
            Ok(next) => next,
            Err(GoalError::Inapplicable | GoalError::Conflict)
                if matches!(&input.change, GoalUserChange::State { .. }) =>
            {
                let receipt = reject_tx(
                    tx,
                    input,
                    requester,
                    GoalUserRefusal::Transition,
                    observed,
                    at,
                )
                .await?;
                return Ok(GoalUserCommandOutcome {
                    receipt,
                    committed_goal: None,
                });
            }
            Err(error) => return Err(error.into()),
        };
        if matches!(requester, GoalControlAuthority::RemoteCoordinator { .. })
            && matches!(&mutation.change, GoalChange::Create { .. })
        {
            match super::require_auto_continue_tx(tx, &input.owner).await {
                Ok(()) => {}
                Err(GoalDbError::AutoContinueDisabled) => {
                    let receipt = reject_tx(
                        tx,
                        input,
                        requester,
                        GoalUserRefusal::Transition,
                        observed,
                        at,
                    )
                    .await?;
                    return Ok(GoalUserCommandOutcome {
                        receipt,
                        committed_goal: None,
                    });
                }
                Err(error) => return Err(error),
            }
        }
        match requester {
            GoalControlAuthority::Owner => sqlx::query("INSERT INTO conversation_goal_user_requests (owner_id,principal_id,request_id,command,expected_version,goal_id,outcome,result_version,recorded_at_us) VALUES (?,?,?,?,?,?,'accepted',?,?)")
                .bind(input.owner.as_str()).bind(input.principal_id.as_bytes().as_slice())
                .bind(input.request_id.as_bytes().as_slice()).bind(input.change.command().0).bind(input.expected_version.get())
                .bind(next.goal_id().as_str()).bind(next.version().get()).bind(at.get())
                .execute(&mut **tx).await?,
            GoalControlAuthority::Coordinator { .. } | GoalControlAuthority::RemoteCoordinator { .. } => sqlx::query("INSERT INTO conversation_goal_coordinator_receipts (preparation_reference,owner_id,outcome,result_version,recorded_at_us) VALUES (?,?,'accepted',?,?)")
                .bind(input.request_id.as_bytes().as_slice()).bind(input.owner.as_str())
                .bind(next.version().get()).bind(at.get()).execute(&mut **tx).await?,
        };
        let entry =
            apply_goal_tx(tx, &mutation, Some(target), at, self.goal_write_policies()).await?;
        crate::coordinator_watches::goal_facts::settled_change_tx(
            tx,
            self.goal_control_installation(),
            &entry,
            target,
        )
        .await
        .map_err(GoalDbError::from)?;
        let committed_goal = super::GoalVersionCommittedNotification::new(
            input.owner.clone(),
            entry.snapshot().version(),
        );
        Ok(GoalUserCommandOutcome {
            receipt: GoalUserReceipt {
                request_id: input.request_id,
                result: GoalUserResult::Accepted {
                    entry: Box::new(entry),
                },
            },
            committed_goal: Some(committed_goal),
        })
    }

    async fn user_goal_command<F: FnOnce()>(
        &self,
        input: &GoalUserCommand,
        expected_transcript: &str,
        clock: impl FnOnce() -> Result<GoalTimestampUs>,
        host_gate_busy: bool,
        on_authority_lost: F,
        #[cfg(test)] cut: UserCommitCut,
    ) -> LocalAuthorityResult<Result<GoalUserReceipt>> {
        let result = self
            .control_goal_command_with_notification(
                input,
                expected_transcript,
                GoalControlAuthority::Owner,
                clock,
                host_gate_busy,
                on_authority_lost,
                #[cfg(test)]
                cut,
            )
            .await;
        Self::without_goal_notification(result)
    }

    fn notify_goal_command_outcome(
        &self,
        result: &LocalAuthorityResult<Result<GoalUserCommandOutcome>>,
    ) {
        if let LocalAuthorityResult::DurableFactEstablished(Ok(outcome)) = result {
            if let Some(committed_goal) = &outcome.committed_goal {
                self.notify_goal_versions_committed(std::slice::from_ref(committed_goal));
            }
        }
    }

    fn without_goal_notification(
        result: LocalAuthorityResult<Result<GoalUserCommandOutcome>>,
    ) -> LocalAuthorityResult<Result<GoalUserReceipt>> {
        match result {
            LocalAuthorityResult::DurableFactEstablished(result) => {
                LocalAuthorityResult::DurableFactEstablished(result.map(|outcome| outcome.receipt))
            }
            LocalAuthorityResult::DurableFactUnclassified => {
                LocalAuthorityResult::DurableFactUnclassified
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    #[cfg_attr(test, allow(clippy::too_many_arguments))]
    async fn control_goal_command_with_notification<F: FnOnce()>(
        &self,
        input: &GoalUserCommand,
        expected_transcript: &str,
        requester: GoalControlAuthority<'_>,
        clock: impl FnOnce() -> Result<GoalTimestampUs>,
        host_gate_busy: bool,
        on_authority_lost: F,
        #[cfg(test)] cut: UserCommitCut,
    ) -> LocalAuthorityResult<Result<GoalUserCommandOutcome>> {
        let remote_claimed = matches!(&requester, GoalControlAuthority::RemoteCoordinator { .. });
        let mut on_authority_lost = Some(on_authority_lost);
        let authority = remote_claimed.then(|| {
            super::control::ControlCommitAuthority::new(
                on_authority_lost
                    .take()
                    .expect("authority-loss callback is available"),
            )
        });
        let mut connection = match self.pool().acquire().await {
            Ok(connection) => connection,
            Err(_) if remote_claimed => {
                return LocalAuthorityResult::DurableFactUnclassified;
            }
            Err(error) => {
                return LocalAuthorityResult::DurableFactEstablished(Err(error.into()));
            }
        };
        let mut tx = match sqlx::Connection::begin_with(&mut *connection, "BEGIN IMMEDIATE").await {
            Ok(tx) => tx,
            Err(_) if remote_claimed => {
                return LocalAuthorityResult::DurableFactUnclassified;
            }
            Err(error) => {
                return LocalAuthorityResult::DurableFactEstablished(Err(error.into()));
            }
        };
        let mut authority = authority.unwrap_or_else(|| {
            super::control::ControlCommitAuthority::new(
                on_authority_lost
                    .take()
                    .expect("authority-loss callback is available"),
            )
        });
        let result = self
            .user_goal_command_tx(
                &mut tx,
                input,
                expected_transcript,
                &requester,
                clock,
                host_gate_busy,
            )
            .await;
        let (attempted, committed_goal) = match result {
            Ok(outcome) => {
                let committed_goal = outcome.committed_goal.clone();
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
                    UserCommitCut::None
                    | UserCommitCut::LostAcknowledgement
                    | UserCommitCut::RolledBack
                    | UserCommitCut::LostClassification
                    | UserCommitCut::LostRollbackAcknowledgement => committed,
                };
                match committed {
                    Ok(()) => {
                        authority.classified();
                        return LocalAuthorityResult::DurableFactEstablished(Ok(outcome));
                    }
                    Err(error) => (GoalDbError::Sqlx(error), committed_goal),
                }
            }
            Err(error) => {
                let rolled_back = tx.rollback().await.is_ok();
                #[cfg(test)]
                let rolled_back =
                    rolled_back && !matches!(cut, UserCommitCut::LostRollbackAcknowledgement);
                if rolled_back && !remote_claimed {
                    authority.classified();
                    return LocalAuthorityResult::DurableFactEstablished(Err(error));
                }
                (error, None)
            }
        };
        let Ok(mut lookup) =
            sqlx::Connection::begin_with(&mut *connection, "BEGIN IMMEDIATE").await
        else {
            return LocalAuthorityResult::DurableFactUnclassified;
        };
        let retained = async {
            if let GoalControlAuthority::Coordinator { caller, source } = &requester {
                super::prepared::require_exact_preparation(&mut lookup, caller, source, input)
                    .await?;
            }
            retained_tx(&mut lookup, input, requester.receipts()).await
        }
        .await;
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
                LocalAuthorityResult::DurableFactEstablished(Ok(GoalUserCommandOutcome {
                    receipt,
                    committed_goal,
                }))
            }
            (Ok(None), Ok(())) if !remote_claimed => {
                authority.classified();
                LocalAuthorityResult::DurableFactEstablished(Err(attempted))
            }
            (Err(GoalDbError::RetryConflict), Ok(())) if !remote_claimed => {
                authority.classified();
                LocalAuthorityResult::DurableFactEstablished(Err(GoalDbError::RetryConflict))
            }
            _ => LocalAuthorityResult::DurableFactUnclassified,
        }
    }
}

#[cfg(test)]
mod tests;
