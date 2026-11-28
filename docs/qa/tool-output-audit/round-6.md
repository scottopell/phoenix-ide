# Round 6: can a deferred tool be discovered? (gate for browser deferral)

## Review of round 5 (reviewer)

Accepted. PR #1204 is merged (`415ddf399`, about 8.5%), and the wins log is updated.

Your discovery baseline raises a problem I want settled before building browser deferral.
Out of 435 searches, 11 returned deferred tools and 1 led to a successful call. Phoenix gives the model no
hint that deferred tools exist: the `tool_search` description is generic, and nothing lists
deferred names. Deferring the 12 browser tools without a hint could quietly cut browser use, which would
cost more than the 1.3% it saves.

There is a natural experiment. `ask_user_question` is deferred on every route that supports
discovery. Anthropic routes have deferred it since at least 2026-10-07 (#852). OpenAI Responses and
Codex routes started deferring it on **2026-11-23** (#1148, native/shared tool discovery). Before
that date those routes loaded it eagerly.

## Questions

1. **AUQ before and after on OpenAI routes.** Give `ask_user_question` calls per 1,000 ordinary parent requests, and the share of
   transcripts with at least one call, on OpenAI Responses and Codex routes, for 2026-11-09..22 against
   2026-11-23..present. Do the same for Anthropic routes over the same windows as a control, since
   Anthropic did not change. If a confounder exists, such as prompt changes or the #1123 Ember change on
   11-20, name it.
2. **Tool searches on the deferred routes:** the query text pattern (category only, e.g. "named a tool",
   "described a task"; no raw text), and whether any query ever named `ask_user_question`.
3. **Browser usage before deferral, as a baseline:** browser calls per 1,000 ordinary parent requests on
   the same routes and windows. This gives the post-deferral comparison a starting point.

## What I'll build depending on the answer

- If AUQ use **dropped** after deferral, deferral needs a hint. The planned change adds a compact
  names-only summary of deferred tools, about 40 tokens, wherever discovery is active. That would also restore AUQ. Browser
  deferral then ships with the hint.
- If AUQ use **held**, the generic discovery path works, and browser deferral can ship as is.

## Output

Append `## Results`, write `round-6.json`, vote, and pass the baton.
