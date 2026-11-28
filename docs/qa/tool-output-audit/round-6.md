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

## Results

### AskUserQuestion and browser activity

| Route and UTC window | Ordinary parent requests | Parent transcripts | AskUserQuestion calls per 1,000 | Transcripts with AskUserQuestion | Browser calls per 1,000 | Transcripts with browser calls |
|---|---:|---:|---:|---:|---:|---:|
| Codex, Nov 9–22 | 22,069 | 282 | 0.589 (13 calls) | 4.26% (12) | 7.386 (163 calls) | 6.03% (17) |
| Codex, Nov 23–28 snapshot | 11,618 | 138 | 0.086 (1 call) | 0.72% (1) | 0.689 (8 calls) | 5.07% (7) |
| Direct or compatible OpenAI Responses, both windows | 0 | 0 | — | — | — | — |
| Anthropic, both windows | 0 | 0 | — | — | — | — |

AskUserQuestion calls per request fell **85.4%**. Excluding Nov 23 gives 0.099 post-cutover calls per 1,000, an 83.2% drop. GPT-6.1 Sol, the largest model present in both windows, went from 10 calls in 16,884 requests to none in 6,584. GPT-6.2 Sol accounts for 41.4% of post-cutover requests and produced the one call. Browser use was eager throughout, and its calls per request fell 90.7% while its transcript share fell by 0.96 percentage points. A few browser-heavy sessions account for much of the change in call intensity.

The model and workload mix changed, the question counts are small, and the snapshot contains no Anthropic control traffic. #1148 added models and changed the default to GPT-6.2 Sol alongside discovery. #1123 changed how the Coordinator asks questions. Coordinator requests and the replacement tool are outside the parent cohort. The direction is consistent with a discoverability problem, although the data cannot attribute the full decline to deferral.

### What search found

| Codex surface, after cutover | Explicit AskUserQuestion name | Other known tool name | Question or clarification terms | Task or keyword | Total |
|---|---:|---:|---:|---:|---:|
| Ordinary parent | 1 | 5 | 4 | 13 | 23 |
| Sub-agent | 0 | 391 | 0 | 31 | 422 |
| Coordinator | 0 | 0 | 0 | 0 | 0 |

All 445 visible searches had durable results. Eleven parent searches returned AskUserQuestion. Their query categories were one explicit AskUserQuestion name, three other known tool names, four question or clarification queries, and three task or keyword queries. One task or keyword result preceded the single later AskUserQuestion call in its transcript. No pre-cutover searches were visible. Query categories are heuristic and rely on the stored catalog to recognize tool names.

The latest policy snapshot records AskUserQuestion as callable and deferred, `tool_search` advertised, and the question tool outside the advertised set in 106 of 138 post-cutover parent transcripts, accounting for 8,656 requests. Eleven transcripts record both tools advertised and include the one post-cutover call. A tool can enter the advertised set after discovery, so last-observed policy cannot establish what an earlier request saw.

### Measurement and vote

The denominator is ordinary, capacity-linked usage attempts in the `user` runtime role, dated by request planning time in UTC. The two windows have eight and six observed request dates, respectively. The last post-cutover day is partial. We attributed tool calls by transcript, matching input, output, and both cache usage counters within 120 seconds of the recorded usage time. Parent attribution covered 22,069 of 22,070 pre-cutover response anchors and all 11,616 post-cutover response anchors with usage. There were two post-cutover attempts without a matched anchor and no observed unmatched question or browser calls. The full parent usage ledger had route linkage for both windows. Fixtures covered the cutover, attribution, query categories, and a search result followed by a question call.

**Vote for the compact names hint as part of browser deferral.** Include only permitted deferred names, check that the model can find AskUserQuestion and the browser bundle on eligible Codex routes, and keep eager behavior where discovery is unavailable. Price the hint in the exact replay. Use the parent browser call and transcript rates above as baselines for later rollout measurement.
