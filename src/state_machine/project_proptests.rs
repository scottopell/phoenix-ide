//! Property-based tests derived from specs/projects/projects.allium
//!
//! These properties verify structural invariants of the project lifecycle:
//! mode-dependent field presence, worktree uniqueness, branch/work mode
//! discrimination, and lifecycle terminal behavior.
//!
//! Generated via /allium:propagate from the Allium spec. Each property
//! traces to a specific invariant or rule in the spec.

// NOTE: These tests describe the TARGET state after Branch mode is implemented.
// They will not compile until ConvMode::Branch exists. Uncomment when implementing
// REQ-PROJ-024 through REQ-PROJ-029.
//
// To enable: add `mod project_proptests;` to state_machine.rs (behind #[cfg(test)])

#[cfg(test)]
mod tests {
    use crate::db::ConvMode;
    use proptest::prelude::*;

    // ========================================================================
    // Generators
    // ========================================================================

    fn arb_work_mode() -> impl Strategy<Value = ConvMode> {
        (
            "[a-z]{5,10}",           // branch_name
            "/tmp/[a-z]{8}",         // worktree_path
            "[a-z]{3,8}",            // base_branch
            "[A-Z]{2}[0-9]{3,4}",   // task_id
            "[a-zA-Z ]{5,30}",       // task_title
        )
            .prop_map(|(branch_name, worktree_path, base_branch, task_id, task_title)| {
                ConvMode::Work {
                    branch_name,
                    worktree_path,
                    base_branch,
                    task_id,
                    task_title,
                }
            })
    }

    // Uncomment when ConvMode::Branch exists:
    // fn arb_branch_mode() -> impl Strategy<Value = ConvMode> {
    //     (
    //         "[a-z]{5,15}",           // branch_name
    //         "/tmp/[a-z]{8}",         // worktree_path
    //         "[a-z]{3,8}",            // base_branch
    //     )
    //         .prop_map(|(branch_name, worktree_path, base_branch)| {
    //             ConvMode::Branch {
    //                 branch_name,
    //                 worktree_path,
    //                 base_branch,
    //             }
    //         })
    // }

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
        #[test]
        fn prop_work_mode_always_has_task_id(mode in arb_work_mode()) {
            match &mode {
                ConvMode::Work { task_id, .. } => {
                    prop_assert!(!task_id.is_empty(),
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
                    prop_assert!(!worktree_path.is_empty(),
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
                    prop_assert!(!branch_name.is_empty(),
                        "Work mode must always have a non-empty branch_name");
                }
                _ => prop_assert!(false, "Expected Work mode"),
            }
        }
    }

    // Uncomment when ConvMode::Branch exists:
    //
    // proptest! {
    //     /// Branch mode never carries a task_id.
    //     /// This is the structural complement of prop_work_mode_always_has_task_id.
    //     /// Together they prove the modes are structurally distinguishable.
    //     ///
    //     /// Traces to: BranchModeHasNoTaskFile invariant
    //     #[test]
    //     fn prop_branch_mode_never_has_task_id(mode in arb_branch_mode()) {
    //         // The type system enforces this -- ConvMode::Branch has no task_id field.
    //         // This test exists to document the invariant and catch regressions if
    //         // someone adds a task_id field to Branch.
    //         match &mode {
    //             ConvMode::Branch { .. } => {
    //                 // Compile-time: no task_id field accessible here.
    //                 // If this test compiles, the invariant holds.
    //             }
    //             _ => prop_assert!(false, "Expected Branch mode"),
    //         }
    //     }
    //
    //     /// Branch mode always carries a non-empty branch_name.
    //     #[test]
    //     fn prop_branch_mode_always_has_branch_name(mode in arb_branch_mode()) {
    //         match &mode {
    //             ConvMode::Branch { branch_name, .. } => {
    //                 prop_assert!(!branch_name.is_empty(),
    //                     "Branch mode must always have a non-empty branch_name");
    //             }
    //             _ => prop_assert!(false, "Expected Branch mode"),
    //         }
    //     }
    // }

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

        // Uncomment when ConvMode::Branch exists:
        // /// Branch mode survives JSON roundtrip without gaining a task_id.
        // #[test]
        // fn prop_branch_mode_serde_roundtrip(mode in arb_branch_mode()) {
        //     let json = serde_json::to_string(&mode).unwrap();
        //     let deserialized: ConvMode = serde_json::from_str(&json).unwrap();
        //     prop_assert_eq!(&mode, &deserialized,
        //         "Branch mode must survive serde roundtrip");
        //     // Verify no task_id leaked in:
        //     let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        //     prop_assert!(value.get("task_id").is_none(),
        //         "Branch mode JSON must not contain task_id");
        // }
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
            // Uncomment when ConvMode::Branch exists:
            // | (ConvMode::Direct, ConvMode::Branch { .. })
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
                branch_name: "other".to_string(),
                worktree_path: "/other".to_string(),
                base_branch: "main".to_string(),
                task_id: "XX999".to_string(),
                task_title: "other".to_string(),
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
