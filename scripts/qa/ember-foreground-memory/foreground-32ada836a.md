# Accepted foreground quiet and contextual recovery

The complete run at `32ada836a3bc7b7ef891d930a46e68fc515ac424` passed both
[predeclared journeys](README.md). Root full-trace review and a fresh independent full-trace/SQLite/numeric review
accepted both. Earlier evidence remains preserved:
the [initial contextual failure](foreground-c14462de7.md) and the
[read-only harness rejection](foreground-d50bbbf54.md) remain separate,
unqualified runs. The static guidance correction was reviewed after the initial
failure; the complete run used the same policy as the paired correction trial.
No fixture, oracle or limit was relaxed.

## Outcomes

**Cobalt pause and quiet.** The greeting selected due Cobalt while Sage was
recently reviewed. Ember read Cobalt and asked its specific date-range/period
question through `ask_user_question`. One option label exceeded the existing
five-word limit; the real tool rejected it and Ember corrected the labels in
the next normal model call, within the original budget. The actual user pause
was delivered through request-bound `respond_to_question` on the same running
runtime. Ember read the record, CAS-updated revision 1→2 to retain the explicit
resume restriction, and settled. Both requested items were then recently
reviewed. The unrelated arithmetic input produced exactly `13.`, zero tools
and no renewed cue. All current-turn prose, including pre-tool text and the
rejected question attempt, was inspected. Exact turn lifecycle and serialized
settlement checks passed.

**Hidden Harbor recovery.** The same ten records and two real handoffs from the
failed baseline were retained. Harbor remained outside the initial eight-entry
index, with no due cue. Ember read the additional two-entry page, then the actual
Harbor record, original assistant source identity and saved stopping point. Real
global search correctly returned no matches because it excludes the Coordinator
chain. Ember then read the exact original transcript containing the authentic
UserApi request and native checkpoint, and asked the saved question through
`ask_user_question`: which control should regain focus after the dialog closes.
There was no duplicate capture, authority change or prefix rewrite. The final
question wait remained legitimately Accepted; it was not falsely marked
Terminal. The response still included verbose baseline advice and did not give
an explicit Harbor-name refresher; that presentation limitation remains.

The real file-backed protocol/search fixture and existing native checkpoint
regression each passed before preparation. Deterministic search returned exact
ordinary-source transcript/message identities while preserving Coordinator-chain
exclusion. No production search scope was widened and no result was canned.

## Configuration, usage and limits

Discovered model and actual outbound model: `gpt-6-astra`; effort: high
(`Explicit(High)` in all twelve requests); connection: Codex; language:
PhoenixNative; authenticated coordinator skill catalog; actual assembled surface:
29 tools. Original closed seeds, source/target identities, prompts, expected
outcomes and prefixes were unchanged. Only static guidance and the reviewed
harness execution/synchronization corrections differed from the first run.

| Journey | Requests | Request Debug bytes | Reported input tokens | Cache-read tokens | Output tokens |
|---|---:|---:|---:|---:|---:|
| Cobalt pause/quiet | 7 | 1,213,666 | 17,922 | 89,344 | 691 |
| Harbor contextual recovery | 5 | 815,742 | 16,613 | 55,808 | 592 |
| Total | 12 | 2,029,408 | 34,535 | 145,152 | 1,283 |

Including cache-read tokens, the total reported input was 179,687 tokens;
cache-creation tokens were zero. The largest returned provider response was
414 reported output tokens and 1,954 serialized content bytes. Observed tool
cycles were 3/2/0 across the Cobalt greeting/pause/arithmetic turns and 5 for
Harbor.

The complete test took 76.82 seconds. The per-journey caps were ten requests and
2,000,000 Debug bytes; the combined caps were twenty requests and 4,000,000
Debug bytes. Host timeouts were 60 seconds per model request and 120 seconds per
whole turn, with eight tool cycles per turn. No provider retry, skipped case,
fallback or external-effect attempt occurred. The corrected label was normal
recovery from an actual tool validation result, not a retried experiment or
provider request.

Every response reported output usage and fit the accepted 2,048-token/16-KiB
returned-output limits. Codex does not enforce the requested token cap on the
wire; this is not a hard generation/billing cap. Input and cache-read counters
are reported separately rather than treating the latter as zero-cost or omitting
them. Debug bytes are not token counts. Per-request elapsed durations were not
recorded; time bounds were host-enforced and the aggregate test duration was
observed. Full request Debug strings were not retained: their hashes and byte
counts were recorded by the wrapper, so these byte totals cannot be independently
reconstructed from full Debug strings after the run.

## Exact evidence hashes

| Artifact | SHA256 |
|---|---|
| Retained executable | `a7b7000770d72dc383810ede39ca6068fbfec035db107d51042e2f43a1fd5e2d` |
| Approved prepared manifest | `7464857f51a8c475b9f18c760c2f2826879871a416886ca125201b3645f4eef7` |
| Original prepared manifest | `14a05618eb40408c2abfd0d8f1f046ca76ad51f200506b0d1e0ce3da2b49cea8` |
| Completed live manifest | `a818f022b53ba88136e24e3d828a0e44a5103ad4f192ba2f05b0216c980eb420` |
| Tool definitions | `be42ab762e8f1347502189808de9760ba2664fb64789cca82c098e6a3815ed37` |
| Endpoint configuration | `1e44b44ac8908fd78e1e50d1f68a2a0dde267444ecc3c778484e1517da82876d` |
| Cobalt system | `118db5737a95618db15c46ddcccb7de5ec4f80ab5b71a9c46e26d9f5d71cb86f` |
| Harbor system | `7fe7b0501446438aabf922c56f1daf2708a742960e9b6a64d356d71e111ff397` |
| Cobalt closed seed | `70a92156712555226779d1f3db6c5d9836149ab07da16d44d44729b0006312dd` |
| Harbor closed seed | `192773ad71719dab50623246ef189110512f9e766943307197bf62f5bbb3dc64` |
| Cobalt full trace | `17d8db0da377bd9ed977dff223bb12ce56e0ca6dfd4205dac4ea8fb07751e134` |
| Harbor full trace | `a3756a27ddb933ada9672e7502e010def9618c584edcf5b2d1c19c990cab7e83` |

Full outputs and databases remain private ignored artifacts. The completed
manifest's original `recorded_pending_independent_review` label is preserved;
review acceptance is recorded here rather than rewriting its bytes.

## Scope of acceptance

This is two fixed synthetic scenarios, not a statistical reliability estimate,
relevance-ranking benchmark, production deployment or UI/HTTP acceptance. It
exercises actual admitted runtime turns, real structured reply validation,
source recovery, native tracking CAS, bounded reads and prefix/authority checks.
It does not exercise scanner lifecycle/composition, worker health/subscriptions,
remote workers, process-crash recovery or phone UI. Final integrated inventory,
scanner qualification, rebased checks and CI remain separate program gates.
