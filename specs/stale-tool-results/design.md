# Stale Tool Result Clearing — Design

## Where this lives (REQ-STR-009)

Tool-result clearing happens on the main LLM request path, in
`Executor::dispatch_llm_request`, which delegates history assembly to
`assemble_cleared_messages`: it reads the conversation's clear watermark and the
last turn's reported prompt size, plans a sweep (`plan_tool_result_clearing`),
persists any advance, and renders the history once (`render_messages`) with the
resulting cleared set — before any provider translation
(`anthropic.rs::translate_request`, the OpenAI Responses translation) runs. A
single pass therefore governs every provider identically, and the persisted
`db::ToolContent` it reads from is never mutated. Extracting the policy from the
dispatch closure keeps its watermark-failure handling (below) unit-testable apart
from a live LLM client.

Clearing is specific to this path. The continuation/summarization path
(`build_llm_messages_static` → `flatten_tool_blocks`) renders each tool result's
body to text and bounds replayed screenshots to a recent window
(`cap_replayed_images`), then declares no tools — so the per-result clearing
verdict has nothing to act on there.

## One mechanism for text and images (REQ-STR-011)

This pass is the single retention mechanism for tool-result bodies — text and
images alike. One verdict per tool result governs its text and its images
together: a retained result keeps both; a cleared result replaces its text with a
placeholder and sends no images. There is no separate, tighter schedule for
images.

A separate image-only window would be a cache pathology, which is why retention
is unified. Retiring images a fixed number of rounds behind the newest round
measures the boundary *from the tail*, so it advances by one round every turn;
each advance rewrites a tool result a few rounds back, and because prompt caches
reuse only a byte-identical request prefix, that rewrite invalidates the cache
from there through the tail — re-billing the most recent, most expensive context
every single turn. The watermark removes content only at a boundary that holds
still across turns (below), so a rewrite happens rarely instead of every turn.

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
    /// view. True when the tool reads re-queryable state — the agent can
    /// re-invoke it to re-obtain what it needs about the current state, so
    /// dropping an old result loses only a stale snapshot, not irreplaceable
    /// information.
    fn clearable(&self) -> bool { false }
}
```

The default is `false`: a newly added tool is never silently cleared. A tool opts
in only when it *reads state the agent can query again*. Crucially, the test is
not byte-reproducibility: in a workspace the agent mutates, re-reading a file
yields its current content, not the earlier snapshot, so no read is
byte-reproducible. The premise of clearing (REQ-STR-002) is that the *exact prior
snapshot* of a re-queryable read is low-value once the agent has acted on it — if
the agent needs current state it re-reads — so sacrificing that snapshot is
acceptable. The read-heavy tools whose output dominates a long session opt in on
that basis — `read_file`, `bash`, `search`, `keyword_search`, `read_image`, the
browser read tools (screenshot, console logs), and the tmux and terminal history
tools.

Tools whose result is *not* a re-queryable read do not opt in, because their
result is the sole record of something the agent cannot re-obtain:
`ask_user_question` (the human's typed answer is gone if dropped), `think`,
`propose_task`, the subagent `submit_result` / `submit_error` handoffs, and
`patch` (its result records that a change was applied — an event, not a queryable
state). For these, REQ-STR-002 forbids removal outright.

`bash` carries a caveat folded into this tradeoff: it can have side effects, so
re-invoking it to re-obtain state is not always free or safe. Clearing never
re-runs `bash` — it only drops the old *output* from the model's view — and the
accepted position is that a stale command's output is a low-value snapshot; if
the agent needs the information it decides whether re-running is appropriate. A
tool whose output must never be dropped even though it nominally "reads" should
leave `clearable()` false.

A `false` default plus an explicit per-tool opt-in makes "this tool was never
considered for clearing" structurally distinct from "this tool's output is safe
to clear" — the decision is in the type, not in a comment or an allowlist that
drifts from the tool set. An unclearable result at or before the watermark is
left intact; the watermark passing it by does not clear it.

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

- **One-time per sweep, then re-warmed.** A sweep is *maximal*: it clears the
  whole prefix outside the recency floor in one move rather than nibbling one
  round at a time, so the cache write it costs buys many turns of reuse — usage
  drops to the floor and only gradually climbs back to the trigger. The
  `clear_at_least` gate further ensures each invalidation frees enough that the
  one-time cost is recovered. Clearing to the floor (rather than stopping at an
  intermediate target) is what spaces sweeps apart: stopping early would leave
  clearable rounds just below the trigger, and each new turn's inflow would
  re-cross it, advancing the watermark every turn.

- **The cached tail is never disturbed.** The Anthropic translation places three
  deterministic cache breakpoints (`AnthropicCacheBreakpointsPlaced` in
  `specs/llm/anthropic.allium`): the last system block, the last tool entry, and
  the last cache-bearing block of the last user message — during a tool loop, the
  trailing tool result. The recency floor keeps the watermark strictly behind the
  most recent rounds, so the trailing result is always Kept and breakpoint (3)
  lands on an uncleared block. Sweeps only ever touch the prefix *before* the
  floor; the current turn's reusable anchor is intact.

- **The cleared prefix is already inside the cached region.** The cleared region
  sits below the recency floor, ahead of the last-tool-entry and last-user
  breakpoints, so it is part of the cached prefix without any dedicated
  coordination — the three existing breakpoints already deliver the reuse. A
  fourth breakpoint pinned at the watermark boundary would be a further
  optimization, isolating the stable prefix from the volatile tail so the tail's
  churn cannot force a re-read of the prefix; Anthropic's four-breakpoint budget
  could accommodate it. It is an optimization, not a precondition for the caching
  benefit.

- **Cache key unchanged.** Requests keep using `PromptCacheKey::stable(conv_id)`.
  The cleared prefix is a property of message content, not the cache cohort;
  keeping the key stable lets the re-warmed shorter prefix be reused by the same
  conversation's later turns.

- **Sustained pressure is a bounded exception (REQ-STR-007).** The "spaced
  sweeps" property holds while pressure is intermittent — the common case. If
  every turn adds enough new clearable context to stay over the trigger, one
  round ages out of the recency floor and is swept each turn, so the watermark
  advances each turn and the suffix from the newly-cleared round to the tail is
  re-billed. This is unavoidable once inflow exceeds what a single sweep can
  amortize: bounded usage, a preserved working set, and a perfectly stable cache
  cannot all three hold, and the design keeps usage bounded and the recency floor
  intact. Even then only the recent suffix — not the whole prefix — is re-billed,
  and clearing the aged round is still a net token saving. When clearing
  everything outside the floor still cannot get under the window, summarization
  (the heavier tier) is the escape.

## Pressure signal and worthwhile-gain gate (REQ-STR-001, REQ-STR-006)

The trigger is the **provider's reported prompt size for the previous turn** —
`input_tokens + cache_read_tokens + cache_creation_tokens`, the full context the
model saw, cached prefix included (the cache still counts against the window).
This is ground truth: it captures the system prompt and tool-schema bulk that an
independent re-estimate over conversation messages would omit, and it cannot
drift below reality. A turn whose reported size exceeds the trigger fraction of
the context window is under pressure. The first turn has no prior usage and is
treated as not under pressure (history is small then). Because the signal is the
*actual* size sent — which already reflects whatever was cleared last turn — it
falls after a sweep with no separate "estimate against the prior cleared set"
bookkeeping.

The signal lags by one turn: a single tool result large enough to jump usage from
below the trigger to over the window in one turn is not reflected until the next
turn's reported size. That next turn clears it; the turn that first exceeds the
window is not recovered by clearing. This is acceptable because clearing exists to
bend the *gradual* growth curve, and the sudden-jump case is caught by the heavier
tier — a `ContextWindowExceeded` response drives continuation/summarization, which
does not depend on the reported-size signal.

Whether a sweep is *worthwhile* is a separate question, answered with the
**image-aware** per-result estimator `estimate_tool_result_tokens` (text
character count plus a fixed cost per image block). The tokens freeable by
clearing every eligible result must reach `clear_at_least`, or the watermark
holds — so a screenshot-heavy round registers its true weight and a sweep that
would save little never disturbs the cache. A text-only estimate must not be used
here: a stale screenshot round would contribute almost nothing and starve
clearing in exactly the image-heavy sessions it exists for. This per-result
estimate also produces the freed-token / cleared-count figures logged for an
advance (REQ-STR-008). It is approximate by design — the consequence of a
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
| `clear_trigger` | Reported-prompt-size high-water mark that triggers a sweep | A fraction of `context_window`, leaving headroom for the reply |
| `keep_recent_rounds` | Recency floor the watermark may not cross | Enough rounds to cover the working set and the trailing breakpoint |
| `clear_at_least` | Minimum tokens a sweep must free, or it holds | Large enough that a cache write is recovered by the savings |

A sweep always clears the entire prefix outside the recency floor, so there is no
separate "target" parameter — `keep_recent_rounds` alone bounds how much is
retained. The defaults are chosen so a short session clears nothing and a long
session sweeps in spaced batches; a smaller-window model crosses `clear_trigger`
sooner and sweeps earlier. Concrete default values are tracked in `executive.md`.
