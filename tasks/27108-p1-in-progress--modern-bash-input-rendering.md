# Align bash tool input rendering with modern op/handle schema

## Problem

The conversation UI's bash tool-call formatter has drifted from the modern bash tool schema. Current backend/tool documentation and `specs/bash/requirements.md` define bash input as:

- `op`: required discriminator: `run | peek | wait | kill`
- `cmd`: command text, required for `op=run`
- `handle`: handle id, required for `op=peek | wait | kill`
- operation-specific adjuncts such as `wait_seconds`, `signal`, `lines`, and `since`

But `ui/src/components/MessageComponents.tsx::formatToolInput()` still renders bash input mostly as if the legacy shape existed:

```ts
const cmd = String(input['cmd'] || input['command'] || '');
...
const peek = input['peek'];
const wait = input['wait'];
const kill = input['kill'];
...
return { display: '$ <bash>', isMultiline: false };
```

That means modern handle-operation calls such as:

```json
{ "op": "peek", "handle": "b-13" }
{ "op": "wait", "handle": "b-13", "wait_seconds": 60 }
{ "op": "kill", "handle": "b-13", "signal": "TERM" }
```

fall through to `$ <bash>` instead of showing what happened. This is the blank/useless bash render observed in production conversation:

`/c/agent-propose-task-failure-fix`

The UI therefore communicates only “some bash call happened,” not the operation, handle, wait window, read window, or signal.

## Why this is bigger than one bad label

This is a schema drift problem across the render path, not just a typo:

1. **Tool input display is stale.**
   - `formatToolInput()` recognizes legacy `peek` / `wait` / `kill` top-level fields.
   - The backend parser now explicitly requires `op` and `handle`; the legacy operation-key fallback was retired.
   - The stale fallback produces `$ <bash>` for valid modern calls.

2. **Copy behavior is stale too.**
   - `rawInput` for bash uses `input['cmd'] || input['command'] || input['peek'] || input['wait'] || input['kill'] || ''`.
   - For modern `op=peek/wait/kill`, the copy button likely copies an empty string, or at best misses the operation context.

3. **Comments contradict the backend.**
   - The UI comment says older `command` aliases and legacy operation keys are still accepted.
   - `crates/phoenix-ide/src/tools/bash/operations.rs` now says retired affordances (`mode`, `command`, legacy `peek`/`wait`/`kill` operation keys) become structured parse errors.

4. **REQ-BASH-015 is currently violated on the input surface.**
   - `specs/bash/requirements.md` requires handle operations to display operation kind + handle id (examples: `peek b-7`, `kill b-7 (TERM)`) rather than a fictitious command string.
   - The output payloads may carry `display` labels, but the tool-call input renderer does not use the modern input shape and therefore can still show `$ <bash>`.

5. **Typed generated response models do not cover input shape.**
   - Bash responses are typed in `ui/src/generated/BashResponse.ts` and rendered structurally.
   - Bash tool-call inputs remain `Record<string, unknown>` (`ContentBlock.input`), so TypeScript cannot catch drift when the backend schema changes.

## Desired behavior

Modern bash tool calls should render accurately from the actual input shape:

- `op=run` + `cmd`: `$ <displayOverride-or-cmd>`
- `op=peek` + `handle`: `peek <handle>`
- `op=wait` + `handle`: `wait <handle>` plus `(up to Ns)` when `wait_seconds` is present
- `op=kill` + `handle`: `kill <handle> (<signal>)`, defaulting to `TERM` only when `signal` is absent
- read-window modifiers should be visible where useful:
  - `lines=N` => e.g. `peek b-13 (last N lines)` or equivalent
  - `since=K` => e.g. `peek b-13 (since K)` or equivalent
- malformed/unrecognized bash inputs should show compact JSON or an explicit invalid-input label, never `$ <bash>`.

## Recommended implementation

1. Fix the upstream Rust type drift first: introduce a modern Rust-owned bash input type and generate it for TypeScript.
   - Add a public `BashToolInput` (name flexible) near the bash tool implementation, e.g. `tools/bash/types.rs` or `tools/bash/operations.rs`.
   - This type should match the accepted modern tool input shape:
     - `op`: `run | peek | wait | kill`
     - `cmd?: string`
     - `handle?: string`
     - `label?: string`
     - `wait_seconds?: number`
     - `signal?: TERM | KILL`
     - `lines?: number`
     - `since?: number`
   - Derive `Serialize`, `Deserialize`, and `ts_rs::TS`, exporting to `ui/src/generated/`.
   - Replace the private `RawBashInput` parser struct with this exported type so the parser, tool execution, and UI type generation share one Rust-owned input shape.
   - Update `state_machine::ToolInput::Bash` to use this modern bash input type instead of the legacy `{ command, mode }` shape. Modern bash calls should no longer fall through to `ToolInput::Unknown` just because they use `op` + `handle`.
   - Run `./dev.py codegen` and export the generated type from `ui/src/generated/sse.ts` or another appropriate generated barrel.

2. Use the generated bash input type in UI formatting.
   - Import the generated `BashToolInput` type in `MessageComponents.tsx`.
   - Add a narrow parser/type guard from `Record<string, unknown>`/`unknown` to the generated type at the UI boundary. The UI still receives opaque message content today, so this boundary check is still necessary.
   - Formatting logic remains hand-written because a typed input is not the same thing as a user-facing label; however, the logic must branch on generated/Rust-owned fields rather than a TypeScript shadow schema.
   - Keep any required legacy rendering only as an explicit compatibility branch for old persisted conversations, not as the primary path.

3. Update `formatToolInput('bash', ...)` to branch on generated modern input fields first.
   - Use `displayOverride` only for `op=run` command simplification.
   - Do not invent `$ <bash>`.

4. Update bash copy-command behavior.
   - For `op=run`, copy the command string.
   - For handle operations, copy a useful operation summary or canonical JSON, not an empty string.
   - Rename tooltip if needed (`Copy command` is wrong for `peek`, `wait`, and `kill`).

5. Update Rust display enrichment and stale comments.
   - `compute_bash_display_data()` currently looks for old `input.command`; update it to parse/use the modern generated bash input type.
   - Preserve command simplification for `op=run`.
   - Produce sensible display labels for handle operations per REQ-BASH-015.
   - Update stale comments in `MessageComponents.tsx` and Rust state/tool code so they match the modern backend parser and spec.

6. Add focused tests.
   - `op=peek` renders `peek b-13`.
   - `op=wait` renders handle + wait window.
   - `op=kill` renders handle + signal.
   - unknown/malformed bash input does not render `$ <bash>`.
   - copy text for handle operations is non-empty and includes `op` + `handle`.

## Acceptance criteria

- Valid modern bash handle-operation tool calls never render `$ <bash>`.
- `ToolInput::Bash` uses the modern Rust-owned bash input type; modern `op` + `handle` calls do not fall through to `ToolInput::Unknown`.
- The generated TypeScript bash input type is used by the UI formatter; formatting logic may remain hand-written, but it branches on generated/Rust-owned fields rather than a manual shadow schema.
- The UI displays `peek`, `wait`, and `kill` calls using `op` + `handle` from the modern schema.
- `wait_seconds`, `signal`, `lines`, and `since` are represented when present.
- The bash input copy button works for modern handle operations and does not copy an empty string.
- Rust display enrichment uses the modern bash input type and no longer looks only for old `input.command`.
- Stale comments claiming legacy bash fields are accepted are removed or corrected.
- Tests cover modern `run`, `peek`, `wait`, `kill`, and malformed/unknown bash input formatting.
- `./dev.py check` passes.
