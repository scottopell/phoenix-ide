# Stale Tool Result Clearing — Executive Summary

## What and Why

A long agent session is dominated by tool output — file reads, command output,
search results — most of which goes stale within a few turns yet is re-sent to
the model on every request, consuming context-window space and re-billed input
tokens. This feature removes stale, recoverable tool results from the model's
view once the request approaches the context window, while keeping the full
record in storage for the human. It lets sessions run far longer at full
fidelity before summarization is forced, and bends the per-turn cost curve of a
long session back from quadratic toward linear.

The work is provider-agnostic and lives on the executor's main request path
(`dispatch_llm_request`), as history is assembled into the provider-agnostic
message list. It subsumes the existing aged-screenshot
retention: a single mechanism now governs each tool result's text and images
together, replacing the image-only window whose tail-relative boundary rewrote
recent context — and busted the prompt cache — every turn. Removal is governed by
a persisted, monotonic per-conversation clear watermark that advances only in
spaced, worthwhile sweeps under context pressure, so the cleared prefix is
identical between sweeps and the cache stays warm; the recency floor keeps the
current turn's trailing-result breakpoint uncleared. Recoverability is a
`Tool::clearable()` capability defaulting to false, so a new tool is never
silently cleared.

## Status

| ID | Title | Status |
|---|---|---|
| REQ-STR-001 | Sustain Long Sessions Without Losing Fidelity | ✅ Complete |
| REQ-STR-002 | Only Remove Re-obtainable Information | ✅ Complete |
| REQ-STR-003 | Preserve the Agent's Immediate Working Set | ✅ Complete |
| REQ-STR-004 | Leave a Marker, Never a Silent Gap | ✅ Complete |
| REQ-STR-005 | Never Lose the Record | ✅ Complete |
| REQ-STR-006 | Make Each Removal Pay for Itself | ✅ Complete |
| REQ-STR-007 | Keep Reuse Savings Stable Across Turns | ✅ Complete |
| REQ-STR-008 | Make Removal Observable | ✅ Complete |
| REQ-STR-009 | Apply Regardless of Model Provider | ✅ Complete |
| REQ-STR-010 | Tune Retention Per Deployment | 🔄 In Progress |
| REQ-STR-011 | Govern a Result's Images and Text Together | ✅ Complete |

## Scope Notes

The behavioural detail — the monotonic watermark and its advancement, the
recency floor, the per-result verdict derived from watermark and tool
clearability, the unified image/text treatment, and the stability and cache-tail
invariants — is specified in `stale-tool-results.allium`.

After unification the retention ladder has two tiers: this feature (recoverable,
cache-aware removal above a high-water mark) and continuation/summarization
(heaviest, lossy, last resort). Clearing's job is to delay or eliminate the need
for the heaviest tier. The former lightest tier — the image-only window — is
folded into this feature and removed.

## Implementation Notes

The feature adds one persisted field, a monotonic `clear_watermark` integer
(message sequence_id) on the conversation, defaulting to zero; existing
conversations need no backfill. It removes the `IMAGE_HISTORY_ROUNDS` window and
`tool_msg_indices_keeping_images`, folding image retirement into the
watermark-governed verdict. The planner (`plan_tool_result_clearing`) and renderer
(`render_messages`) live in the executor's history assembly; clearing is applied
on the main LLM request path via `assemble_cleared_messages` (called from
`dispatch_llm_request`), which reads the watermark and the last turn's reported
prompt size, plans a sweep, persists any advance, and logs the cleared count and
freed tokens. A sweep is maximal — it clears the whole prefix outside the recency
floor at once, but only when the candidate frees at least the greater of the
absolute floor and a fraction of the reported prompt size. `Tool::clearable()`
sources the recoverable set, surfaced via `ToolRegistry::clearable_tool_names()`
(cached per conversation, refreshed on the Explore→Work upgrade).

REQ-STR-010 is partially met (🔄): the retention parameters exist as named
constants (`KEEP_RECENT_ROUNDS`, `CLEAR_TRIGGER_*`, `CLEAR_AT_LEAST_TOKENS`,
`CLEAR_GAIN_FRACTION_*`), tunable by editing, but there is not yet a runtime or
per-deployment configuration surface that overrides them. The remaining work is
to thread these from deployment config.

## Default Parameters

The retention parameters are deployment-tunable per REQ-STR-010. Three have
provisional starting values carried as config defaults in `stale-tool-results.allium`
— `keep_recent_rounds = 3`, `clear_at_least_tokens = 8192`, and
`clear_gain_fraction = 1/20` — chosen as reasonable starting points. The recency
floor keeps at least the two rounds of visual context the prior image window
preserved, plus one. The 5% proportional gate rejects the observed tail case of
freeing about 8–9k tokens on a ~190k prompt while preserving clean large sweeps
and clearable-heavy sessions in the modeling artifact
`docs/experiments/stale-clearing-proportional-gate.html`. They are the
implementation's initial values, to be tuned against benchmarks, not frozen
constants; this note is the single place that records their status, so the config
defaults and this document do not disagree.

One further parameter is derived per request rather than fixed config, so it
carries no Allium default: `clear_trigger` (the high-water mark, a fraction of the
model's context window, compared against the last turn's reported prompt size).
Its fraction is set at implementation against the deployed models' windows and
recorded here once benchmarked. There is no `target_after_clear`: a sweep always
clears the whole prefix outside the recency floor, so `keep_recent_rounds` alone
bounds retention.
