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

## Results

Read-only replay with **60.822B recorded input tokens** and 562,065 response anchors. The replay reuses Round 1 sweep positions, ages and cutoffs. The byte model estimates provider carry. The runtime shrink gate uses its actual estimator of Unicode scalar count divided by four, rounded up.

### B · Exact patch preview replay

- **3,157.5M token-turns saved (5.19%)**, with 38,117 previews clearing. The runtime estimator gives 2,879.5M (4.73%). The unmodified patch body carry is 4,734.0M.
- The strict shrink gate excludes 342 successful patch results, of which 170 would otherwise cross a sweep. Excluded rows total 0.6M of original carried text. The gate avoids 1.1M local-estimator token-turns of expansion (1.3M by byte calibration).
- Error results remain intact · 7,812 results, 131.1M carried token-turns.
- Capped results remain intact · 0 results, 0.0M carried token-turns.
- Successful results without the recognized span remain intact · 1,743 results, 2.4M carried token-turns.
- The replay is 0.38 percentage points below the Round 2 estimate without path duplication and 0.14 points below its explicit-path estimate. Round 3 uses the full current marker, replaces the entire diff block, preserves only hunk headers inside it, and applies the strict shrink gate. The path stays in the existing call. Historical rows ship with no preview span. Retrofitting their shapes is an offline counterfactual for newly written results and implies no immediate saving on those existing rows.

### Revised A · Compact retrievable marker on top of B

- **1,881.3M additional token-turns (3.09%)**. Marker carry changes from 2,650.9M to 769.6M. Local-estimator savings are 1,677.9M (2.76%).
- Existing whole-output clearances · 43,363,054 marker occurrences and 1,642.7M saved token-turns.
- New patch previews · 6,790,165 marker occurrences and 238.5M saved token-turns.
- Foreign cleared results attached to the caller transcript in the replay · 0. The normal renderer selects from the caller transcript. A tool-less continuation summary also sees cleared source content. If it copies a self-relative locator into a successor, that locator would bind to a different transcript.

### Clearing that grows a result

| Marker | Cleared rows at or below marker size | Rows with growth | Local tokens added | Calibrated token-turns added | Share of input |
|---|---:|---:|---:|---:|---:|
| current | 12,965 | 12,849 | 76.3M | 86.1M | 0.142% |
| revised | 7,247 | 7,181 | 9.0M | 10.1M | 0.017% |

Rows are whole-output clearances. B already excludes previews that fail its strict shrink gate.

### Bash command echo

| Result cohort | Operation | Results | With an echo | Echo rate | Reconstructed echo carry |
|---|---|---:|---:|---:|---:|
| Before Nov 24 UTC | all | 220,192 | 217,595 | 98.82% | 672.1M |
| Before Nov 24 UTC | run | 197,943 | 195,623 | 98.83% | 603.1M |
| Before Nov 24 UTC | wait | 20,640 | 20,411 | 98.89% | 63.0M |
| Before Nov 24 UTC | peek | 1,001 | 963 | 96.20% | 3.8M |
| Before Nov 24 UTC | kill | 608 | 598 | 98.36% | 2.2M |
| Nov 24 UTC onward | all | 14,599 | 9,580 | 65.62% | 12.3M |
| Nov 24 UTC onward | run | 13,997 | 8,993 | 64.25% | 11.1M |
| Nov 24 UTC onward | wait | 529 | 516 | 97.54% | 1.0M |
| Nov 24 UTC onward | peek | 63 | 61 | 96.83% | 0.1M |
| Nov 24 UTC onward | kill | 10 | 10 | 100.00% | 0.0M |

The cohort uses stored result date. Echo carry includes later requests, even when they fall in a different cohort. A run result and a handle read have different identity needs.

Bash launch echoes fell after the cutoff, and handle reads still echo by design. An ADR date does not establish the deployed code at each recorded result. The date split confirms a reduction. Recent launch echoes are still present in the stored results.

### Worker votes

- **B · Agree to merge the reviewed implementation once the PR has green CI, clean review, and the reviewer vote.** The frozen implementation is a9e1564b3 with parent d124e85fb. The adversarial pass found no actionable issue in preview creation, UTF-8 span validation, cap and error handling, goal and ordinary tool-result persistence, or rendering. Full stored output remains available to the transcript reader.
- **Revised A · Agree to implement.** Bind the locator to the caller transcript and test exact-message retrieval, invalid sequences, and continuation handoff. The continuation projector already supplies a canonical source pair. Preserve that pair for references copied into a successor.
- **Whole-result growth guard · Defer as a standalone savings change.** The current overhead is 0.142% of input and drops to 0.017% with A. If changing eligibility, preserve the established watermark behavior for existing transcripts.

B and revised A together model **8.28%** on the fixed sweep schedule. The combined estimate leaves cache, retrieval and sweep-timing effects unpriced.

Validation passed locally for 6 core preview tests, 21 runtime clearing tests, the exact replay fixtures, and Rust formatting. The production scan ran read-only.
