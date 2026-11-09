# ADR-246: Ranked search trusts indexed text for hidden-message eligibility

- **Status:** Accepted
- **Date:** 2026-11-09
- **Affects:** REQ-RET-003, REQ-RET-007

## Context

FTS5 search matches large numbers of chunks for broad terms. Ranked retrieval joins each match to its source message and reads `display_data.hidden`; on a broad production-shaped query, this source predicate accounted for most of the observed statement time. Normal hidden-message indexing stores empty searchable text, and the hide operation changes the source and index in one transaction. Standalone post-commit upserts, reconciliation and historical index rows make an index/source discrepancy possible.

## Options considered

1. **Keep the source hidden predicate** — preserves a per-search backstop against stale indexed text, at substantial per-candidate read cost.
2. **Trust blank indexed text** — avoids loading source display payloads for each match, while accepting that a stale index may briefly yield a hidden message until repair.
3. **Normalize hidden state into a source column or derived locator flag** — potentially makes candidate checks cheaper, but requires a migration, writer audit and physical-query validation; a locator flag alone is another derived representation.
4. **Filter after ranking and limiting** — avoids candidate checks but can drop eligible results behind hidden hits.

## Decision

Ranked search relies on the FTS index for hidden-message eligibility and does not recheck source `display_data.hidden` per candidate. The user accepts the possibility of a temporary hidden-message search hit if indexed text disagrees with the source. Keep empty-text indexing, source joins for the remaining query semantics, and startup reconciliation. As a best-effort reduction of a known race, a standalone index upsert must reread source hidden state inside its write transaction before replacing indexed text.

## Consequences

- **Positive:** Broad searches avoid reading and parsing source display payloads for every matching FTS chunk, without a schema migration.
- **Negative:** An old, corrupt or otherwise stale physical FTS row can produce a hidden-message search hit before reconciliation. Reconciliation is repair, not an authorization fence; no guarantee of zero stale exposure follows from this decision.
- **Negative:** Post-commit upserts still use a supplied message snapshot for other index content; the re-read only closes the observed stale-hidden-flag overwrite race. Non-boolean historical `hidden` values may behave differently because SQL's numeric equality and Rust's boolean-only index extractor disagree.
- **Neutral:** Other read surfaces keep their own visibility rules. This changes ranked retrieval, not all message reads.

## References

- `specs/conversation-retrieval/requirements.md` (REQ-RET-003, REQ-RET-007)
- `Fts5Retriever::retrieve`, `build_retrieval_sql`, `fts_upsert`, `fts_reconcile_upsert`, `Database::update_message_display_data`, `message_text::index_text`
