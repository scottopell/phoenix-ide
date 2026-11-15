# ADR-256: New conversations default to automatic context continuation

- **Status:** Accepted; supersedes only ADR-054's default-disabled choice
- **Date:** 2026-11-14
- **Affects:** REQ-BED-021, REQ-API-029, REQ-CONV-024

## Context

The user explicitly requests automatic context continuation ON by default to prevent context-exhaustion stalls caused by forgetting the toggle. The stable ProductConversation row stores a Boolean preference with no provenance distinguishing an old implicit OFF from an explicit opt-out. Updating existing false values would therefore override some intentional preferences.

## Options considered

1. Enable every existing aggregate. Rejected: it cannot preserve explicit opt-out.
2. Change the SQLite column default through a table rebuild. Rejected: rebuilding the highly referenced aggregate table adds migration risk without product value; supported creation already passes through application-owned insertion paths.
3. Initialize fresh eligible aggregates ON at creation, preserving all existing rows and the historical SQL default. Selected.

## Decision

The domain preference default is Enabled. Fresh ordinary ProductConversation insertion paths and new Global Coordinator bootstrap explicitly initialize the persisted preference ON. Supported web, API, native and destination-owned creation share those persistence paths and do not need separate client defaults or new creation-request fields. Explicit false remains a strict Boolean settings operation, not an omitted creation option.

Historical migrations and their default-zero interpretation remain unchanged. No migration or backfill enables existing records. Idempotent inserts retain `ON CONFLICT DO NOTHING`; Coordinator bootstrap returns an existing singleton without resetting its preference. Continuation successors share the same stable aggregate and do not initialize a new preference.

Only the initial preference changes. ADR-054's prospective atomic context-exhaustion admission, lifecycle fences, and generated-context authority remain intact. Enabling does not revive an already-exhausted transcript, dispatch work, create a Goal, grant unrelated authority, or turn errors into unconditional retries. Existing Stop, pause, approval, history and Close gates remain authoritative.

## Consequences

New eligible conversations continue through future context exhaustion without a separate toggle action, and users can opt out. Existing conversations keep their exact saved behavior, including old implicit OFF values. UI status continues to come from the persisted server response rather than guessing ON while loading. Database tests that intentionally exercise OFF establish that precondition explicitly. Synthetic federation coverage observes the destination's preference without changing real peers or enrollment.

Raw SQL inserts which omit the preference retain the historical OFF default; they are not supported product creation entry points. This intentionally avoids rewriting legacy records or changing historical fixture/migration meaning.
