# ADR-042: Close directory retirement trusts its private namespace

- **Status:** Accepted
- **Date:** 2026-08-28
- **Affects:** REQ-WL-002b
- **Supersedes:** ADR-039's stronger implication that destructive retirement must withstand arbitrary concurrent namespace mutation

## Context

Close already derives destructive authority from the owning ProductConversation's committed operation and a sealed WorkScope admission gate. Worktree retirement moves the exact identity-checked target into a random Phoenix-created directory with owner-only permissions before final removal. Earlier hardening treated every descendant lookup as if an adversary could mutate that private directory concurrently. That expands a bounded local lifecycle operation into a general race-proof recursive filesystem deletion facility without improving the supported ownership boundary.

Crashes and ordinary external interference can still replace or obscure the private tombstone root or its moved object before Phoenix opens it. Close must detect that ambiguity without deleting an unverified replacement. Those cases differ from an attacker operating inside a Phoenix-owned owner-only namespace or asking Phoenix to retire resources it does not own.

Repair routing also needs one durable explanation. A `NeedsRepair` phase without typed residual evidence cannot project the remaining resource after reload.

## Options considered

1. Build general recursive deletion that defends every descendant operation against malicious concurrent namespace mutation.
2. Use pathname deletion after the initial worktree identity check.
3. Trust the WorkScope gate and random Phoenix-owned owner-only tombstone, while descriptor-binding and identity-checking the tombstone root and moved object before bounded deletion.

## Decision

Choose option 3.

Normal Close reliability is bounded by the WorkScope admission gate and Phoenix-owned private directories. Phoenix creates a random owner-only tombstone, moves the already identity-checked directory into it, opens the tombstone root without following links, verifies the opened root's filesystem identity, opens the moved object relative to that descriptor, and verifies the opened object's identity before deletion.

If a crash or external mutation makes either identity ambiguous, Phoenix preserves the safe leftover and records typed residual evidence before entering `NeedsRepair`. Every repair route names the exact attempt, scope, typed residual resource, reason, and detail in one atomic persistence transaction so reload projection remains truthful.

Malicious concurrent mutation inside the Phoenix-owned private namespace and mutation outside Phoenix ownership are unsupported. Close does not grow a general adversarial recursive-deletion subsystem for those cases.

## Consequences

- **Positive:** Normal Close remains reliable across supported WorkScope-owned resources without unbounded filesystem complexity.
- **Positive:** Root/object replacement before descriptor binding fails closed and leaves repairable evidence.
- **Positive:** Every `NeedsRepair` transition has a reloadable typed residual.
- **Negative:** A privileged or same-user adversary mutating the private tombstone during deletion is outside the guarantee.
- **Negative:** Post-crash or external ambiguity may leave private tombstones for manual or later repair.

## References

- ADR-039: durable runtime resource identity fails closed outside proven containment
- ADR-040: Close uses WorkScope gates and tmux-only durable identity
- `specs/work-lifecycle/requirements.md`
- `specs/work-lifecycle/work-lifecycle.allium`
