# ADR-191: Native create and rename use shared application actions

- **Status:** Accepted
- **Date:** 2026-10-29
- **Affects:** REQ-GR-007; REQ-CCR-013; REQ-CID-007

## Context

The user commissioned dedicated Coordinator creation and naming tools rather than repeated ad hoc HTTP recipes. Public creation already has durable request acceptance and worker reconciliation; naming already has a canonical title/slug authority and naming revision. Native adapters must not fork those policies or convert agent-authored objectives into human authority. An uncertain rename also cannot safely replay a mutable slug or overwrite a later name.

## Options considered

1. Keep scoped Bash as the only interface: preserves public enforcement but leaves routine intent and retry handling to each invocation.
2. Implement independent native creation and naming paths: duplicates admission and introduces divergent state authorities.
3. Add thin native adapters over shared application actions, with durable operation identity and source provenance: selected.

## Decision

Only the persisted current Global Coordinator receives these two tools. Shared actions retain public creation and title/slug mutation semantics. Committing persistence operations recheck Coordinator identity and trusted source-call ownership. Creation records internal-conversation provenance and delivers literal agent text. Existing user, historical, and Slack origins retain their own decoding and expansion behavior.

Rename pins product, canonical root, and naming revision before mutation and commits an operation receipt atomically with the shared naming write. Retries return the historical receipt without reapplying it. Request/operation identity reuse with changed intent fails. Readback distinguishes acceptance, historical outcome, current state, and actual runtime completion.

This decision narrows the dedicated-tool exclusion in REQ-GR-007 only for creation and naming. It grants no continuation, question-answer, review, task-approval, deployment, compatibility, rollback, or automatic lifecycle authority.

## Consequences

The native tools are not HTTP clients and do not require copied credentials. Existing authenticated public callers retain their ingress checks and server-owned input origin. Addressable provenance and receipts live in relational columns/rows; no parallel name authority or old-slug alias is added. Registry isolation, revoked-current-Coordinator behavior, idempotent acceptance, revision/ABA fencing, and registered migration preservation require focused regression coverage.
