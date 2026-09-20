# Lightweight Project Coordinator

Add an opt-in Project Coordinator profile to ordinary ProductConversations without changing WorkScope, mode, lifecycle, tools, or permissions.

## Scope

- Persist a normalized, revision-fenced coordinator profile/charter for ordinary ProductConversations only.
- Expose bounded HTTP/UI settings for enable, edit, disable, conflicts, and validation.
- Load the current profile into provider prompts and use Project Coordinator compaction wording without embedding the charter in transcript history.
- Preserve WorkScope, mode, lifecycle, registry, tools, Global Coordinator behavior, and permissions; qualify through focused tests, full `./dev.py check`, exact-head hosted CI, and fresh Codex review. No merge or deploy.


## Rebase allocation coordination

ADR-059 and migrations 102 through 108 are allocated on the rebased mainline. ADR-059 supersedes ADR-049's bounded exclusions on ordinary coordination profiles, normalized profile persistence, and profile-selected compaction wording while preserving its protected-handoff and durable-operation decisions. If main gains newer ADRs or migrations before publication, renumber this append-only decision/migration set and its tests without changing its semantics.
