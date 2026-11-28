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
