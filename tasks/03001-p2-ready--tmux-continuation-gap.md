---
created: 2025-05-02
priority: p2
status: ready
artifact: src/tools/tmux/registry.rs
---

## tmux session not transferred on conversation continuation

When a conversation is continued (context exhaustion → `awaiting_continuation` → child conversation), the worktree transfers cleanly because `worktree_path`, `branch_name`, etc. are explicitly copied to the child. The tmux session does **not** transfer.

Root cause: the tmux socket path is deterministically keyed to `conversation_id`:
```
~/.phoenix-ide/tmux-sockets/conv-{conversation_id}.sock
```
The continuation gets a new `conversation_id`, so it resolves to a different socket path. On first tmux use, it lazily spawns a fresh empty session — losing all shell state, running processes, and history from the parent.

The parent's tmux server (at `conv-{parent-id}.sock`) remains alive until the parent is hard-deleted.

**Related bug:** Sub-agents currently receive `TmuxTool` (it's included in `ToolRegistry::for_subagent_explore()` at `src/tools.rs:411`). Sub-agents should not get tmux tool access, nor have a socket allocated for them — the tmux session belongs to the worktree's owning conversation, not its sub-agents.

Relevant code:
- `src/tools/tmux/registry.rs` — `socket_path_for()`, `ensure_live()`, `cascade_on_delete()`
- `src/tools.rs` — `for_subagent_explore()`, `for_subagent_work()`
- `src/db.rs` (around line 621) — continuation field inheritance
- `specs/tmux-integration/tmux-integration.allium` — authoritative behavioral spec
- `specs/bedrock/bedrock.allium` — REQ-BED-030, continuation semantics

---

### Preferred solution: Option E — Worktree-scoped tmux sessions

Derive the socket path from the **worktree path** rather than `conversation_id`. The worktree is the logical unit that travels forward through continuations — it IS the coding environment, and the tmux session IS that environment's shell state. Tying the socket key to the worktree path makes the invariant "one session per coding environment" correct by construction: no continuation machinery needs to explicitly transfer anything, because the key doesn't change.

Socket path derivation: hash the worktree path into a filesystem-safe name, e.g.:
```
~/.phoenix-ide/tmux-sockets/wt-{sha256_prefix(worktree_path)}.sock
```

**Worktree path by mode:**
- **Work / Branch**: `conv_mode.worktree_path()` — explicit typed field
- **Explore**: `conversation.cwd` — REQ-PROJ-028 guarantees every Explore conversation has a worktree; cwd is set to the worktree path and is immutable
- **Direct**: no worktree → falls back to `conv-{id}.sock` (Direct conversations have no coding environment to scope to)

**Implications:**
- `socket_path_for()` takes a worktree path (or falls back to conversation_id for Direct)
- Continuation conversations automatically attach to the parent's tmux session with no explicit handoff logic
- Sub-agents share the parent's worktree but must not have tmux access — remove `TmuxTool` from `for_subagent_explore()` (and therefore `for_subagent_work()`)
- `cascade_on_delete` must only kill the tmux server if the conversation currently owns the worktree (i.e. not superseded by a continuation)
- Spec update required: `specs/tmux-integration/tmux-integration.allium` entity key changes from `conversation_id` to `worktree_path`

---

### Alternative options (for reference)

**Option A — Rename socket file at continuation time**
Rename `conv-{parent_id}.sock` → `conv-{child_id}.sock` inside `create_continuation()`. The server holds the socket by inode; renaming the path is transparent to it.
- ✅ No DB migration; handled in one place
- ⚠️ Parent's `cascade_on_delete` unlink becomes a silent 404 (already best-effort, but still wrong)
- ⚠️ Doesn't fix the conceptual mismatch — socket is still keyed to a conversation, not the environment

**Option B — Symlink child path → parent path**
Create `conv-{child_id}.sock` as a symlink to `conv-{parent_id}.sock` at continuation time.
- ✅ No DB migration; parent path stays valid for cleanup
- ⚠️ Symlink-to-socket portability needs verification
- ⚠️ Same conceptual mismatch as Option A

**Option C — `tmux_socket_path` column on conversations**
Nullable override column; continuation copies parent's socket path. `socket_path_for()` checks it first.
- ✅ Explicit and auditable
- ⚠️ Requires migration + serde/schema changes
- ⚠️ `cascade_on_delete` needs shared-path reference counting

**Option D — Chain-walk in `ensure_live`**
On NoSocket, walk up `parent_conversation_id` chain to find a live ancestor socket.
- ✅ No schema changes
- ⚠️ Unbounded chain walk; `cascade_on_delete` could kill a socket in use by descendant

---

### Acceptance criteria

- `TmuxTool` removed from `for_subagent_explore()` (and therefore `for_subagent_work()`)
- `socket_path_for()` keyed to worktree path for Work/Branch/Explore conversations; falls back to conversation_id for Direct
- Continuation conversations automatically attach to the parent's tmux session with no explicit handoff logic
- `cascade_on_delete` only kills the server if the conversation currently owns the worktree (not superseded by a continuation)
- `specs/tmux-integration/tmux-integration.allium` updated to reflect worktree-scoped session semantics
