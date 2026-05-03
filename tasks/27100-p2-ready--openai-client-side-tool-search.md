---
created: 2026-05-03
priority: p2
status: ready
artifact: src/tools/tool_search.rs
---

<!--
This task file was created without `taskmd new` because the binary was not
available in the working environment. ID 27100 chosen above the existing
high-water mark (27001). Run `./dev.py tasks fix` if the ID needs to be
re-allocated.
-->

# Client-side tool search for the OpenAI / codex Responses path

## Problem

Phoenix uses `ToolDefinition.defer_loading: bool` to mark tools the model
shouldn't see in `tools[]` until they're discovered. On the **Anthropic** path
this is wired through `tool_search_tool_regex_20251119` — the API itself
indexes deferred tools server-side and surfaces them mid-turn via
`server_tool_use` blocks (`src/llm/anthropic.rs:338-516`).

On the **OpenAI Responses** path (including the codex bridge added in PR #14),
`defer_loading` is silently ignored: every tool ships in `tools[]` regardless
(`src/llm/openai.rs:422-437`). For sessions with many MCP tools this:

- bloats the request and shrinks the working context window
- defeats the whole point of the `defer_loading` flag for OpenAI users
- makes phoenix look noticeably more expensive on OpenAI than it should
- means MCP servers with large tool surfaces (Linear, Notion, browser
  automation) become impractical on OpenAI models

OpenAI's Responses API has no server-side tool search. The fix is a
**client-side** emulation that mirrors what Codex CLI does.

## Reference: Codex CLI's mechanism

The official Codex CLI (`openai/codex`) implements this exact pattern. The
relevant code is short and worth reading before designing phoenix's version.

```bash
git clone --depth=1 https://github.com/openai/codex.git /tmp/codex-cli
```

Key files (under `codex-rs/`):

- `core/src/tools/tool_search_entry.rs` (~170 lines) — builds search corpus
  from MCP `ToolInfo` + `DynamicToolSpec`. The "search text" for each entry
  is name + name-with-underscores-as-spaces + description + namespace +
  schema property names + (for MCP) server name + connector metadata.
- `core/src/tools/handlers/tool_search.rs` (~290 lines) — the `tool_search`
  function-tool handler. Builds a BM25 index (`bm25` crate) over the corpus
  on construction, runs queries on each invocation. Default limit 8.
- `tools/src/tool_discovery.rs` — emits the `tool_search` tool definition
  itself (`TOOL_SEARCH_TOOL_NAME = "tool_search"`,
  `TOOL_SEARCH_DEFAULT_LIMIT = 8`).
- `core/src/tools/spec.rs:271-282` — wiring: when any deferred tool is
  registered, the tool registry adds the `tool_search` entry pointing at
  the shared `ToolSearchHandler`.
- `protocol/src/dynamic_tools.rs` — the `defer_loading` flag definition
  (same name + semantics as phoenix's).

The flow at runtime:

1. **Catalog build at session start.** Walk all tools that have
   `defer_loading: true`. For each, create a `ToolSearchEntry` with rich
   `search_text`. Index with BM25.
2. **Wire.** Register a single function tool `tool_search` in the model's
   `tools[]` with schema `{query: string, limit?: int}`. Deferred tools
   are NOT in `tools[]`.
3. **Resolve.** When the model calls `tool_search({query: "linear issues"})`,
   the handler runs BM25 on the corpus and returns each match's full
   `LoadableToolSpec` (name, description, JSON schema).
4. **Dispatch.** The model uses the returned tool name in subsequent turns;
   the existing tool registry routes to the underlying handler. Phoenix
   already has this part — `defer_loading` only controls what's
   *advertised*, not what's *callable*.

## Goal

Phoenix's OpenAI Responses path supports `defer_loading` via a client-side
`tool_search` tool, with no behavior change for the Anthropic path.

## Design sketch

### New module: `src/tools/tool_search.rs`

A built-in tool implementing the existing `Tool` trait:

```rust
pub struct ToolSearch {
    deferred: Vec<DeferredEntry>,
    index: BM25Index,
}

struct DeferredEntry {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    /// Concatenated search corpus (name + description + property names + …)
    search_text: String,
}
```

- `name() -> "tool_search"`
- `input_schema()`: `{type: object, properties: {query: {type: string}, limit: {type: integer, minimum: 1, default: 8}}, required: [query]}`
- `run(input)`: BM25 search, return JSON `{tools: [{name, description, input_schema}, ...]}`

The `bm25` crate is small (~200 LOC of pure Rust, no native deps); add it to
`Cargo.toml`.

### Translation change in `src/llm/openai.rs`

In `translate_to_responses_request`:

```rust
let (tools_to_send, has_deferred) = partition_deferred(&request.tools);
let mut tools = tools_to_send.iter().map(translate_tool).collect();
if has_deferred {
    tools.push(tool_search_definition());
}
```

`partition_deferred` keeps `defer_loading: false` tools, drops the rest.
`tool_search_definition` returns the static `tool_search` function-tool.

The Anthropic translator is untouched — it already does the right thing.

### Tool registration

When the runtime builds the tool registry at session start, if any
`ToolDefinition.defer_loading == true` is registered AND the active model
uses `ApiFormat::OpenAIResponses`, also register a single `ToolSearch`
instance whose corpus is built from the deferred tools.

This means `ToolSearch` is constructed once per session, not per request.
The BM25 index lives for the session's lifetime.

### Dispatch — already works

When the model calls a deferred tool by name, the existing tool registry
already routes to its handler. `defer_loading` only gates what gets
advertised in `tools[]`. No dispatch changes needed.

## Acceptance criteria

- [ ] `bm25` (or equivalent) added to `Cargo.toml`
- [ ] `src/tools/tool_search.rs` implements the `Tool` trait
- [ ] OpenAI request translation strips `defer_loading: true` tools and
      injects `tool_search` when at least one deferred tool exists
- [ ] Anthropic path is byte-for-byte unchanged (covered by existing
      `parity_*` tests in `src/llm/anthropic.rs`)
- [ ] Runtime builds the search corpus once per session and shares the
      `ToolSearch` handler instance across turns
- [ ] Unit tests: corpus builder produces expected `search_text`,
      BM25 returns sensible matches for representative queries, OpenAI
      translator partitions correctly, end-to-end model-calls-search-then-tool
      smoke test against `MockLlmService`
- [ ] Spec stub at `specs/tool-search/executive.md` describing why and the
      Codex-derived design choice

## Tradeoffs to think about

- **Round-trip cost.** Every use of a deferred tool now takes 2 turns
  (search → call). Anthropic's server-side variant avoids this. For
  conversations with ≥3 MCP tools this is still net cheaper than shipping
  full descriptions every turn.
- **Cache key interaction.** `tool_search` itself is identical across
  every turn, so it caches well via `prompt_cache_key`. Deferred tool
  descriptions are not in the request, so they don't bloat the cached
  prefix. Good interaction.
- **BM25 quality.** Codex shipped BM25 — strong signal it's good enough.
  An embedding index would do better but adds dependencies and a build
  step. Defer.
- **Per-model gating vs. always-on.** Codex always advertises
  `tool_search` when ≥1 deferred tool exists. Phoenix could do the same,
  or gate on `spec.supports_tool_search`. Recommend: always-on when
  `ApiFormat::OpenAIResponses` AND any `defer_loading: true` tool
  exists. Provider-driven, not per-model — simpler.

## Out of scope

- Embedding-based search (a follow-up)
- Surfacing tool-search calls in the UI as their own block type — the
  existing `ToolUse` rendering path is fine
- Anthropic-path changes — already works server-side

## Notes

- The Anthropic path's `ContentBlock::ServerToolUse` /
  `ToolSearchToolResult` blocks are NOT reusable for OpenAI — those
  round-trip through Anthropic specifically. The OpenAI side gets a
  regular `ToolUse` for `tool_search`, exactly like any other tool call.
- This task does not depend on PR #14 (codex auth bridge) but benefits
  the same use case (subscription-routed OpenAI users with many MCP
  tools).
- Consider updating `ModelSpec.supports_tool_search` semantics:
  currently it means "Anthropic server-side tool_search". After this
  task it could mean "any tool_search mechanism" or be split into
  `supports_anthropic_tool_search` / always-on for OpenAI.
