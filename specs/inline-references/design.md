# Inline References - Technical Design

## Architecture Overview

Inline references are trigger patterns the user types in the message input that either (a) expand to inject content before the message reaches the LLM, or (b) complete a path and leave it as literal text for the agent to act on. The two behaviors are structurally distinct and intentionally so:

| Trigger | Behavior | Goes through `MessageExpander`? |
|---|---|---|
| `@path/to/file` | File contents injected into LLM message | Yes |
| `/skill-name` | Skill instructions injected into LLM message | Yes |
| `./path/to/file` | Path left as literal text; agent reads as needed | No |

For expansion references (`@`, `/`), a `MessageExpander` layer in `send_chat` resolves them before emitting `Event::UserMessage`, producing `display_text` (stored, shown in history) and `llm_text` (delivered to LLM). For path references (`./`), autocomplete assists the user in typing a correct path, but no server-side transformation occurs at send time.

## Data Model

### REQ-IR-006 Implementation

```rust
struct ExpandedMessage {
    /// Delivered to the LLM (fully resolved)
    llm_text: String,
    /// Stored in DB and shown in conversation history (original shorthand)
    display_text: String,
}
```

`display_text` is what arrives in `send_chat` from the client. `llm_text` is what gets threaded into `Event::UserMessage`. The state machine and DB see only `display_text`; the LLM message-building path uses `llm_text`.

## Expansion Types

### REQ-IR-001 Implementation: File Reference (`@path`)

Syntax: `@path/to/file` anywhere in message text.

Resolution:
1. Scan message text for `@`-prefixed tokens
2. Resolve each path relative to the conversation's `working_dir`
3. Read file contents
4. Replace the token in `llm_text` with a structured block:

```
<file path="src/main.rs">
[file contents]
</file>
```

File lookup uses `working_dir` from the conversation record. Paths may be absolute or relative. Symlinks are followed. Binary files (detected by `is_text_file` flag in the existing file API) are rejected with a user-visible error.

### REQ-IR-002 and REQ-IR-003 Implementation: Skill Reference (`/skill-name`)

Syntax: `/skill-name` at the start of the message, optionally followed by additional text.

Resolution:
1. Detect `/` prefix
2. Extract skill name (first token after `/`) and arguments string (remainder)
3. Call `discover_skills(working_dir)` — already implemented in `src/system_prompt.rs`
4. Read the matched skill's `SKILL.md` file content
5. Perform `$ARGUMENTS` substitution (REQ-IR-003):
   - Replace `$ARGUMENTS` with the full arguments string
   - Replace `$ARGUMENTS[N]` / `$N` with individual whitespace-split tokens
   - If no placeholder present but arguments provided: append `ARGUMENTS: <value>`
   - If no arguments provided: load SKILL.md unmodified
6. Result becomes `llm_text`

Skill loading is **always context loading first**. Argument substitution is additive.

## API Contracts

### REQ-IR-004 and REQ-IR-005 Implementation: Autocomplete Endpoints

Autocomplete candidates are a pure function of a **resolution root** — the same
root the first message's references expand against. `ResolutionRoot` is the
single type that answers "where do `@file` / `./path` / `/skill` resolve":

- `WorkingDir(dir)` — read the live filesystem. Used by Direct conversations and
  by every in-conversation composer (the conversation already resolves against
  its own `cwd`).
- `GitTree { repo_root, reference }` — read a branch's *committed tree* via
  `git ls-tree` / `git cat-file`, with no worktree required. Used by
  branch/managed workflows, whose first message is expanded against a fresh
  worktree of the chosen branch — i.e. that branch's committed tree. Skill
  discovery materializes the ref's `.claude/skills` / `.agents/skills`
  `SKILL.md` files into a temporary directory so the filesystem-based
  `discover_skills` / `invoke_skill` run unchanged.

A single constructor, `ResolutionRoot::for_create(cwd, mode, base_branch)`,
builds the root from a conversation's creation parameters. **Both** the composer
autocomplete endpoints and create-time `message_expander::expand` construct the
root through it, so the candidate set offered to the user and the set the first
message expands against cannot diverge. (This closes the failure mode where
`cwd`-based discovery disagreed with worktree-based expansion: a file present in
the working directory but absent from the branch's committed tree — uncommitted
or untracked — is no longer offered, because it is not in the tree the worktree
will check out.)

Each discovery endpoint has two forms: a conversation-scoped form that reads the
root from the conversation record (`WorkingDir(conversation.cwd)`), and a
directory-scoped form that takes the creation parameters as query params and
calls `for_create`. The directory-scoped form serves the new-conversation
composer, which has chosen a directory + workflow but not yet created a
conversation.

**Skill discovery:**
```
GET /api/conversations/:id/skills                         (conversation-scoped)
GET /api/skills?cwd=<dir>&mode=<m>&base_branch=<b>        (directory-scoped)
Response: { skills: [{ name, description, argument_hint | null, source, path }] }
```
Discovers skills against the resolution root. `argument_hint` comes from the
`argument-hint` frontmatter field in `SKILL.md`. For a `GitTree` root, `path` is
the ref-relative `SKILL.md` location (not an ephemeral materialization path).
`mode`/`base_branch` are absent for Direct (the root is `cwd`).

**File search:**
Reuse the existing `GET /api/files/list?path=<dir>` for directory-level browsing. For fuzzy search across the full tree:
```
GET /api/conversations/:id/files/search?q=<query>&limit=<n>                  (conversation-scoped)
GET /api/files/search?cwd=<dir>&q=<query>&limit=<n>&mode=<m>&base_branch=<b> (directory-scoped)
Response: { items: [{ path, is_text_file }] }
```
A `WorkingDir` root walks the directory with the `ignore` crate (gitignore-aware);
a `GitTree` root lists the ref's committed paths via `git ls-tree`. Both cap at
`limit` (default 50) and fuzzy-match on path components with the same scorer.

## Frontend: Inline Autocomplete

### REQ-IR-004 Implementation (shared file picker)

The autocomplete engine lives in the `useInlineReferences` hook, scoped to a
resolution root rather than a conversation. Both the in-conversation composer
(`InputArea`) and the new-conversation composer consume the hook; this is what
makes inline references work in the new-conversation composer before any
conversation exists.

The hook takes `cwd` plus the workflow's `mode` / `baseBranch`, which it forwards
to the directory-scoped endpoints so discovery resolves against the workflow's
`GitTree` (branch/managed) or `WorkingDir` (Direct) root. The new-conversation
composer passes `conv.submission.{mode, baseBranch}` — the *same* mapping the
create call uses — so what the user is offered and what the first message
expands against are constructed from one source. An in-conversation composer
leaves `mode`/`baseBranch` unset: its conversation already resolves against its
own `cwd`. Because candidates depend on the directory *and* the ref, the skill
cache and the in-flight staleness guards key on a composite (cwd + mode + branch)
so switching workflow or branch refetches against the new root, and a late
response from a previous root is discarded.

Both `@` and `./` triggers open the same `InlineAutocomplete` component. Trigger detection inspects the text around the cursor on each `onChange`:

- `@<partial>` anywhere in the text → open in `expand` mode
- `./` followed by any characters anywhere in the text → open in `path` mode
- `/^<partial>` at start of text → open in `skill` mode (REQ-IR-005)

The component is mode-agnostic: it receives items and a query, renders a filtered fuzzy-matched list, and calls back with the selected value. On selection:

- `expand` mode: inserts `@resolved/path` — which will be expanded at send time
- `path` mode: inserts `./resolved/path` — plain text, no send-time processing
- `skill` mode: inserts `/skill-name ` with `argument_hint` as ghost text

### REQ-IR-008 Implementation (path reference, no expansion)

`./` completion is purely a frontend assist. No backend call is made at send time. The inserted `./path/to/file` string travels to the LLM as-is. The file search endpoint is used identically to the `@` autocomplete — the only difference is what gets inserted and what happens at send time (nothing).

Because there is no expansion, REQ-IR-007 error handling does not apply. The agent receives the path and is responsible for reading it.

## Error Handling

### REQ-IR-007 Implementation

Validation happens in `send_chat` before emitting `Event::UserMessage`. The `MessageExpander::expand()` function returns:

```rust
enum ExpansionResult {
    Ok(ExpandedMessage),
    Err(ExpansionError),
}

enum ExpansionError {
    FileNotFound { path: String },
    FileNotText { path: String },
    SkillNotFound { name: String, available: Vec<String> },
}
```

Errors map to HTTP 422 responses with a structured body the frontend surfaces inline (not a toast — inline error in the input area, blocking send).

## Expansion Order

When a message contains both a `/skill-name` prefix and `@file` references, skill expansion runs first (it transforms the full message body), then file references are resolved within the resulting text. This allows skill content that itself contains `@`-references, though that is an edge case.

## Non-Goals (tracked in Task 571)

`disable-model-invocation`, `user-invocable`, `context: fork`, `agent:` subagent selection, and `!\`command\`` dynamic injection are not covered by this spec.
