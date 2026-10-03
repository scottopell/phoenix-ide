# Project Coordinator Profile Requirements

## User Story

As a Phoenix user, I want an ordinary ProductConversation to carry durable coordination guidance and my own editable charter, so that it can coordinate bounded outcomes across continuations without gaining new authority or relying on stale charter copies in transcript history.

## Requirements

### REQ-PCO-001 — Explicit ordinary-conversation opt-in

WHEN a user enables the Project Coordinator profile for an ordinary ProductConversation
THE SYSTEM SHALL associate the profile with that ProductConversation's stable identity
AND SHALL preserve the association across reload, process restart, and context continuation

WHEN the profile has not been enabled or has been disabled
THE SYSTEM SHALL retain ordinary ProductConversation prompt and compaction behavior

THE SYSTEM SHALL allow more than one ordinary ProductConversation to have the profile
AND SHALL NOT impose project ownership, hierarchy, or uniqueness constraints

THE SYSTEM SHALL NOT apply this profile to the singleton Global Coordinator ProductConversation

---

### REQ-PCO-002 — One authoritative plain-text charter

WHILE an ordinary ProductConversation has the Project Coordinator profile
THE SYSTEM SHALL persist exactly one plain-text charter on that ProductConversation identity
AND SHALL expose the current charter and its revision to the human editing surface

THE SYSTEM SHALL preserve charter text exactly as accepted, including line breaks and surrounding whitespace
AND SHALL reject charter text containing a NUL character or exceeding 32,768 UTF-8 bytes

WHEN a fresh turn or continuation executes
THE SYSTEM SHALL resolve the current charter from the stable ProductConversation identity
AND SHALL NOT copy the charter into a transcript message, continuation handoff, or other competing persisted representation

WHEN a user disables the profile
THE SYSTEM SHALL remove its persisted charter

---

### REQ-PCO-003 — Human editing surface and bounded trust claim

THE SYSTEM SHALL provide an explicit human settings action to enable or disable the profile and edit, cancel, and save the charter

WHEN two human editing surfaces save from the same charter revision
THE SYSTEM SHALL accept at most one save
AND SHALL reject a stale save without overwriting the accepted charter

WHEN saving fails
THE SYSTEM SHALL retain the unsaved editor contents and surface the failure

THE SYSTEM SHALL NOT register a charter mutation tool for an LLM
AND SHALL NOT interpret chat messages, assistant output, tool calls, tool results, Project Coordinator behavior, or Global Coordinator behavior as persisted charter mutations

THE SYSTEM SHALL describe this as absence from supported LLM mutation surfaces
AND SHALL NOT claim that same-user HTTP authentication proves biological-human presence or prevents arbitrary misuse of an authenticated HTTP client

---

### REQ-PCO-004 — Generic coordination guidance

WHEN a Project Coordinator profile turn executes
THE SYSTEM SHALL add concise generic guidance that:

- prefers delegating bounded outcomes when delegation is useful and ordinarily authorized;
- expects assigned workers to iterate within their assigned scope through correction, validation, publication, and qualification;
- keeps scope, priority, shared-resource, exception, verification, and stopping-point arbitration with the coordinator; and
- permits direct work when it is small or preserves essential context.

THE SYSTEM SHALL include the current charter separately from the built-in guidance
AND SHALL NOT hardcode Phoenix repository policy, Codex policy, a roadmap system, machine-capacity policy, or provider-specific operating rules into the generic guidance

---

### REQ-PCO-005 — Coordination compaction without charter duplication

WHEN continuation compaction runs for a Project Coordinator profile
THE SYSTEM SHALL use the existing durable continuation operation and generation fences
AND SHALL use coordination-oriented wording that preserves the current mission, unresolved commitments, owners, blockers, evidence, corrections, authority limits, and explicit retirement or supersession facts

THE SYSTEM SHALL NOT include the charter as compaction input solely because it is the charter
AND SHALL NOT instruct the summary to reproduce the charter

WHEN the continuation executes a later turn
THE SYSTEM SHALL load the then-current charter by ProductConversation identity rather than relying on the handoff to contain it

---

### REQ-PCO-006 — No authority or lifecycle expansion

ENABLING, disabling, or editing the Project Coordinator profile SHALL NOT change the ProductConversation's WorkScope, mode, runtime role, lifecycle eligibility, tool registry, tool admission, messaging reach, or permissions

THE PROFILE SHALL NOT grant conversation or worktree creation, merge, deployment, release, publication, credential, destructive-repair, subscription, scheduler, or background-execution authority

THE SYSTEM SHALL preserve the singleton Global Coordinator's identity, routes, prompt capabilities, lifecycle, and permissions independently of this profile

---

### REQ-PCO-007 — Forward persistence behavior

WHEN a database predates the Project Coordinator profile
THE SYSTEM SHALL preserve every existing ProductConversation and treat it as not opted in

WHEN a database contains valid Project Coordinator profile rows
THE SYSTEM SHALL preserve their ProductConversation association, exact charter text, and revision during supported forward migration

THE SYSTEM SHALL follow `specs/compatibility/requirements.md` for downgrade, rollback, database replacement, and mixed-version behavior
AND SHALL NOT add a feature-specific downgrade guarantee

## Out of Scope

- Structured mission, preference, or roadmap records
- Approval objects or charter proposal workflows
- Assistant or coordinator charter self-editing
- Project hierarchies or one-coordinator-per-project rules
- Worker subscriptions, schedulers, polling, or background coordination
- New tool, WorkScope, mode, lifecycle, or permission authority
- Strong actor attestation beyond the supported-product mutation boundary
