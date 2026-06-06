Unify and document tree-walk discovery precedence across phoenix-skills and phoenix-agents (and any other CWD-walk discovery).

Problem: in the "projects parent" case — a conversation opened on a directory that contains multiple project subdirs, where that directory is itself under $HOME — the walk from working_dir to root populates the seen-names set (including ancestor/$HOME definitions) BEFORE immediate child directories are scanned. A more-specific child-project definition (e.g. /projects/foo/.claude/agents/reviewer.md) therefore loses to a broader ancestor/global one (e.g. ~/.claude/agents/reviewer.md). Both phoenix-skills (discover_skills_with_options) and phoenix-agents (discover_agents_with_home) share this ordering, so they are at least consistent with each other today.

Decide the intended precedence (child-project definitions most likely SHOULD outrank ancestor/global ones) and make it consistent across all discovery mechanisms, with the rule documented in the relevant specs (specs/skills/, specs/agents/). Add tests covering the projects-parent-under-$HOME layout for both crates.

Surfaced by Codex review on PR #228 (named agents); intentionally deferred there to avoid diverging agents from skills in a single PR.
