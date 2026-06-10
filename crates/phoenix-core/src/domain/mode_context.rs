//! Conversation mode context threaded into the system prompt.

/// Conversation mode context for system prompt injection.
/// Carries only the stable, display-oriented fields the prompt needs.
#[derive(Debug, Clone)]
pub enum ModeContext {
    /// Read-only git project. Agent can investigate and propose tasks.
    Explore {
        /// Stable taskmd ID hint captured when the Explore workflow starts.
        next_taskmd_id_hint: Option<String>,
    },
    /// Isolated worktree with write access for an approved task.
    Work {
        branch_name: String,
        base_branch: String,
        worktree_path: String,
    },
    /// Direct mode: full tool access, no lifecycle ceremony.
    Direct,
    /// Branch mode: work directly on an existing branch. No task file.
    Branch {
        branch_name: String,
        base_branch: String,
        worktree_path: String,
    },
}
