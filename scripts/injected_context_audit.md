# Injected-context audit: brief for the agent running it

## Why this exists

Phoenix clears stale **tool results** from the LLM context (REQ-STR-002), but
every **injected** user-role message stays in the context until the transcript ends. Injected messages include skill bodies,
Coordinator watch events, follow-up approvals (which embed the whole plan),
sub-agent result summaries, cross-conversation messages, Slack/remote input,
scheduled reminders and continuation summaries. Each one is re-sent, and billed
(usually as cache reads), on every later request.

We want to know, from real production data, **how much of the input-token bill
these messages cause and which fixes would pay off most**. Candidate fixes
already on the table:

1. Dedup or clear repeated skill bodies (the same skill invoked twice is
   carried twice).
2. Replace the full plan in "Follow-up task approved" messages with a pointer.
3. Collapse superseded Coordinator watch events, or batch events that arrive
   close together into one wake.
4. Cap sub-agent summaries and Close-failure text.

The numbers decide which of these to build. If none of them is worth building,
say so.

## How to run it

The script is read-only, uses only the standard library and needs only `uv`:

```bash
# Live production DB: --snapshot takes a consistent in-memory copy first (SQLite backup API)
uv run scripts/injected_context_audit.py ~/.phoenix-ide/prod.db --snapshot \
    --json /tmp/injected_audit.json > /tmp/injected_audit.md

# Optional: limit the window (ISO date, compared against messages.created_at)
uv run scripts/injected_context_audit.py ~/.phoenix-ide/prod.db --snapshot --since 2026-09-01
```

Other flags: `--top N` (rows in section 5), `--cache-read-mult` and
`--cache-write-mult` (defaults 0.1 and 1.25, the Anthropic prices relative to fresh input), `--burst-seconds` (default 60),
`--large-bytes` (section 9 threshold) and `--default-tokens-per-byte` (used when
a transcript has too few requests to fit).

**Rules:**
- Do not write to the production DB. The script opens it with `mode=ro`. Keep it
  that way if you modify the script.
- **Keep transcript content out of what you report.** Report categories, counts, token
  figures, skill names, tool names and truncated (8-char) transcript IDs. Never
  quote message text, user prompts, file paths, repo names or Slack content.

## What each section measures

| § | Measures | Read it as |
|---|---|---|
| 1 | Corpus size, requests, input-token totals split fresh / cache-write / cache-read | Sanity: does it look like the real usage? |
| 2 | Per-transcript least-squares fit `total_input = a + b·bytes` | `b` ≈ tokens per byte (expect about 0.25–0.35); `a` ≈ system prompt plus tools per request |
| 3 | Carry cost per category: estimated tokens × the number of later requests that re-send the message ("token-turns"), plus a cache-weighted version | **The main result.** Share of all carried context by source |
| 3b | Same as §3, split into coordinator transcripts and ordinary/sub-agent ones | Coordinator waste and skill waste usually live in different places |
| 4 | Mix of the final request in each transcript | What the context looks like at its largest |
| 5 | Top-N individual injected messages by token-turns | Which specific skills or event streams dominate |
| 6 | Redundancy: a skill injected again in the same transcript, a watch event superseded by a later one for the same watch and source, events that report a question no longer pending, and follow-up approvals that copy the plan | **Upper bound on the savings** from fixes 1–3 |
| 7 | Requests whose trigger was injected input rather than a user message, and event pairs within `--burst-seconds` | Savings from batching (fix 3) |
| 8 | Injected share of the context when a transcript was continued | Whether injected messages push transcripts into continuation |
| 9 | Cache read and write share on the request after a large injection | Whether injections break the prompt cache, which costs more than carrying them |

## Known limits (state them, do not hide them)

- Token counts are estimates from the §2 fit (±25%). Images count as 1,500
  tokens each.
- `tool_result` carry is an **upper bound**. Only the final `clear_watermark` is
  stored, so earlier clearing cannot be reconstructed.
- The §7 trigger is a heuristic: the last non-assistant, non-tool message before
  each request.
- Older rows with `origin_kind = unknown_historical` (and remote origins with no
  other signal) are counted as `user_typed`. If that bucket looks too large,
  injected content is probably hiding there.
- Usage figures come from `messages.usage_data` and follow Anthropic semantics
  (`input + cache_creation + cache_read`). Providers that report differently
  will skew §2 and §9; check the per-provider spread if the fit looks odd.

## Modifying it

- Classification is in `classify()`. Add a category there and to the `INJECTED`
  set if you find injected content that is miscounted (for example a text prefix
  that marks a new kind of system message).
- Redundancy keys (`skill:<name>`, `watch:<watch_id>:<source_transcript_id>`)
  are built next to classification.
- If the script crashes on a schema it does not expect, fix it rather than
  dropping the data, and record the change in your report.

## Insights to report back (answer each explicitly)

1. **Top categories:** the top five categories by token-turns, with their shares,
   both raw and cache-weighted. Give the injected categories' combined share of
   all carried context.
2. **Coordinator vs ordinary (§3b):** where does injected cost sit in each kind of
   transcript?
3. **Top 10 messages (§5):** category, detail (skill name, watch, or tool name),
   tokens, carries. Which skills appear more than once?
4. **Duplicate skills (§6):** total token-turns and the share of all carry. Which
   skills account for most of it?
5. **Superseded and stale events (§6):** token-turns and share. Is collapsing
   events worth building?
6. **Follow-up plan copies (§6):** token-turns and share.
7. **Wakes and batching (§7):** turns started by injected input per category, the
   burst count, and the estimated input tokens batching would save. Give that as
   a share of total input.
8. **Continuations (§8):** median and p90 injected share at continuation. Are
   injected messages a main reason transcripts hit the context limit?
9. **Cache behaviour (§9):** after large injections, is the cache-write share
   noticeably higher than on other requests?
10. **Calibration sanity (§2):** tokens per byte and fixed overhead. Do the values
    look plausible? Call out outlier transcripts.
11. **Unclassified data:** any large `system_other` bucket or suspiciously large
    `user_typed` messages (p95 above ~5k tokens). Give the likely source and the
    script change that would classify it.
12. **Recommendation:** rank fixes 1–4, and anything new the data shows, by
    expected savings as a share of total input tokens. Give a one-line rationale
    for each, and say which ones are not worth building.

Deliver the answers as a short markdown report. Attach the `--json` output too. The
script never emits message text and truncates transcript IDs to 8 characters; if you
modify it, check the output again before sharing.
