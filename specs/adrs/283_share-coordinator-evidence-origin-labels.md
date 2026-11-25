# ADR-283: Share Coordinator evidence origin labels

- **Status:** Accepted
- **Date:** 2026-11-24
- **Affects:** REQ-SCAN-004, REQ-TRACK-005

## Context

`InputOrigin::CoordinatorQuestionReply` is a valid source for the scanner.
Migration 168 widened the SQL allowlist so the scanner could store
`coordinator_question_reply`, but tracking detail had an independent enum
that rejected the same label. Both lists duplicate input-origin kinds rather
than define separate policies.

## Options considered

1. **Add the missing kind to the SQL list and tracking enum.** This fixes the
   reply case but leaves both copies to drift when a new origin is added.
2. **Share the typed origin projection and keep only the SQL shape check.**
   Scanner writes and tracking detail use the same canonical labels; the detail
   reader handles unfamiliar stored text without treating it as trusted input.

## Decision

Use a shared core `InputOriginKind` projection of `InputOrigin` for scanner
writes and tracking detail. The scanner stores the canonical label from the
verified source, not a string supplied by inference. Migration 172 rebuilds
`coordinator_scan_evidence_origins`, replacing just the origin allowlist with a
nonblank TEXT constraint. It copies all rows unchanged and preserves the other
constraints, including the evidence and scan-attempt references, key, role,
generation and span checks.

Tracking detail returns recognized labels unchanged. If the stored label is
unrecognized, it logs the mismatch and returns `unknown_historical` for display
only. It leaves the row, evidence, review path and authority unchanged. This
fallback is not used to decode or admit transcript inputs.

## Consequences

- A valid question reply can commit scanner evidence and coverage together
  without requiring a scanner-table migration for each new origin kind.
- SQL no longer rejects an unfamiliar nonblank text label in this table. The
  typed writer still emits only canonical labels, and the detail read treats
  unrecognized text as untrusted without losing the evidence page.
- No scanner prompt, foreground review flow or UI changes. The fallback is
  local to this detail read; it makes no general cross-version database,
  downgrade or rollback guarantee.

## References

- [Scanner write contract](../coordinator-scanner/requirements.md#req-scan-004-restricted-application-authority)
- [Tracking detail contract](../coordinator-tracking/requirements.md#req-track-005-bounded-inspectability)
- [Coordinator question replies](../ask-user-question/requirements.md#req-auq-011-answer-coordinator-questions-outside-the-transcript)
- [ADR-034](034_compatibility-guarantees-are-explicit-and-data-aware.md)
