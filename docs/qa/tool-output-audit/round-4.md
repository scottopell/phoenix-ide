# Round 4: exact replay of revised A as implemented, then pick the next candidate

## Review of round 3 (reviewer)

Accepted. I agree on all three points.
- **B:** your merge vote is recorded. I vote **agree** on the B commits (`a9e1564b3`, unchanged since). The merge
  waits on CI and review of PR #1204.
- **Revised A:** implemented in `88f2aa537` with the continuation binding you asked for:
  - live requests render `[tool result cleared; read_conversation @self#seq-<n>]`;
  - continuation input (`render_existing_cleared_messages`, and the continuation call of
    `assemble_cleared_messages_with_pressure`) renders
    `[tool result cleared; read_conversation @transcript:<conversation>#message-<message>]`;
  - `read_conversation` resolves `@self#seq-<n>` against the caller's tool-context transcript, local
    route only; remote validation rejects it;
  - tests cover exact-message retrieval, a missing row in another transcript, invalid sequences
    (`0`, negative, non-numeric), and the canonical rendering for continuation.
- **The growth guard is deferred**, as you suggested. A brings the overhead down to 0.017%.
- **The bash echo after Nov 24 (64% of run results)** most likely reflects when the change was deployed. Since no
  deploys are allowed, it will drop as the change rolls out. No action.

## 1. Exact replay of A as implemented (needed for the merge vote)

Use the exact strings above on top of B as implemented. Count the canonical form for any
cleared result rendered inside a continuation request. If the ledger cannot separate
continuation requests from live ones, give the share of requests that are continuation requests and price them with the
canonical form.

Report the token-turns saved and the share of recorded input, the sum with B, and **your merge vote
on the combined PR**.

## 2. Next candidate

With B and A in, re-rank the remaining candidates on the new baseline. Price the browser deferral
in particular. The tool definitions are measured in `overhead.md`: about 11.3KB for the 12 browser tools on every request.
Use your round-1 usage figures (344 transcripts, 2.71%, call browser tools) and
estimate the saving if those definitions load only after a `tool_search`, net of the extra
`tool_search` round trip in transcripts that use the browser. Include whether those requests
are cache reads, so we can state the saving in tokens and, roughly, in cost.

Also re-rank against the new baseline:
- bounded bash results with retrieval;
- long-line shortening;
- `read_file` line-number prefixes.

Recommend one candidate for round 5.

## Output

Append `## Results` here, write `round-4.json`, vote, and pass the baton.

## Source review while replay runs

The worker is withholding the combined merge vote at implementation 88f2aa537 until these are fixed.

- **P1 · A marker advertises an unavailable reader on Explore and ordinary sub-agent surfaces.** Live rendering emits the caller-relative locator regardless of the request's callable tools. The restricted Explore and ordinary sub-agent registries include clearable tools such as file reads and search, but lack the transcript reader. A stale result therefore points the agent to an operation it cannot call. Cover these registry entry points with a caller-scoped exact reader or a reachable locator consistent with their existing authority.
- **P2 · Canonical continuation rendering can expand a patch preview.** Preview eligibility is decided by the short live form. Both continuation render paths reuse that eligibility and substitute the longer canonical marker. A fixture with a valid receipt and single hunk satisfies live-cleared size below full size below continuation-cleared size. If the row is in the selected continuation suffix, this violates the strict shrink rule in REQ-STR-013. Use a consistent gate across the emitted forms or price and enforce the canonical gate at continuation rendering.
- **CI · Clippy and musl both reject the size of the message variant in TerminalEvidenceExpectation.** The focused macOS tests passed for B. The Linux lint gate still needs resolution at the combined PR head.

The worker holds Round 4 while the offline replay and candidate comparison finish.

## Results

Read-only replay against **60.872B recorded input tokens**. The comparison isolates output formatting and tool definitions under the reconstructed sweep schedule. The re-ranked options remain counterfactual.

### Exact marker formats

- **A saves 1,882.0M token-turns (3.09%)** in ordinary requests. Existing markers contribute 1,643.5M and existing patch preview markers 238.5M. The smaller marker also admits 3 patch previews, saving 27.4k additional token-turns.
- Ordinary response anchors number 562,768. The continuation messages have no positive-usage anchors in this snapshot. Capacity telemetry tags 891 compaction attempts (0.16% of ledger rows). Timestamp alignment places 891 of those attempts against recorded message order, with 70.0M input.
- Pricing every eligible pre-existing marker at those continuation positions with the canonical form gives a ceiling of 71,272 occurrences and 0.7M additional savings (0.001%). The selected bounded suffix is absent from durable history, so include zero to that ceiling for the timestamp-aligned compactions. The earlier untagged corpus cannot be placed.
- B contributes 3,157.7M. Together B and A model **5,039.7M to 5,040.4M token-turns (about 8.28%)**. The lower combined estimate leaves 55.832B input.

### Review evidence and vote

| Tool-policy evidence | Requests with a marker | Without an advertised reader | Sub-agent | User |
|---|---:|---:|---:|---:|
| positioned history | 158,930 | 12,560 | 12,156 | 404 |
| latest snapshot proxy | 1,793 | 0 | 0 | 0 |
| no tool snapshot | 121,095 | unknown | unknown | unknown |

One timestamp-aligned continuation had a patch result that would grow under the canonical format if retained in its suffix. This is an upper count because exact compaction suffixes are unavailable.

**Combined PR merge vote · Disagree at the reviewed A implementation.** The patch-preview vote stays affirmative on its own. Publish a caller-scoped reader across the affected live surfaces and make the continuation shrink check agree with the bytes it emits. The PR also needs green CI and clean review.

### Browser deferral

Observed 344 browser-calling transcripts (2.70%). Tool policy is available on 304,411 ordinary anchors. 271,296 of those had active eager browser definitions.

| Evidence scope | Pre-discovery requests priced | Extra discovery requests | Net client-side input | Net stub input |
|---|---:|---:|---:|---:|
| positioned history | 255,622 | 186 | 803.6M | 474.4M |
| latest snapshot proxy | 3,083 | 6 | 9.9M | 5.8M |

- Client-side scenario removes 826.9M schema tokens and charges 13.3M for one discovery turn per observed browser-using transcript with active definitions. Net is **813.5M (1.34% of recorded input, 1.46% of the rebased input)**. The metadata-stub scenario nets 480.2M (0.86% rebased). A blanket full-bundle scenario including uninstrumented rows gives 1,587.4M (2.84% rebased) and overstates exposure.
- 93.42% of the modeled savings requests had cache reads. Their observed input was 86.05% cached. Proportional allocation attributes 705.2M of gross schema savings to cache reads, with 11.2M cached input charged back for discovery.
- At a 0.10× cache-read weight the net client-side estimate is 188.9M fresh-input-equivalent tokens or 1.31% of weighted recorded input.
- At a 0.50× cache-read weight the net client-side estimate is 466.5M fresh-input-equivalent tokens or 1.33% of weighted recorded input.

The first browser invocation stands in for discovery timing. The extra call uses that response’s recorded input mix and adds 256 fresh tokens for exchange content. The model keeps the entire browser bundle loaded after discovery and does not allocate cache by wire segment. The production route metadata supports a client-side discovery scenario. Exact payload and additional discovery behavior still need replay.

### Remaining candidates on the B+A baseline

| Candidate | Modeled savings | Share of rebased input | Risk or qualification |
|---|---:|---:|---|
| Bound Bash output at 16 KiB with retrieval | 3,560.3M | 6.38% | Gross ceiling before retrieving omitted content. Preserve durable diagnostics. |
| Shorten long Bash lines | 1,222.2M | 2.19% | Overlaps the Bash cap. Long records can contain diagnostic evidence. |
| Defer browser definitions with client discovery | 813.5M | 1.46% | Net of one discovery request. Scope comes from tool policy evidence. |
| Use compact read-file range numbering | 701.2M | 1.26% | Gross ceiling before the new range header. Keep exact line anchors. |
| Remove only read-file number padding | 280.1M | 0.50% | Preserves anchors. Whitespace can already tokenize cheaply. |

**Round 5 recommendation · Test browser deferral first.** It changes discovery instead of the stored evidence. Replay the actual client-side tool payload and confirm first discovery from eligible parent and sub-agent routes.

Validation passed for 21 runtime clearing tests and both direct locator tests. Those tests cover ordinary lookup and remote rejection. The two findings above follow the built-in registry and continuation paths outside that coverage.
