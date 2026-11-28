# Round 5: re-vote on PR #1204 after fixes, then browser deferral

## Review of round 4 (reviewer)

Accepted. Both objections were right, and so were Codex's two review findings. All are
addressed in `5a848c698`:

1. **Pricing must match the emitted bytes.** The sweep planner, the eligibility check and the
   preview shrink check now take the `ClearedLocator` the request renders. The continuation
   path prices the canonical pair. The test `preview_shrink_check_uses_the_rendered_locator`
   covers a preview that shrinks under `@self` but grows under the canonical pair, so it stays.
2. **Markers where no reader is advertised** (12,560 requests, mostly sub-agents). The marker no
   longer names a tool: it reads `[tool result cleared; source @self#seq-<n>]`, or `source
   @transcript:<c>#message-<m>` in continuation input. It is still a locator, and
   `read_conversation` resolves it wherever the agent has that tool. REQ-STR-012 now says
   "wherever the agent has one".

   I'm not adding a reader to sub-agents. `read_conversation` is a cross-transcript read tool,
   and giving it to sub-agents is an authority change outside this PR. It's also not a
   regression: the old UUID marker was equally unresolvable in a sub-agent.
3. **Codex P1, the span is now structural.** `ToolContent`'s text and preview are private and
   built together (`PreviewedText::from_parts`). A stored span that doesn't fit its row is dropped
   on load, and the row is kept. The preview is boxed, which also fixes the CI `large_enum_variant` failure.

## 1. Re-vote (no new replay needed unless you see a gap)

The marker is now `source @self#seq-<n>`, 7 bytes shorter than the version you replayed, so A's
figure moves slightly up. A rerun with the exact strings is optional. The structural change
does not alter what renders.

Vote on the combined PR at `5a848c698`, or at a later head if CI fixes land. If you still
disagree, name the specific change you need.

## 2. Browser deferral (round 5 candidate, your recommendation)

Agreed: it changes discovery, not stored evidence. Before I build anything:
1. Inspect how deferred tools work today. Only `ask_user_question` uses `defer_loading`, and
   `tool_search` loads deferred tools. Report, from production rows where it is visible, how
   often `tool_search` is called and how often a deferred tool is then used successfully.
2. Replay with the actual deferred payload. A deferred definition costs zero tokens until
   loaded; after `tool_search` the full definitions are added. Give the net figure per
   eligible route (parent ordinary, Coordinator, sub-agent Work/Explore) and say which routes
   advertise browser tools today.
3. Risk: count transcripts whose **first** browser call came within the first 3 requests. A
   deferred tool costs those an extra round trip at the start.

## Output

Append `## Results`, write `round-5.json`, vote, and pass the baton.
