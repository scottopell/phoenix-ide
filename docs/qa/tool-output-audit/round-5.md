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

## Results

### Re-vote on PR 1204

**Agree to merge the combined patch-preview and locator implementation reviewed at 5a848c698 once CI and review are clean.** The neutral source marker preserves the locator without advertising a missing tool. The continuation path now prices its canonical marker for sweep eligibility and preserves previews that would grow. The preview is assembled with its text, its stored range is validated, and rewriting the text invalidates the preview.

- The final live marker is 11 bytes shorter than the Round 4 replay string. The exact live replay estimates **2,036.9M token-turns (3.34%)** for A and 3,157.8M (5.18%) for B. Together they model **8.53%** of 60.920B recorded input. The new percentages use the fixed live-request schedule and exclude the sub-million-token continuation adjustment.

Local validation passed seven core preview tests, fifteen Phoenix runtime preview tests, and the self-sequence reference test. Replay fixtures exercised discovery overhead, cache allocation, the final locator, and surface labels.

### Production discovery baseline

| Surface | Search calls | Transcripts with a search | Persisted search results | Returned deferred tools | Later successful deferred use |
|---|---:|---:|---:|---:|---:|
| Ordinary parent | 22 | 18 | 22 | 11 | 1 |
| Coordinator | 0 | 0 | 0 | 0 | 0 |
| Work sub-agent | 5 | 4 | 5 | 0 | 0 |
| Explore sub-agent | 352 | 295 | 352 | 0 | 0 |
| Legacy sub-agent mode | 56 | 53 | 56 | 0 | 0 |

435 visible searches across 370 transcripts correspond to 0.077% as many calls as ordinary response anchors. Of 435 durable results, 11 returned at least one deferred definition and one led to a successful call to a named deferred tool (9.09% of searches returning definitions). Later use can match more than one result. The observed rollout is small.

### Browser discovery and exact wire shapes

Replay one client-side search at first browser use. The explicit limit of 20 exceeds the 12-tool bundle and the default limit of eight. On pre-discovery requests the serialized browser function declarations leave the tools array. After discovery the full function declarations, deferred flags and serialized search exchange move into later prompt input. The extra request is priced at the observed first-use input minus the removed declarations.

| Surface | Eager browser requests with policy evidence | Browser transcripts | Within first 3 | Searches priced | Net input with policy evidence | Net input on matched Codex attempts |
|---|---:|---:|---:|---:|---:|---:|
| Ordinary parent | 160,643 | 158 | 9 | 66 | 483.4M | 287.6M |
| Coordinator | 0 | 0 | 0 | 0 | 0.0M | 0.0M |
| Work sub-agent | 24,290 | 6 | 0 | 5 | 74.7M | 50.8M |
| Explore sub-agent | 57,933 | 180 | 44 | 122 | 160.5M | 108.0M |
| Legacy sub-agent mode | 29,098 | 1 | 0 | 0 | 90.5M | 66.8M |

- The client-side replay nets **809.1M input tokens (1.33% of recorded input, 1.45% after B+A)** across the tool-policy evidence. The stricter subset with a uniquely matched Codex capacity record nets 513.2M (0.92% after B+A). Proportional cache allocation attributes 690.4M net tokens to cache reads.
- **53 of 345** browser transcripts (15.36%) used the browser within the first three measured requests. Those callers would see the extra round trip early. All browser-calling transcripts had an eligible response anchor.
- The fixed B+A and browser scenarios together model 9.86% of recorded input. Use the narrower matched-route figure for firm cohort attribution. The replay is an estimate of input and excludes output pricing, provider tokenization differences, and discovery failures or added retries.

**Recommendation · Prototype browser deferral on eligible Codex client routes.** Verify one explicit browser bundle search before the first call, and retain eager behavior on unsupported routes. Include a first-three-request latency check for browser-heavy workflows.
