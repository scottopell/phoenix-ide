---
created: 2026-04-24
priority: p3
status: ready
artifact: specs/sse_wire/sse_wire.allium
---

# Allium language feedback: temporal ordering invariants

Observed while distilling `sse_wire.allium` (task 02680). Collecting
for upstream submission to the Allium project.

## The limitation

Allium's expression-bearing invariants (`invariant Name { expr }`) can
only assert properties over entity state at a **single point in time**.
From the language reference:

> Expression-bearing invariants assert properties over entity state at
> a single point in time. They answer the question "given the current
> state of all entities, does this property hold?" Not all important
> properties have this shape.
>
> **Not expressible** (use prose comments or `@invariant` in contracts):
> - **Temporal ordering**: "event 2 sees the entity state left by event
>   1." This is about the order in which rules executed, not a static
>   property.

This leaves a class of load-bearing safety properties inexpressible
as checkable, named invariants:

- "No `token` events after `agent_done` for the same LLM turn" —
  about event ordering on a stream
- "`message_updated` only follows `message` for the same message_id" —
  about causal ordering between two event types
- "The DB write commits before the broadcast fires" — the
  persist-before-broadcast ordering

These had to be downgraded to prose in `@guidance` blocks, which the
checker does not evaluate.

## The workaround found (and its limits)

For **persist-before-broadcast** specifically, we found a structural
model that recovers checkability:

1. Introduce a `PersistedMessage` entity (created when DB write
   completes) and a `StreamMessage` join entity (created when the
   broadcast fires).
2. Express the ordering as cross-entity membership:

```allium
invariant PersistBeforeBroadcast {
    for stream in SseStreams:
        for sm in stream.stream_messages:
            exists PersistedMessage{
                message_id: sm.message_id,
                conversation_id: stream.conversation_id
            }
}
```

This works because the causal dependency ("broadcast happens after
persist") can be restated as a structural co-existence assertion
("for every StreamMessage there is a PersistedMessage"). The join
entity collapses a temporal ordering property into a set-membership
property.

**Limits of the workaround:** This restatement is only possible when
the ordering can be framed as "entity A exists only if entity B
exists." It does not generalise to:

- **Sequence ordering on a single stream**: "no `token` events after
  `agent_done` on the same stream" — there is no entity whose
  *existence* encodes this; it is purely about the order of values in
  `last_delivered_seq`.
- **Intra-entity before/after**: "`message_updated` carries a
  `sequence_id` strictly greater than the `message` event for the
  same `message_id`" — this requires comparing two sequence numbers
  in a causal chain, not asserting entity membership.
- **Monotonicity across rules**: "once `agent_done` fires for request
  R, `last_delivered_seq` never decreases below that value" — single-
  point-in-time invariants cannot compare current state to prior
  state.

Those three properties remained as `@guidance` prose.

## Suggested feedback for upstream

### Option A: `precedes` assertion in invariants

A new expression form that references the rule execution order
within the invariant body:

```allium
-- hypothetical syntax
invariant AgentDoneTerminatesTokens {
    for stream in SseStreams:
        AgentDoneDelivered(stream) precedes TokenDelivered(stream)
            implies false
}
```

Would require the checker to reconstruct execution traces and test the
assertion against all reachable states — essentially a linear temporal
logic property (LTL: `□(agent_done → ¬◇token)`).

### Option B: `@invariant` promoted to a first-class named assertion

Today `@invariant` inside a `contract` is prose-only. A lighter
lift: allow `@invariant Name` at module scope (not just in contracts)
so temporal properties can be **named** (for linking, cross-referencing,
and documentation) even if the checker cannot evaluate them:

```allium
-- hypothetical: named prose invariant at module scope
@invariant AgentDoneTerminatesTokens
    -- For any stream, once agent_done is delivered with sequence_id S,
    -- no token event with sequence_id > S is delivered on that stream.
    -- Enforced by: AgentDoneBroadcast @guidance (implementation contract).
```

This is a documentation improvement, not a verification improvement,
but it closes the gap between "named, checkable" and "named but only
mentioned in prose."

### Option C: Explicit event-log entity pattern in the stdlib/patterns

Document the join-entity workaround (as used for `PersistBeforeBroadcast`)
as a named pattern in the Allium patterns reference. This doesn't
add language features but gives spec authors a known idiom for when
causal ordering can be reframed as structural co-existence.

## Context for upstream

- Discovered in: `specs/sse_wire/sse_wire.allium`, task 02680
- Allium version: 3.2.3 (language version 3)
- The workaround is in production use; Option B would be the
  least-invasive improvement
- Related art: TLA+ has temporal operators; Alloy has `after`/`before`
  in its trace model; Event-B has event sequencing constraints

## Related Allium feedback tasks

- **Task 02683** — `allium-cross-spec-resolution-feedback` (p3).
  `allium check` doesn't resolve cross-spec references: a bogus
  `imported/NotARealEntity` is accepted without a diagnostic, and
  typoed trigger names (e.g. `UserTriggerContinuation` vs the actual
  `UserTriggersContinuation`) only surface as info-level, drowned in
  legitimate cross-spec subscription infos. Orthogonal concern to the
  temporal-ordering gap documented here; same upstream recipient.
  Bundle them for submission if convenient.
- **Task 08661** — `allium-skill-cross-spec-coordination-pattern`
  (p3). Phoenix-local skill documentation proposal for a "Cross-Spec
  Coordination" pattern in the Allium skill's `patterns.md`. Not
  upstream feedback, but covers the same structural territory
  (cross-spec rule subscriptions) that 02683 flags as an enforcement
  gap. Useful context for the upstream recipient: Phoenix has been
  working around this by convention and thinks the pattern deserves
  documentation even before any language change.
