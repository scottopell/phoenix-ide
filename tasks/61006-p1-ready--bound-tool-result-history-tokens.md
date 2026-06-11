Tool outputs and history are not byte-bounded; one pathological case is conversation-fatal and the general case amplifies cache-read cost every turn.

## Byte-cap tool results (the fatal one)
Tool-output bounds are per-tool and LINE-based, not byte-based. bash returns a 200-line tail from a 4MB ring whose "lines" can be up to 64KB partial-flush chunks; read_file defaults to 2000 lines with no line-length cap. `cat` of a 2MB single-line minified file -> ~32 x 64KB "lines" -> entire 2MB enters the tool result, is persisted (tool_output_to_outcome, executor.rs ~51, no cap), resent every subsequent turn, and ~500k tokens trips ContextWindowExceeded (NotResumable) immediately.
Fix: byte-cap the LLM-bound tool result (~50-100KB) at ToolOutput -> ToolOutcome conversion with a "truncated, N bytes omitted" marker.

## Prune aged screenshots
browser/tools.rs ~370 attaches base64 images; they persist in ToolContent.images and replay every turn (executor.rs ~2754) with no prune pass. 20 screenshots ~= 30k permanent prefix tokens.
Fix: replace images older than N turns with a text placeholder.

## Snapshot the system prompt per conversation
system prompt is rebuilt from the live filesystem every request (executor.rs ~2074 -> system_prompt.rs ~132): re-reads every AGENTS.md up-tree and re-scans skill dirs. Any mid-session change (agent edits AGENTS.md, concurrent worktree touches a shared parent AGENTS.md) busts the cache from block 0 -> re-pay cache-write on the whole context.
Fix: snapshot per conversation (like the taskmd hint already is) or content-hash and log busts.

Caching is otherwise sound (3 correct cache_control anchors, no per-request variability in the prefix, retries capped at 3 with no 400 retries). Found in spiritual-core audit 2026-06-10.
