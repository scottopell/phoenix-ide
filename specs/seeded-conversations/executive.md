# Seeded Conversations — Executive Summary

## Requirements Summary

A seeded conversation is one created by a UI action that hands the user a ready-to-go sub-conversation with a pre-filled draft prompt. The two driving examples: (1) the terminal panel detects missing shell integration and offers to set it up — the work needs to happen in `$HOME` with a specific prompt that the current conversation already understands; (2) the tasks panel offers a "Start working" action that opens a fresh conversation in the project root with the task content pre-loaded for review. Without this feature, the user manually creates a conversation, picks a directory, remembers the context, and types a prompt — all of which the spawning UI already knows.

The transparency contract has three guarantees: the user can always tell where a conversation came from (parent breadcrumb), what was pre-filled and whether they submitted it (review-first — the draft hydrates the input but never auto-submits), and how to get back to the conversation that spawned it (breadcrumb link + browser back, both work).

## Technical Summary

Four REQs cover the surface:

- **REQ-SEED-001 (Pre-Filled Draft):** the spawning UI writes the draft prompt to `localStorage` under key `seed-draft:<conversation-id>` *before* navigating to the new conversation. On mount, the conversation page hydrates the input from that key and clears the key after read so re-visits don't re-hydrate. No backend persistence — drafts are ephemeral UI state.
- **REQ-SEED-002 (Caller-Specified Mode + Auto):** the spawn endpoint accepts `mode = "direct" | "managed" | "auto"`. Explicit values bypass detection; `"auto"` walks up from the target cwd looking for `.git` and resolves to `managed` (git repo found) or `direct` (no repo). The resolved mode is returned in the response so the UI renders consistently. No new trust boundary — seeded conversations inherit the same access checks as user-created ones.
- **REQ-SEED-003 (Parent Link Breadcrumb):** a new nullable column `seed_parent_id` on `conversations` records the spawning conversation. The API response includes `seed_parent_id` and a server-resolved `seed_parent_slug` for the breadcrumb link target. The UI renders a `.conversation-seed-breadcrumb` element that links back; if the parent has been deleted, the breadcrumb degrades to unlinked text.
- **REQ-SEED-004 (Seed Label):** a second nullable column `seed_label` carries a short, human-readable string the spawner provides ("Shell integration setup (zsh)", "Task: refactor auth middleware"). Display-only — does not affect routing, lifecycle, or runtime behaviour.

There is no lifecycle coupling between parent and seeded conversation. The spawned conversation runs independently; the parent does not observe its progress; no event propagates between them. The link is purely decorative + navigational.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-SEED-001:** Conversation Creation with Pre-Filled Draft Prompt | ✅ Complete | UI hydrator at `ui/src/pages/ConversationPage.tsx:412-422`; spawners write key at `ConversationPage.tsx:706` and `ui/src/components/TaskViewer.tsx:101`; clear-after-read so revisits show empty input |
| **REQ-SEED-002:** Caller-Specified Mode, No New Access Checks | ✅ Complete | Auto resolution at `crates/phoenix-ide/src/api/handlers.rs:610-619` (walks up looking for `.git`); explicit `direct`/`managed` pass through unchanged; `resolved_mode` surfaced in response |
| **REQ-SEED-003:** Parent Link for UI Breadcrumb | ✅ Complete | Schema migration `crates/phoenix-ide/src/db.rs:164-169` (`seed_parent_id` column); accessor + slug resolution `runtime.rs:296`; UI styling at `ui/src/index.css:8149-8170`; API exposure at `ui/src/api.ts:73-78` |
| **REQ-SEED-004:** Seed Label | ✅ Complete | Schema migration `db.rs:166-169` (`seed_label` column); persisted via `db.rs:398-428` insert path; surfaced alongside breadcrumb |

**Progress:** 4 of 4 complete.

## Out of Scope (per requirements.md, still deferred)

- Backend-persistent draft prompts — localStorage continues to be sufficient
- Spawn-result notifications or lifecycle coupling between parent and child
- Capability restrictions on spawned conversations (they inherit normal trust)
- Multi-parent / DAG-shaped conversation relationships
- Exposing "spawn" as an LLM-callable tool (this remains a user-initiated UI action only)

## Cross-Spec Relationships

- **`specs/terminal/`**: REQ-TERM-020 (shell integration setup spawner) is the canonical first user of the seed mechanism. The terminal panel builds the seed prompt at `ui/src/components/TerminalPanel.tsx:176,757` and uses `mode = "direct"` for `$HOME`.
- **`specs/projects/`**: the tasks panel uses seeded conversations for "Start working on this task" with `mode = "managed"` in the project root.
- **`specs/bedrock/`**: a seeded conversation is just a conversation — bedrock's state machine is unchanged. The seed columns are decorative metadata, not bedrock state.
