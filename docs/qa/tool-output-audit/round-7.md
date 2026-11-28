# Round 7: exact replay of browser deferral with the names hint

## Review of round 6 (reviewer)

Accepted. The `ask_user_question` drop after deferral (about 85% per request on Codex) is strong enough evidence,
even with the confounders you named, so the hint ships with browser deferral. Implemented in `7520de328`:

- **Browser tools:** all 12 now return `defer_loading() == true`. Eager routes ignore the flag; this is
  unchanged by ADR-278.
- **`deferred_tool_hint`** appends to the system prompt, only for `Anthropic`, `OpenAiClient` and `Local` discovery and only when at least one permitted (callable) deferred tool exists:

  `\n\nNot yet loaded, available through tool search: <sorted names, comma-separated, at most 40>[ and <k> more]. Search for a tool by name or task before calling it.`

  Hosted OpenAI search and eager routes get no hint. The text depends only on the tool surface,
  so the cached prefix stays stable.

## 1. Exact replay (needed for the PR vote)

On the round-5 cohort (eligible routes with tool-policy evidence), price:
1. the removal of the 12 browser schemas from pre-discovery requests;
2. **plus the hint**, on every request of every hint-mode route, including those that never had
   browser tools, since `ask_user_question` and MCP deferred tools already trigger it. Use the
   exact string above with the names each request would list;
3. **plus one discovery round trip** per browser-using transcript. Default `limit` is 8; the 12 browser
   tools may need two searches. Price one and two.

Give net token-turns and the share of recorded input. Split by route (parent, Work and Explore
sub-agents) and state the cache-read share.

## 2. Follow-up baseline

Record the round-6 baselines (browser and `ask_user_question` calls per 1,000 parent requests, per route) in
`round-7.json` under `baseline`. After this ships, the next audit compares against them.

## 3. Vote

Vote on opening the PR with `7520de328` as is. If the replay shows that two searches are typical, say so:
the fix would be a search-limit default or a `browser` bundle query, and I'll add it before the PR.

## Output

Append `## Results`, write `round-7.json`, vote, and pass the baton.

## Results

### Review and vote

**Withhold approval to open the PR as-is at `7520de328`.** The ordinary dispatcher appends the hint before request accounting. The two complete ordinary-request projections used for continuation review and successor capacity still use the prompt without the hint. Both the local fixed-overhead estimate and the request passed to provider counting omit it. A handoff near the capacity boundary can therefore be marked ready even though its real opening consumes some of the working room reserved under REQ-BED-020b.

Share the same hint application with both successor projections. Use the resolved discovery mode and the permitted catalog before the fresh projection strips deferred declarations. Add a regression at the continuation budget boundary. The production snapshot establishes the projection paths exist and does not show an observed boundary failure.

The static hint says the named tools are “not yet loaded” even after a client discovery receipt has loaded them. Keep the stable names list and describe search as the path for tools that still need loading. This avoids prompting a redundant search after successful discovery.

Local validation passed the browser registration test, all three hint tests, and all 22 provider tool-search tests. Replay fixtures exercised exact hint formatting, default and explicit limits, provider route matching, and hint-only cost.

### Production replay

The snapshot contains **60.995B recorded input tokens**. The modeled cohort has 271,490 ordinary anchors on a qualified Codex model with tool policy evidence. For 165,693 of those anchors, a capacity plan confirms the Codex route. The remainder use the transcript's Codex route context and a matched model and usage record. All capacity-linked ordinary traffic in the snapshot is Codex. Anthropic, hosted OpenAI and local Chat Completions contribute no measured replay rows.

| Discovery strategy | Extra search requests | Net input saved | Share of all recorded input | Net cache-read tokens saved | Capacity-confirmed net input |
|---|---:|---:|---:|---:|---:|
| One browser search with explicit limit 20 | 191 | 782.0M | 1.28% | 667.5M (85.4%) | 500.6M (0.82%) |
| Default eight followed by a full limit-20 search | 382 | 744.2M | 1.22% | 635.1M (85.3%) | 484.3M (0.79%) |
| Default eight with targeted follow-ups as needed | 216 | 787.3M | 1.29% | 672.0M (85.4%) | 505.4M (0.83%) |

| Transcript surface | Modeled requests | Hint input | Net, one search | Net, two searches | Net, targeted follow-ups |
|---|---:|---:|---:|---:|---:|
| Ordinary parent | 159,744 | 16.2M | 464.6M | 436.5M | 467.7M |
| Work sub-agent | 24,507 | 2.2M | 73.1M | 72.6M | 72.9M |
| Explore sub-agent | 58,141 | 5.0M | 156.4M | 147.2M | 158.8M |
| Legacy sub-agent mode | 29,098 | 2.6M | 87.9M | 87.9M | 87.9M |
| Coordinator | 0 | 0 | 0 | 0 | 0 |

Eager browser schemas account for 864.1M of input on replayed requests. The exact hint adds 26.1M, including the question tool where permitted, before any extra requests. The one-search scenario adds 43.3M of carried search exchanges and 12.7M of extra request input. The replay checked for pre-existing deferred catalogs on eligible requests with zero browser schemas. No such requests had a callable search surface in this snapshot. The Coordinator's Codex surface and identified sub-agent terminal turns use eager policy. No modeled hint hit the 40-name cap. Applying Round 5's saved tokens for merged clearing to the newer ledger gives **about 9.80% combined** with one browser search, or **9.34%** with capacity-confirmed browser evidence.

We projected the recorded schema shapes and the exact hint, ranked the deferred catalog with the implementation's scoring and limits, and replayed typed client-search call and output items. Retained tool policy is evidence of earlier catalogs, with positioned changes where available and current callable policy as a fallback. Input tokens are calibrated from text bytes. Cache-read allocation uses the observed request proportions. Provider tokenization, one-time cache-prefix changes, reuse of the nine existing browser-related search queries, and model behavior remain unmeasured.

### Does the default usually require a second search?

Of 191 modeled browser transcripts, 187 (97.9%) would find every browser tool used in the first browser response within the default eight results for a broad browser search. Across the full observed transcript, 173 (90.6%) needed one search, 11 needed two, and seven needed three or more in the targeted simulation. The four initial misses were three parent transcripts and one Work sub-agent transcript. Four of five Work sub-agent transcripts needed a second search at some point, a small cohort. Explicit limit 20 covered every observed call in the replay. The results support keeping the general default at eight. A browser-specific hint for a full bundle can use limit 20 when the task calls for multiple browser tools.

### Follow-up baseline

The JSON carries forward the Round 6 route rates and sample sizes. Codex parent browser calls per 1,000 requests were 7.386 before November 23 and 0.689 after. AskUserQuestion calls per 1,000 were 0.589 and 0.086. The direct OpenAI Responses and Anthropic cells have no observations.
