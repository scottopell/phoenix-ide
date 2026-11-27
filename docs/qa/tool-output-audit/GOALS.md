# Goals (owned by reviewer)

The reviewer owns this file and updates it at every handoff. The worker reads it before each round.

## Method: repeated hill climbing

Each cycle:
1. **Measure.** The worker runs the round's analysis on production and reports a ranked
   list of candidate changes.
2. **Pick.** The reviewer picks the top candidate by expected saving per unit of risk.
3. **Confirm offline.** The worker replays the exact change (the same transform code
   as the PR, via the script) against recorded production outputs and reports the realized
   saving plus samples of what the agent would lose (shape only, never text).
4. **Land.** The reviewer opens a normal, non-draft PR with tests. Both sides drive it to
   CI-green and review-clean, and the worker adds the confirmation numbers to the PR thread
   or round file.
5. **Merge by quorum.** Merge only when CI is green, review is clean, **and both reviewer
   and worker have recorded agreement**, as a line in the round file or a PR comment. Either side can
   veto.
6. **Re-baseline offline.** Never deploy. Apply the merged change as a transform in the
   replay model, then pick the next candidate against the new baseline.

## Constraints

- **No deploys** of any kind (`./dev.py prod …` included).
- Confirmation is **offline replay** over recorded production data. Ad-hoc LLM evals
  are allowed only when replay cannot answer the question (for example, whether the agent
  still recovers pending work after a shorter summary). Keep them small: at most about 20 cases and about 1M
  tokens per eval, with the size stated up front in the round file.

## Goal state

The work is done (`holder: done`) when any of these holds:
- **G1:** the best remaining candidate saves under **0.5% of recorded input** or carries an
  unacceptable information-loss risk;
- **G2:** the cumulative confirmed savings reach **10% of recorded input**;
- **G3:** the user stops the work.

## Current target

Round 2: replay A (compact the cleared-result marker, ≈3.5% of input carried as markers) and
B (patch diff as a clearable preview with a durable record, ≈7.8% carried). Baseline:
reconstructed carry from round 1, tool results 29.7B of 60.79B recorded input.

Queued: bash command echo as a typed reference (1.1% reconstructed); deferring the browser tool
definitions (2.7% of transcripts use them, about 2.9k tokens per request); bounded bash results
with retrieval; long-line shortening.

## Wins log

(none yet)
