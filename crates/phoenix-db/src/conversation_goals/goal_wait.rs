use std::time::Duration;

use super::{GoalDbError, Result};
use phoenix_core::domain::close::TranscriptConversationId;
use phoenix_core::domain::conversation_goal::{GoalHistoryEntry, GoalVersion};
use phoenix_core::domain::product_conversation::ProductConversationId;
use sqlx::{Sqlite, Transaction};

const MAX_GOAL_WAIT_TIMEOUT: Duration = Duration::from_secs(25);
use crate::Database;

/// A bounded request to wait until a Goal reaches at least a specific version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoordinatorGoalWaitRequest {
    minimum_version: GoalVersion,
    timeout: Duration,
}

/// A timeout rejected while constructing a Coordinator Goal wait request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CoordinatorGoalWaitRequestError {
    #[error("Coordinator Goal wait timeout must be greater than zero")]
    ZeroTimeout,
    #[error("Coordinator Goal wait timeout is too large")]
    TimeoutTooLarge,
}

impl CoordinatorGoalWaitRequest {
    /// Build a bounded Goal version wait.
    ///
    /// # Errors
    /// Rejects zero or timeouts longer than 25 seconds.
    pub fn new(
        minimum_version: GoalVersion,
        timeout: Duration,
    ) -> std::result::Result<Self, CoordinatorGoalWaitRequestError> {
        if timeout.is_zero() {
            return Err(CoordinatorGoalWaitRequestError::ZeroTimeout);
        }
        if timeout > MAX_GOAL_WAIT_TIMEOUT {
            return Err(CoordinatorGoalWaitRequestError::TimeoutTooLarge);
        }
        Ok(Self {
            minimum_version,
            timeout,
        })
    }

    #[must_use]
    pub const fn minimum_version(self) -> GoalVersion {
        self.minimum_version
    }

    #[must_use]
    pub const fn timeout(self) -> Duration {
        self.timeout
    }
}

/// Whether a Goal version wait observed its requested version before its deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorGoalWaitStatus {
    Satisfied,
    TimedOut,
}

/// One authoritative Coordinator Goal read produced by a bounded version wait.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CoordinatorGoalWaitResult {
    current: Option<GoalHistoryEntry>,
    history: Vec<GoalHistoryEntry>,
    destination_transcript_id: TranscriptConversationId,
    status: CoordinatorGoalWaitStatus,
}

enum GoalReadAuthority<'a> {
    Coordinator(&'a str),
    Federated(&'a crate::FederationSendAuthority),
}

impl GoalReadAuthority<'_> {
    async fn require(&self, tx: &mut Transaction<'_, Sqlite>) -> Result<()> {
        match self {
            Self::Coordinator(caller) => {
                super::prepared::require_coordinator(tx, caller).await?;
            }
            Self::Federated(authority) => {
                crate::federation_enrollment::require_federation_send_authority_tx(tx, authority)
                    .await?;
            }
        }
        Ok(())
    }
}

impl CoordinatorGoalWaitResult {
    #[must_use]
    pub fn current(&self) -> Option<&GoalHistoryEntry> {
        self.current.as_ref()
    }

    #[must_use]
    pub fn history(&self) -> &[GoalHistoryEntry] {
        &self.history
    }

    #[must_use]
    pub fn destination_transcript_id(&self) -> &TranscriptConversationId {
        &self.destination_transcript_id
    }

    #[must_use]
    pub const fn status(&self) -> CoordinatorGoalWaitStatus {
        self.status
    }

    fn current_version(&self) -> GoalVersion {
        self.current
            .as_ref()
            .map_or(GoalVersion::INITIAL, |entry| entry.snapshot().version())
    }

    fn with_status(mut self, status: CoordinatorGoalWaitStatus) -> Self {
        self.status = status;
        self
    }
}

impl Database {
    async fn read_goal_wait_result(
        &self,
        authority: &GoalReadAuthority<'_>,
        owner: &ProductConversationId,
        after: GoalVersion,
    ) -> Result<CoordinatorGoalWaitResult> {
        let mut tx = self.pool().begin().await?;
        authority.require(&mut tx).await?;
        let live = Self::get_ordinary_product_conversation_on(&mut tx, owner).await?;
        super::admission::require_target_tx(&mut tx, owner, &live.latest_transcript_row_id).await?;
        let destination_transcript_id =
            TranscriptConversationId::parse(live.latest_transcript_row_id)
                .map_err(|_| GoalDbError::Integrity("ordinary destination transcript id"))?;
        let row = sqlx::query("SELECT h.*,g.objective FROM conversation_goal_history h JOIN conversation_goal_ids g USING(goal_id,owner_id) WHERE owner_id=? ORDER BY version DESC LIMIT 1")
            .bind(owner.as_str()).fetch_optional(&mut *tx).await?;
        let current = match row {
            Some(row) => Some(super::history_entry(&mut tx, &row).await?),
            None => None,
        };
        let history = super::history_tx(&mut tx, owner, after).await?;
        tx.rollback().await?;
        Ok(CoordinatorGoalWaitResult {
            current,
            history,
            destination_transcript_id,
            status: CoordinatorGoalWaitStatus::TimedOut,
        })
    }

    async fn wait_for_goals(
        &self,
        authority: GoalReadAuthority<'_>,
        owner: &ProductConversationId,
        after: GoalVersion,
        request: &CoordinatorGoalWaitRequest,
    ) -> Result<CoordinatorGoalWaitResult> {
        let mut notifications = self.goal_version_committed.subscribe();
        let deadline = tokio::time::Instant::now()
            .checked_add(request.timeout())
            .ok_or(GoalDbError::Invalid("Coordinator Goal wait timeout"))?;

        loop {
            let read = self.read_goal_wait_result(&authority, owner, after).await?;
            if read.current_version() >= request.minimum_version() {
                return Ok(read.with_status(CoordinatorGoalWaitStatus::Satisfied));
            }

            match tokio::time::timeout_at(deadline, notifications.recv()).await {
                Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    return Err(GoalDbError::Integrity("goal notification channel closed"));
                }
                Err(_) => {
                    let final_read = self.read_goal_wait_result(&authority, owner, after).await?;
                    let status = if final_read.current_version() >= request.minimum_version() {
                        CoordinatorGoalWaitStatus::Satisfied
                    } else {
                        CoordinatorGoalWaitStatus::TimedOut
                    };
                    return Ok(final_read.with_status(status));
                }
            }
        }
    }

    /// Wait for an ordinary `ProductConversation`'s Goal to reach `minimum_version`.
    ///
    /// The returned current Goal, history page, and destination transcript identity
    /// come from the same authoritative database snapshot.
    ///
    /// # Errors
    /// Rejects a non-current Coordinator, an unavailable ordinary target,
    /// malformed retained Goal data, or a storage failure.
    pub async fn wait_for_coordinator_goals(
        &self,
        caller: &str,
        owner: &ProductConversationId,
        after: GoalVersion,
        request: CoordinatorGoalWaitRequest,
    ) -> Result<CoordinatorGoalWaitResult> {
        self.wait_for_goals(
            GoalReadAuthority::Coordinator(caller),
            owner,
            after,
            &request,
        )
        .await
    }

    /// Wait for a Goal version while rechecking the authenticated peer enrollment.
    ///
    /// # Errors
    /// Rejects an inactive peer, an unavailable ordinary target, malformed retained Goal data,
    /// or a storage failure.
    pub async fn wait_for_federated_goals(
        &self,
        authority: &crate::FederationSendAuthority,
        owner: &ProductConversationId,
        after: GoalVersion,
        request: CoordinatorGoalWaitRequest,
    ) -> Result<CoordinatorGoalWaitResult> {
        self.wait_for_goals(
            GoalReadAuthority::Federated(authority),
            owner,
            after,
            &request,
        )
        .await
    }

    pub(crate) fn notify_goal_version_committed(
        &self,
        owner: ProductConversationId,
        version: GoalVersion,
    ) {
        let _ = self
            .goal_version_committed
            .send(super::GoalVersionCommittedNotification::new(owner, version));
    }

    pub(crate) fn notify_goal_versions_committed(
        &self,
        committed: &[super::GoalVersionCommittedNotification],
    ) {
        for notification in committed {
            let _ = self.goal_version_committed.send(notification.clone());
        }
    }

    #[cfg(test)]
    pub(crate) fn goal_version_waiter_count(&self) -> usize {
        self.goal_version_committed.receiver_count()
    }
}

#[cfg(test)]
mod tests;
