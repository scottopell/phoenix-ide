---
created: 2026-03-01
priority: p2
status: ready
---

# Streaming markdown rendering is broken/unreadable

## Summary

Now that we have token-by-token streaming, markdown constructs that need
complete structure to render — tables, code blocks, diagrams, lists — are
rendered incrementally as raw partial syntax, making the output nonsensical
and hard to read until the construct completes.

## Context

With streaming enabled, the UI renders each token as it arrives. A markdown
table like:

```
| Col A | Col B |
|-------|-------|
| 1     | 2     |
```

…shows up character-by-character as `| C`, `| Co`, `| Col`, etc., which is
visual noise rather than useful progressive disclosure. The same applies to
fenced code blocks (syntax highlighting flickers), Mermaid/ASCII diagrams,
and nested lists.

This is a known hard problem — incremental markdown parsing requires either:
- Buffering until a construct boundary is detected
- Using a streaming-aware markdown parser that can emit partial ASTs
- Hybrid: render paragraphs immediately, buffer block-level constructs

There may be off-the-shelf solutions (e.g., `markdown-it` streaming modes,
`react-markdown` with custom buffering, `mdx` incremental compilation).
Investigation needed before committing to an approach.

## Acceptance Criteria

- [ ] Investigate existing streaming markdown libraries/approaches
- [ ] Tables render only when complete (or at least row-by-row)
- [ ] Fenced code blocks don't flicker syntax highlighting mid-token
- [ ] Inline markdown (bold, links, backticks) still renders progressively
- [ ] No regression in non-streaming (complete message) rendering
- [ ] Latency to first visible token remains under ~200ms
