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
(`build_llm_messages_static`), generalizing the existing aged-screenshot
retention from images-only to full tool-result bodies. Clearing is anchored to
recent tool rounds so it is stable across turns, which preserves the prompt-cache
reuse that makes long sessions affordable; the cached tail of the current turn is
never disturbed. Recoverability is a `Tool::clearable()` capability defaulting to
false, so a new tool is never silently cleared.

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

## Scope Notes

The behavioural detail — the retention verdict per tool-result message, the
activation / recoverability / worthwhile-gain conjunction, and the stability and
cache-tail invariants — is specified in `stale-tool-results.allium`.

Clearing is the mid-weight member of a three-tier retention ladder already
partly present in the executor: screenshot pruning (lightest, always on), this
feature (mid, recoverable removal above a high-water mark), and
continuation/summarization (heaviest, lossy, last resort). Clearing's job is to
delay or eliminate the need for the heaviest tier.

## Default Parameters (to confirm at implementation)

The retention parameters need concrete defaults set against the deployed models'
context windows: `clear_trigger` (input-token high-water mark as a fraction of
the context window), `keep_recent_rounds` (rounds always retained in full), and
`clear_at_least` (minimum tokens a removal must free). These are deployment-tunable
per REQ-STR-010; the chosen defaults will be recorded here once benchmarked.
