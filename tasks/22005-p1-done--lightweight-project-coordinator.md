# Lightweight Project Coordinator

Add an opt-in Project Coordinator profile to ordinary ProductConversations without changing WorkScope, mode, lifecycle, tools, or permissions.

## Scope

- Persist a normalized, revision-fenced coordinator profile/charter for ordinary ProductConversations only.
- Expose bounded HTTP/UI settings for enable, edit, disable, conflicts, and validation.
- Load the current profile into provider prompts and use Project Coordinator compaction wording without embedding the charter in transcript history.
- Preserve WorkScope, mode, lifecycle, registry, tools, Global Coordinator behavior, and permissions; qualify through focused tests, full `./dev.py check`, exact-head hosted CI, and fresh Codex review. No merge or deploy.


## Rebase allocation coordination

The profile ADR's branch-local 075 name collides with PR836's public deployment decision and is not an allocation. PR796 carries unpublished Kache decisions 076/077. Coordinate the profile's next contiguous registry slot at integration; 078 is provisional only if 075–077 have landed. Rename only the unlanded profile decision and its references, without placeholders or changes to landed decision text. Migrations 113–119 also require registry verification at integration.

## Qualification checkpoint

Task reopened: public head 3ddb8eb8 fails compilation and is not qualified. Fully paginated review inventory: 122 threads, two unresolved (tool/replay prompt cost and revision successor at JavaScript's safe-integer boundary). Preserved partial edits are being completed with lossless decimal-string revisions, generated TypeScript, and focused persistence/router/runtime/UI regressions. No merge or deployment authority.
