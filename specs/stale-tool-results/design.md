# Stale Tool Result Clearing — Design

## Where this lives (REQ-STR-009)

Tool-result clearing is a transform applied while the conversation history is
assembled into the provider-agnostic message list, in
`Executor::build_llm_messages_static`. That function reads the persisted messages
for a conversation and folds them into `Vec<LlmMessage>`; clearing is a retention
pass over that fold, before any provider translation
(`anthropic.rs::translate_request`, the OpenAI Responses translation) runs. A
single pass therefore governs every provider identically, and the persisted
`db::ToolContent` it reads from is never mutated.

## One mechanism for text and images (REQ-STR-011)

This pass is the single retention mechanism for tool-result bodies — text and
images alike. It subsumes the narrower image-only retirement that
`build_llm_messages_static` performs today via `tool_msg_indices_keeping_images`
and the `IMAGE_HISTORY_ROUNDS` window, which is removed. Every tool result gets
one verdict; that verdict governs the result's text and its images together. A
retained result keeps both; a cleared result replaces its text with a placeholder
and sends no images.

The image-only window had to go, not just for tidiness, but because it was the
core cache pathology. It retired images a fixed two rounds behind the newest
round — a boundary measured *from the tail*, so it advanced by one round every
turn. Each advance rewrote the tool result two rounds back, and because prompt
caches reuse only a byte-identical request prefix, that rewrite invalidated the
cache from two rounds behind the tail through the tail — re-billing the most
recent, most expensive context on every single turn. The unified mechanism
removes content only at a boundary that holds still across turns (below), so the
rewrite happens rarely instead of every turn.

## The clear watermark (REQ-STR-007)

Retention is governed by a per-conversation **clear watermark**: a monotonic
sequence position with the meaning *"every clearable tool result at or before
this point is cleared from the model's view."* It is persisted on the
conversation row as a single integer column (a message `sequence_id`), defaulting
to zero — "nothing cleared" — for a new or pre-existing conversation.

The watermark is the state that makes removal stable across turns (REQ-STR-007).
A purely positional rule computed fresh each turn cannot be stable: "keep the
last N rounds, clear the rest" measures from the tail, so its boundary advances
every turn and rewrites recent context every turn — the same pathology as the old
image window. "Clear everything older than N rounds, but only while over the
high-water mark" is worse: a sweep that drops usage back under the mark would, on
the next turn, compute *nothing over the mark* and un-clear what it just cleared,
flapping the prefix in both directions. Stability requires remembering how far
clearing has reached. The watermark is that memory, and it only ever moves
forward.

A tool result's verdict is then a pure function of the watermark:

> **Cleared** iff the result's round is at or before the watermark **and** its
> producing tool is clearable; otherwise **Kept**.

Because the watermark is fixed between sweeps, the set of cleared results — and
therefore the removed portion of every request — is byte-identical from one turn
to the next, and the cached prefix is reused. Only a sweep changes it.

## Advancing the watermark (REQ-STR-001, REQ-STR-006)

The watermark advances during history assembly, at most once per turn, and only
when all of these hold:

1. **Pressure (REQ-STR-001).** The estimated input tokens for the assembled
   request exceed `clear_trigger`, a high-water mark derived as a fraction of the
   model's `context_window`, leaving headroom for the reply. Below it the
   transcript fits comfortably and the watermark holds.

2. **A recency floor to stay behind.** The watermark may never advance into the
   most recent `keep_recent_rounds` rounds. That floor is a tail-relative
   boundary, but — unlike the old image window — it clears nothing as it moves;
   it only *caps* how far the monotonic watermark may reach. The recent rounds it
   protects are the agent's working set (REQ-STR-003) and contain the trailing
   tool result that anchors the request's last cache breakpoint (see Caching,
   below).

3. **A worthwhile sweep (REQ-STR-006).** Advancing the watermark to the candidate
   position must clear clearable results worth at least `clear_at_least` tokens.
   If the rounds newly swept would free less, the watermark holds this turn and
   waits for more clearable output to accumulate. This bounds how often a sweep
   disturbs the cached prefix.

When it advances, the watermark moves forward — over whole rounds, never
splitting a round's tool-use/result pairing — far enough that estimated usage
drops under a target below the trigger (so the next sweep is many turns away), or
to the recency floor if the target cannot be reached without crossing it. Within
a conversation, input usage only grows, so after a sweep usage climbs back toward
`clear_trigger` over subsequent turns and the next sweep fires once; the result is
roughly one cache-invalidating sweep per refill interval, not one per turn.

## Recoverability is a tool capability (REQ-STR-002)

The watermark says *how far* clearing reaches; `clearable()` says *which* results
within that reach may actually be cleared. The `Tool` trait gains the capability:

```rust
trait Tool {
    // ...
    /// Whether a stale result from this tool may be cleared from the model's
    /// view. True only when re-invoking the tool reproduces the information —
    /// the result carries no state that cannot be recovered.
    fn clearable(&self) -> bool { false }
}
```

The default is `false`: a newly added tool is never silently cleared. A tool opts
in only when its output is reconstructable by re-invocation. The read-heavy tools
whose output dominates a long session opt in — `read_file`, `bash`, `search`,
`keyword_search`, `read_image`, the browser read tools, the tmux and terminal
history tools, `process_inspection`. Tools whose result is unreproducible or
consequential do not: `ask_user_question` (the human's typed answer cannot be
regenerated), `think`, `propose_task`, the subagent `submit_result` /
`submit_error` handoffs, and `patch` (its result records a mutation that
re-invocation would not reproduce).

A `false` default plus an explicit per-tool opt-in makes "this tool was never
considered for clearing" structurally distinct from "this tool's output is safe
to clear" — the recoverability decision is in the type, not in a comment or an
allowlist that drifts from the tool set. An unclearable result at or before the
watermark is left intact; the watermark passing it by does not clear it.

The pass needs the producing tool for each tool result to look up clearability.
The tool name is carried on the assistant `ToolUse` block paired with each
`ToolResult` by `tool_use_id`; the pass joins results to their calls on that id.

## What clearing produces (REQ-STR-004, REQ-STR-005, REQ-STR-011)

Clearing a result replaces the textual body sent to the model with a fixed
placeholder — `[tool result cleared to save context]` — and sends no image
blocks for it. The paired assistant `ToolUse` block is left untouched, so the
model still sees that the call was made and with what arguments; only the result
body is elided. The decision is a typed verdict per tool-result message, not an
in-place string mutation threaded through the fold:

```rust
enum ToolResultRetention {
    /// Send the result body and images verbatim.
    Kept,
    /// Replace the body with the placeholder and drop images.
    Cleared,
}
```

The retained `db::ToolContent` in storage is the single source of truth and is
read-only here; the cleared form exists only in the `Vec<LlmMessage>` handed to
the provider for one request. The only persisted state the feature adds is the
watermark integer; the cleared *content* is recomputed from the watermark on
every assembly and never stored. So there is no serialized "cleared" body that
could be lost on a write or need a migration — REQ-STR-005 holds by construction,
and a reconnecting UI that rebuilds from storage shows every result in full.

## Interaction with prompt caching (REQ-STR-006, REQ-STR-007)

Prompt caching reuses the longest request prefix that is byte-identical to a
prior request. Tool results live in user-role messages, so changing whether a
result is cleared changes a user message and moves the first differing token
earlier, invalidating the cached prefix from there forward. This cost is
unavoidable for any in-place context reduction; the design bounds it to rare,
worthwhile moments rather than paying it every turn:

- **Stable between sweeps.** Because the watermark is fixed between sweeps and the
  verdict is a pure function of it, every turn that does not advance the watermark
  sends an identical cleared prefix and reads it from cache. The current turn's
  new content is appended past the trailing breakpoint, which is the cheap,
  expected growth.

- **One-time per sweep, then re-warmed.** The turn a sweep advances the watermark
  pays a cache write for the new, shorter prefix; subsequent turns reuse it. The
  `clear_at_least` gate ensures each such invalidation frees enough tokens that
  the one-time cost is recovered by the ongoing savings.

- **The cached tail is never disturbed.** The Anthropic translation places three
  deterministic cache breakpoints (`AnthropicCacheBreakpointsPlaced` in
  `specs/llm/anthropic.allium`): the last system block, the last tool entry, and
  the last cache-bearing block of the last user message — during a tool loop, the
  trailing tool result. The recency floor keeps the watermark strictly behind the
  most recent rounds, so the trailing result is always Kept and breakpoint (3)
  lands on an uncleared block. Sweeps only ever touch the prefix *before* the
  floor; the current turn's reusable anchor is intact.

- **A breakpoint behind the cleared region.** Placing a cache breakpoint at or
  just after the watermark boundary lets the stable cleared prefix be cached and
  reused across the many turns between sweeps, while the volatile recent tail sits
  past it under its own trailing breakpoint. Anthropic's four-breakpoint budget
  accommodates both; coordinating the watermark breakpoint with the existing
  three is part of the implementation against the `llm` spec.

- **Cache key unchanged.** Requests keep using `PromptCacheKey::stable(conv_id)`.
  The cleared prefix is a property of message content, not the cache cohort;
  keeping the key stable lets the re-warmed shorter prefix be reused by the same
  conversation's later turns.

## Token estimation (REQ-STR-001, REQ-STR-006)

Pressure detection and the `clear_at_least` / target calculations reuse
`estimate_text_tokens`, the same estimator `cap_messages_to_token_budget` and the
continuation overflow guard use, so the high-water-mark decision is consistent
with the executor's other token-budget logic. Estimation is approximate by
design — the trigger sits below the true window, and the consequence of a
slightly-off estimate is sweeping one round early or late, never a dropped result
or a 400.

## Retention ladder

After unification, two retention mechanisms remain, in increasing order of loss:

- **Stale tool-result clearing** (this spec) drops the full body — text and
  images — of aged, clearable, recoverable results once usage crosses the
  high-water mark. The information is recoverable by re-invocation, and removal is
  stable and cache-aware.
- **Continuation / summarization** (`request_continuation`, `flatten_tool_blocks`,
  `cap_messages_to_token_budget`) is the heavier last resort: it collapses the
  whole transcript into a summary when the window is nearly full. It is lossy in a
  way clearing is not, because a summary cannot be re-expanded.

Clearing holds the input-token curve down across a long session, pushing the
point at which summarization becomes necessary much later — out of reach entirely
for most sessions.

## Persistence and configuration (REQ-STR-010)

The feature adds one persisted field: a monotonic `clear_watermark` integer
(message `sequence_id`) on the conversation, defaulting to zero. It is a column,
not a blob field — a single scalar the assembly reads and the sweep advances,
schema-enforced and never queried inside a JSON document. Existing conversations
default to zero (nothing cleared) and begin clearing once they cross the
high-water mark; no backfill is owed.

Three parameters govern retention, resolved per conversation from the model's
context window and deployment configuration:

| Parameter | Meaning | Default basis |
|---|---|---|
| `clear_trigger` | Input-token high-water mark that triggers a sweep | A fraction of `context_window`, leaving headroom for the reply |
| `target_after_clear` | Usage a sweep aims to fall back under | Below `clear_trigger`, so sweeps are spaced many turns apart |
| `keep_recent_rounds` | Recency floor the watermark may not cross | Enough rounds to cover the working set and the trailing breakpoint |
| `clear_at_least` | Minimum tokens a sweep must free, or it holds | Large enough that a cache write is recovered by the savings |

The defaults are chosen so a short session clears nothing and a long session
sweeps in spaced batches; a smaller-window model crosses `clear_trigger` sooner
and sweeps earlier. Concrete default values are tracked in `executive.md`.
