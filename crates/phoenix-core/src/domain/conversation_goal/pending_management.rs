use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    GoalActor, GoalAgentSource, GoalAttribution, GoalChange, GoalDisposition, GoalId, GoalMutation,
    GoalText, GoalVersion,
};
use crate::domain::product_conversation::ProductConversationId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "reason",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum GoalCommandDisposition {
    Underway,
    Blocked(GoalText),
    Complete(GoalText),
    Exhausted(GoalText),
}

impl From<GoalCommandDisposition> for GoalDisposition {
    fn from(disposition: GoalCommandDisposition) -> Self {
        match disposition {
            GoalCommandDisposition::Underway => Self::Underway,
            GoalCommandDisposition::Blocked(reason) => Self::Blocked(reason),
            GoalCommandDisposition::Complete(reason) => Self::Complete(reason),
            GoalCommandDisposition::Exhausted(reason) => Self::Exhausted(reason),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalManagementRequest {
    Create {
        owner: ProductConversationId,
        expected_version: GoalVersion,
        objective: GoalText,
    },
    State {
        owner: ProductConversationId,
        expected_version: GoalVersion,
        goal_id: GoalId,
        disposition: GoalCommandDisposition,
    },
}

impl GoalManagementRequest {
    #[must_use]
    pub fn owner(&self) -> &ProductConversationId {
        match self {
            Self::Create { owner, .. } | Self::State { owner, .. } => owner,
        }
    }
}

/// Uncommitted management-tool input and its host-derived source identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalPendingManagement {
    source: GoalAgentSource,
    generation: u64,
    request: GoalManagementRequest,
}

impl GoalPendingManagement {
    #[must_use]
    pub fn new(source: GoalAgentSource, generation: u64, request: GoalManagementRequest) -> Self {
        Self {
            source,
            generation,
            request,
        }
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn source(&self) -> &GoalAgentSource {
        &self.source
    }

    #[must_use]
    pub fn request(&self) -> &GoalManagementRequest {
        &self.request
    }

    #[must_use]
    pub fn mutation(&self) -> GoalMutation {
        let identity = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&self.source).expect("typed management source serializes"),
            )
        );
        let action_id =
            GoalText::parse(format!("goal-management:{identity}")).expect("fixed prefix with hash");
        let (owner, expected_version, change) = match &self.request {
            GoalManagementRequest::Create {
                owner,
                expected_version,
                objective,
            } => (
                owner.clone(),
                *expected_version,
                GoalChange::Create {
                    goal_id: GoalId::parse(format!("managed-goal:{identity}"))
                        .expect("fixed prefix with hash"),
                    objective: objective.clone(),
                },
            ),
            GoalManagementRequest::State {
                owner,
                expected_version,
                goal_id,
                disposition,
            } => (
                owner.clone(),
                *expected_version,
                GoalChange::SetDisposition {
                    goal_id: goal_id.clone(),
                    disposition: disposition.clone().into(),
                },
            ),
        };
        GoalMutation {
            owner,
            expected_version,
            attribution: GoalAttribution {
                requested_by: GoalActor::Coordinator(self.source.clone()),
                applied_by: GoalActor::Coordinator(self.source.clone()),
            },
            action_id,
            change,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation_goal::{GoalExecution, GoalSnapshot, GoalState};

    fn source(call: &str) -> GoalAgentSource {
        GoalAgentSource {
            product_conversation_id: ProductConversationId::parse("coordinator").unwrap(),
            transcript_id: GoalText::parse("coordinator-transcript").unwrap(),
            execution: GoalExecution::parse(7, 11).unwrap(),
            message_id: GoalText::parse("canonical-assistant").unwrap(),
            tool_use_id: GoalText::parse(call).unwrap(),
        }
    }

    fn create() -> GoalManagementRequest {
        GoalManagementRequest::Create {
            owner: ProductConversationId::parse("target-owner").unwrap(),
            expected_version: GoalVersion::INITIAL,
            objective: GoalText::parse(" exact objective\n").unwrap(),
        }
    }

    #[test]
    fn conversation_goal_management_intent_reconstruction_keeps_the_exact_source_and_identity() {
        let intent = GoalPendingManagement::new(source("create-call"), 2, create());
        let reconstructed: GoalPendingManagement =
            serde_json::from_value(serde_json::to_value(&intent).unwrap()).unwrap();
        assert_eq!(reconstructed, intent);
        assert_eq!(reconstructed.mutation(), intent.mutation());
        assert_eq!(
            intent.mutation().attribution.requested_by,
            GoalActor::Coordinator(source("create-call"))
        );
        assert_eq!(
            intent.mutation().attribution.applied_by,
            GoalActor::Coordinator(source("create-call"))
        );
        assert!(!matches!(
            intent.mutation().attribution.requested_by,
            GoalActor::GeneratedHost { .. }
        ));
        assert!(!matches!(
            intent.mutation().attribution.applied_by,
            GoalActor::GeneratedHost { .. }
        ));
        assert_eq!(intent.generation(), 2);
        let first = intent.mutation().advance(None).unwrap();
        let another = GoalPendingManagement::new(source("another-call"), 2, create())
            .mutation()
            .advance(None)
            .unwrap();
        assert_ne!(first.goal_id, another.goal_id);
        assert_eq!(first.objective, another.objective);
        assert_eq!(first.objective.as_str(), " exact objective\n");
    }

    #[test]
    fn conversation_goal_management_call_identity_does_not_change_with_rewritten_arguments() {
        let original = GoalPendingManagement::new(source("same-call"), 2, create());
        let rewritten = GoalPendingManagement::new(
            source("same-call"),
            2,
            GoalManagementRequest::Create {
                owner: ProductConversationId::parse("target-owner").unwrap(),
                expected_version: GoalVersion::INITIAL,
                objective: GoalText::parse("rewritten objective").unwrap(),
            },
        );
        assert_eq!(
            original.mutation().action_id,
            rewritten.mutation().action_id
        );
        assert_ne!(original.mutation(), rewritten.mutation());
    }

    #[test]
    fn conversation_goal_management_state_recovery_retains_the_same_canonical_goal() {
        let current = GoalSnapshot {
            owner: ProductConversationId::parse("target-owner").unwrap(),
            goal_id: GoalId::parse("existing-goal").unwrap(),
            version: GoalVersion::parse(4).unwrap(),
            objective: GoalText::parse("canonical objective").unwrap(),
            state: GoalState::Blocked(GoalText::parse("waiting for access").unwrap()),
        };
        let intent = GoalPendingManagement::new(
            source("resume-call"),
            2,
            GoalManagementRequest::State {
                owner: current.owner.clone(),
                expected_version: current.version,
                goal_id: current.goal_id.clone(),
                disposition: GoalCommandDisposition::Underway,
            },
        );
        let recovered = intent.mutation().advance(Some(&current)).unwrap();
        assert_eq!(recovered.goal_id, current.goal_id);
        assert_eq!(recovered.objective, current.objective);
        assert_eq!(recovered.version.get(), 5);
        assert_eq!(recovered.state, GoalState::Underway);
    }

    #[test]
    fn conversation_goal_management_request_cannot_claim_source_actor_or_creation_identity() {
        for field in [
            "actor",
            "source",
            "principal_id",
            "goal_id",
            "authenticated",
        ] {
            let mut input = serde_json::to_value(create()).unwrap();
            input[field] = serde_json::json!("untrusted");
            assert!(serde_json::from_value::<GoalManagementRequest>(input).is_err());
        }
        for disposition in [
            serde_json::json!({"state":"suspended","reason":"legacy"}),
            serde_json::json!({"state":"blocked","reason":" "}),
        ] {
            let input = serde_json::json!({"op":"state","owner":"target-owner","expected_version":4,"goal_id":"existing-goal","disposition":disposition});
            assert!(serde_json::from_value::<GoalManagementRequest>(input).is_err());
        }
        let intent = GoalPendingManagement::new(source("create-call"), 2, create());
        let mut invalid_source = serde_json::to_value(intent).unwrap();
        invalid_source["source"]["execution"]["turn_id"] = serde_json::json!(0);
        assert!(serde_json::from_value::<GoalPendingManagement>(invalid_source).is_err());
    }
}
