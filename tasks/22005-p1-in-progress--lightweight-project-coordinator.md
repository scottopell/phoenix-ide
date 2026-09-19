# Lightweight Project Coordinator

Add an opt-in Project Coordinator profile to ordinary ProductConversations without changing WorkScope, mode, lifecycle, tools, or permissions.

## Scope

- Persist a normalized, revision-fenced coordinator profile/charter for ordinary ProductConversations only.
- Expose bounded HTTP/UI settings for enable, edit, disable, conflicts, and validation.
- Load the current profile into provider prompts and use Project Coordinator compaction wording without embedding the charter in transcript history.
- Preserve WorkScope, mode, lifecycle, registry, tools, Global Coordinator behavior, and permissions; qualify through focused tests, full `./dev.py check`, exact-head hosted CI, and fresh Codex review. No merge or deploy.


## Rebase allocation coordination

ADR-056 and migration 101 are allocated on the rebased mainline. ADR-056 supersedes ADR-049's bounded exclusions on ordinary coordination profiles, normalized profile persistence, and profile-selected compaction wording while preserving its protected-handoff and durable-operation decisions. If main gains a newer ADR or migration before publication, renumber this append-only decision/migration and its tests without changing its semantics.
