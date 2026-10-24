//! Ordinary-conversation goal storage and checkpoint vocabulary.

use serde::{Deserialize, Deserializer, Serialize};

use super::product_conversation::ProductConversationId;

mod pending_report;
pub use pending_report::{GoalPendingReport, GoalReportBinding, GoalReportCall, GoalReportOutcome};
mod tool_round_result;
pub use tool_round_result::ToolRoundResult;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GoalError {
    #[error("goal value must not be blank")]
    Blank,
    #[error("goal counter or timestamp outside SQLite integer range")]
    Range,
    #[error("goal changed since observation")]
    Conflict,
    #[error("goal change is not applicable")]
    Inapplicable,
    #[error("agent state reports require their admitted execution binding")]
    ReportBindingRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct GoalText(String);

impl<'de> Deserialize<'de> for GoalText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl GoalText {
    /// # Errors
    /// Rejects blank text without normalizing the retained value.
    pub fn parse(value: impl Into<String>) -> Result<Self, GoalError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(GoalError::Blank);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GoalId(GoalText);

impl GoalId {
    /// # Errors
    /// Rejects blank identifiers.
    pub fn parse(value: impl Into<String>) -> Result<Self, GoalError> {
        Ok(Self(GoalText::parse(value)?))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct GoalVersion(i64);

impl<'de> Deserialize<'de> for GoalVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(i64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl GoalVersion {
    pub const INITIAL: Self = Self(0);

    /// # Errors
    /// Rejects negative values.
    pub fn parse(value: i64) -> Result<Self, GoalError> {
        if value < 0 {
            return Err(GoalError::Range);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    fn next(self) -> Result<Self, GoalError> {
        self.0.checked_add(1).map(Self).ok_or(GoalError::Range)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct GoalTimestampUs(i64);

impl<'de> Deserialize<'de> for GoalTimestampUs {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(i64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl GoalTimestampUs {
    /// # Errors
    /// Rejects negative values.
    pub fn parse(value: i64) -> Result<Self, GoalError> {
        if value < 0 {
            return Err(GoalError::Range);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GoalExecution {
    workflow_id: i64,
    turn_id: i64,
}

impl<'de> Deserialize<'de> for GoalExecution {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct WireExecution {
            workflow_id: i64,
            turn_id: i64,
        }
        let wire = WireExecution::deserialize(deserializer)?;
        Self::parse(wire.workflow_id, wire.turn_id).map_err(serde::de::Error::custom)
    }
}

impl GoalExecution {
    /// # Errors
    /// Rejects nonpositive durable execution identifiers.
    pub fn parse(workflow_id: i64, turn_id: i64) -> Result<Self, GoalError> {
        if workflow_id <= 0 || turn_id <= 0 {
            return Err(GoalError::Range);
        }
        Ok(Self {
            workflow_id,
            turn_id,
        })
    }

    #[must_use]
    pub const fn workflow_id(self) -> i64 {
        self.workflow_id
    }

    #[must_use]
    pub const fn turn_id(self) -> i64 {
        self.turn_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalAgentSource {
    pub product_conversation_id: ProductConversationId,
    pub transcript_id: GoalText,
    pub execution: GoalExecution,
    pub message_id: GoalText,
    pub tool_use_id: GoalText,
}

/// `UserApi` denotes the trusted input channel, not proof of biological authorship.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalActor {
    UserApi {
        transcript_id: GoalText,
        message_id: GoalText,
    },
    OwningAgent(GoalAgentSource),
    Coordinator(GoalAgentSource),
    GeneratedHost {
        action_id: GoalText,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalAttribution {
    pub requested_by: GoalActor,
    pub applied_by: GoalActor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalState {
    Underway,
    Blocked(GoalText),
    Suspended(GoalText),
    Complete(GoalText),
    Exhausted(GoalText),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalDisposition {
    Underway,
    Blocked(GoalText),
    Suspended(GoalText),
    Complete(GoalText),
    Exhausted(GoalText),
}

impl GoalDisposition {
    #[must_use]
    pub fn state(&self) -> GoalState {
        match self {
            Self::Underway => GoalState::Underway,
            Self::Blocked(reason) => GoalState::Blocked(reason.clone()),
            Self::Suspended(reason) => GoalState::Suspended(reason.clone()),
            Self::Complete(reason) => GoalState::Complete(reason.clone()),
            Self::Exhausted(reason) => GoalState::Exhausted(reason.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalSnapshot {
    pub owner: ProductConversationId,
    pub goal_id: GoalId,
    pub version: GoalVersion,
    pub objective: GoalText,
    pub state: GoalState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalFence {
    pub owner: ProductConversationId,
    pub goal_id: GoalId,
    pub version: GoalVersion,
}

impl GoalSnapshot {
    #[must_use]
    pub fn fence(&self) -> GoalFence {
        GoalFence {
            owner: self.owner.clone(),
            goal_id: self.goal_id.clone(),
            version: self.version,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalAdmissionObservation {
    NoGoal { observed_version: GoalVersion },
    Goal(GoalFence),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalExecutionBinding {
    pub owner: ProductConversationId,
    pub execution: GoalExecution,
    pub observation: GoalAdmissionObservation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalChange {
    Create {
        goal_id: GoalId,
        objective: GoalText,
    },
    SetDisposition {
        goal_id: GoalId,
        disposition: GoalDisposition,
    },
    Report {
        binding: GoalExecutionBinding,
        disposition: GoalDisposition,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalMutation {
    pub owner: ProductConversationId,
    pub expected_version: GoalVersion,
    pub action_id: GoalText,
    pub attribution: GoalAttribution,
    pub change: GoalChange,
}

impl GoalMutation {
    /// Pure CAS and report fencing; admission/commission authorization belongs to callers.
    ///
    /// # Errors
    /// Rejects stale observations, inapplicable changes, unfenced reports and counter overflow.
    pub fn advance(&self, current: Option<&GoalSnapshot>) -> Result<GoalSnapshot, GoalError> {
        let version = current.map_or(GoalVersion::INITIAL, |s| s.version);
        if version != self.expected_version || current.is_some_and(|s| s.owner != self.owner) {
            return Err(GoalError::Conflict);
        }
        if let GoalChange::Create { goal_id, objective } = &self.change {
            if current.is_some_and(|s| s.goal_id == *goal_id) {
                return Err(GoalError::Inapplicable);
            }
            return Ok(GoalSnapshot {
                owner: self.owner.clone(),
                goal_id: goal_id.clone(),
                version: version.next()?,
                objective: objective.clone(),
                state: GoalState::Underway,
            });
        }
        let current = current.ok_or(GoalError::Conflict)?;
        if matches!(
            current.state,
            GoalState::Complete(_) | GoalState::Exhausted(_) | GoalState::Suspended(_)
        ) {
            return Err(GoalError::Inapplicable);
        }
        let mut next = current.clone();
        next.version = version.next()?;
        match &self.change {
            GoalChange::Create { .. } => return Err(GoalError::Inapplicable),
            GoalChange::SetDisposition {
                goal_id,
                disposition,
            } => {
                if goal_id != &current.goal_id {
                    return Err(GoalError::Conflict);
                }
                if matches!(self.attribution.requested_by, GoalActor::OwningAgent(_)) {
                    return Err(GoalError::ReportBindingRequired);
                }
                next.state = disposition.state();
            }
            GoalChange::Report {
                binding,
                disposition,
            } => {
                if binding.owner != self.owner
                    || binding.observation != GoalAdmissionObservation::Goal(current.fence())
                {
                    return Err(GoalError::Conflict);
                }
                if !matches!(current.state, GoalState::Underway) {
                    return Err(GoalError::Inapplicable);
                }
                match &self.attribution.requested_by {
                    GoalActor::OwningAgent(source)
                        if source.product_conversation_id == self.owner
                            && source.execution == binding.execution => {}
                    GoalActor::OwningAgent(_)
                    | GoalActor::UserApi { .. }
                    | GoalActor::Coordinator(_)
                    | GoalActor::GeneratedHost { .. } => {
                        return Err(GoalError::ReportBindingRequired)
                    }
                }
                next.state = disposition.state();
            }
        }
        if matches!(next.state, GoalState::Underway)
            && !matches!(current.state, GoalState::Blocked(_))
        {
            return Err(GoalError::Inapplicable);
        }
        Ok(next)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalHistoryEntry {
    pub snapshot: GoalSnapshot,
    pub mutation: GoalMutation,
    pub recorded_at_us: GoalTimestampUs,
}

#[cfg(test)]
mod tests;
