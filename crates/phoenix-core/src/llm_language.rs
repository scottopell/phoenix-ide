//! LLM-facing text translation layer.
//!
//! All text that gets sent TO the LLM (system prompts, mode context blocks,
//! tool descriptions, chain-Q&A prompts) is keyed by an `LlmLanguage` so the
//! user can swap the "voice" Phoenix uses with the model without altering
//! any user-facing UI text.
//!
//! Currently two languages:
//! - `PhoenixNative` — the default prose Phoenix has used historically.
//! - `Caveman` — radically terse, instructional-shorthand variants for
//!   experimentation.
//!
//! The language is stored per-conversation (column `conversations.llm_language`)
//! and is fixed at conversation creation from the global app default
//! (`app_settings.default_llm_language`). Chain continuations and sub-agent
//! conversations inherit their parent's language.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum LlmLanguage {
    #[default]
    PhoenixNative,
    Caveman,
}

impl LlmLanguage {
    /// All known languages in display order. Single source of truth for the
    /// settings API response and any UI dropdown so the available choices
    /// can't drift from what the backend accepts. Adding a variant here
    /// makes it visible to clients, but you still have to supply
    /// translations (base prompt, mode blocks, chain prompts, tool
    /// overrides as needed) elsewhere in this file for it to be useful.
    pub const ALL: &'static [Self] = &[Self::PhoenixNative, Self::Caveman];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PhoenixNative => "phoenix-native",
            Self::Caveman => "caveman",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|lang| lang.as_str() == s)
    }

    /// Parse with fallback to default for unknown / NULL values from old DB rows.
    #[must_use]
    pub fn parse_or_default(s: &str) -> Self {
        Self::parse(s).unwrap_or_default()
    }
}

// =============================================================================
// System-prompt text variants.
// =============================================================================

#[must_use]
pub fn base_prompt(lang: LlmLanguage) -> &'static str {
    match lang {
        LlmLanguage::PhoenixNative => {
            "You are a helpful AI assistant with access to tools for executing code, editing files, and searching codebases. Use tools when appropriate to accomplish tasks.

Be concise in your responses. When using tools, explain what you're doing briefly."
        }
        LlmLanguage::Caveman => {
            "You smart caveman. You have tools. Use tools do task. Why use many word when few word do trick. Use word like cave. Talk short. Do thing. Reply short."
        }
    }
}

#[must_use]
pub fn sub_agent_suffix(lang: LlmLanguage) -> &'static str {
    match lang {
        LlmLanguage::PhoenixNative => {
            "\n\nYou are a sub-agent working on a specific task. When you complete your task, call submit_result with your findings. If you encounter an unrecoverable error, call submit_error. Your conversation will end after calling either tool."
        }
        LlmLanguage::Caveman => {
            "\n\nYou small caveman. Big caveman give you one job. Done? Call submit_result. Stuck? Call submit_error. Then sleep."
        }
    }
}

// Mode context blocks. Each accepts the same fields the phoenix-native
// version uses so language is a pure swap.

#[must_use]
pub fn mode_explore(lang: LlmLanguage, tasks_dir_name: &str) -> String {
    match lang {
        LlmLanguage::PhoenixNative => format!(
            "\n\nYou are in Explore mode. This conversation is read-only \
             for source files -- you can read files, search, analyze, and \
             discuss the codebase, but you cannot modify code.\n\n\
             Workflow for proposing work:\n\
             1. Draft (or reuse) a task file. The body is free-form \
                markdown -- start with an `# H1` title, then the plan.\n   \
                Use the taskmd convention: name the file \
                `NNNNN-pX-status--slug.md` (status one of `ready`, \
                `in-progress`, or `brainstorming`) under `{tasks_dir_name}/`. \
                It's just a filename -- no tooling needed -- and it gives \
                the task a stable id/priority/status/slug plus an automatic \
                `ready` -> `in-progress` rename on approval, so prefer it. \
                To draft one, use `patch` with operation `overwrite` (the \
                Explore-mode `patch` allowlist is scoped to \
                `{tasks_dir_name}/`).\n   \
                If you genuinely can't follow that convention, any other \
                `.md` file is still accepted as a plain brief -- but it \
                carries no metadata and gets no status rename, so it's \
                strictly less useful; reach for it only as a fallback. A \
                taskmd-pattern filename is accepted ONLY under \
                `{tasks_dir_name}/`; a plain `.md` file may live anywhere \
                in the worktree (e.g. point `propose_task` at an existing \
                `docs/plan.md`).\n\
             2. Call `propose_task` with `task_file` set to the path \
                (e.g. `{tasks_dir_name}/12345-p2-ready--my-slug.md`). The \
                user will review and can approve, request revisions, or \
                reject. On approval, an isolated worktree is created and \
                you gain full write access.\n\n\
             The `patch` tool is restricted to `{tasks_dir_name}/` in this \
             mode. `bash` is unavailable. If the user asks you to change \
             code directly, explain that you must propose a task first."
        ),
        LlmLanguage::Caveman => format!(
            "\n\nYou in look-only cave. Look at code. No change code. \
             To do work: write plan in `{tasks_dir_name}/` (file like \
             `12345-p2-ready--slug.md`), then call propose_task with that file. \
             Big caveman say yes? You get new cave with write power. \
             No bash here. patch only in `{tasks_dir_name}/`."
        ),
    }
}

#[must_use]
pub fn mode_work(
    lang: LlmLanguage,
    branch_name: &str,
    base_branch: &str,
    worktree_path: &str,
) -> String {
    match lang {
        LlmLanguage::PhoenixNative => format!(
            "\n\nYou are in Work mode on branch {branch_name}, targeting \
             {base_branch}.\n\
             Your working directory is {worktree_path}. All file edits and \
             bash commands MUST stay inside this worktree. Do NOT modify \
             files in the main checkout or repo root.\n\
             Use bash and the patch tool to make changes.\n\n\
             When the task is complete, let the user know it's ready; they \
             review and merge the branch into {base_branch} via a pull \
             request -- Phoenix does not perform the merge. If your task \
             file follows the taskmd convention (`NNNNN-pX-status--slug.md`), \
             also mark it done yourself before handing off: rename the file \
             from `...-{{status}}--{{slug}}.md` to `...-done--{{slug}}.md` \
             (the filename is the sole source of truth for task status -- \
             nothing renames it for you) and commit that rename on this \
             branch alongside your work."
        ),
        LlmLanguage::Caveman => format!(
            "\n\nYou in work cave. Branch: {branch_name}. Aim at: {base_branch}. \
             Cave path: {worktree_path}. Stay in cave. No touch outside cave. \
             Use bash and patch. Done? Tell big caveman. Big caveman do merge. \
             If task file like `NNNNN-pX-status--slug.md`, you rename \
             `status` to `done` and commit."
        ),
    }
}

/// Optional hint appended to the Explore-mode prose telling the agent the
/// next available taskmd ID for this worktree. Language-aware so caveman
/// stays terse.
#[must_use]
pub fn next_taskmd_id_hint(lang: LlmLanguage, tasks_dir_name: &str, next_id: &str) -> String {
    match lang {
        LlmLanguage::PhoenixNative => format!(
            "\n\nThe next available taskmd ID for this worktree is \
             `{next_id}` -- use it when drafting a new task file \
             (e.g. `{tasks_dir_name}/{next_id}-p2-ready--my-slug.md`)."
        ),
        LlmLanguage::Caveman => format!(
            "\n\nNext taskmd id: `{next_id}`. Use for new file (e.g. `{tasks_dir_name}/{next_id}-p2-ready--slug.md`)."
        ),
    }
}

#[must_use]
pub const fn pr_auto_fix_instruction_prefix() -> &'static str {
    "Address the PR feedback captured in `"
}

#[must_use]
pub fn pr_auto_fix_instruction(lang: LlmLanguage, artifact_path: &str) -> String {
    let prefix = pr_auto_fix_instruction_prefix();
    match lang {
        LlmLanguage::PhoenixNative => format!(
            "{prefix}{artifact_path}`. That file is a point-in-time actionable snapshot of the failing CI checks (with failure logs inline where Phoenix could extract them) and unresolved/actionable review feedback. Use it as your starting point; if a failing check has no inline log, fetch the current logs yourself before fixing. Review-thread items carry a `thread_id` (a `PRRT_…` node id) — pass that, not the comment `id`, to the `resolveReviewThread` GraphQL mutation when marking threads resolved. Fix the issues in this worktree, run targeted tests, commit the changes, and summarize what changed."
        ),
        LlmLanguage::Caveman => format!(
            "{prefix}{artifact_path}`. File is actionable snapshot: failed CI (logs inline when Phoenix grab them) and unresolved/actionable review feedback. No inline log? Fetch current log yourself. Review-thread item carry `thread_id` (`PRRT_…` node id) — feed that, not comment `id`, to `resolveReviewThread` mutation when mark thread resolved. Fix in this cave. Run focused tests. Commit changes. Say what changed."
        ),
    }
}

#[must_use]
pub fn mode_direct(lang: LlmLanguage) -> &'static str {
    match lang {
        LlmLanguage::PhoenixNative => {
            "\n\nYou have full tool access. You are working directly in this directory \
             with no plan/approve workflow or branch isolation. Changes happen on the \
             current branch."
        }
        LlmLanguage::Caveman => {
            "\n\nAll tool yours. Work here. No plan dance. Change land on current branch."
        }
    }
}

#[must_use]
pub fn mode_branch(
    lang: LlmLanguage,
    branch_name: &str,
    base_branch: &str,
    worktree_path: &str,
) -> String {
    match lang {
        LlmLanguage::PhoenixNative => format!(
            "\n\nYou are in Branch mode on existing branch {branch_name}, \
             targeting {base_branch}.\n\
             Your working directory is {worktree_path}. All file edits and \
             bash commands MUST stay inside this worktree. Do NOT modify \
             files in the main checkout or repo root.\n\
             You are working directly on an existing branch -- there is no \
             task file. Commit your changes directly to {branch_name}.\n\n\
             When the work is complete, let the user know. They will handle \
             merging or pushing when ready."
        ),
        LlmLanguage::Caveman => format!(
            "\n\nYou in branch cave: {branch_name}. Aim at: {base_branch}. \
             Cave path: {worktree_path}. Stay in cave. No task file. \
             Commit straight to {branch_name}. Done? Tell big caveman."
        ),
    }
}

// =============================================================================
// Chain Q&A system prompts.
// =============================================================================

#[must_use]
pub fn chain_qa_agent_system_prompt(lang: LlmLanguage) -> &'static str {
    match lang {
        LlmLanguage::PhoenixNative => {
            "You are answering a recall question about a Phoenix continuation chain — a \
sequence of conversations continued one into the next as each exhausted its context. \
They share one body of work. You have two read-only tools:

- search_conversations(query): find the messages most relevant to a query across the \
  whole chain, ranked by relevance. Use natural-language queries.
- read_conversation(conversation_id, cursor?): read the full content of one chain member, \
  including complete tool output. It returns one bounded page; if it reports more remains, \
  call it again with the returned cursor to continue.

A chain skeleton (member ids, titles, and continuation summaries) is provided to orient you. \
Search to locate relevant messages, then read the conversations that look promising in full \
before answering. Search again if the first pass misses. When you can answer, reply with the \
answer as plain text and no tool call. Answer ONLY from what the chain actually contains; if \
the chain does not support a confident answer, say so and state what is missing. You cannot \
modify anything — this is recall, not work."
        }
        LlmLanguage::Caveman => {
            "You answer question about long chain of caveman talk. Many talks, one big work. \
You have two tools: search_conversations(query) find best matching messages in whole chain; \
read_conversation(conversation_id, cursor?) read one talk full, page by page with cursor. \
Skeleton list given to start. Search, then read good ones full, then answer with plain words \
and no tool. Only say what chain say. If chain not say, you say chain not say. You cannot \
change anything, only look."
        }
    }
}

// =============================================================================
// Tool descriptions — only an override map. Tools not listed here fall back
// to their `description()` (phoenix-native). The prototype covers bash, patch,
// read_file, search, think.
// =============================================================================

/// Returns a translated description for the named tool, or `None` if the
/// tool has no override in this language (caller falls back to
/// phoenix-native). Matched explicitly per-language so a future variant
/// doesn't silently inherit some other language's strings.
#[must_use]
pub fn tool_description_override(tool_name: &str, lang: LlmLanguage) -> Option<&'static str> {
    let table: &[(&str, &str)] = match lang {
        LlmLanguage::PhoenixNative => return None,
        LlmLanguage::Caveman => CAVEMAN_TOOL_DESCRIPTIONS,
    };
    table
        .iter()
        .find(|(name, _)| *name == tool_name)
        .map(|(_, desc)| *desc)
}

/// Caveman tool descriptions. Tools not in this table fall back to their
/// phoenix-native `description()`.
const CAVEMAN_TOOL_DESCRIPTIONS: &[(&str, &str)] = &[
    (
        "think",
        "Think before do. Plan steps. Spot bad idea early. No tool fire, no file change.",
    ),
    (
        "read_file",
        "Read file. Get numbered line. For big file, use offset and limit.",
    ),
    (
        "search",
        "Search word across many file. Pick file with grep-like pattern.",
    ),
    (
        "bash",
        "Run bash command. Four flavor: \
         `run` (start), `peek` (look at running thing), `wait` (block until done), `kill` (stop). \
         Run not detach. Long thing keep run until you wait or kill. Same id only one time.",
    ),
    (
        "patch",
        "Change file. Do exact replace, append at end, or overwrite whole file. Tell op and content.",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_default() {
        assert_eq!(LlmLanguage::default(), LlmLanguage::PhoenixNative);
        assert_eq!(LlmLanguage::PhoenixNative.as_str(), "phoenix-native");
        assert_eq!(LlmLanguage::Caveman.as_str(), "caveman");
        assert_eq!(
            LlmLanguage::parse("phoenix-native"),
            Some(LlmLanguage::PhoenixNative)
        );
        assert_eq!(LlmLanguage::parse("caveman"), Some(LlmLanguage::Caveman));
        assert_eq!(LlmLanguage::parse("nope"), None);
        // Unknown -> default keeps old DB rows working.
        assert_eq!(
            LlmLanguage::parse_or_default(""),
            LlmLanguage::PhoenixNative
        );
    }

    #[test]
    fn caveman_base_prompt_differs_and_is_shorter() {
        let native = base_prompt(LlmLanguage::PhoenixNative);
        let caveman = base_prompt(LlmLanguage::Caveman);
        assert_ne!(native, caveman);
        assert!(
            caveman.len() < native.len(),
            "caveman base prompt should be shorter; was {} vs {}",
            caveman.len(),
            native.len()
        );
    }

    #[test]
    fn pr_auto_fix_instruction_mentions_artifact_and_omits_push_policy() {
        let native =
            pr_auto_fix_instruction(LlmLanguage::PhoenixNative, ".phoenix/pr-context/pr-7.json");
        assert!(native
            .starts_with("Address the PR feedback captured in `.phoenix/pr-context/pr-7.json`"));
        assert!(native.contains("failing CI checks"));
        assert!(native.contains("unresolved/actionable review feedback"));
        assert!(native.contains("commit the changes"));
        assert!(!native.to_lowercase().contains("push"));
        // Both languages must steer the agent to the resolvable thread id;
        // a per-comment id is rejected by resolveReviewThread.
        assert!(native.contains("thread_id"));
        assert!(native.contains("resolveReviewThread"));

        let caveman =
            pr_auto_fix_instruction(LlmLanguage::Caveman, ".phoenix/pr-context/pr-7.json");
        assert_ne!(native, caveman);
        assert!(caveman
            .starts_with("Address the PR feedback captured in `.phoenix/pr-context/pr-7.json`"));
        assert!(caveman.contains("failed CI"));
        assert!(caveman.contains("unresolved/actionable review feedback"));
        assert!(!caveman.to_lowercase().contains("push"));
        assert!(caveman.contains("thread_id"));
        assert!(caveman.contains("resolveReviewThread"));
    }

    #[test]
    fn caveman_tool_overrides_are_string_overrides_only_in_caveman() {
        assert!(tool_description_override("bash", LlmLanguage::PhoenixNative).is_none());
        assert!(tool_description_override("bash", LlmLanguage::Caveman).is_some());
        // Tools without an override return None even in caveman mode (caller
        // falls back to phoenix-native description).
        assert!(tool_description_override("browser_navigate", LlmLanguage::Caveman).is_none());
    }
}
