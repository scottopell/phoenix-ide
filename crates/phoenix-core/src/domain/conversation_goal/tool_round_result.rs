use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::domain::db_schema::ToolResult;

use super::GoalPendingReport;

#[derive(Debug, Clone, PartialEq)]
pub enum ToolRoundResult {
    Finalized(ToolResult),
    PendingGoalReport {
        report: GoalPendingReport,
        duration_ms: u64,
    },
}

impl From<ToolResult> for ToolRoundResult {
    fn from(result: ToolResult) -> Self {
        Self::Finalized(result)
    }
}

impl ToolRoundResult {
    #[must_use]
    pub fn tool_use_id(&self) -> &str {
        match self {
            Self::Finalized(result) => &result.tool_use_id,
            Self::PendingGoalReport { report, .. } => report.call().tool_use_id.as_str(),
        }
    }

    #[must_use]
    pub fn duration_ms(&self) -> Option<u64> {
        match self {
            Self::Finalized(result) => result.duration_ms,
            Self::PendingGoalReport { duration_ms, .. } => Some(*duration_ms),
        }
    }

    #[must_use]
    pub fn finalized(&self) -> Option<&ToolResult> {
        match self {
            Self::Finalized(result) => Some(result),
            Self::PendingGoalReport { .. } => None,
        }
    }
}

impl Serialize for ToolRoundResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Finalized(result) => result.serialize(serializer),
            Self::PendingGoalReport {
                report,
                duration_ms,
            } => {
                #[derive(Serialize)]
                struct Pending<'a> {
                    pending_goal_report: &'a GoalPendingReport,
                    duration_ms: u64,
                }
                Pending {
                    pending_goal_report: report,
                    duration_ms: *duration_ms,
                }
                .serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for ToolRoundResult {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.get("pending_goal_report").is_some() {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Pending {
                pending_goal_report: GoalPendingReport,
                duration_ms: u64,
            }
            let pending: Pending =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Ok(Self::PendingGoalReport {
                report: pending.pending_goal_report,
                duration_ms: pending.duration_ms,
            })
        } else {
            let result = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Ok(Self::Finalized(result))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation_goal::{
        GoalAdmissionObservation, GoalExecution, GoalExecutionBinding, GoalFence, GoalId,
        GoalReportCall, GoalReportOutcome, GoalText, GoalVersion,
    };
    use crate::domain::product_conversation::ProductConversationId;

    fn text(value: &str) -> GoalText {
        GoalText::parse(value).unwrap()
    }
    fn pending() -> ToolRoundResult {
        let owner = ProductConversationId::parse("round-owner").unwrap();
        let binding = GoalExecutionBinding {
            owner: owner.clone(),
            execution: GoalExecution::parse(13, 29).unwrap(),
            observation: GoalAdmissionObservation::Goal(
                GoalFence::new(
                    owner,
                    GoalId::parse("round-goal").unwrap(),
                    GoalVersion::parse(3).unwrap(),
                )
                .unwrap(),
            ),
        };
        let report = GoalPendingReport::new(
            binding.try_into().unwrap(),
            GoalReportCall {
                transcript_id: text("round-transcript"),
                message_id: text("round-assistant"),
                tool_use_id: text("round-call"),
            },
            GoalReportOutcome::Complete(text("finished")),
        );
        ToolRoundResult::PendingGoalReport {
            report,
            duration_ms: 5,
        }
    }

    #[test]
    fn finalized_round_entry_keeps_exact_existing_tool_result_encoding() {
        let result = ToolResult::success("call".into(), "ordinary result".into());
        let entry = ToolRoundResult::from(result.clone());
        assert_eq!(entry.duration_ms(), None);
        assert_eq!(
            serde_json::to_vec(&entry).unwrap(),
            serde_json::to_vec(&result).unwrap()
        );
        let restored: ToolRoundResult =
            serde_json::from_slice(&serde_json::to_vec(&entry).unwrap()).unwrap();
        assert_eq!(restored.finalized(), Some(&result));
    }

    #[test]
    fn pending_round_entry_has_no_finalized_content_or_success_flag() {
        let entry = pending();
        assert!(entry.finalized().is_none());
        assert_eq!(entry.tool_use_id(), "round-call");
        assert_eq!(entry.duration_ms(), Some(5));
        let value = serde_json::to_value(&entry).unwrap();
        assert!(value.get("outcome").is_none());
        let restored: ToolRoundResult = serde_json::from_value(value).unwrap();
        assert_eq!(restored, entry);
    }

    #[test]
    fn management_is_not_a_pending_round_variant() {
        let value = serde_json::json!({
            "pending_goal_management": {},
            "duration_ms": 9
        });
        assert!(serde_json::from_value::<ToolRoundResult>(value).is_err());
    }

    #[test]
    fn ambiguous_pending_and_finalized_payload_is_not_silently_reduced_to_either() {
        let mut value = serde_json::to_value(pending()).unwrap();
        value["outcome"] = serde_json::json!({"type":"success","output":"premature"});
        value["tool_use_id"] = serde_json::json!("round-call");
        value["tool_name"] = serde_json::json!("goal_report");
        assert!(serde_json::from_value::<ToolRoundResult>(value).is_err());
    }
}
