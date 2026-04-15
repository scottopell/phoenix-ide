---
created: 2026-04-15
priority: p2
status: ready
artifact: src/api/handlers.rs
---

# Branch search exact match not appearing in results

## Summary

Typing "main" in the branch picker search does not show "main" in the
results list, despite "main" existing as both a local and remote branch.

## Reproduction

1. Open /new, select a git repo with a "main" branch, choose Managed mode
2. Click the branch picker input, type "main"
3. Expect: "main" appears at the top of the search results (exact match)
4. Actual: "main" does not appear at all; only prefix matches like
   "main-ali-test1" are shown

## Likely Cause

The search endpoint filters `git ls-remote` results by substring match.
If "main" is the remote's HEAD, it may be returned by ls-remote as
`refs/remotes/origin/HEAD` (a symbolic ref) and filtered out by the
`ends_with("/HEAD")` check, or the ref might not appear in `--heads`
output if it's only represented as the symbolic HEAD.

Alternatively, the local branch "main" might not appear because the
search path only queries ls-remote (remote refs), not local branches.
A search for "main" should also include local branch matches.

## Context

Filed during QA of the branch picker rewrite (REQ-PROJ-020/021).
The sort logic (exact > prefix > substring) was verified working in
the sort_by implementation, so this is a filtering issue, not a sort issue.
