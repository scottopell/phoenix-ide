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

The mechanism is general — additional built-in skills are added by dropping
a directory under `src/skills/builtin/` and committing.

## Technical Summary

`src/skills/builtin/` is embedded into the binary via `rust_embed::RustEmbed`.
At startup, `crate::skills::builtin::extract_to(&target_dir)` writes every
embedded file to `target_dir/<skill>/<...>` (idempotent — only rewrites files
whose contents differ). Default target is `<HOME>/.phoenix-ide/builtin-skills/`.

`SkillSource` is an enum: `Filesystem { path, source_dir }` for user-installed
skills; `Builtin { path }` for extracted built-ins. Both variants carry a real
filesystem path, so `invoke_skill`, the system-prompt catalog, the HTTP API,
and the UI panel all use one read path. The variant exists so the catalog
can render `(built-in)` and the UI can group built-ins separately, but no
component branches on it for content access.

Discovery scans the user's `.claude/skills/` and `.agents/skills/` first,
then the built-in extract directory. The existing name-dedup ("first seen
wins") gives the user override for free. Extracted directories whose names
are no longer embedded in the current binary are ignored, so removed built-ins
do not remain visible after an upgrade.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BS-001:** Source distinction | ✅ Complete | `SkillSource` enum tags filesystem vs built-in |
| **REQ-BS-002:** Filesystem precedence on name collision | ✅ Complete | Built-in scan runs after filesystem walk; name dedup wins |
| **REQ-BS-003:** Catalog rendering | ✅ Complete | `(built-in)` annotation in system prompt; `"Built-in"` group in UI |
| **REQ-BS-004:** Invocation parity | ✅ Complete | Single `read_to_string` path; both sources use real disk paths |
| **REQ-BS-005:** spEARS workflow skill | ✅ Complete | `spears/SKILL.md` + workflow references extracted at startup |
| **REQ-BS-006:** Allium with companion files | ✅ Complete | `allium/SKILL.md` + `allium/references/language-reference.md` extracted at startup |

## Cross-Spec References

- `specs/skills/` — defines discovery, invocation, frontmatter stripping,
  argument substitution, and the catalog. Built-ins reuse all of it.
