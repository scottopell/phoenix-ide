---
created: 2026-04-20
priority: p2
status: ready
artifact: src/api/handlers.rs
---

# Auto-detect PR merge for conversation lifecycle

## Summary

Currently "Mark as merged" is user-initiated. Could auto-detect via
`gh pr list --head <branch> --state merged` or `git branch --merged`.
When detected, auto-archive the conversation or show a prompt.

## Context

Deferred from discovery session. User chose manual for simplicity but
expressed interest in auto-detection as follow-up.

## Done When

Decision on approach (polling vs webhook vs on-page-load check).
Implementation if decided to proceed.
