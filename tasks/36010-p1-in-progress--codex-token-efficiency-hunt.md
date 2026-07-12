# Investigate and implement Codex token-efficiency opportunities

## Problem

Phoenix now matches upstream Codex Responses Lite and WebSocket continuation behavior, but it has not completed a broader source-to-runtime audit of context-quota efficiency. Current upstream codex-rs may contain additional prompt-shaping, history management, compaction, tool-schema, or cache-stability techniques that reduce uncached input tokens or preserve useful context longer.

## Objective

Use a freshly pinned upstream openai/codex revision as the source of truth, measure Phoenix's current token behavior, identify material parity gaps, and implement only optimizations supported by source evidence and reproducible token measurements.

## Required investigation

- Pin and record the current upstream codex-rs commit.
- Trace prompt construction, history shaping, tool serialization, compaction, cache keys, model metadata, and continuation behavior.
- Separate prompt-cache savings from WebSocket payload savings.
- Measure cold, warm, tool-loop, and post-compaction turns.
- Compare total input, cached-read, cache-write, uncached input, and context-window usage.
- Identify mutable prefix components such as system prompts, tool definitions, task hints, skill instructions, metadata, and ordering.
- Preserve correctness and conversation continuity; do not optimize by silently discarding relevant context.

## Acceptance Criteria

- [ ] A pinned upstream source audit identifies every material token-efficiency technique relevant to Phoenix.
- [ ] Baseline Phoenix measurements are reproducible and sanitized.
- [ ] Candidate improvements are ranked by expected quota impact, risk, and implementation cost.
- [ ] Prompt-cache gains are reported separately from transport-byte reductions.
- [ ] Implemented changes include regression tests and before/after token measurements.
- [ ] Unsupported or speculative upstream behavior is documented but not shipped.
- [ ] Follow-up tasks are created for independent work that should not be bundled.
- [ ] `./dev.py check` passes.
