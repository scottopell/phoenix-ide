# ADR-147: Readable generated handoffs retain explicit source authority

- **Status:** Accepted; supersedes the blanket manual-intent authority clause of ADR-054
- **Date:** 2026-10-25
- **Affects:** REQ-BED-020b, REQ-BED-021, REQ-BED-030, REQ-LLM-004m

## Context

Human review of a generated handoff does not turn generated claims into user instructions. ADR-054 distinguishes manual and automatic initiation as its authority boundary, but explicit review of generated summaries and shortening candidates needs a more precise source boundary. Existing completed receipts already record accepted authority and cannot be reinterpreted from wording.

Encoded generated openings also obscure the natural language the successor needs. Readable English is required for new and existing live generated openings, while the original transcript must remain durable evidence. The user accepts the necessary cache miss when this projection changes.

## Options considered

1. **Retain ADR-054's blanket manual-intent authority rule.** Simple initiation-based classification, but review of generated claims promotes them to instructions.
2. **Infer generated provenance from matching text or an English wrapper.** Avoids explicit source plumbing, but a literal user instruction can have identical bytes and text cannot prove authority.
3. **Use explicit source kinds and durable receipts, with readable live projection.** Requires source validation and candidate revision fencing, but keeps initiation, provenance, and execution authority separate.
4. **Keep old encoded live openings for cache stability.** Avoids a prefix change while preserving the readability failure on ongoing conversations.

## Decision

Choose option 3. An explicit exact persisted summary source or current generated-candidate source retains generated predecessor context authority, including after human review and acceptance. An explicit manual/literal source, including edits, is a user-authorized instruction. Source-only retained review inputs cannot be accepted as generated candidates. Existing durable acceptance receipts retain their recorded authority.

Project generated predecessor context as readable English with an explicit untrusted provenance boundary and losslessly escaped delimiters. Apply this to existing live generated openings using their durable authority, without rewriting persisted transcripts. Accept cache loss rather than retain encoded model-visible content. Ordinary expanded messages and user-authorized openings retain their own contracts.

Only the blanket manual-intent authority clause in ADR-054 is superseded. Its prospective default-OFF consent sampling, automatic admission ownership, topology, idempotence, bounded recovery, and race semantics remain in force.

## Consequences

- **Positive:** Human review no longer grants authority to generated claims merely by initiating continuation.
- **Positive:** Successors can read generated handoffs and consult original evidence.
- **Negative:** Explicit sources require exact identity/text validation, current candidate revision checks, and preview acceptance.
- **Negative:** Existing live generated openings can lose cached prefixes when their provider projection becomes readable.
- **Neutral:** Historical receipts and transcript bytes remain authoritative evidence; they are not reclassified or rewritten.

## References

- [ADR-054](054_automatic-continuation-admission-binds-consent-and-context-authority.md)
- [ADR-146](146_context-capacity-preserves-output-and-semantic-continuity.md)
- [Bedrock requirements](../bedrock/requirements.md)
- `ContinueHandoffSource`, `continue_conversation`, `ContinuationOpeningRead`
