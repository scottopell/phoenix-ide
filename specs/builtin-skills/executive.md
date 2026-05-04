# Built-in Skills - Executive Summary

## Requirements Summary

Phoenix ships a small library of skills compiled into the binary as
`&'static str` constants. Built-in skills appear in the same catalog as
filesystem-discovered skills (`.claude/skills/`, `.agents/skills/`), are
invokable through the same `skill` tool and `/name` slash command, and render
in the UI skill panel under a separate "Built-in" group. A filesystem skill
with the same name as a built-in shadows the built-in.

The first built-in is `caveman` (token-efficient response style), followed by
`caveman-commit` and `caveman-review`. The mechanism is general — additional
built-ins can be added by appending to a registry constant.

## Technical Summary

`SkillMetadata.source` becomes a `SkillSource` enum with `Filesystem { path,
source_dir }` and `Builtin` variants. `discover_skills` walks the filesystem
first, then appends entries from a `BuiltinSkill` registry; existing
dedup-by-name (first-seen wins) gives filesystem precedence without new code.
`invoke_skill` matches on the source: filesystem reads via
`std::fs::read_to_string`, built-ins return their `&'static str` content
directly. The system prompt catalog renders built-ins as `(built-in)` instead
of a path. The HTTP API serializes the source variant so the UI can group
built-ins separately.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-BS-001:** Source distinction | 🔄 In progress | `SkillSource` enum on `SkillMetadata` |
| **REQ-BS-002:** Filesystem precedence on name collision | 🔄 In progress | Built-ins appended after filesystem walk; existing name dedup wins |
| **REQ-BS-003:** Catalog rendering | 🔄 In progress | `(built-in)` annotation in system prompt; `"Built-in"` group in UI |
| **REQ-BS-004:** Invocation parity | 🔄 In progress | Same `invoke_skill` signature; source-dispatched read |
| **REQ-BS-005:** Caveman speech mode skill | 🔄 In progress | Lite/full/ultra/wenyan levels |
| **REQ-BS-006:** Caveman commit + review skills | 🔄 In progress | Two additional built-ins |

## Cross-Spec References

- `specs/skills/` — defines discovery, invocation, frontmatter stripping,
  argument substitution, and the catalog. Built-in skills reuse all of these
  unchanged; only the read step is source-dispatched.
