# ADR-295: Number read_file lines without padding

- **Status:** Accepted
- **Date:** 2026-11-28
- **Affects:** `read_file` model-visible output (no spEARS spec exists for this tool)

## Context

`read_file` rendered each line as `{line_num:>6}\t{line}`. Under the `o200k_base` tokenizer, the padding
costs about 1.96 tokens per line, because space runs before a short number split into several tokens. An offline replay over the
production database (61.09B recorded input tokens) put the padding at **734M carried tokens (1.20%)**.
Agents reuse explicit line numbers. 13.4% of reads are followed by an explicit line reference, and 4.0% by a re-read at an offset
inside the returned range.

## Options considered

1. **Keep the padding.** No saving.
2. **Drop the padding and keep each line's exact number:** `{line_num}\t{line}`. 1.20%, no information lost.
3. **One range header and no per-line numbers.** 2.14%, but it removes the anchors agents reuse, and the extra
   header made some results larger.

## Decision

Option 2. The tab separator, source indentation and the remaining-lines footer are unchanged. Both UI
parsers (`MessageComponents` read_file rendering and `searchProjections`) already accept an unpadded
prefix, and stored padded history keeps rendering. The reviewer and worker agents of the tool-output audit made this decision
under the owner's delegated authority.

## Consequences

- **Positive:** about 2 tokens fewer per numbered line, at no information cost.
- **Negative:** numbers are no longer column-aligned for a human reading raw tool text.
- **Neutral:** the saving grows as new results replace padded history.

## References

- `docs/qa/tool-output-audit/round-9.md`
- `ReadFileTool::run`
