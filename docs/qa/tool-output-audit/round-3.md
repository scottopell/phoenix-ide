# Round 3: exact replay of B as implemented, and revised A

## Review of round 2 (reviewer)

Accepted. I agree with both votes.

- **B** is implemented in commit `a9e1564b3` on this branch, in `crates/` and `specs/`; the
  PR follows. The vote to merge waits for the exact replay below.
- **A** goes back for a format that keeps exact retrieval (below).
- **Queued "bash command echo" candidate: dropped.** `run` results have not echoed the command since
  ADR-290 (2026-11-24). Round 1's figure reflects older rows. If you can, give the share of
  bash results from before and after 2026-11-24 that still contain the echo, to confirm it's gone.

Coordination note: while you hold the baton, I keep committing code under `crates/` and `specs/` on
this branch for the PR. You own `docs/qa/` and `scripts/`, so we will not conflict.

## 1. Exact replay of B as implemented (needed for the merge vote)

Implemented rule, mirrored from `cleared_tool_result_text` and `collect_tool_result_facts`:
- The preview span is the `<diff>\n…</diff>` block of a **successful** patch result. Historical rows carry
  no span; derive it from the stored text, which has that exact shape. Rows without a block keep the old behaviour.
- **Cleared form** = the text before the span + (the hunk headers, i.e. lines starting with `@@ `,
  joined by `\n`, then `\n`, if there are any) + the **current** marker + the text after the span.
  The current marker is `[tool result cleared to save context] [persisted source: conversation=<36>, message=<36>, sequence=<n>]`.
- A patch result is clearable **only if** `estimate(cleared form) < estimate(full result)`, using
  the local estimator (one token per four bytes, rounded up).
- Error results keep their full text. So does any result whose stored text would not fit under the 100 KiB cap.

Report:
1. Token-turns saved, and the share of recorded input.
2. The number of patch results excluded by the shrink rule, and the token-turns that rule protects.
3. A sanity check against your round-2 B figure (5.33–5.57%): explain any gap.
4. **Your merge vote on B.** Per GOALS.md, I'll add mine on the PR once CI and review are clean.

## 2. Revised A: a short marker that still resolves

Three calls in the whole corpus targeted a cleared source, against 43M marker occurrences, so
the locator is almost never used. It still has to resolve when it is. The proposal:

`[tool result cleared; read_conversation @self#seq-<n>]` (about 50 bytes)

`read_conversation` would accept `@self#seq-<n>`, resolved against the caller's own
transcript through its tool context. That is new code, which I'll write if the replay supports it.

Report:
1. Token-turns with this exact string against the current marker, as a share of recorded input,
   **on top of B** (B's cleared patch results also carry the marker).
2. Whether every cleared result is in the caller's own transcript at render time (round 2 found 0
   foreign results; confirm under this definition).
3. Your vote.

## 3. New: clearing that grows a result

While implementing B, I found that a cleared result whose body is smaller than the marker gets
larger when cleared. The current marker is about 57 tokens; a short bash or search result is often less. This
applies to every clearable tool, not only patch.

Report: the number of cleared results whose estimated tokens are at most the marker's, and the
token-turns that clearing **added**, under the current marker and under revised A. If it matters, the fix is
one line: don't clear a result its marker would not shrink.

## Output

Append `## Results` to this file, write numbers to `round-3.json`, vote, and pass the baton.
The privacy rules in BATON.md apply.
