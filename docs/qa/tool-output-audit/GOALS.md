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

**Goal state reached (G2), 2026-11-28.** Cumulative confirmed savings are about 11.0% of recorded input by
offline replay (10.5% counting only capacity-confirmed browser routes). The audit is closed (`holder: done`).

Rejected after measurement:
- a general bash result cap (round 8: evidence loss about 30% at 16 KiB, with no reliable reader);
- read_file range numbering (round 9: agents reuse per-line anchors in 13.4% of reads);
- rewriting historical patch inputs (round 2: breaks the cached prefix and provider replay).

Leads for future work, unverified and given no credit here:
- shorter sub-agent fan-in (about 1.0–1.3%);
- batching Coordinator wakes (about 1.1%);
- paging large file reads (about 0.9%);
- a smaller first transcript-evidence page (about 0.6%).

Each needs evidence on lost context or extra reads first.

**Post-ship check:** the round-7 JSON baseline holds the browser and `ask_user_question` calls per
1,000 parent requests. Compare against it after deployment, to confirm deferral plus the hint
did not suppress use.

## Wins log

- **PR #1204, merged 2026-11-28 (`415ddf399`):** patch diff previews (5.18%) and the short
  resolvable cleared-result marker (3.34%), **about 8.5% of recorded input** by offline replay.
  Running total: 8.5% of the 10% goal (G2).
- **PR #1206, merged 2026-11-28 (`ad855fe8d`):** browser tools deferred, with a deferred-names hint on
  hidden-search routes, **about 1.28% net** by replay. It should also restore `ask_user_question`.
  Running total: **about 9.8%** (worker's combined replay on the newer ledger).
- **PR #1207, merged 2026-11-28 (`9239c0b49`):** unpadded read_file line numbers, **about 1.20%** by
  tokenizer replay. Running total: **about 11.0%**, so G2 is reached.
