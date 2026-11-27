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
