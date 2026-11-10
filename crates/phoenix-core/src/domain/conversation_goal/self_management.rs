use serde::{Deserialize, Serialize};

use super::{GoalExecution, GoalReportCall, GoalText};
use crate::domain::product_conversation::ProductConversationId;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelfGoalOperation {
    Create { objective: GoalText },
    EnableAutoContinuation,
}

impl SelfGoalOperation {
    #[must_use]
    pub fn tool_name(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create_goal",
            Self::EnableAutoContinuation => "enable_auto_continuation",
        }
    }

    /// Parse only model-controlled arguments. Identity is supplied separately by the runtime.
    /// # Errors
    /// Returns an error for an unknown tool name or arguments outside its strict schema.
    pub fn parse(name: &str, input: serde_json::Value) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Create {
            objective: GoalText,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Enable {}
        match name {
            "create_goal" => serde_json::from_value::<Create>(input)
                .map(|args| Self::Create {
                    objective: args.objective,
                })
                .map_err(|e| e.to_string()),
            "enable_auto_continuation" => serde_json::from_value::<Enable>(input)
                .map(|_| Self::EnableAutoContinuation)
                .map_err(|e| e.to_string()),
            _ => Err("not an ordinary self-goal tool".into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingSelfGoal {
    pub owner: ProductConversationId,
    pub execution: GoalExecution,
    pub generation: i64,
    pub call: GoalReportCall,
    pub operation: SelfGoalOperation,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn arguments_cannot_convey_identity_or_extra_rights() {
        for field in [
            "target",
            "conversation_id",
            "owner",
            "goal_id",
            "version",
            "enabled",
            "execution",
            "generation",
        ] {
            let mut create = json!({"objective":"ship it"});
            create[field] = json!("forged");
            assert!(SelfGoalOperation::parse("create_goal", create).is_err());
            let mut enable = json!({});
            enable[field] = json!("forged");
            assert!(SelfGoalOperation::parse("enable_auto_continuation", enable).is_err());
        }
        for input in [
            json!({}),
            json!({"objective":""}),
            json!({"objective":"   "}),
            json!({"objective":false}),
        ] {
            assert!(SelfGoalOperation::parse("create_goal", input).is_err());
        }
        assert!(SelfGoalOperation::parse("create_goal", json!({"objective":"ship it"})).is_ok());
        assert!(SelfGoalOperation::parse("enable_auto_continuation", json!({})).is_ok());
        assert!(SelfGoalOperation::parse("disable_auto_continuation", json!({})).is_err());
    }
}
