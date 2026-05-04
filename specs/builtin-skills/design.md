# Built-in Skills - Technical Design

## Architecture Overview

Skills today come from one place: the filesystem. `discover_skills` walks
`.claude/skills/` and `.agents/skills/` directories from CWD up to root (plus
`$HOME` and immediate children of CWD), and `invoke_skill` reads SKILL.md via
`std::fs::read_to_string(&skill.path)`.

Built-in skills add a second source: `&'static str` content compiled into the
binary. They flow through the same metadata type, the same catalog injection,
and the same expansion function. The only branch is the read step.

## Data Model

```rust
// src/system_prompt.rs
pub enum SkillSource {
    Filesystem {
        /// Absolute path to SKILL.md
        path: PathBuf,
        /// Discovery directory, e.g. ".claude/skills" or ".agents/skills"
        source_dir: String,
    },
    /// Skill is bundled with the phoenix binary; content lives in
    /// src/skills/builtin.rs
    Builtin,
}

pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub source: SkillSource,
}
```

The previous `path: PathBuf` and `source: String` fields collapse into the
enum. Callers that need the path now match on `SkillSource::Filesystem` —
this makes "code that assumes a filesystem path" structurally explicit.

## Built-in Registry

```rust
// src/skills/builtin.rs (new)
pub struct BuiltinSkill {
    pub name: &'static str,
    pub description: &'static str,
    pub argument_hint: Option<&'static str>,
    pub content: &'static str,
}

pub const CAVEMAN: BuiltinSkill = BuiltinSkill {
    name: "caveman",
    description: "Talk like caveman. Drops articles and filler. Cuts ~75% \
                  of output tokens without losing technical accuracy.",
    argument_hint: Some("[lite|full|ultra|wenyan]"),
    content: include_str!("builtin/caveman.md"),
};

pub const ALL: &[&BuiltinSkill] = &[&CAVEMAN, &CAVEMAN_COMMIT, &CAVEMAN_REVIEW];
```

`include_str!` keeps the markdown editable as a `.md` file (so syntax
highlighting, formatting, and review work normally) while still compiling
into the binary as `&'static str`. Frontmatter in the `.md` file is a
documentation aid — the registry holds the canonical metadata.

## Discovery Order and Precedence (REQ-BS-002)

`discover_skills_with_home`:

1. Walk filesystem (existing logic) — collects filesystem skills with the
   existing dedup (canonical path, content hash, name).
2. **New step**: iterate `builtin::ALL`. For each, call the existing
   `seen_names.insert(name)` guard. If the name is already taken by a
   filesystem skill, the built-in is skipped.
3. Sort by name (existing).

This matches the existing "first seen wins" semantic and keeps the rule
simple: filesystem entries are seen first, so they shadow built-ins of the
same name. No new precedence machinery.

## Invocation (REQ-BS-004)

```rust
// src/skills.rs invoke_skill
let raw_content = match &skill.source {
    SkillSource::Filesystem { path, .. } => std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read skill file: {e}"))?,
    SkillSource::Builtin => builtin::find(&skill.name)
        .map(|b| b.content.to_string())
        .ok_or_else(|| format!("Built-in skill '{}' content missing", skill.name))?,
};
```

`builtin::find(name)` is a linear scan over `ALL` (the registry is small;
trading O(N) lookup for the simplest possible registry shape). The
`ok_or_else` branch should be unreachable — a `Builtin` source variant only
exists because discovery placed it there from the registry — but the error
preserves invariant: source variant always implies a readable body.

## Catalog Rendering (REQ-BS-003)

System prompt (`build_system_prompt_with_home`):

```rust
let location = match &skill.source {
    SkillSource::Filesystem { path, .. } => format!("`{}`", path.display()),
    SkillSource::Builtin => "(built-in)".to_string(),
};
let _ = writeln!(prompt, "\n- **{}** — {} {location}", skill.name, skill.description);
```

UI (`SkillsPanel.tsx`):

```ts
function groupLabel(skill: SkillEntry): string {
  if (skill.source === "builtin") return "Built-in";
  return groupLabelFromPath(skill.path);
}
```

The `Built-in` group is sorted to appear first (or pinned, depending on the
existing sort). Built-ins are visually identical to filesystem skills inside
the panel — name, description, argument hint — only the group label differs.

## Wire Format

The Rust-side `SkillSource` enum is collapsed to flat string fields when
serialized for the HTTP API. This keeps `SkillEntry` byte-compatible with the
pre-existing wire shape (REQ-IR-005) — no breaking change for consumers — and
the additional information (`source_dir`, distinguishing built-ins) is
recoverable from the `source` value alone.

```jsonc
// Filesystem skill
{
  "name": "build",
  "description": "Build the project",
  "source": ".claude/skills",     // discovery directory
  "path": "/abs/path/SKILL.md"
}

// Built-in skill
{
  "name": "caveman",
  "description": "Talk like caveman ...",
  "source": "builtin",            // sentinel value
  "path": ""                      // empty: no on-disk SKILL.md
}
```

The mapping happens in the API handler (`src/api/handlers.rs`
`list_conversation_skills`):

```rust
match &s.source {
    SkillSource::Filesystem { path, source_dir } => {
        (source_dir.clone(), path.to_string_lossy().to_string())
    }
    SkillSource::Builtin => ("builtin".to_string(), String::new()),
}
```

The TypeScript `SkillEntry` type in `ui/src/api.ts` documents both cases.
Consumers branch on `skill.source === "builtin"` to distinguish built-ins
from filesystem skills.

## Override Test Matrix

| Filesystem has `caveman` | Registry has `caveman` | Result |
|---|---|---|
| no  | no  | not present |
| no  | yes | built-in served |
| yes | no  | filesystem served |
| yes | yes | filesystem served (built-in shadowed) |

## Out of Scope

- **Disabling built-ins via config.** Users who want caveman gone can either
  drop an empty `~/.claude/skills/caveman/SKILL.md` (filesystem shadow with
  empty body) or — if removing entirely matters — that's a separate spec.
- **MCP tool description shrink.** Same caveman ecosystem, but a different
  concern (compresses MCP descriptions, not skills) and lives in a separate
  change.
- **Statusline / stats.** Phoenix is a web UI, not a Claude Code CLI;
  caveman's stats badge doesn't apply.

## Testing Strategy

- Unit: registry lookup hit and miss; `SkillSource` serde round-trip.
- Discovery: built-ins appear when no filesystem skill exists; filesystem
  shadows built-in of same name; built-in not present means catalog excludes
  it.
- System prompt: catalog renders `(built-in)` for built-in entries and the
  path for filesystem entries.
- Invocation: `invoke_skill` returns the registry content for a built-in;
  same return shape as filesystem.
- UI: `SkillsPanel` groups built-ins under "Built-in".
