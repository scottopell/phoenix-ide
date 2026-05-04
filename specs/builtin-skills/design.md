# Built-in Skills - Technical Design

## Architecture Overview

Built-in skills are an embedded directory tree compiled into the phoenix
binary. At server startup, the tree is materialized to a real on-disk
location (`<HOME>/.phoenix-ide/builtin-skills/<name>/`) so each built-in
becomes a normal filesystem skill from every consumer's point of view —
discovery, invocation, the system-prompt catalog, the HTTP API, and the UI
all read from disk via the existing skill machinery.

The architectural value of the disk-extraction approach: built-ins can ship
companion files (references, scripts, examples) without any new API or
abstraction. The LLM sees a `Base directory for this skill: /abs/path/`
header just like with user skills and can `cat references/foo.md` directly.

## Embedded Layout

```
src/skills/builtin/
  caveman/
    SKILL.md
  allium/
    SKILL.md
    references/
      language-reference.md
```

Each subdirectory is one skill. `SKILL.md` is required (with the standard
`name`/`description`/`argument-hint` frontmatter); any other files travel
along as companions and are visible to the LLM through the skill's base
directory.

`#[derive(rust_embed::RustEmbed)] #[folder = "src/skills/builtin/"]` embeds
the tree at compile time.

## Extraction

```rust
// src/skills/builtin.rs
pub const EXTRACT_SUBDIR: &str = "builtin-skills";

pub fn default_extract_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".phoenix-ide").join(EXTRACT_SUBDIR))
}

pub fn extract_to(target_dir: &Path) -> std::io::Result<()> {
    for path in BuiltinAssets::iter() {
        let asset = BuiltinAssets::get(&path).expect("asset must exist");
        let dest = target_dir.join(path.as_ref());
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Idempotent: skip the write when the file already matches.
        let needs_write = match std::fs::read(&dest) {
            Ok(existing) => existing != asset.data.as_ref(),
            Err(_) => true,
        };
        if needs_write {
            std::fs::write(&dest, asset.data.as_ref())?;
        }
    }
    Ok(())
}
```

Called from `main.rs` once the data directory exists. Failure is non-fatal:
extraction errors are logged at `warn!` and the server continues — built-ins
simply won't appear in the catalog.

The extraction is intentionally additive — it does not delete files that
were present in a prior phoenix version but removed in this one. This keeps
the operation race-safe across concurrent phoenix processes (dev worktree +
prod), at the cost of leaving stale files when a built-in is renamed or
removed. Operators can wipe `~/.phoenix-ide/builtin-skills/` to reset.

## Data Model

```rust
// src/system_prompt.rs
pub enum SkillSource {
    Filesystem {
        /// Absolute path to the SKILL.md file
        path: PathBuf,
        /// Discovery directory, e.g. ".claude/skills" or ".agents/skills"
        source_dir: String,
    },
    /// Skill is bundled with the phoenix binary. The path points at the
    /// extracted SKILL.md under <HOME>/.phoenix-ide/builtin-skills/<name>/.
    Builtin { path: PathBuf },
}

pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub source: SkillSource,
}
```

The variant is a *tag*, not a content discriminator. Both variants carry a
real path. Helpers on `SkillMetadata` (`skill_dir`, `skill_md_path`,
`display_location`) hide the distinction from most callers.

## Discovery Order and Precedence (REQ-BS-002)

`discover_skills_with_options(working_dir, home_override, builtin_dir)`:

1. Walk filesystem from `working_dir` → root, scanning `SKILL_DIRS` at each
   level. Existing dedup (canonical path, content hash, name).
2. Scan immediate children of `working_dir` (projects-directory case).
3. Scan `$HOME/.claude/skills/` and `$HOME/.agents/skills/` if not visited.
4. **Scan `builtin_dir`** (when `Some`) via `collect_builtin_skills_from_dir`.
   Reuses the same dedup state, so any name already collected from the user
   filesystem shadows the built-in.
5. Sort by name.

Test seam: pass `builtin_dir = None` to assert filesystem-only behavior.
Production goes through `discover_skills(working_dir)` which uses
`default_extract_dir()`.

## Invocation (REQ-BS-004)

```rust
// src/skills.rs invoke_skill
let raw_content = std::fs::read_to_string(skill.skill_md_path())
    .map_err(|e| format!("Failed to read skill '{skill_name}': {e}"))?;
```

Built-ins and filesystem skills share one read path because both have real
paths. The `Base directory for this skill: <skill_dir>` header given to the
LLM points at the real on-disk directory in either case, so companion-file
reads work identically.

## Catalog Rendering (REQ-BS-003)

System prompt (`build_system_prompt_with_options`):

```rust
let location = skill.display_location();
//   Filesystem -> "(`/abs/path/SKILL.md`)"
//   Builtin    -> "(built-in)"
let _ = writeln!(prompt, "\n- **{}** — {} {location}", skill.name, skill.description);
```

The catalog deliberately hides the extracted path for built-ins to keep the
catalog terse. The LLM still has a real path available via the skill's
`Base directory` line when the skill is invoked.

UI (`SkillsPanel.tsx`):

```ts
function groupLabel(skill: SkillEntry): string {
  if (skill.source === "builtin") return "Built-in";
  return groupLabelFromPath(skill.path);
}
```

The `Built-in` group is reordered to the front of the Map after grouping so
phoenix-bundled skills appear above user-installed ones.

## Wire Format

`SkillEntry` is a flat shape — no breaking change from the pre-existing wire
format. The Rust-side `SkillSource` enum is collapsed to flat string fields
in the API handler:

```jsonc
// Filesystem skill
{
  "name": "build",
  "description": "Build the project",
  "source": ".claude/skills",
  "path": "/abs/path/SKILL.md"
}

// Built-in skill
{
  "name": "caveman",
  "description": "Talk like caveman ...",
  "source": "builtin",
  "path": "/home/user/.phoenix-ide/builtin-skills/caveman/SKILL.md"
}
```

Mapping in `src/api/handlers.rs` `list_conversation_skills`:

```rust
match &s.source {
    SkillSource::Filesystem { path, source_dir } => {
        (source_dir.clone(), path.to_string_lossy().to_string())
    }
    SkillSource::Builtin { path } => {
        ("builtin".to_string(), path.to_string_lossy().to_string())
    }
}
```

The TypeScript `SkillEntry` in `ui/src/api.ts` documents both cases.
Consumers branch on `skill.source === "builtin"` to distinguish. Note the
`path` is now always populated — the SkillViewer fetches body via the
existing `/api/files/read?path=...` endpoint regardless of source. There is
no built-in-specific HTTP endpoint.

## Override Test Matrix

| Filesystem has `caveman` | Extract dir has `caveman` | Result |
|---|---|---|
| no  | no  | not present |
| no  | yes | built-in served |
| yes | no  | filesystem served |
| yes | yes | filesystem served (built-in shadowed) |

## Out of Scope

- **Disabling built-ins via config.** Workaround: drop an empty filesystem
  `SKILL.md` to shadow.
- **Pruning stale extracts.** Renamed/removed built-ins leave their files in
  the extract dir. Manual cleanup via `rm -rf ~/.phoenix-ide/builtin-skills/`.
- **Per-version isolation.** Multiple phoenix binaries sharing one `$HOME`
  will overwrite each other's extracted files at startup. Acceptable today;
  revisit if needed.

## Testing Strategy

- Unit: `extract_to` writes files, is idempotent, restores tampered files.
- Discovery: built-in appears when no filesystem skill exists; filesystem
  shadows built-in of same name; coexistence when names differ.
- System prompt: catalog renders `(built-in)` for built-in entries and the
  path for filesystem entries; the extract path does not leak into the
  catalog line.
- Invocation: `invoke_skill` reads from the extracted path for built-ins
  (verified end-to-end with the real registry — confirms caveman + allium
  are present).
- Companion-file access: the allium reference file is on disk after
  extraction (the value-prop test for the disk-extraction design).
