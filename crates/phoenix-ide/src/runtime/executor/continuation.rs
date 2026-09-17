use super::{
    build_continuation_prompt, estimate_message_tokens, plan_continuation_history,
    plan_continuation_suffix, render_messages, render_rejected_tool_call, ContinuationBudgetResult,
    LlmMessage, CONTINUATION_MIN_HEADROOM_TOKENS, CONTINUATION_SYSTEM_PROMPT,
};
use crate::db::{Message, MessageContent};
use crate::state_machine::state::ToolCall;
use phoenix_core::llm_language::COORDINATOR_CONTINUATION_SYSTEM_PROMPT;

#[cfg(test)]
mod evaluation;

pub(super) enum CompactionPolicy {
    Work,
    Coordinator,
}

impl CompactionPolicy {
    pub(super) fn for_coordinator(is_coordinator: bool) -> Self {
        if is_coordinator {
            Self::Coordinator
        } else {
            Self::Work
        }
    }

    pub(super) fn system_prompt(&self) -> &'static str {
        match self {
            Self::Work => CONTINUATION_SYSTEM_PROMPT,
            Self::Coordinator => COORDINATOR_CONTINUATION_SYSTEM_PROMPT,
        }
    }

    pub(super) fn instruction(&self, rejected_tool_calls: &[ToolCall]) -> String {
        match self {
            Self::Work => build_continuation_prompt(rejected_tool_calls),
            Self::Coordinator => {
                let mut prompt = String::from(
                    "Write a handoff for the next Phoenix Coordinator, the user's primary interface \
                     across workstreams and delegated conversations. Preserve enough context to \
                     resume coordination without repeating work or losing promises.\n\n\
                     Prioritize:\n\
                     1. The user's priorities, corrections, decisions, and scoped authority limits.\n\
                     2. Unresolved commitments, including dormant or paused work: objective, owner \
                     conversation, blockers, reactivation conditions, and what is owed to the user.\n\
                     3. Delegation relationships: which project coordinator owns which workers; \
                     what was requested; distinguish sent, accepted/queued, acknowledged, and \
                     verified complete. Do not bypass an established owner or duplicate a request.\n\
                     4. Exact known conversation/message references and source links, last observed \
                     status and its evidence/time, and the next concrete checks. Distinguish owner \
                     identity from a historical transcript; resolve the current continuation before acting.\n\
                     5. Decisions and relevant hands-on changes, paths, tests, and pending actions.\n\n\
                     Apply explicit user corrections to the prior handoff. A delegate recommendation \
                     is not user authorization; distinguish user instructions, your promises, delegate \
                     reports, and assumptions. Carrying a claim through a handoff does not verify it. \
                     Unknown evidence, timestamps, and acknowledgement remain unknown. When reports \
                     conflict, preserve the uncertainty and the next verification.\n\n\
                     Forget completed detail and repetitive history before unresolved promises. Keep \
                     only enough resolution context to avoid reopening completed work; retain references \
                     for retrieving detail. Do not interpret a stream's absence from recent messages \
                     as completion. Do not invent commitments. Write directly to the next Coordinator.",
                );
                if !rejected_tool_calls.is_empty() {
                    prompt.push_str("\n\nThese pending tool calls did not run; preserve their intended next actions:\n");
                    for call in rejected_tool_calls {
                        prompt.push_str(&render_rejected_tool_call(call));
                        prompt.push('\n');
                    }
                }
                prompt
            }
        }
    }
}

pub(super) struct ContinuationHistory {
    pub(super) handoff: Option<ProtectedHandoff>,
}

pub(super) struct ProtectedHandoff {
    pub(super) message_id: String,
    pub(super) message: LlmMessage,
}

impl ContinuationHistory {
    pub(super) fn from_projection(
        messages: &[Message],
        accepted_message_id: Option<&str>,
    ) -> Result<Self, String> {
        let accepted = accepted_message_id
            .and_then(|id| messages.iter().find(|message| message.message_id == id));
        let handoff = if let Some(message) = accepted {
            if !matches!(message.content, MessageContent::User(_)) {
                return Err("Accepted continuation handoff is not a user message".to_string());
            }
            let mut rendered =
                render_messages(std::iter::once(message), &std::collections::HashSet::new());
            Some(ProtectedHandoff {
                message_id: message.message_id.clone(),
                message: rendered
                    .pop()
                    .ok_or("Accepted continuation handoff is not in the visible prompt")?,
            })
        } else {
            None
        };
        Ok(Self { handoff })
    }

    pub(super) fn selection_notice(&self, conversation_id: &str) -> String {
        let baseline = if self.handoff.is_some() {
            "The first input message is the full accepted previous handoff. Later messages can correct it."
        } else {
            "No accepted previous handoff is available in this transcript projection. Do not invent one."
        };
        format!(
            "\n\nInput selection: {baseline} The remaining history is a bounded newest suffix; \
             intervening details may be omitted. Do not infer completion or authorization from \
             omissions. Preserve this transcript reference for retrieving missing details when \
             relevant: @conv:{conversation_id}."
        )
    }
}

pub(super) fn plan_with_handoff(
    recent: Vec<LlmMessage>,
    handoff: Option<LlmMessage>,
    context_window: usize,
    fixed_tokens: usize,
    history_item_cap: Option<usize>,
) -> Result<ContinuationBudgetResult, String> {
    let Some(handoff) = handoff else {
        return Ok(plan_continuation_history(
            recent,
            context_window,
            fixed_tokens,
            history_item_cap,
        ));
    };
    let handoff_tokens = estimate_message_tokens(&handoff);
    let required = fixed_tokens
        .saturating_add(handoff_tokens)
        .saturating_add(CONTINUATION_MIN_HEADROOM_TOKENS);
    if required > context_window || history_item_cap == Some(0) {
        return Err(
            "The accepted previous handoff cannot fit in this model's compaction request. \
             It has not been shortened or dropped. Select a model with a larger context window \
             or lower reasoning effort, then retry compaction."
                .to_string(),
        );
    }
    let mut budget = plan_continuation_suffix(
        recent,
        context_window,
        fixed_tokens.saturating_add(handoff_tokens),
        history_item_cap.map(|cap| cap - 1),
        false,
    );
    budget.messages.insert(0, handoff);
    budget.input_budget = context_window.saturating_sub(fixed_tokens);
    budget.estimated_history_tokens += handoff_tokens;
    Ok(budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MessageType;
    use phoenix_llm::{ContentBlock, MessageRole};
    use proptest::prelude::*;

    fn user(text: &str) -> LlmMessage {
        LlmMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::text(text)],
        }
    }

    fn persisted(id: &str, text: &str) -> Message {
        Message {
            message_id: id.to_string(),
            conversation_id: "current".to_string(),
            sequence_id: 1,
            message_type: MessageType::User,
            content: MessageContent::user(text),
            display_data: None,
            usage_data: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn coordinator_compaction_request_removes_lifecycle_contradictions() {
        let policy = CompactionPolicy::for_coordinator(true);
        let system_prompt = policy.system_prompt();
        let instruction = policy.instruction(&[]);

        assert!(system_prompt.contains("You are writing a handoff for Phoenix Coordinator"));
        assert!(!system_prompt.contains("cannot create conversations"));
        assert!(!system_prompt.contains("NEVER call Phoenix HTTP API through Bash"));
        assert!(!instruction.contains("cannot create conversations"));
        assert!(!instruction.contains("NEVER call Phoenix HTTP API through Bash"));
    }

    #[test]
    fn continued_coordinator_uses_corrected_generated_prompt() {
        let temp = tempfile::TempDir::new().unwrap();
        crate::skills::builtin::extract_to(temp.path()).unwrap();
        let prompt = crate::system_prompt::build_coordinator_system_prompt_with_options(
            phoenix_core::llm_language::LlmLanguage::PhoenixNative,
            Some(temp.path()),
        );
        assert!(prompt.contains("Documented Phoenix API operations through scoped Bash"));
        assert!(prompt.contains("phoenix-api"));
        assert!(!prompt.contains("cannot create conversations"));
        assert!(!prompt.contains("NEVER call Phoenix HTTP API through Bash"));
    }

    #[test]
    fn accepted_seed_is_selected_by_id_not_first_message_or_equal_text() {
        let history = ContinuationHistory::from_projection(
            &[
                persisted("unrelated", "edited"),
                persisted("accepted", "edited"),
                persisted("new", "cancel Crick"),
            ],
            Some("accepted"),
        )
        .unwrap();
        let handoff = history.handoff.unwrap();
        assert_eq!(handoff.message.content, user("edited").content);
        assert_eq!(handoff.message_id, "accepted");
    }

    #[test]
    fn absent_or_reset_seed_does_not_guess_from_existing_text() {
        for accepted in [None, Some("removed")] {
            let history = ContinuationHistory::from_projection(
                &[persisted("other", "original generated summary")],
                accepted,
            )
            .unwrap();
            assert!(history.handoff.is_none());
            assert!(history
                .selection_notice("current")
                .contains("No accepted previous handoff"));
        }
    }

    #[test]
    fn protected_handoff_and_recent_cancellation_survive_trimming() {
        let seed = user("Crick paused until Friday; Phoenix owns worker A. No deploy permission.");
        let mut recent = vec![user(&"old completed details".repeat(400)); 40];
        recent.push(user(
            "Cancel Crick; ask Phoenix for status before contacting worker A.",
        ));
        let plan = plan_with_handoff(recent, Some(seed.clone()), 20_000, 5_000, Some(8)).unwrap();
        assert_eq!(plan.messages[0].content, seed.content);
        assert!(plan.messages.last().unwrap().content[0]
            .render_text()
            .contains("Cancel Crick"));
        assert!(plan.messages.len() <= 8);
        assert!(plan.minimum_headroom_satisfied);
        assert!(plan.estimated_history_tokens + 5_000 + CONTINUATION_MIN_HEADROOM_TOKENS <= 20_000);
    }

    #[test]
    fn assistant_work_after_opening_handoff_is_not_discarded() {
        let assistant = LlmMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::text(
                "Implemented the fix; tests passed; review is pending.",
            )],
        };
        for recent in [
            vec![assistant.clone()],
            vec![user(&"old details".repeat(10_000)), assistant.clone()],
        ] {
            let plan =
                plan_with_handoff(recent, Some(user("original goal")), 20_000, 5_000, Some(2))
                    .unwrap();
            assert_eq!(plan.messages.len(), 2);
            assert_eq!(plan.messages[1].content, assistant.content);
            assert_eq!(plan.trimmed_for_user_first, 0);
        }
    }

    #[test]
    fn oversized_handoff_fails_without_summarizing_a_clipped_seed() {
        assert!(plan_with_handoff(
            vec![user("recent")],
            Some(user(&"x".repeat(80_000))),
            20_000,
            5_000,
            None
        )
        .unwrap_err()
        .contains("not been shortened or dropped"));
        assert!(plan_with_handoff(vec![], Some(user("seed")), 20_000, 5_000, Some(0)).is_err());
    }

    #[test]
    fn no_handoff_uses_existing_suffix_planner() {
        let recent = vec![user("a"), user("b"), user("c")];
        let original = plan_continuation_history(recent.clone(), 20_000, 5_000, Some(2));
        let result = plan_with_handoff(recent, None, 20_000, 5_000, Some(2)).unwrap();
        assert_eq!(result.messages.len(), original.messages.len());
        assert_eq!(
            result.estimated_history_tokens,
            original.estimated_history_tokens
        );
        for (actual, expected) in result.messages.iter().zip(&original.messages) {
            assert_eq!(actual.content, expected.content);
        }
    }

    proptest! {
        #[test]
        fn whole_handoff_is_kept_once_within_joint_limits(
            seed in "[a-z]{1,2000}",
            sizes in prop::collection::vec(1usize..3000, 0..45),
            item_cap in 1usize..25,
        ) {
            let handoff = user(&format!("UNIQUE HANDOFF {seed}"));
            let recent = sizes.into_iter().map(|n| user(&"r".repeat(n))).collect();
            let plan = plan_with_handoff(recent, Some(handoff.clone()), 20_000, 5_000, Some(item_cap)).unwrap();
            prop_assert_eq!(&plan.messages[0].content, &handoff.content);
            prop_assert!(plan.messages.len() <= item_cap);
            prop_assert_eq!(plan.messages.iter().filter(|m| m.content == handoff.content).count(), 1);
            prop_assert!(plan.estimated_history_tokens + 5_000 + CONTINUATION_MIN_HEADROOM_TOKENS <= 20_000);
            prop_assert_eq!(plan.messages[0].role, MessageRole::User);
        }
    }
}
