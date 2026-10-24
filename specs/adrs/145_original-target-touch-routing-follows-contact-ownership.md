# ADR-145: Original-target touch routing follows contact ownership

- **Status:** Accepted
- **Date:** 2026-10-23
- **Affects:** REQ-MLRU-017, REQ-MLRU-014

## Context

A controlled browser investigation delivered a trusted touch end to the original DOM target after its subtree had detached. Neither the transcript scroller nor the document received that event. The existing two-second gesture-evidence abandonment had already left the policy reading and idle without granting tail follow. Expiring evidence and losing the event's routing path were therefore distinct concerns.

Listener installation can also fail while a second contact remains physically down. Treating successfully installed listeners as the set of owned contacts would let the first release incorrectly report zero remaining fingers.

## Options considered

1. **Keep only stable-ancestor listeners** — retains existing cleanup simplicity but cannot receive the observed detached-target end.
2. **Use a longer listener timer** — bounds retention duration but introduces an arbitrary interval after which a real held contact loses release routing. The existing gesture deadline cannot establish physical release.
3. **Keep lifecycle-bounded original-target routing separate from owned contacts and gesture evidence** — preserves actual release delivery when listener installation succeeds without extending gesture evidence or inventing a physical release.

## Decision

Use the third option. Track admitted live touch IDs and original targets independently of installed listeners. Retain at most one listener registration per owned live contact, within a component, actual scroller, and owner-generation lifetime. Same-element callback-ref churn preserves that lifetime; owner/scroller retirement and component unmount end it. Trusted matching end/cancel, live-list absence, or identifier reuse releases the corresponding ownership and resources.

Install both listeners transactionally. On failure, invalidate the partial listener registration, attempt both removals, report the failure, and preserve owned-contact bookkeeping. Successful listener installation enables detached-target release routing; failed installation does not promise later detached release capture. An observed connected release still uses owned-contact admission.

Do not add an elapsed-time resource cap or change gesture-evidence abandonment. A late real end cannot recreate expired movement evidence or restore follow merely because its routing registration survived.

## Consequences

- A missing end with no subsequent admitted input may retain the last owned target references and listeners until owner retirement. Cardinality is bounded by admitted live contacts, not an invented device-finger limit; duration is not bounded.
- Event identity, target/identifier admission, and generation invalidation prevent duplicate or retired callbacks from resolving a new contact transaction.
- Listener availability is not evidence of remaining physical contacts.
- A successful product release receipt does not establish native momentum, paint behavior, image causality, or whole-held continuity. Those acceptance claims require their own evidence.
