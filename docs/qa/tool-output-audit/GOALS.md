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
5. **Re-measure** after the change ships, then continue with the next candidate.

## Goal state

The work is done (`holder: done`) when any of these holds:
- **G1:** the best remaining candidate saves under **0.5% of recorded input** or carries an
  unacceptable information-loss risk;
- **G2:** the cumulative confirmed savings reach **10% of recorded input**;
- **G3:** the user stops the work.

## Current target

Round 1: establish the tool-output baseline (BRIEF.md). No change has been picked yet.

## Wins log

(none yet)
