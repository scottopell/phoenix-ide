Implement the explicitly approved lightweight Project Coordinator v1 from GitHub issue #782, authority comment https://github.com/scottopell/phoenix-ide/issues/782#issuecomment-5721894247. Scope and qualification follow that authority.


## Bounded implementation plan

- Add timeless Project Coordinator profile requirements and an ADR establishing that it is an opt-in purpose of an ordinary ProductConversation, orthogonal to the privileged Global Coordinator.
- Persist the profile and its single plain-text charter in normalized relational ProductConversation-owned storage; expose a bounded human settings mutation plus read projection, with no LLM tool/chat mutation path and no stronger actor-attestation claim.
- Load current profile/charter by stable ProductConversation identity for every fresh turn and continuation; select coordination-oriented compaction wording without copying the charter into transcript or handoff.
- Add the smallest accessible settings/editor UI with explicit opt-in/out, edit/cancel/save, dirty/failure handling, and focused persistence/API/runtime/React coverage.
- Preserve WorkScope, mode, lifecycle, registry, tools, Global Coordinator behavior, and permissions; qualify through focused tests, full `./dev.py check`, exact-head hosted CI, and fresh Codex review. No merge or deploy.


## Rebase allocation coordination

ADR-055 and migration 101 are allocated on the rebased mainline. If main gains a newer ADR or migration before publication, renumber this append-only decision/migration and its tests without changing its semantics.
