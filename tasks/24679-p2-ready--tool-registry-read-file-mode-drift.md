---
created: 2026-04-14
priority: p2
status: ready
artifact: src/tools.rs
---

# `read_file` missing from Direct / Work tool registry

## Problem

`ReadFileTool` is registered in some `ToolRegistry` constructors but not
others. In particular it is absent from `new_with_options(..)`, which
powers `ToolRegistry::direct()` and (via `explore_with_sandbox()`) the
sandboxed Explore registry.

```
src/tools.rs:204  explore_no_sandbox()        → has ReadFileTool   ✓
src/tools.rs:247  direct()                    → new_with_options() ✗
src/tools.rs:296  new_with_options(..)        → no ReadFileTool    ✗
src/tools.rs:255  for_subagent_explore()      → has ReadFileTool   ✓
src/tools.rs:283  for_subagent_work()         → ← explore()        ✓
```

Concretely, a conversation in **Direct mode** that asks the LLM to call
`read_file` gets back:

```
Unknown tool: read_file   (src/runtime/executor.rs:1084)
```

and the LLM has no way to read a file range in that mode unless it
shells out through `bash` (which is not equivalent — bash doesn't give
it the line-range, structured-output, or image-safety that `read_file`
does).

## Repro

1. `./dev.py up`
2. Create a Direct-mode conversation with the mock provider
3. Send any message — mock's `ReadFileToolCall` scenario emits
   `read_file(Cargo.toml, 1..30)`
4. Observe tool result: `Unknown tool: read_file, is_error: true`
5. SQLite confirms real backend iteration (not a UI dupe):
   ```
   select message_type, count(*) from messages where conversation_id = ...
   -- user 1, agent N, tool N   (each row a distinct mock_toolu_* id)
   ```

## Fix

Add `ReadFileTool` to `new_with_options(..)`. That covers Direct,
Work-via-`explore_with_sandbox`, and the legacy `standard()` test path in
one change. Explore and sub-agent registries already have it, so the
mode matrix collapses to "every mode has `read_file`" — consistent with
`SearchTool`'s placement (which is only in `explore_no_sandbox()`; a
follow-up may want to audit that too).

Add a test that enumerates every `ToolRegistry::*()` constructor and
asserts the set of tool names against a single source of truth, so this
drift can't recur silently.

## Done when

- [ ] `ReadFileTool` is in the Direct / Work tool registry
- [ ] `find_tool("read_file")` returns `Some` for every non-sub-agent
      registry constructor
- [ ] Regression test enumerates constructors and checks tool names
- [ ] Mock `ReadFileToolCall` scenario produces a real read_file result
      end-to-end in a Direct conversation

## Related

- Task `24680-p1-ready--parent-conversation-turn-cap.md` — the infinite
  loop this bug produced when combined with no iteration cap
