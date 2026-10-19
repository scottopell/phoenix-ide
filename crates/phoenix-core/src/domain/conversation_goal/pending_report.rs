use serde::{Deserialize, Deserializer, Serialize};

use super::{
    GoalActor, GoalAdmissionObservation, GoalAgentSource, GoalAttribution, GoalChange,
    GoalDisposition, GoalError, GoalExecution, GoalExecutionBinding, GoalFence, GoalMutation,
    GoalText,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoalReportBinding {
    execution: GoalExecution,
    goal: GoalFence,
}

impl TryFrom<GoalExecutionBinding> for GoalReportBinding {
    type Error = GoalError;

    fn try_from(binding: GoalExecutionBinding) -> Result<Self, Self::Error> {
        let GoalAdmissionObservation::Goal(goal) = binding.observation else {
            return Err(GoalError::ReportBindingRequired);
        };
        if goal.owner != binding.owner || goal.version.get() == 0 {
            return Err(GoalError::StaleFence);
        }
        Ok(Self {
            execution: binding.execution,
            goal,
        })
    }
}

impl<'de> Deserialize<'de> for GoalReportBinding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct WireBinding {
            execution: GoalExecution,
            goal: GoalFence,
        }
        let wire = WireBinding::deserialize(deserializer)?;
        if wire.goal.version.get() == 0 {
            return Err(serde::de::Error::custom(GoalError::StaleFence));
        }
        Ok(Self {
            execution: wire.execution,
            goal: wire.goal,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalReportCall {
    pub transcript_id: GoalText,
    pub message_id: GoalText,
    pub tool_use_id: GoalText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalReportOutcome {
    Blocked(GoalText),
    Complete(GoalText),
    Exhausted(GoalText),
}

impl GoalReportOutcome {
    fn disposition(&self) -> GoalDisposition {
        match self {
            Self::Blocked(reason) => GoalDisposition::Blocked(reason.clone()),
            Self::Complete(reason) => GoalDisposition::Complete(reason.clone()),
            Self::Exhausted(reason) => GoalDisposition::Exhausted(reason.clone()),
        }
    }
}

/// Pending data, not a successful result or proof of authenticated tool provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalPendingReport {
    admission: GoalReportBinding,
    call: GoalReportCall,
    action_id: GoalText,
    outcome: GoalReportOutcome,
}

impl GoalPendingReport {
    #[must_use]
    pub fn new(
        admission: GoalReportBinding,
        call: GoalReportCall,
        action_id: GoalText,
        outcome: GoalReportOutcome,
    ) -> Self {
        Self {
            admission,
            call,
            action_id,
            outcome,
        }
    }

    #[must_use]
    pub fn call(&self) -> &GoalReportCall {
        &self.call
    }

    #[must_use]
    pub fn mutation(&self) -> GoalMutation {
        let goal = &self.admission.goal;
        let binding = GoalExecutionBinding {
            owner: goal.owner.clone(),
            execution: self.admission.execution,
            observation: GoalAdmissionObservation::Goal(goal.clone()),
        };
        let source = GoalAgentSource {
            product_conversation_id: goal.owner.clone(),
            execution: self.admission.execution,
            transcript_id: self.call.transcript_id.clone(),
            message_id: self.call.message_id.clone(),
            tool_use_id: self.call.tool_use_id.clone(),
        };
        GoalMutation {
            owner: goal.owner.clone(),
            expected_version: goal.version,
            action_id: self.action_id.clone(),
            attribution: GoalAttribution {
                requested_by: GoalActor::OwningAgent(source),
                applied_by: GoalActor::GeneratedHost {
                    action_id: self.action_id.clone(),
                },
            },
            change: GoalChange::Report {
                binding,
                disposition: self.outcome.disposition(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation_goal::{GoalId, GoalVersion};
    use crate::domain::product_conversation::ProductConversationId;

    fn text(value: &str) -> GoalText {
        GoalText::parse(value).unwrap()
    }
    fn binding() -> GoalExecutionBinding {
        let owner = ProductConversationId::from_db("report-owner".into());
        GoalExecutionBinding {
            owner: owner.clone(),
            execution: GoalExecution::parse(31, 47).unwrap(),
            observation: GoalAdmissionObservation::Goal(GoalFence {
                owner,
                goal_id: GoalId::parse("report-goal").unwrap(),
                version: GoalVersion::parse(9).unwrap(),
            }),
        }
    }

    #[test]
    fn pending_report_derives_one_exact_owner_execution_and_version_without_result_text() {
        let admitted = binding();
        let pending = GoalPendingReport::new(
            admitted.clone().try_into().unwrap(),
            GoalReportCall {
                transcript_id: text("segment"),
                message_id: text("assistant"),
                tool_use_id: text("tool-call"),
            },
            text("stable-action"),
            GoalReportOutcome::Blocked(text("restore access")),
        );
        let command = pending.mutation();
        assert_eq!(command.owner, admitted.owner);
        assert_eq!(command.expected_version.get(), 9);
        let GoalChange::Report {
            binding,
            disposition,
        } = command.change
        else {
            panic!("report only")
        };
        assert_eq!(binding, admitted);
        assert!(matches!(disposition, GoalDisposition::Blocked(_)));
        let GoalActor::OwningAgent(source) = command.attribution.requested_by else {
            panic!("owning execution only")
        };
        assert_eq!(source.product_conversation_id, command.owner);
        assert_eq!(source.execution, admitted.execution);
        assert_eq!(source.tool_use_id.as_str(), "tool-call");
        let encoded = serde_json::to_value(&pending).unwrap();
        assert!(encoded.get("output").is_none());
        assert!(encoded.get("is_error").is_none());
        assert_eq!(
            serde_json::from_value::<GoalPendingReport>(encoded).unwrap(),
            pending
        );
    }

    #[test]
    fn no_goal_or_inconsistent_fence_cannot_construct_pending_report_binding() {
        let mut missing = binding();
        missing.observation = GoalAdmissionObservation::NoGoal {
            observed_version: GoalVersion::parse(9).unwrap(),
        };
        assert_eq!(
            GoalReportBinding::try_from(missing),
            Err(GoalError::ReportBindingRequired)
        );
        let mut mismatch = binding();
        mismatch.owner = ProductConversationId::from_db("different".into());
        assert_eq!(
            GoalReportBinding::try_from(mismatch),
            Err(GoalError::StaleFence)
        );
        let mut zero = binding();
        let GoalAdmissionObservation::Goal(ref mut fence) = zero.observation else {
            panic!("fixture")
        };
        fence.version = GoalVersion::INITIAL;
        assert_eq!(
            GoalReportBinding::try_from(zero),
            Err(GoalError::StaleFence)
        );
    }

    #[test]
    fn persisted_pending_report_requires_its_original_call_and_nonzero_fence() {
        let pending = GoalPendingReport::new(
            binding().try_into().unwrap(),
            GoalReportCall {
                transcript_id: text("segment"),
                message_id: text("assistant"),
                tool_use_id: text("call"),
            },
            text("action"),
            GoalReportOutcome::Complete(text("done")),
        );
        let original = serde_json::to_value(&pending).unwrap();
        for field in ["admission", "call", "action_id", "outcome"] {
            let mut missing = original.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<GoalPendingReport>(missing).is_err());
        }
        let mut invalid = original;
        invalid["admission"]["goal"]["version"] = serde_json::json!(0);
        assert!(serde_json::from_value::<GoalPendingReport>(invalid).is_err());
    }
}
