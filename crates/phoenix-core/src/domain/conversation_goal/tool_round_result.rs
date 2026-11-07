use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::domain::db_schema::ToolResult;

use super::GoalPendingReport;

#[derive(Debug, Clone, PartialEq)]
pub enum ToolRoundResult {
    PendingSelfGoal {
        pending: super::PendingSelfGoal,
        duration_ms: Option<u64>,
    },
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
    /// A finalized-only sink must refuse, not omit, an unfinished entry.
    /// # Errors
    /// Returns the unchanged pending intent when this result has not been finalized.
    pub fn into_finalized(self) -> Result<ToolResult, Box<Self>> {
        match self {
            Self::Finalized(result) => Ok(result),
            pending @ (Self::PendingSelfGoal { .. } | Self::PendingGoalReport { .. }) => {
                Err(Box::new(pending))
            }
        }
    }

    #[must_use]
    pub fn tool_use_id(&self) -> &str {
        match self {
            Self::Finalized(result) => &result.tool_use_id,
            Self::PendingSelfGoal { pending, .. } => pending.call.tool_use_id.as_str(),
            Self::PendingGoalReport { report, .. } => report.call().tool_use_id.as_str(),
        }
    }

    #[must_use]
    pub fn duration_ms(&self) -> Option<u64> {
        match self {
            Self::Finalized(result) => result.duration_ms,
            Self::PendingSelfGoal { duration_ms, .. } => *duration_ms,
            Self::PendingGoalReport { duration_ms, .. } => Some(*duration_ms),
        }
    }

    #[must_use]
    pub fn finalized(&self) -> Option<&ToolResult> {
        match self {
            Self::Finalized(result) => Some(result),
            Self::PendingGoalReport { .. } | Self::PendingSelfGoal { .. } => None,
        }
    }
}

impl Serialize for ToolRoundResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Finalized(result) => result.serialize(serializer),
            Self::PendingSelfGoal {
                pending,
                duration_ms,
            } => {
                #[derive(Serialize)]
                struct Pending<'a> {
                    pending_self_goal: &'a super::PendingSelfGoal,
                    duration_ms: Option<u64>,
                }
                Pending {
                    pending_self_goal: pending,
                    duration_ms: *duration_ms,
                }
                .serialize(serializer)
            }
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
        if value.get("pending_self_goal").is_some() {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Pending {
                pending_self_goal: super::PendingSelfGoal,
                duration_ms: Option<u64>,
            }
            let pending: Pending =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Ok(Self::PendingSelfGoal {
                pending: pending.pending_self_goal,
                duration_ms: pending.duration_ms,
            })
        } else if value.get("pending_goal_report").is_some() {
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
            let result: ToolResult =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Ok(Self::from(result))
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

#[cfg(test)]
mod finalized_sink_tests {
    use super::*;
    use crate::domain::conversation_goal::{
        GoalExecution, GoalReportCall, GoalText, SelfGoalOperation,
    };

    fn pending() -> ToolRoundResult {
        ToolRoundResult::PendingSelfGoal {
            pending: super::super::PendingSelfGoal {
                owner: crate::domain::product_conversation::ProductConversationId::parse("owner")
                    .unwrap(),
                execution: GoalExecution::parse(9, 13).unwrap(),
                generation: 17,
                call: GoalReportCall {
                    transcript_id: GoalText::parse("owner").unwrap(),
                    message_id: GoalText::parse("assistant").unwrap(),
                    tool_use_id: GoalText::parse("call").unwrap(),
                },
                operation: SelfGoalOperation::EnableAutoContinuation,
            },
            duration_ms: Some(2),
        }
    }

    #[test]
    fn finalized_carrier_preserves_the_existing_serialization() {
        let result = ToolResult::success("call".into(), "ok".into());
        let legacy = serde_json::to_value(&result).unwrap();
        let carrier = ToolRoundResult::from(result.clone());
        assert_eq!(serde_json::to_value(&carrier).unwrap(), legacy);
        assert_eq!(
            serde_json::from_value::<ToolRoundResult>(legacy).unwrap(),
            carrier
        );
        assert_eq!(carrier.into_finalized().unwrap(), result);
    }

    #[test]
    fn pending_roundtrip_preserves_authority_and_refuses_finalized_sink() {
        let pending = pending();
        let restored =
            serde_json::from_str::<ToolRoundResult>(&serde_json::to_string(&pending).unwrap())
                .unwrap();
        assert_eq!(restored, pending);
        assert_eq!(restored.tool_use_id(), "call");
        assert!(restored.finalized().is_none());
        assert_eq!(*restored.into_finalized().unwrap_err(), pending);
    }
}
