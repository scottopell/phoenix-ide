use serde::{Deserialize, Deserializer, Serialize};

use super::{
    GoalActor, GoalAdmissionObservation, GoalAgentSource, GoalAttribution, GoalChange,
    GoalDisposition, GoalError, GoalExecution, GoalExecutionBinding, GoalFence, GoalMutation,
    GoalState, GoalText,
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
        Ok(Self {
            execution: binding.execution,
            goal,
        })
    }
}

impl<'de> Deserialize<'de> for GoalReportBinding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireBinding {
            execution: GoalExecution,
            goal: GoalFence,
        }
        let wire = WireBinding::deserialize(deserializer)?;
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
#[serde(
    tag = "status",
    content = "reason",
    rename_all = "snake_case",
    deny_unknown_fields
)]
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

    pub(super) fn state(&self) -> GoalState {
        self.disposition().state()
    }
}

impl TryFrom<GoalDisposition> for GoalReportOutcome {
    type Error = GoalError;

    fn try_from(disposition: GoalDisposition) -> Result<Self, Self::Error> {
        match disposition {
            GoalDisposition::Blocked(reason) => Ok(Self::Blocked(reason)),
            GoalDisposition::Complete(reason) => Ok(Self::Complete(reason)),
            GoalDisposition::Exhausted(reason) => Ok(Self::Exhausted(reason)),
            GoalDisposition::Underway | GoalDisposition::Suspended(_) => {
                Err(GoalError::Inapplicable)
            }
        }
    }
}

pub(super) mod report_disposition_wire {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{GoalDisposition, GoalReportOutcome};

    pub fn serialize<S: Serializer>(
        outcome: &GoalReportOutcome,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        outcome.disposition().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<GoalReportOutcome, D::Error> {
        GoalDisposition::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

/// Pending data, not a successful result or proof of authenticated tool provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalPendingReport {
    admission: GoalReportBinding,
    call: GoalReportCall,
    outcome: GoalReportOutcome,
}

impl GoalPendingReport {
    #[must_use]
    pub fn new(
        admission: GoalReportBinding,
        call: GoalReportCall,
        outcome: GoalReportOutcome,
    ) -> Self {
        Self {
            admission,
            call,
            outcome,
        }
    }

    #[must_use]
    pub fn execution(&self) -> GoalExecution {
        self.admission.execution
    }

    #[must_use]
    pub fn call(&self) -> &GoalReportCall {
        &self.call
    }

    #[must_use]
    pub fn outcome(&self) -> &GoalReportOutcome {
        &self.outcome
    }

    fn action_id(&self) -> GoalText {
        let goal = &self.admission.goal;
        let source = (
            goal.owner().as_str(),
            goal.goal_id().as_str(),
            goal.version().get(),
            self.admission.execution.workflow_id(),
            self.admission.execution.turn_id(),
            self.call.transcript_id.as_str(),
            self.call.message_id.as_str(),
            self.call.tool_use_id.as_str(),
            &self.outcome,
        );
        let bytes = serde_json::to_vec(&source)
            .expect("checked report source fields serialize without arbitrary JSON");
        let fingerprint = crate::domain::sm_event::exact_payload_fingerprint(&bytes);
        GoalText::parse(format!("goal-report:{fingerprint}"))
            .expect("fixed prefix makes report action identity nonblank")
    }

    #[must_use]
    pub fn mutation(&self) -> GoalMutation {
        let goal = &self.admission.goal;
        let action_id = self.action_id();
        let binding = GoalExecutionBinding::new(
            goal.owner().clone(),
            self.admission.execution,
            GoalAdmissionObservation::Goal(goal.clone()),
        )
        .expect("binding owner is derived from its goal fence");
        let source = GoalAgentSource {
            product_conversation_id: goal.owner().clone(),
            execution: self.admission.execution,
            transcript_id: self.call.transcript_id.clone(),
            message_id: self.call.message_id.clone(),
            tool_use_id: self.call.tool_use_id.clone(),
        };
        GoalMutation {
            owner: goal.owner().clone(),
            expected_version: goal.version(),
            action_id: action_id.clone(),
            attribution: GoalAttribution {
                requested_by: GoalActor::OwningAgent(source),
                applied_by: GoalActor::GeneratedHost { action_id },
            },
            change: GoalChange::Report {
                binding,
                disposition: self.outcome.clone(),
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
        let owner = ProductConversationId::parse("report-owner").unwrap();
        GoalExecutionBinding {
            owner: owner.clone(),
            execution: GoalExecution::parse(31, 47).unwrap(),
            observation: GoalAdmissionObservation::Goal(
                GoalFence::new(
                    owner,
                    GoalId::parse("report-goal").unwrap(),
                    GoalVersion::parse(9).unwrap(),
                )
                .unwrap(),
            ),
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
        assert!(matches!(disposition, GoalReportOutcome::Blocked(_)));
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
    fn no_goal_or_zero_fence_cannot_construct_pending_report_binding() {
        let mut missing = binding();
        missing.observation = GoalAdmissionObservation::NoGoal {
            observed_version: GoalVersion::parse(9).unwrap(),
        };
        assert_eq!(
            GoalReportBinding::try_from(missing),
            Err(GoalError::ReportBindingRequired)
        );
        let valid = GoalReportBinding::try_from(binding()).unwrap();
        let mut zero = serde_json::to_value(valid).unwrap();
        zero["goal"]["version"] = serde_json::json!(0);
        assert!(serde_json::from_value::<GoalReportBinding>(zero).is_err());
    }

    #[test]
    fn report_action_identity_is_recomputed_from_exact_source_and_intended_payload() {
        let original = GoalPendingReport::new(
            binding().try_into().unwrap(),
            GoalReportCall {
                transcript_id: text("segment:a"),
                message_id: text("assistant:b"),
                tool_use_id: text("call"),
            },
            GoalReportOutcome::Complete(text("done")),
        );
        let mut changed_payload = original.clone();
        changed_payload.outcome = GoalReportOutcome::Blocked(text("different request body"));
        assert_ne!(
            original.mutation().action_id,
            changed_payload.mutation().action_id
        );
        let mut changed_source = original.clone();
        changed_source.call.message_id = text("assistant");
        changed_source.call.transcript_id = text("segment:a:b");
        assert_ne!(
            original.mutation().action_id,
            changed_source.mutation().action_id
        );
        let restored: GoalPendingReport =
            serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
        assert_eq!(original.mutation(), restored.mutation());
        assert!(serde_json::to_value(&original)
            .unwrap()
            .get("action_id")
            .is_none());
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
            GoalReportOutcome::Complete(text("done")),
        );
        let original = serde_json::to_value(&pending).unwrap();
        for field in ["admission", "call", "outcome"] {
            let mut missing = original.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<GoalPendingReport>(missing).is_err());
        }
        let mut invalid = original;
        invalid["admission"]["goal"]["version"] = serde_json::json!(0);
        assert!(serde_json::from_value::<GoalPendingReport>(invalid).is_err());
    }
}
