# ADR-005: User tool invocation is limited to self-service tools; director tools deferred

- **Status:** Accepted
- **Date:** 2026-07-06
- **Affects:** REQ-UTI-003, REQ-UTI-006, REQ-IT-002

## Context

User tool invocation (`specs/user-tool-invocation/`) lets a user run Phoenix
tools directly — via a composer sigil — without an agent turn, recording each as
a user-originated tool round. The question is which tools that syntax should
expose. The registry mixes tools that do work (`bash`, `patch`,
`keyword_search`), tools that are LLM-internal (`think`), tools that are
inter-agent protocol (`submit_result`, `ask_user_question`), and tools that
assert orchestration decisions (`spawn_agents`, `propose_task`).

An early cut split the registry into "worker" and "agent-control" tools and
exposed the worker set. That taxonomy sorts the tools wrong: `think` is a worker
tool with no human use, `patch` is dominated by editing in the terminal
(`!nvim`), while `spawn_agents` and `propose_task` — nominally "control" — are
exactly the calls a human director might want to make directly. So the axis is
not what *kind* of tool it is, but whether a human would author it and whether
anything better already serves that journey.

## Options considered

1. **Worker-vs-control taxonomy.** Expose every "worker" tool; exclude
   "agent-control." One simple bucket, but it mis-sorts: it admits `think` (no
   user journey) and `patch` (dominated by the terminal) while excluding
   `spawn_agents` / `propose_task` (meaningful to a director).
2. **Eligibility criterion, narrow to self-service.** Expose a tool only when
   authorship is *meaningful* (a human would run it), it is *self-service*
   (produces a result and returns the conversation to idle, launching no agent
   activity), and it is *not dominated* by a native affordance. Survivors: `bash`
   (the inline terminal) plus tools with no shell equivalent — chiefly MCP /
   project integrations. Defer "user-as-director" tools.
3. **Expose the whole registry, including director tools now.** Broadest reach,
   but `spawn_agents` / `propose_task` *launch* agent activity rather than
   returning to idle (breaking the no-agent-turn property), and a user-authored
   `propose_task` collapses the propose/approve loop into author-and-self-approve
   — a distinct interaction that needs its own design.

## Decision

Adopt option 2: eligibility is a **criterion, not a fixed list** — meaningful +
self-service + not-dominated — and the scope is kept **narrow to self-service**.
The criterion is expressed in the spec so a newly registered tool is evaluated
against it rather than hardcoded. `bash` is the flagship member and the inline
terminal (ADR-004) is its specialization; the remaining members are integrations
with no shell equivalent.

User-as-director tools (`spawn_agents`, `propose_task`) are meaningful but are
**deferred**, not exposed, because they are not self-service (they start agent
work) and because a user-authored variant has different state semantics that no
concrete use case has yet pinned down. Keeping the initial scope self-service
preserves a clean "no agent turn, returns to idle" contract.

## Consequences

- **Positive:** the eligibility rule survives new tools without edits, and never
  hardcodes a list that rots.
- **Positive:** the no-agent-turn / return-to-idle contract stays clean, because
  every eligible tool is self-service by construction.
- **Negative:** the built-in worker tools (`patch`, `keyword_search`,
  `read_image`) are all dominated by the terminal, so the general `$tool` syntax
  has few members beyond `bash` until MCP / project integrations arrive.
- **Negative:** director use cases (a user directly fanning out sub-agents or
  authoring a task) wait for a follow-up design.
- **Neutral:** the inline terminal is the concrete `bash` consumer of the model;
  the umbrella spec is otherwise requirements-only until a second consumer lands.

## References

- Related ADRs: ADR-000 (adopt spEARS v2); ADR-004 (inline terminal, the `bash`
  specialization).
- Feature spec: `specs/user-tool-invocation/requirements.md`
- Executive summary: `specs/user-tool-invocation/executive.md`
