---
created: 2026-04-24
priority: p3
status: ready
artifact: specs/auth/auth.allium
---

# Allium language feedback: cross-spec references aren't resolved

Observed while reviewing task 24696's multi-spec changes on
2026-04-24. Collecting for upstream submission to the Allium project,
alongside task 27001 (temporal-ordering invariants) and task 08661
(skill cross-spec coordination pattern).

## The limitation

`allium check` is file-local. A reference to an imported spec's entity,
enum, value, or surface is syntactically validated (the `imported/Name`
form parses) but **not semantically resolved** — the checker does not
walk into the imported spec to verify the referenced name exists.

## Reproduction

Starting from a clean `specs/auth/auth.allium` (which imports
`specs/bedrock/bedrock.allium`):

```
$ allium check specs/auth/auth.allium
1 file(s) checked, 2 error(s), 2 warning(s).   # pre-existing
```

Mutate `auth.allium` to replace every `bedrock/Conversation` with
`bedrock/NotARealEntity` — a completely bogus cross-spec reference:

```
$ sed -i 's|bedrock/Conversation|bedrock/NotARealEntity|g' specs/auth/auth.allium
$ allium check specs/auth/auth.allium
1 file(s) checked, 2 error(s), 2 warning(s).   # same count
```

**Zero new diagnostics.** The checker accepts `bedrock/NotARealEntity`
without questioning whether it exists in the imported spec.

The `use` directive itself is similarly lenient — a file can
`use "./nonexistent.allium" as bedrock` and still check clean so long
as the local file's syntax is valid.

## Why this matters — concrete bug caught by human review only

Task 24696's Phase 2 wrote three rules in `specs/projects/projects.allium`:

```
rule WorktreeTransferredOnContinuation {
    when: UserTriggerContinuation(parent, new_conv)
    ...
}
```

…but the bedrock event this rule was meant to subscribe to is named
`UserTriggersContinuation(conversation)` — different spelling
(singular/plural) and different arity (1 vs 2). The rules subscribed
to a name that was neither defined locally nor existed in bedrock
under the spelling used.

`allium check` passed cleanly. The drift was caught only by a human
review pass that happened to cross-reference the info-level
`unreachableTrigger` messages against bedrock by hand.

Info-level `unreachableTrigger` is the right default for cross-spec
subscriptions in general (most are legitimate — a rule listening for
an event the runtime emits). But when 22 legitimate infos drown out
3 typo infos, the signal is lost.

## Second concrete bug (2026-04-24, found post-merge of 24696)

While tightening the spec preconditions on `ConfirmAbandon` and
`MarkAsMerged` in `specs/projects/projects.allium`, I discovered the
rules had been carrying `requires: conversation.status = idle` for an
unknown amount of time. The bedrock `Conversation` entity (defined in
`specs/bedrock/bedrock.allium`) has **no `status` attribute** — only
`core_status` (the run-status axis: idle/llm_requesting/etc.) and
`parent_status` (the lifecycle axis: awaiting_recovery/
awaiting_user_response/context_exhausted/terminal).

The rule was attribute-typo'd, referencing a field that doesn't exist
on the imported entity. `allium check` passed cleanly because the
checker doesn't walk into bedrock to verify which fields
`bedrock/Conversation` actually has. There are at least 4 occurrences
of `conversation.status = ...` in `projects.allium` (lines 529, 533,
549, 593 prior to the fix); each one is a no-op precondition that
doesn't constrain anything.

This is not a "did you mean?" miss — the right fix isn't a fuzzy
suggestion (`status` -> `core_status` is plausible but `status` ->
`parent_status` is also plausible, and the answer depends on what the
rule means to gate). It's a structural-resolution miss: the checker
should reject `conversation.status` outright when bedrock declares no
such attribute on `Conversation`.

This bug had real semantic teeth. The runtime check
`if !matches!(conv.state, ConvState::Idle)` was *stricter* than what
the spec said (`status = idle` resolved to nothing, so the spec
imposed no state constraint at all). When a sub-agent in 24696 Phase 4
asked "should I allow ContextExhausted here too?" the spec had no
answer to give — the precondition that should have anchored the
discussion was vacuous.

Severity argument: the cross-spec bug in B1 (subscribing to a
non-existent event) prevented a rule from ever firing — bad, but
loudly visible at runtime. This bug is worse: a precondition that
silently never constrains anything looks correct in code review and
hides actual divergence between the spec contract and the runtime.

## Suggested features (any one would have caught B1; #1 also catches the second bug)

1. **Cross-spec resolution when imports are present.** When
   `foo.allium` `use`s `bar.allium` and references `bar/Thing` or
   `var.attribute` where `var` is typed as `bar/Thing`, the checker
   walks bar's declarations to verify `Thing` exists *and* that
   `Thing` has the named attribute. Unresolved references become
   errors. This catches both the cross-spec event-name typo (B1) and
   the cross-spec attribute typo (the second bug above).
2. **"Did you mean?" fuzzy matching on unreachable triggers.** Given
   `when: UserTriggerContinuation(...)` with no provider, if an
   imported surface provides `UserTriggersContinuation(...)`, emit a
   higher-severity diagnostic with the suggestion.
3. **First-class event declarations.** Today triggers are just
   identifiers. If events were declared like entities (e.g.
   `event UserSendsMessage { conversation: Conversation, text: String }`)
   the checker would have a canonical list to validate against, and
   unknown triggers would be `type.undefinedReference`-level errors.

## Relationship to other allium feedback tasks

- **27001** — temporal-ordering invariants can't be expressed
  structurally. Orthogonal concern, same upstream target.
- **08661** — proposal to add a "Cross-Spec Coordination" pattern to
  the local Allium skill's `patterns.md`. Related by subject matter
  (cross-spec coordination) but focused on Phoenix-side documentation,
  not upstream language changes. The case study in 08661 (Phoenix
  needing to watch `bedrock.Conversation`'s terminal state from
  `terminal.allium`) is actually a similar shape to the bug this task
  reports: cross-spec coordination that the current grammar doesn't
  structurally support and must be documented/worked-around in each
  spec by convention.

## Out of scope

- Fixing Phoenix specs to work around this limitation (already done —
  task 24696's B1 fix introduces the missing `UserStartsContinuationConversation`
  event on the bedrock UserConversation surface to close the loop
  manually).
- Changes to the local Allium skill (that's 08661).
