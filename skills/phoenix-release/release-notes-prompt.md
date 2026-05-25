# Sub-agent prompt: phoenix-ide release notes

Spawn a `general-purpose` agent with the prompt below. Substitute `<PREV>` (last release tag), `<NEW>` (new release tag), and `<URL>` (GitHub release URL) before sending.

The agent is responsible for investigation, grouping, and drafting — not posting. You verify and post.

---

## Prompt (copy verbatim, substitute placeholders)

> Write polished release notes for phoenix-ide `<NEW>` to replace the auto-generated GitHub release body at `<URL>`.
>
> # Context
>
> phoenix-ide is an LLM-powered coding agent: Rust backend (axum, SQLite, state machine-driven conversation runtime) + React/TypeScript/XState frontend. Single static binary, embedded UI. See `AGENTS.md` at the repo root for full architecture.
>
> The previous release was `<PREV>`. `<NEW>` is HEAD. Look at the commit count between them — if it's large (hundreds), this is a long-overdue write-up and needs to land well.
>
> Audience: existing users self-hosting phoenix-ide on Linux, plus a smaller technical audience evaluating it. They want to know: what's new I can use, what was fixed, what changed under the hood that might affect me. They do NOT want a commit-log dump.
>
> # Your task
>
> Produce a markdown release-notes body suitable for `gh release edit <NEW> --notes-file -`. **Do NOT post it.** Output the text and let me review.
>
> # How to investigate
>
> 1. `git log <PREV>..<NEW> --oneline` for the full list.
> 2. For each commit that looks user-facing (features, UX changes, fixes a user would notice), look at the body: `git show <sha> --stat` or read the PR if `(#NNN)` is in the subject (`gh pr view NNN`).
> 3. Look in `tasks/` for completed task files between these dates — they often have richer "why" context than commits.
> 4. Skim `specs/` for any new top-level spec dirs added in the range — those usually represent significant new capabilities.
> 5. Internal-only churn (refactors, correctness audits, test infra, taskmd plumbing, agent-facing doc tweaks) gets a short bucket; do NOT list each one.
>
> # Structure (suggested, deviate if better fits the content)
>
> ```
> ## Highlights
> 2-4 bullets. The actual headline features a user would care about.
>
> ## New features
> Grouped, each ~1-2 sentences explaining the user value (not the implementation).
>
> ## Fixes
> User-visible bugs fixed. Skip internal refactor "fixes."
>
> ## Performance
> Anything measurable.
>
> ## Under the hood
> One paragraph or 3-4 bullets covering refactors/correctness work as a class —
> NOT a list of every audit. Mention this is a code-quality push if it is.
>
> ## Upgrading
> Any migration/config notes. (Probably none — confirm.)
>
> ## Full changelog
> Link: https://github.com/scottopell/phoenix-ide/compare/<PREV>...<NEW>
> ```
>
> # Style rules
>
> - Lead with user value, not the mechanism. "Cmd+P now searches the active worktree" not "refactored search store to scope by workspace."
> - Group related PRs into one bullet when they're one capability (e.g. "Settings dropdown + codex quota chip + notification panel consolidated into a single gear-icon menu" rather than three bullets).
> - Cite PR numbers in parentheses where they add traceability: `(#137)`. Don't cite raw commit SHAs.
> - No emoji. No "we're excited to announce." No marketing voice.
> - Be honest about scope. If "Under the hood" dominated the release, say so.
> - If something is half-shipped or behind a flag, say so.
> - Length budget: target ~40-80 lines of markdown. Dense, scannable, no filler.
>
> # Deliverable
>
> Print the full markdown body in a fenced code block, ready to pipe into `gh release edit <NEW> --notes-file -`. Below that, one short paragraph (3-5 lines) on judgment calls you made: anything you bucketed as "internal" that could arguably be user-facing, anything ambiguous, anything you couldn't find good context for. The orchestrator will verify those claims before posting.

---

## After the agent returns

1. **Verify the judgment-calls paragraph** — every claim about migrations, env vars, removed code, or behavior the agent inferred from commits should be grep-checked against the current code.
2. **Prepend the AI banner** (see SKILL.md, Step 7).
3. Write to `/tmp/<NEW>-notes.md`, run `gh release edit <NEW> --notes-file …`, print the URL.
