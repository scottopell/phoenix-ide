---
created: 2026-03-28
priority: p2
status: done
artifact: src/runtime/
---

# Sub-agent timeout wastes tokens on network I/O

**Resolution:** Absorbed into task 08629 (sub-agent modes, REQ-PROJ-008).
Max-turn enforcement (20 explore / 50 work) replaces wall-clock timeout as
primary runaway prevention. Wall-clock timeout bumped from 5 to 20 minutes
as a generous safety net for stuck tool execution.

## Summary

Sub-agents that perform slow network I/O (e.g. downloading multi-MB files)
frequently hit their time limit, consuming their full token budget before
producing any output. The calling agent gets nothing back except a timeout
error and must either retry (doubling the cost) or abandon the task entirely.

## Context

Observed concretely while trying to spawn a radar-meteorology review agent
against a weather-radar codebase. The review task required downloading ~9 MB
of live NEXRAD Level II files from an S3 bucket before it could do any
analysis. Two sequential spawn attempts both timed out — one agent, then two
parallel agents — burning tokens three times with zero useful output.

The root problem is that sub-agents have a fixed wall-clock time limit, but
that limit is designed around LLM inference time, not network transfer time.
A 9 MB download at a modest 5 MB/s takes ~2s, but retries, DNS, TLS
handshake, and slow hosts can push this to 30–60s. If the agent is doing
several sequential downloads (one per radar site) it can exhaust its budget
before writing a single line of analysis.

## Acceptance Criteria

- [ ] Identify where sub-agent wall-clock timeout is configured and whether
      it can be separated from "thinking/inference" time vs "tool execution"
      time — a long `bash` tool call should not count the same as a hung LLM
      request
- [ ] Add a way for the spawning agent to pass a custom timeout hint per task
      (e.g. `timeout_seconds: 300` for tasks known to involve large downloads)
- [ ] OR: document a spawn pattern that pre-fetches / pre-computes expensive
      data in the parent agent and passes results inline to the sub-agent,
      so sub-agents only do analysis (fast) not data acquisition (slow) —
      and surface this pattern in agent authoring docs / CLAUDE.md
- [ ] Consider streaming partial results back from a timed-out sub-agent
      rather than returning nothing — even a partially-completed review is
      more useful than a timeout error

## Notes

- The immediate workaround is to do all slow I/O (downloads, long builds,
  test runs) in the parent agent, then pass the data as inline context to
  sub-agents that only do reasoning. This keeps sub-agent work fast and
  unlikely to timeout.
- Related: bash tool calls with `mode="slow"` already get a 15-minute
  timeout in the parent — sub-agents should inherit or be configurable
  to the same limit.
- Token waste profile from the observed incident: 3 spawn attempts ×
  ~full context window each = significant waste with zero output returned.
