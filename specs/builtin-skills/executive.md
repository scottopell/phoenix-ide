# Built-in Skills - Executive Summary

## Requirements Summary

Phoenix ships a small library of skills compiled into the binary as an
embedded directory tree. At server startup, the tree is materialized to
`<HOME>/.phoenix-ide/builtin-skills/<name>/` so each built-in becomes a real
filesystem skill: same path semantics, same companion-file workflow, same
`Base directory` behavior. Built-ins flow through the existing skill
discovery, catalog, invocation, and UI surfaces. A filesystem skill of the
same name (e.g. `~/.claude/skills/spears/`) shadows the built-in.

The first batch ships:

- **`/allium`** — formal behavioral specification language, with the canonical
  `references/language-reference.md` shipped alongside the SKILL.md.
- **`/spears`** — spEARS requirements-driven development workflow, with
  workflow-specific reference files shipped alongside the SKILL.md.
- **`phoenix-api`** — Global Coordinator-only non-exhaustive best-practice recipes
  for user-authorized actions through supported public Phoenix HTTP APIs and scoped Bash.

The mechanism is general — additional built-in skills are added by dropping
a directory under `src/skills/builtin/` and committing.

## Technical Summary

`src/skills/builtin/` is embedded into the binary via `rust_embed::RustEmbed`.
At startup, `crate::skills::builtin::extract_to(&target_dir)` writes every
embedded file to `target_dir/<skill>/<...>` (idempotent — only rewrites files
whose contents differ). Default target is `<HOME>/.phoenix-ide/builtin-skills/`.

`SkillSource` is an enum: `Filesystem { path, source_dir }` for user-installed
skills; `Builtin { path }` for extracted built-ins. Ordinary conversation
skills use their filesystem source path. The Coordinator uses a distinct
path-free catalog authenticated directly from immutable embedded bytes, then
invokes the embedded skill and companion references without reading the
extraction cache.

Discovery scans the user's `.claude/skills/` and `.agents/skills/` first,
then the built-in extract directory. The existing name-dedup ("first seen
wins") gives the user override for free. Extracted directories whose names
are no longer embedded in the current binary are ignored, so removed built-ins
do not remain visible after an upgrade.

Audience metadata defaults to ordinary conversations. The `global-coordinator`
audience is honored only for Phoenix built-ins: Coordinator discovery parses
immutable embedded definitions without scanning the filesystem, and the
audience-bound Skill tool enforces the same boundary during invocation.
Filesystem skills cannot self-promote into the Coordinator catalog. Invocation
reads embedded instructions and companion references directly and marks the
result as trusted built-in skill content; it never trusts the mutable extraction
cache. The transcript and UI receive only an authenticated-delivery receipt;
while the live runtime retains the authenticated payload, its bytes are overlaid
into the latest matching tool result during provider-request assembly.

Extraction prunes unexpected files inside built-in directories that remain
bundled. Directories for removed or renamed built-ins may remain on disk but are
excluded by the embedded-name allowlist. Phoenix has no disable configuration;
a valid filesystem skill with the same name replaces an ordinary built-in by
normal precedence, while an invalid or empty definition does not disable it.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BS-001:** Source distinction | ✅ Complete | `SkillSource` enum tags filesystem vs built-in |
| **REQ-BS-002:** Filesystem precedence on name collision | ✅ Complete | Built-in scan runs after filesystem walk; name dedup wins |
| **REQ-BS-003:** Catalog rendering | ✅ Complete | `(built-in)` annotation in system prompt; `"Built-in"` group in UI |
| **REQ-BS-004:** Invocation parity | ✅ Complete | Filesystem skills invoke from their readable source path; all built-ins expand immutable embedded definitions, while authenticated Coordinator invocation uses path-free embedded metadata and includes embedded companion references |
| **REQ-BS-005:** spEARS workflow skill | ✅ Complete | `spears/SKILL.md` + workflow references extracted at startup |
| **REQ-BS-006:** Allium with companion files | ✅ Complete | `allium/SKILL.md` + `allium/references/language-reference.md` extracted at startup |
| **REQ-SK-008:** Runtime audience binding | ✅ Complete | `phoenix-api` is cataloged and invocable only by the Global Coordinator; visible/persisted results are an opaque receipt, while live-runtime authenticated bytes are overlaid during provider-request assembly |
| **Coordinator API recipes** | Implemented; focused qualification recorded below | Non-exhaustive public-API guidance preserves destination and scoped execution boundaries; creation and structured-question response recipes distinguish idempotency from freshness and acceptance from execution |

## Non-exhaustive API guidance qualification

ADR-113 records the recipe policy. The generated Coordinator suffix, embedded skill/reference, and normative guidance allow other supported public APIs grounded in inspected current deployment/source contracts and actual user authority. Source inspection alone does not establish deployed behavior or permission. Existing destination authentication, TLS, nonlocal encrypted transport, credential handling, active WorkScope, immutable identity/intent, fresh-read, and public-server-enforcement boundaries remain intact. Gap inventories are illustrative and qualified by inspected deployment/source, permitting verified supported alternatives and bounded state reads within user authority, not invented guarantees/receipts, database/private-handler bypasses, or background monitors.

The question recipe is verified against `respond_to_question`, `RespondToQuestionPayload`, `validate_question_request_identity`, the `UserQuestionResponse` transition, and `QuestionPanel.buildAnswerMap`. It permits exact user-provided answers or decisions made within explicit delegated decision scope for every question; generic inspect/unblock instructions are not decision authority and approvals must never be fabricated or delegation extended. It requires complete answers, exact full question keys and labels/custom text, current opaque `request_id` (not `tool_use_id`), comma-space multi-select strings, one POST and original-target observation. The backend does not enforce completeness. Request identity is freshness, not idempotency; uncertain outcomes are not automatically replayed and 409 is not success. Optional notes are consumed; submitted annotation previews are not.

Focused Rust regressions in `builtin.rs`, the catalog metadata expectation, `system_prompt.rs` and `continuation.rs` are authored but not executed locally under the no-build constraint. Offline drift tests use the existing Python unittest discovery harness; they validate recipe JSON and source anchors without calling a live API. No live question was read or answered, and no merge/deployment or current-instruction activation is implied.

## Public literal-chat recipe qualification

The embedded `phoenix-api` recipe and `PhoenixApiDestinationAuthority` guidance require `GET /api/version` to return complete HTTP-200 JSON with exact boolean `chat_literal_text_v1: true` from the selected authenticated verified-TLS destination without redirects before any literal POST or exact retry. They use only `/api/conversations/{original_transcript_id}/chat/literal` with explicit `expansion_policy: "literal_text"`, never `/chat` fallback. The dedicated route closes the historical unknown-field-ignore race without granting live-replacement compatibility (ADR-118, REQ-API-030, REQ-COMP-015).

Missing/false/malformed support stops before POST. Original transcript/message identity and intent, destination authority, active WorkScope, normal lifecycle/auth/attachment guards, and local-only `send_conversation_message` boundaries remain intact. Offline drift and E2E harness self-tests qualify the recipe's JSON/source correspondence and zero-POST preflight failures; real Rust router/application and mock-provider E2E execution remain pending under the no-build constraint. No live request, credential operation, deployment or instruction activation is implied.

## Cross-Spec References

- `specs/skills/` — defines discovery, invocation, frontmatter stripping,
  argument substitution, and the catalog. Built-ins reuse all of it.
