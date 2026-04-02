# Skill System - Technical Design

## Architecture Overview

A skill is a directory containing `SKILL.md` with YAML frontmatter (metadata)
and a markdown body (prompt instructions). The system discovers skills at
startup, presents them in the UI and system prompt, and delivers their content
to the LLM when invoked.

Two invocation paths exist (user `/skill` and LLM Skill tool). Both converge
to a single delivery function that produces the same LLM-facing representation.

## SKILL.md Format (REQ-SK-001)

```
---
name: build
description: Build and test the Rust project
argument-hint: [--release]
---

# Skill body (delivered to LLM)

Build and test this project. If $ARGUMENTS contains --release, use release mode.
```

### Supported frontmatter fields

| Field | Required | Type | Purpose |
|-------|----------|------|---------|
| `name` | yes | string | Skill identifier, used in `/name` invocation |
| `description` | yes | string | One-line description for autocomplete and catalog |
| `argument-hint` | no | string | Hint text shown after `/name` in autocomplete |

### Frontmatter stripping

`parse_skill_frontmatter` extracts metadata and returns the body separately.
The body is everything after the closing `---` delimiter. The LLM receives
only the body, never the raw `---` block.

## Delivery Format (REQ-SK-002, REQ-SK-003)

When a skill is invoked, the system constructs a message with this structure:

```
Base directory for this skill: /path/to/skill-directory

[skill body with arguments substituted]
```

This content is delivered as a **user-role message** with an `is_meta: true`
flag (or equivalent marker) that distinguishes it from messages the user
actually typed. In the conversation history, it appears as a user turn but
is visually and semantically distinct.

### Message type

The `MessageContent` enum needs a variant (or flag) that represents
"system-generated user message." Options:

1. Add `is_meta: bool` to `UserContent` 
2. Add a `SkillInvocation` variant to `MessageContent`
3. Use `MessageContent::System` but deliver it in the user role to the LLM

Option 2 is cleanest for the type system -- it carries the skill name, the
expanded body, and the original arguments as typed fields. The message builder
converts it to a user-role message when constructing the LLM request.

```rust
pub enum MessageContent {
    User(UserContent),
    Agent(Vec<ContentBlock>),
    Tool(ToolContent),
    System(SystemContent),
    Error(ErrorContent),
    Continuation(ContinuationContent),
    /// Skill invocation -- delivered as a user-role message to the LLM
    /// but marked as system-generated in conversation history (REQ-SK-002)
    Skill(SkillContent),
}

pub struct SkillContent {
    /// The skill name (e.g., "build")
    pub name: String,
    /// The fully expanded skill body (frontmatter stripped, arguments
    /// substituted, base directory prepended)
    pub body: String,
    /// The original user text that triggered the invocation (for display)
    pub trigger: String,
}
```

The LLM message builder treats `SkillContent` as `MessageRole::User` with the
`body` as the text content.

## Argument Substitution (REQ-SK-004)

Processing order (important -- positional patterns must be substituted before
the bare `$ARGUMENTS` to prevent the bare replacement from corrupting indexed
variants):

1. Replace `$ARGUMENTS[N]` with the Nth whitespace-delimited argument (1-based)
2. Replace `$N` shorthand with the same
3. Replace `$ARGUMENTS` with the full argument string
4. If no `$ARGUMENTS` placeholder exists and arguments were provided, append
   `\nARGUMENTS: {args}` to the body

Arguments are split on whitespace. Quoted strings are not specially handled
(shell-level quoting is a future enhancement).

## Invocation Convergence (REQ-SK-005)

### Path 1: User types `/skill-name` (or `/skill-name args`)

1. Message expander detects `/skill-name` via `tokenize_references`
2. Validates against discovered skills (prevents false positives on file paths)
3. Calls `invoke_skill(skill_name, arguments, working_dir)`
4. Returns `ExpandedMessage` where `display_text` is the original user input
   and `llm_text` is replaced by the skill invocation message
5. The message is persisted as `MessageContent::Skill(SkillContent { ... })`

### Path 2: LLM calls Skill tool

1. Skill tool's `run()` receives `skill_name` and `args`
2. Calls the same `invoke_skill(skill_name, arguments, working_dir)`
3. Returns the skill content as `ToolOutput::success`
4. **Alternative (preferred):** The Skill tool is intercepted at the state
   machine level (like `ask_user_question`) and emits a
   `MessageContent::Skill` message instead of a tool result. This ensures
   the LLM sees the skill as a user-role message, not a tool result.

### Shared function

```rust
/// Invoke a skill: read SKILL.md, strip frontmatter, prepend base directory,
/// substitute arguments. Returns the fully expanded skill body.
pub fn invoke_skill(
    skill_name: &str,
    arguments: &str,
    working_dir: &Path,
) -> Result<SkillInvocation, ExpansionError> {
    // 1. Discover and find the skill
    // 2. Read SKILL.md
    // 3. Strip frontmatter (parse_skill_frontmatter)
    // 4. Prepend "Base directory for this skill: {skill_dir}"
    // 5. Substitute arguments
    // 6. Return SkillInvocation { name, body, skill_dir }
}
```

Both paths call this function. The difference is how the result is delivered:
Path 1 wraps it in `ExpandedMessage`, Path 2 wraps it in a tool interceptor
that emits `MessageContent::Skill`.

## Discovery (REQ-SK-006)

See `src/system_prompt.rs` `discover_skills_with_home`. Scans:
1. `.claude/skills/` and `.agents/skills/` at each directory level from CWD
   to root
2. Immediate children of CWD (for "projects directory" case)
3. `$HOME/.claude/skills/` and `$HOME/.agents/skills/` (when $HOME is not
   an ancestor of CWD)

Dedup: canonical path (symlinks), content hash (copies), name (first-seen wins).

## System Prompt Catalog (REQ-SK-007)

The system prompt includes a skills section listing discovered skills:

```
Available skills:
  /build - Build and test the Rust project [--release]
  /lint - Run Go linters [--fix]
  /docs - Generate README from project structure
```

This is metadata only -- no skill bodies are included. The LLM uses this
catalog to decide when to invoke the Skill tool or suggest `/skill` to the
user.

## Testing Strategy

### Unit Tests
- Frontmatter parsing: valid, missing name, missing description, no frontmatter
- Argument substitution: $ARGUMENTS, $1/$2, named args, no placeholder, no args
- Invocation: both paths produce identical skill body content
- Base directory prepended correctly

### Integration Tests
- User `/skill` path: message persisted as Skill type, LLM sees user-role
- LLM Skill tool path: same representation as user path
- Discovery: project skills, user skills, child directory skills, dedup

### Property Tests
- Arbitrary skill names and arguments round-trip through substitution
- Frontmatter stripping never includes `---` blocks in output
