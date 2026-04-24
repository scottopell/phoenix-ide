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

## Suggested features (any one would have caught B1)

1. **Cross-spec resolution when imports are present.** When
   `foo.allium` `use`s `bar.allium` and references `bar/Thing`, the
   checker walks bar's declarations and looks for `Thing`. Unresolved
   references become errors (or loud warnings).
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
