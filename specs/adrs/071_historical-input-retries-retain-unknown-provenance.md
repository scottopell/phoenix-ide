# ADR-071: Historical input retries retain unknown provenance

- **Status:** Accepted
- **Date:** 2026-09-30
- **Affects:** REQ-GR-014, REQ-DWF-039, REQ-COMP-001

## Context

Provenance rollout added origin to admission identity. Accepted historical work has no recorded source, while an API retry acquires the API channel at the trusted boundary. Strict comparison consequently rejected an unchanged retry. ADR-070 did not establish a cross-version replay guarantee; this decision defines the bounded exception for those accepted inputs.

## Decision

An API retry may match an unknown historical origin, subject to all other existing identity, payload, attachment, and expansion-policy checks. The accepted input retains its original unknown origin and prepared payload. Recorded origins still require equality. Internal conversation, generated, and subscription input do not gain a wildcard match against historical input.

Existing pre-provenance steering fingerprints may be checked using their original origin-less representation for API retries. New receipts include origin. This does not permit changed payloads or rewrite old receipts.

## Consequences

Direct replay, queued steering replay, and persisted-message replay need regression coverage with an API-origin retry and unknown stored input. Historical authorship is never inferred. This exception adds no downgrade, mixed-version runtime, or live database replacement support.
