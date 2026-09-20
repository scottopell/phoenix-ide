/// Returns whether the exact resolved parent model is manually qualified to
/// orchestrate parallel Work sub-agents.
///
/// Unknown and newly introduced model IDs fail closed. Child execution choices
/// and parent reasoning effort do not participate in this decision.
#[must_use]
pub fn supports_parallel_work_subagents(resolved_parent_model_id: &str) -> bool {
    matches!(
        resolved_parent_model_id,
        "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-6-astra"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualification_is_identical_at_every_supported_effort() {
        use crate::domain::llm_types::ModelEffort;

        for effort in ModelEffort::ALL {
            for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-6-astra"] {
                assert!(
                    supports_parallel_work_subagents(model),
                    "{model} at {effort}"
                );
            }
            assert!(
                !supports_parallel_work_subagents("gpt-5.6-luna"),
                "Luna at {effort}"
            );
        }
    }

    #[test]
    fn exact_allowlist_fails_closed() {
        for qualified in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-6-astra"] {
            assert!(supports_parallel_work_subagents(qualified), "{qualified}");
        }
        for unqualified in [
            "gpt-5.6-luna",
            "gpt-6-astra-preview",
            "GPT-6-ASTRA",
            "claude-opus-5",
            "custom/gpt-6-astra",
            "",
        ] {
            assert!(
                !supports_parallel_work_subagents(unqualified),
                "{unqualified} must not inherit qualification"
            );
        }
    }
}
