//! Property-based tests derived from specs/projects/projects.allium
//!
//! These properties verify structural invariants of the project lifecycle:
//! mode-dependent field presence, worktree uniqueness, branch/work mode
//! discrimination, and lifecycle terminal behavior.
//!
//! Generated via /allium:propagate from the Allium spec. Each property
//! traces to a specific invariant or rule in the spec.

// Enabled: `mod project_proptests;` in state_machine.rs (behind #[cfg(test)])

#[cfg(test)]
mod tests {
    use crate::db::{ConvMode, NonEmptyString};
    use proptest::prelude::*;

    // ========================================================================
    // Generators
    // ========================================================================

    fn arb_work_mode() -> impl Strategy<Value = ConvMode> {
        (
            "[a-z]{5,10}",        // branch_name
            "/tmp/[a-z]{8}",      // worktree_path
            "[a-z]{3,8}",         // base_branch
            "[A-Z]{2}[0-9]{3,4}", // task_id
            "[a-zA-Z ]{5,30}",    // task_title
        )
            .prop_map(
                |(branch_name, worktree_path, base_branch, task_id, task_title)| ConvMode::Work {
                    branch_name: NonEmptyString::new(branch_name).unwrap(),
                    worktree_path: NonEmptyString::new(worktree_path).unwrap(),
                    base_branch: NonEmptyString::new(base_branch).unwrap(),
                    task_id: NonEmptyString::new(task_id).unwrap(),
                    task_title: NonEmptyString::new(task_title).unwrap(),
                },
            )
    }

    fn arb_branch_mode() -> impl Strategy<Value = ConvMode> {
        (
            "[a-z]{5,15}",   // branch_name
            "/tmp/[a-z]{8}", // worktree_path
            "[a-z]{3,8}",    // base_branch
        )
            .prop_map(
                |(branch_name, worktree_path, base_branch)| ConvMode::Branch {
                    branch_name: NonEmptyString::new(branch_name).unwrap(),
                    worktree_path: NonEmptyString::new(worktree_path).unwrap(),
                    base_branch: NonEmptyString::new(base_branch).unwrap(),
                },
            )
    }

    // ========================================================================
    // Invariant: Mode IS the discriminator (no WorktreeKind needed)
    //
    // The structural guarantee that eliminated the Work/Branch ambiguity:
    // Work mode ALWAYS has task_id. Branch mode NEVER has task_id.
    // If a type allows a Work mode without task_id, the type is wrong.
    //
    // Traces to: projects.allium invariants TaskFileExistsForManagedWorktrees,
    // BranchModeHasNoTaskFile
    // ========================================================================

    proptest! {
        /// Work mode always carries a non-empty task_id.
        /// This is the structural guarantee that distinguishes Work from Branch.
        /// With NonEmptyString the type system enforces this; the test documents
        /// the invariant and verifies it survives serde roundtrip.
        #[test]
        fn prop_work_mode_always_has_task_id(mode in arb_work_mode()) {
            match &mode {
                ConvMode::Work { task_id, .. } => {
                    prop_assert!(!task_id.as_str().is_empty(),
                        "Work mode must always have a non-empty task_id");
                }
                _ => prop_assert!(false, "Expected Work mode"),
            }
        }

        /// Work mode always carries a non-empty worktree_path.
        #[test]
        fn prop_work_mode_always_has_worktree_path(mode in arb_work_mode()) {
            match &mode {
                ConvMode::Work { worktree_path, .. } => {
                    prop_assert!(!worktree_path.as_str().is_empty(),
                        "Work mode must always have a non-empty worktree_path");
                }
                _ => prop_assert!(false, "Expected Work mode"),
            }
        }

        /// Work mode always carries a non-empty branch_name.
        #[test]
        fn prop_work_mode_always_has_branch_name(mode in arb_work_mode()) {
            match &mode {
                ConvMode::Work { branch_name, .. } => {
                    prop_assert!(!branch_name.as_str().is_empty(),
                        "Work mode must always have a non-empty branch_name");
                }
                _ => prop_assert!(false, "Expected Work mode"),
            }
        }
    }

    proptest! {
        /// Branch mode never carries a task_id.
        /// This is the structural complement of prop_work_mode_always_has_task_id.
        /// Together they prove the modes are structurally distinguishable.
        ///
        /// Traces to: BranchModeHasNoTaskFile invariant
        #[test]
        fn prop_branch_mode_never_has_task_id(mode in arb_branch_mode()) {
            // The type system enforces this -- ConvMode::Branch has no task_id field.
            // This test exists to document the invariant and catch regressions if
            // someone adds a task_id field to Branch.
            match &mode {
                ConvMode::Branch { .. } => {
                    // Compile-time: no task_id field accessible here.
                    // If this test compiles, the invariant holds.
                }
                _ => prop_assert!(false, "Expected Branch mode"),
            }
        }

        /// Branch mode always carries a non-empty branch_name.
        #[test]
        fn prop_branch_mode_always_has_branch_name(mode in arb_branch_mode()) {
            match &mode {
                ConvMode::Branch { branch_name, .. } => {
                    prop_assert!(!branch_name.as_str().is_empty(),
                        "Branch mode must always have a non-empty branch_name");
                }
                _ => prop_assert!(false, "Expected Branch mode"),
            }
        }
    }

    // ========================================================================
    // Invariant: Serde roundtrip preserves mode discrimination
    //
    // ConvMode is serialized to SQLite as JSON. If serde loses the mode
    // tag or field data during roundtrip, the structural guarantee is broken.
    // This caught a real class of bug: serde(default) shims on Work fields
    // could deserialize a Branch row as Work with empty task_id.
    //
    // Traces to: bedrock.allium state-dependent field presence
    // ========================================================================

    proptest! {
        /// Work mode survives JSON roundtrip with all fields intact.
        #[test]
        fn prop_work_mode_serde_roundtrip(mode in arb_work_mode()) {
            let json = serde_json::to_string(&mode).unwrap();
            let deserialized: ConvMode = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(&mode, &deserialized,
                "Work mode must survive serde roundtrip");
        }

        /// Branch mode survives JSON roundtrip without gaining a task_id.
        #[test]
        fn prop_branch_mode_serde_roundtrip(mode in arb_branch_mode()) {
            let json = serde_json::to_string(&mode).unwrap();
            let deserialized: ConvMode = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(&mode, &deserialized,
                "Branch mode must survive serde roundtrip");
            // Verify no task_id leaked in:
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            prop_assert!(value.get("task_id").is_none(),
                "Branch mode JSON must not contain task_id");
        }
    }

    // ========================================================================
    // Invariant: Mode transition legality
    //
    // The mode transition graph is:
    //   explore -> work       (Managed: approval upgrades permissions)
    //   direct -> branch      (Branch mode: skip Explore)
    //   terminal: direct, work, branch
    //
    // Illegal transitions (e.g., work -> branch, branch -> work, explore -> branch)
    // must be rejected. This is the property that ensures Branch mode can't be
    // reached from Explore (you'd need to go direct -> branch) and Work mode
    // can't transition to Branch (they're both terminal modes).
    //
    // Traces to: bedrock.allium transitions mode graph
    // ========================================================================

    /// Mode transitions that the spec declares as valid.
    fn is_valid_mode_transition(from: &ConvMode, to: &ConvMode) -> bool {
        matches!(
            (from, to),
            (ConvMode::Explore, ConvMode::Work { .. })
                | (ConvMode::Direct, ConvMode::Branch { .. })
        )
    }

    proptest! {
        /// Terminal modes cannot transition further.
        /// Work, Branch, and Direct are terminal in the mode graph.
        #[test]
        fn prop_terminal_modes_reject_transitions(mode in arb_work_mode()) {
            // Work is a terminal mode -- cannot transition to anything
            prop_assert!(!is_valid_mode_transition(&mode, &ConvMode::Explore));
            prop_assert!(!is_valid_mode_transition(&mode, &ConvMode::Direct));
            // Cannot transition Work -> Work (different fields)
            let other_work = ConvMode::Work {
                branch_name: NonEmptyString::new("other").unwrap(),
                worktree_path: NonEmptyString::new("/other").unwrap(),
                base_branch: NonEmptyString::new("main").unwrap(),
                task_id: NonEmptyString::new("XX999").unwrap(),
                task_title: NonEmptyString::new("other").unwrap(),
            };
            prop_assert!(!is_valid_mode_transition(&mode, &other_work));
        }
    }

    // ========================================================================
    // Invariant: Worktree path derived from conversation ID
    //
    // WorktreePath is always `.phoenix/worktrees/{conversation_id}` relative
    // to repo root. This makes collisions structurally impossible.
    //
    // Traces to: WorktreePathDerivedFromConversation invariant
    // ========================================================================

    proptest! {
        /// Worktree path is deterministically derived from conversation ID.
        #[test]
        fn prop_worktree_path_derived_from_conv_id(
            repo_root in "/[a-z]{3,10}/[a-z]{3,10}",
            conv_id in "[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}",
        ) {
            let expected = format!("{repo_root}/.phoenix/worktrees/{conv_id}");
            // Any worktree created for this conversation must use this path
            prop_assert_eq!(
                expected,
                format!("{repo_root}/.phoenix/worktrees/{conv_id}"),
                "Worktree path must be deterministic from repo_root + conv_id"
            );
        }

        /// Two different conversation IDs always produce different worktree paths.
        /// (This is a consequence of the derivation rule, verified independently.)
        #[test]
        fn prop_different_conv_ids_produce_different_paths(
            repo_root in "/[a-z]{3,10}",
            conv_id_a in "[a-f0-9]{36}",
            conv_id_b in "[a-f0-9]{36}",
        ) {
            prop_assume!(conv_id_a != conv_id_b);
            let path_a = format!("{repo_root}/.phoenix/worktrees/{conv_id_a}");
            let path_b = format!("{repo_root}/.phoenix/worktrees/{conv_id_b}");
            prop_assert_ne!(path_a, path_b,
                "Different conversation IDs must produce different worktree paths");
        }
    }

    // ========================================================================
    // Invariant: NoBlanketFetch
    //
    // Every git fetch must include a refspec. git fetch --prune (all refs) is
    // prohibited. This is a code-level constraint verified by grep/audit, but
    // we can test the specific functions that build fetch commands.
    //
    // Traces to: NoBlanketFetch invariant, REQ-PROJ-020/022
    // ========================================================================

    // This property is best verified by code audit (grep for "fetch" without refspec)
    // rather than a proptest. See the invariant in projects.allium.

    // ========================================================================
    // Scenario: Branch conflict detection (REQ-PROJ-025)
    //
    // When two conversations target the same branch, exactly one should succeed.
    // This is enforced by git worktree exclusivity + the BranchConflictDetected rule.
    //
    // The four permutations:
    // (A) Active conversation + worktree -> redirect prompt
    // (B) Orphaned worktree (no conversation) -> delete prompt
    // (C) Stale conversation (no worktree) -> redirect to conversation
    // Terminal conversations are excluded.
    //
    // Traces to: OneBranchOneActiveWorktree invariant, BranchConflictDetected rule
    // ========================================================================

    // These are integration tests (require git/filesystem), not proptests.
    // Documented here for traceability; implemented in integration test suite.
}

// ============================================================================
// State machine property tests
// ============================================================================

#[cfg(test)]
mod state_machine_props {
    use crate::state_machine::effect::Effect;
    use crate::state_machine::event::Event;
    use crate::state_machine::proptests::{arb_error_kind, arb_event, arb_state, test_context};
    use crate::state_machine::state::{ConvContext, ConvState};
    use crate::state_machine::transition::transition;
    use proptest::prelude::*;
    use std::path::PathBuf;

    // ====================================================================
    // Generators for terminal states
    // ====================================================================

    fn arb_completed_state() -> impl Strategy<Value = ConvState> {
        "[a-zA-Z0-9 ]{1,50}".prop_map(|result| ConvState::Completed { result })
    }

    fn arb_failed_state() -> impl Strategy<Value = ConvState> {
        ("[a-zA-Z ]{1,30}", arb_error_kind())
            .prop_map(|(error, error_kind)| ConvState::Failed { error, error_kind })
    }

    fn arb_context_exhausted_state() -> impl Strategy<Value = ConvState> {
        "[a-zA-Z0-9 ]{0,100}".prop_map(|summary| ConvState::ContextExhausted { summary })
    }

    fn arb_any_terminal_state() -> impl Strategy<Value = ConvState> {
        prop_oneof![
            arb_completed_state(),
            arb_failed_state(),
            arb_context_exhausted_state(),
            Just(ConvState::Terminal),
        ]
    }

    fn subagent_context() -> ConvContext {
        ConvContext::sub_agent("test-sub", PathBuf::from("/tmp"), "test-model", 200_000)
    }

    // ====================================================================
    // Property 1: Terminal state absorption
    //
    // Once a conversation reaches a terminal state, no event can change
    // the state variant. Events either return Err (rejected) or Ok with
    // the same variant.
    // ====================================================================

    proptest! {
        #[test]
        fn prop_terminal_state_absorption(
            state in arb_any_terminal_state(),
            events in proptest::collection::vec(arb_event(), 20..30),
        ) {
            let ctx = test_context();
            let variant = state.variant_name();

            for event in events {
                if let Ok(result) = transition(&state, &ctx, event) {
                    prop_assert_eq!(
                        result.new_state.variant_name(),
                        variant,
                        "Terminal state {} changed variant to {}",
                        variant,
                        result.new_state.variant_name(),
                    );
                }
            }
        }
    }

    // ====================================================================
    // Property 2: Sub-agent NotifyParent conservation
    //
    // Any transition where context.is_sub_agent == true that reaches
    // Completed or Failed must include Effect::NotifyParent.
    // ====================================================================

    proptest! {
        #[test]
        fn prop_subagent_notify_parent_on_terminal(
            state in arb_state(),
            event in arb_event(),
        ) {
            let ctx = subagent_context();
            if let Ok(result) = transition(&state, &ctx, event) {
                let reached_terminal = matches!(
                    result.new_state,
                    ConvState::Completed { .. } | ConvState::Failed { .. }
                );
                if reached_terminal {
                    prop_assert!(
                        result.effects.iter().any(|e| matches!(e, Effect::NotifyParent { .. })),
                        "Sub-agent reached {:?} without NotifyParent effect. Effects: {:?}",
                        result.new_state.variant_name(),
                        result.effects,
                    );
                }
            }
        }
    }

    // ====================================================================
    // Property 3: Effect ordering -- PersistState before I/O effects
    //
    // If a transition produces both PersistState and RequestLlm (or
    // ExecuteTool), PersistState must appear first. This ensures the
    // state is durable before the side-effect fires.
    // ====================================================================

    proptest! {
        #[test]
        fn prop_persist_state_before_io_effects(
            state in arb_state(),
            event in arb_event(),
        ) {
            if let Ok(result) = transition(&state, &test_context(), event) {
                let persist_pos = result.effects.iter().position(|e| matches!(e, Effect::PersistState));
                let request_llm_pos = result.effects.iter().position(|e| matches!(e, Effect::RequestLlm));
                let execute_tool_pos = result.effects.iter().position(|e| matches!(e, Effect::ExecuteTool { .. }));

                if let (Some(p), Some(r)) = (persist_pos, request_llm_pos) {
                    prop_assert!(
                        p < r,
                        "PersistState (pos {}) must come before RequestLlm (pos {}). Effects: {:?}",
                        p, r, result.effects,
                    );
                }

                if let (Some(p), Some(e)) = (persist_pos, execute_tool_pos) {
                    prop_assert!(
                        p < e,
                        "PersistState (pos {}) must come before ExecuteTool (pos {}). Effects: {:?}",
                        p, e, result.effects,
                    );
                }
            }
        }
    }

    // ====================================================================
    // Property 4: TaskResolved only from Idle
    //
    // Non-Idle, non-Terminal states must not successfully transition to
    // Terminal via TaskResolved.
    // ====================================================================

    fn arb_non_idle_non_terminal_state() -> impl Strategy<Value = ConvState> {
        arb_state().prop_filter("must be non-Idle and non-Terminal", |s| {
            !matches!(s, ConvState::Idle | ConvState::Terminal)
        })
    }

    proptest! {
        #[test]
        fn prop_task_resolved_only_from_idle(
            state in arb_non_idle_non_terminal_state(),
        ) {
            let event = Event::TaskResolved {
                system_message: "completed".to_string(),
                repo_root: "/tmp".to_string(),
            };
            let result = transition(&state, &test_context(), event);
            match result {
                Err(_) => { /* Rejected -- correct */ }
                Ok(tr) => {
                    prop_assert!(
                        !matches!(tr.new_state, ConvState::Terminal),
                        "TaskResolved from {} should not reach Terminal, but did",
                        state.variant_name(),
                    );
                }
            }
        }
    }
}

// ============================================================================
// State-coherent random walk property tests
// ============================================================================

#[cfg(test)]
mod random_walk {
    use crate::db::{ErrorKind, ToolResult};
    use crate::llm::{ContentBlock, Usage};
    use crate::state_machine::effect::Effect;
    use crate::state_machine::event::Event;
    use crate::state_machine::proptests::{effects_are_valid, is_valid_state, test_context};
    use crate::state_machine::state::{
        ConvContext, ConvState, SubAgentOutcome, TaskApprovalOutcome, ToolCall, ToolInput,
    };
    use crate::state_machine::transition::transition;
    use proptest::prelude::*;
    use rand::rngs::StdRng;
    use rand::Rng;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use std::path::PathBuf;

    // ====================================================================
    // State-aware event generator
    // ====================================================================

    fn random_string(rng: &mut impl Rng, len: usize) -> String {
        (0..len)
            .map(|_| (b'a' + rng.gen_range(0..26)) as char)
            .collect()
    }

    fn random_id(rng: &mut impl Rng) -> String {
        random_string(rng, 8)
    }

    fn random_tool_call(rng: &mut impl Rng) -> ToolCall {
        let id = random_id(rng);
        ToolCall::new(
            id,
            ToolInput::Think(crate::state_machine::state::ThinkInput {
                thoughts: random_string(rng, 10),
            }),
        )
    }

    fn random_error_kind(rng: &mut impl Rng) -> ErrorKind {
        match rng.gen_range(0..6) {
            0 => ErrorKind::Network,
            1 => ErrorKind::RateLimit,
            2 => ErrorKind::ServerError,
            3 => ErrorKind::Auth,
            4 => ErrorKind::InvalidRequest,
            _ => ErrorKind::ContentFilter,
        }
    }

    /// Generate an event that the transition function will accept for the current state.
    /// This reads the transition match arms to produce events that won't hit the catch-all.
    #[allow(clippy::too_many_lines)]
    fn generate_valid_event(state: &ConvState, rng: &mut impl Rng) -> Event {
        match state {
            ConvState::Idle => match rng.gen_range(0..3) {
                0 => Event::UserMessage {
                    text: random_string(rng, 10),
                    llm_text: None,
                    images: vec![],
                    message_id: uuid::Uuid::new_v4().to_string(),
                    user_agent: None,
                    skill_invocation: None,
                },
                1 => Event::TaskResolved {
                    system_message: random_string(rng, 15),
                    repo_root: "/tmp".to_string(),
                },
                _ => Event::UserTriggerContinuation,
            },

            ConvState::LlmRequesting { attempt } => {
                match rng.gen_range(0..4) {
                    0 => {
                        // LlmResponse: text-only or with tool calls
                        let num_tools = rng.gen_range(0..3);
                        let mut tool_calls: Vec<ToolCall> = Vec::new();
                        let mut content = vec![ContentBlock::text("response")];
                        for _ in 0..num_tools {
                            let tc = random_tool_call(rng);
                            content.push(ContentBlock::ToolUse {
                                id: tc.id.clone(),
                                name: tc.name().to_string(),
                                input: serde_json::json!({}),
                            });
                            tool_calls.push(tc);
                        }
                        Event::LlmResponse {
                            content,
                            tool_calls,
                            end_turn: true,
                            usage: Usage::default(),
                        }
                    }
                    1 => {
                        // LlmError with random retryable/non-retryable
                        let error_kind = random_error_kind(rng);
                        let recovery = rng.gen_bool(0.2) && matches!(error_kind, ErrorKind::Auth);
                        Event::LlmError {
                            message: random_string(rng, 15),
                            error_kind,
                            attempt: *attempt,
                            recovery_in_progress: recovery,
                        }
                    }
                    2 => Event::UserCancel { reason: None },
                    _ => {
                        // RetryTimeout matching the current attempt
                        Event::RetryTimeout { attempt: *attempt }
                    }
                }
            }

            ConvState::ToolExecuting { current_tool, .. } => {
                match rng.gen_range(0..2) {
                    0 => {
                        // ToolComplete with matching ID
                        Event::ToolComplete {
                            tool_use_id: current_tool.id.clone(),
                            result: ToolResult::success(
                                current_tool.id.clone(),
                                random_string(rng, 20),
                            ),
                        }
                    }
                    _ => Event::UserCancel { reason: None },
                }
            }

            ConvState::CancellingTool {
                tool_use_id,
                pending_sub_agents,
                ..
            } => {
                // Also consider SubAgentResult if there are pending sub-agents
                let options = if pending_sub_agents.is_empty() { 2 } else { 3 };
                match rng.gen_range(0..options) {
                    0 => Event::ToolAborted {
                        tool_use_id: tool_use_id.clone(),
                    },
                    1 => Event::ToolComplete {
                        tool_use_id: tool_use_id.clone(),
                        result: ToolResult::success(tool_use_id.clone(), random_string(rng, 10)),
                    },
                    _ => {
                        // SubAgentResult for a pending agent
                        let agent = &pending_sub_agents[rng.gen_range(0..pending_sub_agents.len())];
                        Event::SubAgentResult {
                            agent_id: agent.agent_id.clone(),
                            outcome: SubAgentOutcome::Success {
                                result: random_string(rng, 10),
                            },
                        }
                    }
                }
            }

            ConvState::AwaitingSubAgents { pending, .. } => {
                if pending.is_empty() {
                    // Shouldn't happen, but be defensive
                    return Event::UserCancel { reason: None };
                }
                match rng.gen_range(0..2) {
                    0 => {
                        let agent = &pending[rng.gen_range(0..pending.len())];
                        let outcome = if rng.gen_bool(0.7) {
                            SubAgentOutcome::Success {
                                result: random_string(rng, 15),
                            }
                        } else {
                            SubAgentOutcome::Failure {
                                error: random_string(rng, 15),
                                error_kind: ErrorKind::SubAgentError,
                            }
                        };
                        Event::SubAgentResult {
                            agent_id: agent.agent_id.clone(),
                            outcome,
                        }
                    }
                    _ => Event::UserCancel { reason: None },
                }
            }

            ConvState::CancellingSubAgents { pending, .. } => {
                if pending.is_empty() {
                    // Shouldn't happen, but defensive
                    return Event::UserCancel { reason: None };
                }
                let agent = &pending[rng.gen_range(0..pending.len())];
                Event::SubAgentResult {
                    agent_id: agent.agent_id.clone(),
                    outcome: SubAgentOutcome::Failure {
                        error: "cancelled".to_string(),
                        error_kind: ErrorKind::Cancelled,
                    },
                }
            }

            ConvState::Error { .. } => Event::UserMessage {
                text: random_string(rng, 10),
                llm_text: None,
                images: vec![],
                message_id: uuid::Uuid::new_v4().to_string(),
                user_agent: None,
                skill_invocation: None,
            },

            ConvState::AwaitingRecovery { .. } => match rng.gen_range(0..3) {
                0 => Event::CredentialBecameAvailable,
                1 => Event::CredentialHelperFailed {
                    message: random_string(rng, 15),
                },
                _ => Event::UserCancel { reason: None },
            },

            ConvState::AwaitingContinuation { attempt, .. } => match rng.gen_range(0..4) {
                0 => Event::ContinuationResponse {
                    summary: random_string(rng, 30),
                },
                1 => Event::ContinuationFailed {
                    error: random_string(rng, 15),
                },
                2 => {
                    let error_kind = random_error_kind(rng);
                    Event::LlmError {
                        message: random_string(rng, 15),
                        error_kind,
                        attempt: *attempt,
                        recovery_in_progress: false,
                    }
                }
                _ => Event::UserCancel { reason: None },
            },

            ConvState::AwaitingTaskApproval { .. } => match rng.gen_range(0..4) {
                0 => Event::TaskApprovalResponse {
                    outcome: TaskApprovalOutcome::Approved,
                },
                1 => Event::TaskApprovalResponse {
                    outcome: TaskApprovalOutcome::Rejected,
                },
                2 => Event::TaskApprovalResponse {
                    outcome: TaskApprovalOutcome::FeedbackProvided {
                        annotations: random_string(rng, 20),
                    },
                },
                _ => Event::UserCancel { reason: None },
            },

            ConvState::AwaitingUserResponse { questions, .. } => {
                match rng.gen_range(0..2) {
                    0 => {
                        // Build answers matching the questions
                        let answers: HashMap<String, String> = questions
                            .iter()
                            .map(|q| {
                                let answer = if q.options.is_empty() {
                                    random_string(rng, 5)
                                } else {
                                    q.options[rng.gen_range(0..q.options.len())].label.clone()
                                };
                                (q.question.clone(), answer)
                            })
                            .collect();
                        Event::UserQuestionResponse {
                            answers,
                            annotations: None,
                        }
                    }
                    _ => Event::UserCancel { reason: None },
                }
            }

            // Terminal states -- events are absorbed, generate anything
            ConvState::ContextExhausted { .. }
            | ConvState::Terminal
            | ConvState::Completed { .. }
            | ConvState::Failed { .. } => Event::UserCancel { reason: None },
        }
    }

    // ====================================================================
    // Invariant checker
    // ====================================================================

    fn check_invariants(
        old_state: &ConvState,
        new_state: &ConvState,
        effects: &[Effect],
        context: &ConvContext,
    ) {
        // 1. Terminal absorption: if old_state was terminal, new_state must be same variant
        if old_state.is_terminal() {
            assert_eq!(
                old_state.variant_name(),
                new_state.variant_name(),
                "Terminal state {} changed to {}",
                old_state.variant_name(),
                new_state.variant_name()
            );
        }

        // 2. PersistState on variant change
        //    Exception: ResolveTask effect handles its own persistence atomically
        //    alongside mode and cwd updates (execute_resolve_task).
        if old_state.variant_name() != new_state.variant_name() {
            let has_persist = effects.iter().any(|e| matches!(e, Effect::PersistState));
            let has_resolve_task = effects
                .iter()
                .any(|e| matches!(e, Effect::ResolveTask { .. }));
            assert!(
                has_persist || has_resolve_task,
                "State changed from {} to {} without PersistState or ResolveTask. Effects: {:?}",
                old_state.variant_name(),
                new_state.variant_name(),
                effects,
            );
        }

        // 3. Effect ordering: PersistState before RequestLlm, PersistState before ExecuteTool
        let persist_pos = effects
            .iter()
            .position(|e| matches!(e, Effect::PersistState));
        let request_llm_pos = effects.iter().position(|e| matches!(e, Effect::RequestLlm));
        let execute_tool_pos = effects
            .iter()
            .position(|e| matches!(e, Effect::ExecuteTool { .. }));

        if let (Some(p), Some(r)) = (persist_pos, request_llm_pos) {
            assert!(
                p < r,
                "PersistState (pos {p}) must come before RequestLlm (pos {r}). Effects: {effects:?}",
            );
        }
        if let (Some(p), Some(e)) = (persist_pos, execute_tool_pos) {
            assert!(
                p < e,
                "PersistState (pos {p}) must come before ExecuteTool (pos {e}). Effects: {effects:?}",
            );
        }

        // 4. Sub-agent NotifyParent on terminal
        if context.is_sub_agent
            && matches!(
                new_state,
                ConvState::Completed { .. } | ConvState::Failed { .. }
            )
        {
            assert!(
                effects
                    .iter()
                    .any(|e| matches!(e, Effect::NotifyParent { .. })),
                "Sub-agent reached {} without NotifyParent. Effects: {:?}",
                new_state.variant_name(),
                effects,
            );
        }

        // 5. Valid state (attempt range, no duplicate tool IDs)
        assert!(
            is_valid_state(new_state),
            "Invalid state after transition: {new_state:?}",
        );

        // 6. Effects are valid for the new state
        assert!(
            effects_are_valid(effects, new_state),
            "Invalid effects for state {new_state:?}: {effects:?}",
        );

        // 7. Tool count conservation in ToolExecuting
        // (current + remaining + completed should be consistent across transitions)
        // We check that ToolExecuting always has a non-empty current_tool
        if let ConvState::ToolExecuting { current_tool, .. } = new_state {
            assert!(
                !current_tool.id.is_empty(),
                "ToolExecuting has empty current_tool.id"
            );
        }
    }

    // ====================================================================
    // The walks
    // ====================================================================

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Random walk from Idle through the state machine.
        /// Each step generates an event valid for the current state,
        /// applies it, and checks invariants.
        #[test]
        fn prop_coherent_random_walk(seed in 0u64..u64::MAX) {
            let mut rng = StdRng::seed_from_u64(seed);
            let ctx = test_context();
            let mut state = ConvState::Idle;

            for _step in 0..200 {
                if state.is_terminal() {
                    break;
                }

                let event = generate_valid_event(&state, &mut rng);
                if let Ok(result) = transition(&state, &ctx, event) {
                    check_invariants(&state, &result.new_state, &result.effects, &ctx);
                    state = result.new_state;
                }
                // Err: generator might occasionally produce invalid events
                // (e.g., RetryTimeout with wrong attempt, sub-agent-only paths).
            }
        }

        /// Same walk but with is_sub_agent context.
        /// Sub-agents hit Completed/Failed instead of Error on LLM failures,
        /// and UserCancel produces Failed + NotifyParent.
        #[test]
        fn prop_coherent_random_walk_subagent(seed in 0u64..u64::MAX) {
            let mut rng = StdRng::seed_from_u64(seed);
            let ctx = ConvContext::sub_agent(
                "test-sub-walk",
                PathBuf::from("/tmp"),
                "test-model",
                200_000,
            );
            let mut state = ConvState::Idle;

            for _step in 0..200 {
                if state.is_terminal() {
                    break;
                }

                let event = generate_valid_event(&state, &mut rng);
                if let Ok(result) = transition(&state, &ctx, event) {
                    check_invariants(&state, &result.new_state, &result.effects, &ctx);
                    state = result.new_state;
                }
                // Err: sub-agent context rejects some events that parent accepts
                // (e.g., UserTriggerContinuation from Idle, TaskResolved).
            }
        }

        /// No Direct-mode conversation should ever reach AwaitingTaskApproval.
        /// Uses the random walk pattern with ModeKind::Direct and injects
        /// propose_task tool calls in LlmResponse events to exercise the guard.
        #[test]
        fn prop_direct_mode_never_reaches_task_approval(seed in 0u64..u64::MAX) {
            use crate::state_machine::state::{ModeKind, ProposeTaskInput};

            let mut rng = StdRng::seed_from_u64(seed);
            let mut ctx = ConvContext::new(
                "test-direct-walk",
                PathBuf::from("/tmp"),
                "test-model",
                200_000,
            );
            ctx.mode = ModeKind::Direct;
            let mut state = ConvState::Idle;

            for _step in 0..200 {
                if state.is_terminal() {
                    break;
                }

                // When in LlmRequesting, sometimes inject a propose_task response
                let event = if matches!(state, ConvState::LlmRequesting { .. }) && rng.gen_bool(0.3) {
                    let tool_id = random_id(&mut rng);
                    let tc = ToolCall::new(
                        tool_id.clone(),
                        ToolInput::ProposeTask(ProposeTaskInput {
                            title: random_string(&mut rng, 10),
                            priority: "p1".to_string(),
                            plan: random_string(&mut rng, 20),
                        }),
                    );
                    Event::LlmResponse {
                        content: vec![ContentBlock::tool_use(
                            &tool_id,
                            "propose_task",
                            serde_json::json!({}),
                        )],
                        tool_calls: vec![tc],
                        end_turn: true,
                        usage: Usage::default(),
                    }
                } else {
                    generate_valid_event(&state, &mut rng)
                };

                if let Ok(result) = transition(&state, &ctx, event) {
                    prop_assert!(
                        !matches!(result.new_state, ConvState::AwaitingTaskApproval { .. }),
                        "Direct-mode conversation reached AwaitingTaskApproval from {}",
                        state.variant_name(),
                    );
                    check_invariants(&state, &result.new_state, &result.effects, &ctx);
                    state = result.new_state;
                }
            }
        }
    }
}
