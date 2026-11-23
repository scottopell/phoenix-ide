# ADR-275: Cap quoted review lines at 200 UTF-16 units on a character boundary

- **Status:** Accepted
- **Date:** 2026-11-23
- **Affects:** REQ-PF-009, REQ-PF-016

## Context

REQ-PF-009 required the quoted source line in review notes to be the exact,
untruncated text. The UI capped it at 200 characters anyway, with a test
asserting the cap. The cap used `String.prototype.slice`, which counts UTF-16
code units: an emoji straddling unit 200 left a lone high surrogate. The
browser sends that as a `\ud83d` escape and the server's strict JSON parser
rejects the whole request, so the notes message cannot be sent, and every
retry fails the same way. The same cut existed in the markdown viewer and in
task-approval feedback.

## Options considered

1. **Drop the cap, as the spec said.** Quoted lines are complete, but one note
   on a minified line pastes unbounded text into the conversation, where it is
   re-sent every turn until compaction.
2. **Keep a 200-unit cap, cut on a character boundary, and amend the spec.**
   The quoted prefix is still an exact, greppable substring.

## Decision

Option 2, as chosen by the project owner. Quoted line content is cut by
`safePrefix(text, QUOTED_LINE_MAX_UNITS)` from `ui/src/utils/textBounds.ts`,
which backs off one unit rather than split a surrogate pair. All UI string
truncation for display or sending goes through `safePrefix` / `safeSuffix`. The
ast-grep rules `ts-no-utf16-string-truncation` and
`tsx-no-utf16-string-truncation` reject the raw `slice` / `substring` /
`substr` truncation idioms: a cut next to an ellipsis marker, or a cut of a
`.join(...)` result.

## Consequences

- **Positive:** review notes and task-approval feedback with emoji always send;
  displayed previews no longer show U+FFFD at the cut.
- **Negative:** a cut can be one unit shorter than the budget. The lint rule is
  structural, not type-aware: a raw cut written without an ellipsis or `.join`
  is not detected.
- **Neutral:** output for text without astral characters is unchanged.

## References

- `safePrefix`, `safeSuffix`, `formatNotesForSend`
- Rust-side analogue: `phoenix_core::text_bounds` (byte caps on char boundaries)
