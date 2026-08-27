# ADR-039: Creation publication uses request-bound identity and immutable starting pins

- **Status:** Accepted
- **Date:** 2026-08-26
- **Affects:** REQ-CCR-001, REQ-CCR-002, REQ-CCR-003, REQ-CCR-005, REQ-CCR-005A, REQ-CCR-006A, REQ-DWF-CREATE-001, REQ-DWF-CREATE-002, REQ-DWF-CREATE-003, REQ-DWF-CREATE-004, REQ-PROJ-017, REQ-PROJ-022, REQ-GITREP-003

## Context

Conversation creation still carries design remnants from earlier staged-shell and reservation-oriented shapes. Those remnants make the contract harder to reason about: a visible shell can appear before the starting state is whole, request replay and worker identity can blur together, and Git-backed creation can appear to recompute its starting point after acceptance rather than preserving one durable fact.

The simplified contract narrows creation to one request-bound durable job, one immutable starting pin when Git-backed creation resolves a starting commit, and one atomic user-visible publication boundary. Cancellation and deletion still need cleanup, but ownership and cleanup safety are sometimes genuinely ambiguous after a crash or interrupted external effect. The contract therefore needs an explicit non-destructive ambiguity outcome instead of a requirement to guess, silently clean, or fabricate success.

The older staged and reconciliation decisions remain valuable historical context, but they no longer describe the desired product-facing contract for creation publication.

## Options considered

1. **Keep staged shell publication plus reservations** — preserve the earlier shape where the shell is visible before publication is whole and cleanup is modeled around durable reservations. This keeps more intermediate structure, but it preserves the ambiguity between accepted intent and published conversation state and keeps obsolete reservation/early-publication semantics in the normative path.
2. **Publish only after whole creation completes, keyed by request identity** — accept one durable job per request, replay by request identifier, derive one immutable Git starting pin, and publish the conversation only when the starting state is whole. This removes partial-publication ambiguity and makes retry behavior easier to explain, but it constrains implementations to maintain a stricter publication boundary.
3. **Treat worker claims or runtime bootstrap as the durable identity** — let a later worker or bootstrap phase mint the authoritative identity while the original request remains advisory. This simplifies some worker-local logic, but it breaks externally retryable acceptance and makes stale-worker or stale-bootstrap races harder to reject structurally.

## Decision

Phoenix uses request-bound durable identity for conversation creation. Acceptance persists exactly one durable creation job for a given request identifier and replays that accepted result when the same request and intent are retried. Reusing the request identifier with different intent is a conflict.

Git-backed creation records one immutable starting pin before ready publication. When a remote named `origin` exists, Phoenix discovers that remote's current authoritative default branch and freshly fetches its tip under the repository mutation lock, then pins that exact fetched commit. Any discovery, authentication, authorization, transport, or fetch failure is an explicit retryable creation-job failure with no fallback. When no remote named `origin` exists, Phoenix resolves only local `refs/heads/main`; a missing or unresolvable local `main` is an explicit retryable creation-job failure with no fallback. Once the exact starting OID is pinned for the accepted job, retry, restart, and claim replacement never refresh it.

User-visible publication is atomic. After worktree materialization succeeds, the ProductConversation, transcript state, attached usable WorkScope, and immutable starting pin appear together or not at all on normal user-facing surfaces. Objective or navigation work requested to run after creation is queued only after that publication boundary. Creation-time expansion behavior, unresolved-shell publication, early-publication staging, pre-scope approval publication, and reservation-centric cleanup semantics are removed from the normative creation contract.

Cleanup remains ownership-bound and non-destructive under ambiguity. Cleanup removes only owned unpublished staging resources. If retries or replacement workers cannot prove ownership, equivalence, or cleanup safety exactly, Phoenix preserves an explicit ambiguity outcome for manual resolution rather than deleting or adopting on guesswork.

This decision supersedes the reservation-oriented creation and reconciliation aspects of ADR-007 and the staged ProductConversation publication aspects of ADR-031 for conversation creation. Those ADRs remain historical records and are not edited.

## Consequences

- **Positive:** Request replay, worker fencing, and published conversation identity become easier to reason about because the durable identity is the request, not a later worker token.
- **Positive:** Git-backed creation tells one consistent story about where a conversation started, and later repository movement does not rewrite that origin fact.
- **Positive:** Users never observe a half-created conversation on normal surfaces.
- **Negative:** Implementations that expose intermediate shells, early publication, reservation-led cleanup, creation-time expansion, or fallback default-branch selection need further migration before they satisfy the spec.
- **Negative:** Explicit ambiguity outcomes require product surfaces and operators to tolerate manual follow-up instead of assuming cleanup is always automatically knowable.
- **Neutral:** Historical ADRs that described staged creation remain in the chain as context, but the newer creation contract is now the consulted authority for spec work.

## References

- Supersedes the reservation-oriented creation/reconciliation decision in ADR-007 and the staged creation-publication aspects of ADR-031
- `specs/conversation-creation/requirements.md`
- `specs/conversation-creation/conversation-creation.allium`
- `specs/durable-workflows/requirements.md`
- `specs/durable-workflows/creation-profile.allium`
- `specs/git-repository/requirements.md`
- `specs/bedrock/requirements.md`
