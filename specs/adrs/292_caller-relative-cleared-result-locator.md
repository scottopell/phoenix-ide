# ADR-292: Cleared-result markers name their source relative to the caller

- **Status:** Accepted
- **Date:** 2026-11-27
- **Affects:** REQ-STR-004, REQ-STR-012; Allium `ToolResultCleared` guidance

## Context

Each cleared tool result was replaced by
`[tool result cleared to save context] [persisted source: conversation=<uuid>, message=<uuid>, sequence=<n>]`,
about 220 bytes and 57–72 tokens, carried on every later request. An offline replay over the
production database priced markers at about 2.65B token-turns. Out of 43M marker occurrences, only 3
`read_conversation` calls targeted a cleared source, but REQ-STR-012 requires the locator to stay
resolvable. The conversation ID is redundant inside the transcript's own requests. Clearing applies only
to that transcript's rows.

## Options considered

1. **Drop the locator.** Smallest, but it violates REQ-STR-012 and loses exact retrieval.
2. **Keep only the message ID.** It still costs a UUID per marker, and `read_conversation` has no
   message-only form.
3. **A caller-relative `@self#seq-<n>`, resolved by `read_conversation` against the caller's transcript,
   with the exact pair wherever text can leave the transcript.**

## Decision

Option 3. Live requests render `[tool result cleared; source @self#seq-<n>]`, about 50 bytes.
`read_conversation`, both local and the local route of the federated tool, resolves `@self#seq-<n>`
through the caller's tool context to `@transcript:<id>#message-<id>`. Remote reads reject it. Continuation
input renders `@transcript:<conversation>#message-<message>`, because a summarizer may copy a locator
into a successor transcript, where `@self` would bind to the wrong rows. Replay: about 3.09% of
recorded input on top of ADR-291. The decision was made by the reviewer and worker agents under the owner's
delegated authority.

## Consequences

- **Positive:** about 1.9B fewer carried tokens; markers no longer grow small results much (the growth
  overhead drops from 0.142% to 0.017% of input).
- **Negative:** a new reference form to maintain. A `@self` locator copied by some other path
  outside the transcript would misresolve; continuation is the one known path, and it is covered.
- **Neutral:** stored transcripts are unchanged; only rendering differs.

## References

- ADR-291
- `docs/qa/tool-output-audit/round-2.md`, `round-3.md`
- `ClearedLocator`, `cleared_tool_result_marker`, `GlobalReadService::resolve_self_reference`,
  `read_local_conversation`
