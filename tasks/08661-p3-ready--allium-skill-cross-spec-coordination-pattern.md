---
created: 2026-04-10
priority: p3
status: ready
artifact: .agents/skills/allium/references/patterns.md
---

# Allium Skill: Add Cross-Spec Coordination Pattern

## Summary

Add a **Cross-Spec Coordination** section to the Allium skill's `patterns.md`
reference, covering when to use state-watch vs named lifecycle events across spec
boundaries. Driven by a concrete case study discovered during the terminal spec
elicitation.

## Context and Case Study

This task exists because of a real ambiguity encountered while writing
`specs/terminal/terminal.allium`. The discussion below is preserved verbatim as a
reference for whoever implements the skill improvement.

### The Situation

The rule `TerminalAbandonedWithConversation` in terminal.allium needs to fire
whenever a bedrock `Conversation` reaches a terminal state. Terminal states in
bedrock are `{ context_exhausted, terminal, completed, failed }`, captured by the
derived value `is_terminal` (bedrock.allium line 252).

The problem: bedrock has **13+ separate rules** that each transition into one of
these four states. There is no single named event that fires on all of them.

### The Three Candidate Solutions

**Option A — enumerate source rules (rejected immediately):**
```allium
when: bedrock/TaskResolved(conversation, _)
   or bedrock/SubAgentImplicitCompletion(conversation)
   or bedrock/SubAgentSubmitsResult(conversation, _)
   or ... -- 13+ more
```
Brittle. Missing one is a silent bug. Every bedrock change requires updating
terminal.allium. Rejected.

**Option B — cross-spec state-watch on derived value (first attempt):**
```allium
when: conversation.is_terminal becomes true
```
Semantically correct. `is_terminal` is a published concept in bedrock’s entity
model — bedrock defined and named it, so consuming it feels right. But:
- The Allium language reference only shows derived-condition triggers on **local**
  entities (`when: interview: Interview.all_feedback_in`). Cross-spec derived-value
  watch is valid by extension but not explicitly documented.
- Every other cross-spec trigger in this codebase uses named events
  (`when: bedrock/UserApprovesTask(...)`). This would be the first state-watch
  across a spec boundary.
- The implementation question “how does the runtime detect `is_terminal` flipping”
  is left to @guidance prose. Named events are more actionable.

**Option C — add `ConversationBecameTerminal` to bedrock (adopted):**
```allium
-- In bedrock.allium: new relay rule
rule ConversationReachedTerminalState {
    when: conversation: Conversation.is_terminal becomes true
    ensures: ConversationBecameTerminal(conversation)
}

-- In terminal.allium:
when: bedrock/ConversationBecameTerminal(conversation)
```
Bedrock emits a named lifecycle event. Terminal subscribes to it. This is the
exact pattern used elsewhere in the codebase (`bedrock/UserApprovesTask` in
projects.allium). The cross-spec contract is explicit, navigable with grep, and
extensible: any future spec (metrics, file-watcher, etc.) can subscribe to the
same event without re-discovering `is_terminal`.

The relay rule internally uses `is_terminal becomes true` — a state-watch —
but confines it to bedrock's own entity. The cross-spec boundary is clean.

### The Design Insight

The deeper question this surfaced: **when should an upstream spec emit a named
lifecycle event vs trust downstream specs to watch its state?**

The Allium language reference (`language-reference.md`) shows both patterns as
valid for cross-spec use:
```allium
-- Named event (chained trigger, cross-spec)
when: oauth/SessionCreated(session)

-- State-watch on imported entity field (cross-spec)
when: feedback/Request.status transitions_to submitted
```
But it provides no guidance on which to use when. The patterns.md file also has
no cross-spec coordination section.

### Proposed Decision Rule (for the skill)

**Emit a named lifecycle trigger when:**
- Multiple downstream specs could plausibly react to the same event
- The event represents a meaningful semantic moment the upstream spec “owns”
  (lifecycle boundary, not just data change)
- Other specs already use named-event cross-spec subscription (convention
  consistency)
- The event has a descriptive name that carries intent (`ConversationBecameTerminal`
  vs `conversation.is_terminal becomes true`)

**Use cross-spec state-watch when:**
- The upstream spec has published a named derived value that cleanly captures
  the condition (`is_valid`, `all_feedback_in`)
- The relationship is structural: “react when this condition holds” with no
  semantic announcement needed
- The upstream spec is external/immutable and cannot be modified to emit events
- Only one downstream spec will ever care about this condition

**Never use cross-spec derived-value watch when:**
- A named event from the upstream spec already exists and covers the same cases
- Multiple downstream specs need the same signal (named event is DRY)
- The condition is a lifecycle boundary the upstream spec should own

### The Relay Rule Pattern

When converting an existing spec to emit a new lifecycle event, the relay rule
pattern is DRY and safe:

```allium
-- Relay: derived-value → named event
-- Covers ALL paths to the terminal condition without listing them.
-- New paths added to the spec are automatically covered.
rule ConversationReachedTerminalState {
    when: conversation: Conversation.is_terminal becomes true
    ensures: ConversationBecameTerminal(conversation)

    @guidance
        -- This relay exists to convert a structural condition into a named,
        -- subscribable lifecycle announcement. Downstream specs subscribe to
        -- ConversationBecameTerminal rather than watching is_terminal directly.
        -- Tradeoff: fires for sub-agents (completed/failed) too. Consumers
        -- that care only about top-level conversations check is_sub_agent.
}
```

Alternative: add `ensures: ConversationBecameTerminal(conversation)` to each
of the 13 terminal-transition rules directly. More explicit per-rule, but
 requires updating every new terminal-state rule — a footgun.

## What to Add to patterns.md

A new pattern section: **Cross-Spec Coordination**.

It should cover:

1. **State-watch on imported field** — when and how (already in language-reference
   but not in patterns with worked example)
2. **Named lifecycle events** — when an upstream spec should emit them
3. **The relay rule pattern** — converting a derived-value condition into a named
   event DRY-ly
4. **The decision rule** — named event vs state-watch, with the criteria above
5. **Cross-spec derived-value watch** — valid syntax, when appropriate, and why
   named events are usually better at spec boundaries
6. **Worked example** — bedrock/ConversationBecameTerminal as the canonical case:
   - Show the relay rule in bedrock
   - Show terminal.allium subscribing to it
   - Contrast with the rejected alternatives

## Acceptance Criteria

- [ ] New **Cross-Spec Coordination** section added to
      `.agents/skills/allium/references/patterns.md`
- [ ] Section covers all 6 points listed above
- [ ] Worked example uses `ConversationBecameTerminal` / `TerminalAbandonedWithConversation`
      as the canonical illustration (real code, not invented example)
- [ ] Decision rule is stated as a scannable list (not buried in prose)
- [ ] Relay rule pattern is shown with the tradeoff note about sub-agents
- [ ] SKILL.md routing table updated if a new routing entry is warranted
- [ ] No changes to actual `.allium` files (those were fixed in task 24657)

## Notes

The actual `.allium` fixes — adding `ConversationBecameTerminal` to bedrock.allium
and updating terminal.allium to use `bedrock/ConversationBecameTerminal` — were
implemented in task 24657 (terminal spec task). This task is purely about
improving the skill so future agents don’t rediscover the same ambiguity.

The discussion that produced this analysis was an interactive elicitation session
between Scott and an agent working on the terminal spec in April 2026. The full
conversation covered: why formal specs exist in an LLM-first world, what Allium
entities and rules are, the three trigger types, cross-spec trigger patterns, and
the specific state-watch vs named-event tradeoff. That conversation is the primary
reference for understanding the motivating case.
