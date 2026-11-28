# ADR-294: Defer browser tools, and name deferred tools where the model cannot see them

- **Status:** Accepted
- **Date:** 2026-11-28
- **Affects:** REQ-LLM-004n, REQ-LLM-004o (new), REQ-AUQ-008

## Context

The 12 browser tools add about 11KB of schema, roughly 2.9k tokens, to every request. Only 2.7% of transcripts call
them. ADR-278 provides deferred discovery on qualified routes, but a deferred tool is invisible
to the model on Anthropic native, Codex client and local search. Nothing tells the model it
exists. In the production audit, `ask_user_question` calls per request on Codex routes fell about 85%
when it became deferred (2026-11-23), and only 1 of 435 tool searches led to a deferred tool being
used.

## Options considered

1. **Defer the browser tools alone.** Saves the schema, but repeats the `ask_user_question` loss for
   browser work.
2. **Keep everything eager.** No risk, no saving.
3. **Defer the browser tools and add a stable, bounded list of deferred names to the system prompt on
   the routes where deferred tools are invisible.**

## Decision

Option 3. `deferred_tool_hint` lists the permitted deferred tools (sorted, at most 40 names, the
rest counted) only for Anthropic, OpenAI client and local discovery. Hosted OpenAI search already
declares each deferred tool's name and description; eager routes defer nothing. The list depends only on the tool surface,
so the cached prompt prefix stays stable. The decision was made by the reviewer and worker agents of the
tool-output audit under the owner's delegated authority, after the discoverability gate in
round 6.

## Consequences

- **Positive:** about 2.9k fewer tokens per request on qualified routes that never use the browser,
  about 1.3% of input by replay. The list should also restore `ask_user_question` and help deferred MCP tools.
- **Negative:** browser users on those routes need one search round trip first. About 15% of them use the browser
  in their first three requests. The hint adds about 40–60 tokens to every request on hint routes.
- **Neutral:** eager routes are unchanged.

## References

- ADR-278; `docs/qa/tool-output-audit/round-5.md`, `round-6.md`
- `deferred_tool_hint`, `browser_tools`, `ToolDiscoveryMode`
