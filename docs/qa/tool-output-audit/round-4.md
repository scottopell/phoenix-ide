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
