use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    GoalAgentSource, GoalError, GoalExecution, GoalFence, GoalSnapshot, GoalState, GoalText,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalStartRequester {
    Owner { principal_id: Uuid },
    Coordinator { source: GoalAgentSource },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoalStartInput {
    pub request_id: Uuid,
    pub requester: GoalStartRequester,
    goal: GoalFence,
}

impl GoalStartInput {
    /// # Errors
    /// Refuses a goal observation without a committed version.
    pub fn new(
        request_id: Uuid,
        requester: GoalStartRequester,
        goal: GoalFence,
    ) -> Result<Self, GoalError> {
        if goal.version.get() == 0 {
            return Err(GoalError::Range);
        }
        Ok(Self {
            request_id,
            requester,
            goal,
        })
    }

    #[must_use]
    pub fn goal(&self) -> &GoalFence {
        &self.goal
    }
}

impl<'de> Deserialize<'de> for GoalStartInput {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            request_id: Uuid,
            requester: GoalStartRequester,
            goal: GoalFence,
        }
        let fields = Fields::deserialize(deserializer)?;
        Self::new(fields.request_id, fields.requester, fields.goal)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalStartReceipt {
    input: GoalStartInput,
    execution: GoalExecution,
    historical_transcript: GoalText,
    live_turn_id: Option<i64>,
    observation: GoalSnapshot,
}

impl GoalStartReceipt {
    #[must_use]
    pub fn input(&self) -> &GoalStartInput {
        &self.input
    }

    #[must_use]
    pub const fn execution(&self) -> GoalExecution {
        self.execution
    }

    #[must_use]
    pub fn historical_transcript(&self) -> &GoalText {
        &self.historical_transcript
    }

    #[must_use]
    pub const fn live_turn_id(&self) -> Option<i64> {
        self.live_turn_id
    }

    #[must_use]
    pub fn observation(&self) -> &GoalSnapshot {
        &self.observation
    }

    /// # Errors
    /// Refuses changed goal observations or an unrelated live association.
    pub fn new(
        input: GoalStartInput,
        execution: GoalExecution,
        historical_transcript: GoalText,
        live_turn_id: Option<i64>,
        observation: GoalSnapshot,
    ) -> Result<Self, GoalError> {
        if input.goal() != &observation.fence()
            || observation.state != GoalState::Underway
            || live_turn_id.is_some_and(|turn| turn != execution.turn_id())
        {
            return Err(GoalError::Conflict);
        }
        Ok(Self {
            input,
            execution,
            historical_transcript,
            live_turn_id,
            observation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        conversation_goal::{GoalId, GoalVersion},
        product_conversation::ProductConversationId,
    };

    fn fence(version: i64) -> GoalFence {
        GoalFence {
            owner: ProductConversationId::parse("owner").unwrap(),
            goal_id: GoalId::parse("goal").unwrap(),
            version: GoalVersion::parse(version).unwrap(),
        }
    }

    #[test]
    fn typed_start_has_no_chat_payload_and_rejects_uncommitted_observation() {
        let requester = GoalStartRequester::Owner {
            principal_id: Uuid::new_v4(),
        };
        let request_id = Uuid::new_v4();
        assert!(GoalStartInput::new(request_id, requester.clone(), fence(0)).is_err());
        let input = GoalStartInput::new(request_id, requester, fence(1)).unwrap();
        let encoded = serde_json::to_value(&input).unwrap();
        assert_eq!(encoded.as_object().unwrap().len(), 3);
        assert_eq!(
            serde_json::from_value::<GoalStartInput>(encoded.clone()).unwrap(),
            input
        );
        let mut forged = encoded;
        forged["message"] = serde_json::json!("not a message");
        assert!(serde_json::from_value::<GoalStartInput>(forged).is_err());
    }

    #[test]
    fn requester_uuids_use_checked_json_string_encoding() {
        let request_id = Uuid::parse_str("6aaec39e-8a5d-4f42-9a66-082ed320af13").unwrap();
        let principal_id = Uuid::parse_str("ea96d1d2-71df-4638-901c-726ea0e56584").unwrap();
        let input = GoalStartInput::new(
            request_id,
            GoalStartRequester::Owner { principal_id },
            fence(1),
        )
        .unwrap();

        let value = serde_json::to_value(&input).unwrap();
        assert_eq!(value["request_id"], "6aaec39e-8a5d-4f42-9a66-082ed320af13");
        assert_eq!(
            value["requester"]["principal_id"],
            "ea96d1d2-71df-4638-901c-726ea0e56584"
        );
        assert_eq!(
            serde_json::from_value::<GoalStartInput>(value.clone()).unwrap(),
            input
        );

        let mut invalid = value;
        invalid["request_id"] = serde_json::json!("not-a-uuid");
        assert!(serde_json::from_value::<GoalStartInput>(invalid).is_err());
    }

    #[test]
    fn historical_start_receipt_retains_observation_after_live_detachment() {
        let input = GoalStartInput::new(
            Uuid::new_v4(),
            GoalStartRequester::Owner {
                principal_id: Uuid::new_v4(),
            },
            fence(1),
        )
        .unwrap();
        let execution = GoalExecution::parse(1, 2).unwrap();
        let observation = GoalSnapshot {
            owner: fence(1).owner,
            goal_id: fence(1).goal_id,
            version: fence(1).version,
            objective: GoalText::parse("do the work").unwrap(),
            state: GoalState::Underway,
        };
        assert!(GoalStartReceipt::new(
            input.clone(),
            execution,
            GoalText::parse("old-transcript").unwrap(),
            Some(3),
            observation.clone()
        )
        .is_err());
        let receipt = GoalStartReceipt::new(
            input,
            execution,
            GoalText::parse("old-transcript").unwrap(),
            None,
            observation,
        )
        .unwrap();
        assert_eq!(receipt.live_turn_id, None);
        assert_eq!(receipt.execution.turn_id(), 2);
    }
}
