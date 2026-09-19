<<<<<<<< HEAD:specs/adrs/059_workscope-authority-projects-one-runtime-capability.md
# ADR-059: WorkScope authority projects one runtime capability
|||||||| parent of 4565f3129 (fix: retire failed approval actors):specs/adrs/055_workscope-authority-projects-one-runtime-capability.md
# ADR-055: WorkScope authority projects one runtime capability
========
# ADR-058: WorkScope authority projects one runtime capability
>>>>>>>> 4565f3129 (fix: retire failed approval actors):specs/adrs/058_workscope-authority-projects-one-runtime-capability.md

- **Status:** Accepted
- **Date:** 2026-09-13
- **Affects:** REQ-BED-028, REQ-BED-046, REQ-BASH-013a, REQ-PROJ-008; runtime tool and sub-agent authority

## Context

An Explore-origin conversation can retain its mode provenance after the user approves work. Approval changes the attached WorkScope's authority from Restricted to Work. The runtime historically selected capabilities from several independent facts: mode selected a tool registry and Work-child admission, while WorkScope authority populated tool context and could trigger a mutable registry upgrade.

That allowed one conversation to expose writable patch tools while Bash remained in the Explore OS sandbox and Work sub-agents remained forbidden. Reconstructing the runtime repeated the split because mode-based branches rebuilt Restricted consumers despite durable WorkScope authority.

The transition crosses a durable commit boundary. Once WorkScope authority is persisted, a process may stop before the in-memory runtime is reconfigured. Recovery must have one authoritative fact from which every consumer can be rebuilt.

## Decision

Persisted WorkScope authority is the sole capability authority for an attached conversation. Conversation mode remains provenance and lifecycle context; it does not independently grant or deny execution capabilities.

A runtime uses one typed capability projection containing authority-dependent tool definitions, dispatch policy, Bash isolation, provider-facing prompt context, tool context, and sub-agent admission. Approval atomically persists the approved-task objective and Work authority, then constructs and publishes the complete Work projection before post-approval execution resumes. The objective records reviewed intent; it is not a prerequisite for capability already granted to Direct, Work, Branch, or other valid creation paths.

A failed projection or publication does not resume with mixed capabilities. Runtime reconstruction derives the complete projection from persisted WorkScope authority. Precreated provider/tool work remains bound to the authority under which it was admitted and cannot cross the publication boundary as newly privileged work.

## Options considered

### Mutate each consumer after persistence

Rejected. Independent mutable fields made omission representable and provided no proof that every consumer transitioned before resume.

### Change Explore-origin mode to Work

Rejected. Mode records provenance and established conversation context. Overloading it as capability authority would contradict the WorkScope-owned lifecycle decision in ADR-026 and duplicate durable authority.

### Rebuild the actor after every approval

Rejected as the primary contract. Reconstruction is necessary for crash recovery but cannot make partial live publication safe, and actor replacement is broader than the capability transition.

### Re-read WorkScope authority at every tool call

Rejected as the primary structure. Repeated checks can support defense, but they do not make provider definitions, prompt capability, dispatch, and admission one coherent snapshot.

## Compatibility

The authority projection change is forward-only. A forward migration repairs legacy Direct WorkScopes that were incorrectly classified as Restricted Explore and strengthens timestamp storage-class checks without changing an already-applied migration; rollback requires the ordinary offline paired database restore governed by `specs/compatibility/requirements.md` and is not otherwise guaranteed.

## Consequences

- Explore-origin conversations with approved WorkScope authority receive unsandboxed Work Bash and may spawn one Work child under existing single-writer rules.
- Unapproved Explore-origin conversations remain fully Restricted even when a Git worktree exists.
- Mode-based capability branches and no-op authority-upgrade hooks are removed or narrowed to provenance-only behavior.
- Approval and reconstruction tests must exercise real authority-dependent consumers, including Bash and Work-child admission.
- This decision defines an internal authority transition; it adds no cross-version, downgrade, rollback, or live-resource-replacement compatibility guarantee.

## Supersession

This ADR refines ADR-026's separation of ProductConversation lifecycle from WorkScope resource ownership and applies ADR-039's fail-closed resource identity policy to runtime capability projection. It does not supersede either decision.
