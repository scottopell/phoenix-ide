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

The work is provider-agnostic and lives in the executor's history-assembly pass
(`build_llm_messages_static`). It subsumes the existing aged-screenshot
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
| REQ-STR-001 | Sustain Long Sessions Without Losing Fidelity | ❌ Not Started |
| REQ-STR-002 | Only Remove Recoverable Information | ❌ Not Started |
| REQ-STR-003 | Preserve the Agent's Immediate Working Set | ❌ Not Started |
| REQ-STR-004 | Leave a Marker, Never a Silent Gap | ❌ Not Started |
| REQ-STR-005 | Never Lose the Record | ❌ Not Started |
| REQ-STR-006 | Make Each Removal Pay for Itself | ❌ Not Started |
| REQ-STR-007 | Keep Reuse Savings Stable Across Turns | ❌ Not Started |
| REQ-STR-008 | Make Removal Observable | ❌ Not Started |
| REQ-STR-009 | Apply Regardless of Model Provider | ❌ Not Started |
| REQ-STR-010 | Tune Retention Per Deployment | ❌ Not Started |
| REQ-STR-011 | Govern a Result's Images and Text Together | ❌ Not Started |

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
watermark-governed verdict.

## Default Parameters

The retention parameters are deployment-tunable per REQ-STR-010. Two have
provisional starting values carried as config defaults in `stale-tool-results.allium`
— `keep_recent_rounds = 3` and `clear_at_least_tokens = 8192` — chosen as
reasonable starting points (the floor keeps at least the two rounds of visual
context the prior image window preserved, plus one). They are the implementation's
initial values, to be tuned against benchmarks, not frozen constants; this note
is the single place that records their status, so the config defaults and this
document do not disagree.

Two further parameters are derived per request rather than fixed config, so they
carry no Allium default: `clear_trigger` (input-token high-water mark, a fraction
of the model's context window) and `target_after_clear` (usage a sweep falls back
under). Their fractions are set at implementation against the deployed models'
windows and recorded here once benchmarked.
