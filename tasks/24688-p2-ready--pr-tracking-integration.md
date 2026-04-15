---
created: 2026-04-15
priority: p2
status: ready
artifact: ui/src/components/StateBar.tsx
---

# PR tracking integration for managed conversations

## Summary

Show the PR status (open, merged, CI status) in the conversation UI when the
base branch has an associated PR. Similar to the purple merged badge in Claude
Code and other tools -- a quick visual indicator that links the conversation to
the PR lifecycle.

## User Need

"I would love to track here in phoenix BTW, super useful in claude code and
other apps I use, see purple badge with merged logo and skim right to the
next one"

When working on a branch with an open PR, the user wants to:
- See PR status at a glance without leaving Phoenix
- Know if CI is passing/failing on the branch
- Navigate to the PR from the conversation

## Possible Approach

- Detect PR via `gh pr list --head <branch_name>` or GitHub API
- Show a badge in the StateBar next to the branch name
- Color-coded: green (open, checks passing), yellow (open, checks pending),
  red (open, checks failing), purple (merged)
- Clickable to open the PR URL

## Context

Filed during QA of branch picker. The branch picker makes branch selection
easy; PR tracking would close the loop by connecting the branch to its
review lifecycle.
