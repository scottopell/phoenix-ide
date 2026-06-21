---
title: keyword_search
summary: Two-stage conceptual code search — ripgrep then an LLM filter — for finding code by idea, not exact string.
category: reference
keywords: [keyword_search, search, ripgrep, conceptual, find]
related: [reference/tools/bash.md, reference/glossary.md]
---

# keyword_search

> **At a glance:** find code by *concept* when you don't have an exact string.
> Runs ripgrep across the repo, then an LLM ranks the hits against your intent.
> Auto-skips terms that match too much.

## What it does

Two stages: ripgrep gathers candidate lines for the search terms (10 lines of
context, from the git root), then a fast LLM filters and ranks them against the
stated query and returns the relevant ones with reasons.

## Parameters

| Field | Meaning |
|-------|---------|
| `query` | what you're actually looking for (the LLM ranks against this) |
| `search_terms` | terms in **descending importance** — the first survive a size squeeze |

## Limits & gotchas

- **Per-term cap 64 KB** of ripgrep output — broader terms are silently skipped.
- **Combined cap 128 KB** — lowest-priority terms are dropped until it fits.
- If *every* term is too broad, it returns an error rather than guessing.
- **Not for precise lookups.** For an exact error string, symbol, filename, or
  stack frame, a direct search ([bash](bash.md) + `rg`) is faster and exact.

## Related

- [bash](bash.md) — precise `rg`/`grep` when you know the string
- [Glossary](../glossary.md) — canonical terms
