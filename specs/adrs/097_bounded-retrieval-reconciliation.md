# ADR-097: Bounded orphan pruning and terminal retrieval readiness

Date: 2026-10-12

Status: Accepted

## Context

A deployed startup reconciliation attempt returned SQLite extended code 517
(`SQLITE_BUSY_SNAPSHOT`). The one-shot worker exited while its boolean readiness
remained false, causing later search requests to describe a terminated attempt
as warming. The terminal log did not identify the failed statement.

Inspection found that orphan pruning read the locator set in a deferred
transaction and then upgraded that snapshot to a writer. A deterministic WAL
fixture reproduced code 517 after another connection committed between those
operations. The per-message upsert helper already acquired write admission
before reading; reserving the writer for the whole orphan sweep would instead
make lock scope proportional to the index size.

## Decision

Discover locator candidates outside the write transaction with keyset pagination,
a fixed maximum locator boundary, and pages of 128 rows. For each page containing
orphan candidates, acquire `BEGIN IMMEDIATE`, revalidate the current locator and
source message, and atomically delete the physical FTS row and locator. Commit
before moving to the next page. Normal mutation hooks remain responsible for
concurrent mutations outside the pass's coverage.

Represent startup reconciliation as pending, running, ready, or failed. A single
owned attempt guard publishes terminal interruption when dropped during panic
unwinding or cancellation, including cancellation before the worker's first poll.
Progress has one authoritative representation. Only successful orchestration
completion publishes ready. Do not add retry, request-triggered repair, or a
second worker.

Expose status and sanitized numeric database error codes at the authenticated
`GET /api/deployment/retrieval` diagnostic without issuing database queries.
Search remains gated on readiness, but reports terminal failure truthfully.

## Alternatives

- Increasing busy timeout or sleeping inside the stale transaction cannot make
  its snapshot eligible for a write upgrade.
- Reserving the writer before an unbounded discovery sweep risks long writer
  exclusion.
- Retrying an entire reconciliation pass introduces a recovery policy and extra
  work; neither is required for this correction.
- Serving the partial index as ready would hide incomplete recall.

## Consequences

A failed later page can leave earlier pages committed; each page preserves
physical-index/locator atomicity, and overall readiness remains failed. The page
size bounds candidate operations, not elapsed lock time independent of FTS work.
The status endpoint makes failure distinguishable without invoking search.

This change does not alter schema, authorize production repair, or promise
cross-version rollback. An activation and recovery decision remains separate.
The reproduced prune path is a confirmed vulnerability, not proof of the exact
statement responsible for the deployed event.
