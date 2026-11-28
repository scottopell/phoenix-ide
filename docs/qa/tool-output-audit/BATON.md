# Baton: tool-output audit

holder: reviewer
round: 7
updated: 2026-11-28
ask: Open the browser-deferral PR at 4ad3751fa and drive it through quorum; return the next audit brief or mark the goal reached.

## Roles

- **worker**: has the production database. Writes and runs the analysis, commits
  results, passes the baton.
- **reviewer**: no database access. Reviews results and the script, fixes script
  bugs, writes the next round's ask, passes the baton.

## Protocol

1. Branch: `claude/data-loss-token-waste-hunt-3enn9f`. Before acting, run
   `git fetch origin claude/data-loss-token-waste-hunt-3enn9f && git merge --ff-only FETCH_HEAD`.
2. Act only when `holder:` names you. The holder owns every file under
   `docs/qa/tool-output-audit/` and `scripts/tool_output_audit.py` until it passes.
3. To pass: set `holder:` to the other role, bump `updated:`, write the next request in `ask:` (one line; details go
   in the round file), add a line to the log below, commit, and push. That commit is the handoff;
   make no further commits until the baton returns.
4. If a push is rejected, fetch and merge (never force-push), then push again.
5. Poll for the baton with:
   `git fetch -q origin claude/data-loss-token-waste-hunt-3enn9f && git show FETCH_HEAD:docs/qa/tool-output-audit/BATON.md | sed -n 3,6p`
   Poll every 20–30 minutes, and stop after 6 hours without a change.
6. `holder: done` ends the exchange. Either side may set it, with a reason in the log.

## Privacy rules (bind every round)

- Never write to the production database. Open it `mode=ro`; snapshot only to memory.
- Committed output contains **no message text, command lines, file paths, repo names,
  URLs or full transcript IDs**. Allowed: tool names, the command-family labels
  defined in BRIEF.md, counts, sizes, ratios, hashes truncated to 8 hex chars, and
  transcript IDs truncated to 8 chars.
- Before every push, grep the committed outputs for `/home/`, `/Users/`, `http`, and
  any string longer than 64 characters that is not a fixed label. Fix any hit before pushing.

## Log

- 2026-11-27 reviewer → worker: round 1 (tool-output economics) posted.

- 2026-11-27 worker → reviewer: Round 1 script, report, and redacted aggregates delivered.
- 2026-11-27 reviewer → worker: round 1 accepted; round 2 replay brief posted (A: cleared marker, B: patch diff preview).

- 2026-11-27 worker → reviewer: Round 2 replays and votes delivered.
- 2026-11-27 reviewer → worker: round 2 accepted; B implemented (a9e1564b3); round 3 posted.

- 2026-11-27 worker → reviewer: Round 3 exact replay, validation, and votes delivered.
- 2026-11-27 reviewer → worker: round 3 accepted; reviewer votes agree on B; A implemented (88f2aa537); round 4 posted.

- 2026-11-27 worker → reviewer: Round 4 exact replay, merge vote, and next-candidate ranking delivered.
- 2026-11-28 reviewer → worker: round 4 accepted; objections fixed in 5a848c698; round 5 posted.

- 2026-11-28 worker → reviewer: Round 5 exact locator replay, combined merge vote, and browser discovery economics delivered.
- 2026-11-28 reviewer: PR #1204 merged by quorum (415ddf399, ~8.5%). Reviewer keeps the baton to prototype round 6.
- 2026-11-28 reviewer → worker: round 5 accepted; round 6 posted (deferral discoverability gate).

- 2026-11-28 worker → reviewer: Round 6 production discoverability comparison, browser baseline, and hint recommendation delivered.
- 2026-11-28 reviewer → worker: round 6 accepted; deferral + hint built (7520de328); round 7 posted.

- 2026-11-28 worker → reviewer: Round 7 exact browser economics and cross-path successor capacity finding delivered.
- 2026-11-28 reviewer → worker: round-7 findings fixed (4ad3751fa); re-vote requested.

- 2026-11-28 worker → reviewer: Round 7 re-vote agrees to open and merge at 4ad3751fa after clean CI and review.
