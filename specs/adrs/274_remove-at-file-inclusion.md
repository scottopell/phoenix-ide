# ADR-274: Remove `@file` content inclusion and leave `@` literal

- **Status:** Accepted
- **Date:** 2026-11-22
- **Affects:** REQ-IR-001 (removed), REQ-IR-004, REQ-IR-006, REQ-IR-007,
  REQ-IR-010, REQ-FE-011; `inline-references.allium`

## Context

`@path` in a user message inlined the whole file into the text sent to the
model. Reads were uncapped (`ResolutionRoot::read_file`), a file named twice
was inlined twice, and the expanded text lives in the user message, so it is
re-sent on every later turn until compaction; tool-result clearing does not
reach it. `./path` already lets the agent read only what it needs. The owner
reports never using `@` inclusion and wants `@` free for a different expansion.

## Options considered

1. **Keep `@file` and cap it** (byte limit, deduplicate repeated paths). This
   bounds the waste but keeps a feature nobody uses, its path-classification
   heuristics, and a missing-file send block, and keeps `@` taken.
2. **Remove `@file` and leave `@` literal.** `./path` covers pointing at files;
   `@` becomes plain text until something else claims it.

## Decision

Option 2. The expander tokenizes only `/`; every `@` token reaches the model as
typed, with no file read and no send block. The composer drops the `@`
autocomplete trigger, and the file tree drops the "Insert @file reference" menu
item and inserts `./path` on drag-and-drop. `ExpansionError` loses its
`FileNotFound` / `FileNotText` variants. The wire `error_type` values
`file_not_found` and `file_not_text` are no longer produced. The
`expand_references` / `literal_text` chat policies keep their names and now
govern skill expansion only.

## Consequences

- **Positive:** no unbounded file contents in user messages; fewer send-time
  failure modes; `@` is free for a new meaning.
- **Negative:** users who typed `@path` expecting inclusion now send the
  literal text, and the agent must read the file itself. A matched `/skill`
  is now the only expansion that can block a send, so the REQ-IR-009 recovery
  path is reached far less often.
- **Neutral:** messages already stored keep the `llm_text` they were sent with.
  Nothing is rewritten.

## References

- `specs/inline-references/requirements.md` (REQ-IR-007, REQ-IR-010)
- `message_expander::expand`, `detectTrigger`, `FileTreeContextMenu`
