---
created: 2025-01-28
priority: p3
status: brainstorming
artifact: pending
---

# Advanced skill features: subagent execution, invocation control, dynamic context

## Summary

Following task 570 (basic skill slash-command invocation), several advanced claude-code skill features were explicitly deferred. This task tracks them so we can evaluate use cases as they emerge.

## Related

- **Task 570** — prerequisite; basic `/skill-name` invocation must be done first.

## Features

### 1. Invocation control frontmatter

```yaml
disable-model-invocation: true   # only user can invoke via slash command
user-invocable: false            # only model can invoke automatically
```

Currently all skills appear in the system prompt catalog and can be invoked both ways. These fields let skill authors lock down who/what can trigger them:

- `disable-model-invocation: true` — strip this skill from `<available_skills>` in the system prompt (model never sees it); only surfaces in the `/` slash-command autocomplete
- `user-invocable: false` — hide from the `/` autocomplete dropdown; model still sees the catalog entry and can load it

Backend: `parse_skill_frontmatter()` already parses the frontmatter — add fields, thread through `SkillMetadata`, and gate catalog injection + autocomplete accordingly.

### 2. `context: fork` — run skill in a subagent

```yaml
context: fork
agent: Explore   # optional; defaults to general-purpose
```

When set, invoking the skill spawns a subagent with the expanded skill content as its task prompt rather than injecting it into the current conversation. Results are summarized back into the main context. Useful for expensive searches or memory operations that shouldn't pollute the active context window.

This is a natural extension of the existing `spawn_agents` / subagent infrastructure.

### 3. `!\`command\`` dynamic context injection

Shell commands embedded in SKILL.md are executed server-side before the skill content is sent to the LLM, with their output substituted inline:

```markdown
PR diff: !`gh pr diff`
Changed files: !`gh pr diff --name-only`
```

This is preprocessing — the LLM receives the rendered output, not the backtick expression. Requires sandboxing / allowlist considerations (same security model as the `bash` tool).

### 4. `allowed-tools` scoping per skill

```yaml
allowed-tools: Read, Grep, Glob
```

When a skill is active, restrict the LLM to only the listed tools without per-use approval prompts. Requires hooking into the tool permission layer during the skill's active turn.

### 5. `agent: <name>` subagent selection

When used with `context: fork`, selects which subagent profile drives execution (`Explore`, `Plan`, custom agents from `.claude/agents/`). Depends on feature 2 above.

## Acceptance Criteria

- [ ] `disable-model-invocation: true` removes skill from system prompt catalog
- [ ] `user-invocable: false` removes skill from `/` autocomplete
- [ ] `context: fork` spawns a subagent with skill content as task; result summarized into main context
- [ ] `agent: <name>` selects subagent profile when `context: fork` is set
- [ ] `!\`command\`` placeholders are executed and substituted before skill content reaches LLM
- [ ] `allowed-tools` is respected for the duration of the skill-triggered turn
- [ ] `./dev.py check` passes
