# ADR-073: Preserve prelocator prepared-turn fingerprints

- **Status:** Accepted
- **Date:** 2026-10-02
- **Affects:** REQ-COMP-007

## Context

The source-locator release added a nullable field to internal-conversation origin without changing prepared envelope version 2. Normalized durable rows reconstruct origin from columns. Historical accepted checksums bind an encoding without the field; reconstruction with null blocked startup reconciliation before listener activation.

## Options considered

- Rewrite accepted checksums or bypass validation: violates accepted identity and corruption detection.
- Omit null globally: changes current accepted payloads and loses a single canonical current encoding.
- Verify a narrowly typed historical encoding against the stored checksum: chosen.

## Decision

Try current exact encoding first. Only version-2 internal-conversation input with no recorded locator can additionally use the prelocator origin encoding. Return those original bytes only when their existing fingerprint verifies. Do not rewrite storage or infer provenance. Other corruption remains fatal.

## Consequences

Regression coverage includes normalized attachment restoration, authoritative load/discovery, replay with a later invocation, unchanged rows, and rejection of corrupt fingerprints. No downgrade, mixed-version runtime, or production database repair guarantee is added.
