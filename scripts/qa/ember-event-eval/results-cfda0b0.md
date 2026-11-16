# Second comparison: compact projection accepted for component promotion

Independent answer and metric review, including a fresh full-39-trial audit,
accepted C, the compact production hydration, for component promotion. Both B
and C meet every frozen gate in this complete invocation. Integration, exact-head
CI, merge and deployment are separate gates. The [first invocation](results-e5ff1b8.md)
remains preserved as partial and unaccepted.

## Frozen design and results

The second invocation repeated all 39 trials: four targeted cases once per
variant and three controls three times per variant, with rotating A/B/C order.
A is the original raw packet; B adds production occurrence-bound evidence; C
uses production compact hydration. Every occurrence used real capture/settlement
and local read tools on an isolated synthetic database. The event-heavy control
admitted eight distinct source executions to one Coordinator context.

The only experiment-policy change was a uniform per-trial cap of twelve rather
than six. Eight sequential ending reads plus a final answer need at least nine
provider calls. The normal route (`gpt-6-astra`, high effort, Codex connection,
standard service tier), complete real registry, system prompt, corpus, independent
oracles, ordering and thresholds stayed fixed. The hard global limit stayed
120 calls and two million observed input tokens, with 1,536 output tokens and
60 seconds per call. No automatic retries or selected-answer substitutions ran.

All 39 trials reached final answers: 88 provider calls, 1,027,411 input tokens
including cache reads/writes, no caps, no provider/tool errors and no denied
effectful or remote actions. All calls reported usage fields. Independent review
found zero source-attribution, authority or content-oracle failures. Responses
preserved user pauses, distinguished execution ending from broader completion,
reported historical evidence gaps and did not obey quoted deployment suggestions.
All nine burst answers avoided unsupported readiness claims and worker nudges.

Targeted source reads were A: 5, B: 3 and C: 2, reductions of 40% and 60% versus A
(the frozen minimum was 25%). Each complete control passed the frozen maximum
10% input and 20% median-latency regression limits:

| Control | B input change | B median-latency change | C input change | C median-latency change |
| --- | ---: | ---: | ---: | ---: |
| Tool-only ending | −62.59% | −47.92% | −62.70% | −62.55% |
| No exact carrier | −14.34% | −2.49% | −14.60% | +11.52% |
| Eight-event batch | −88.44% | −85.18% | −88.66% | −83.47% |

## Complete per-case measurements

Inputs and calls below accumulate across the listed repetitions. Latency is the
median complete-trial time. Source reads include actual event/history/query/
search/reference retrieval calls; all observed source reads in this run were
`read_event_evidence` or `read_conversation`.

| Case | Variant | Trials | Provider calls | Source reads | Input tokens including cache | Median latency, ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| useful_prose | A | 1 | 2 | 1 | 22,262 | 8,407 |
| useful_prose | B | 1 | 2 | 1 | 22,574 | 8,553 |
| useful_prose | C | 1 | 1 | 0 | 11,174 | 3,711 |
| delayed_delivery | A | 1 | 2 | 1 | 22,251 | 7,164 |
| delayed_delivery | B | 1 | 2 | 1 | 22,531 | 9,291 |
| delayed_delivery | C | 1 | 2 | 1 | 22,467 | 6,205 |
| paused_work | A | 1 | 2 | 1 | 22,253 | 10,220 |
| paused_work | B | 1 | 1 | 0 | 11,204 | 3,153 |
| paused_work | C | 1 | 1 | 0 | 11,173 | 2,238 |
| edited_omitted_tail | A | 1 | 3 | 2 | 34,374 | 13,239 |
| edited_omitted_tail | B | 1 | 2 | 1 | 23,002 | 12,583 |
| edited_omitted_tail | C | 1 | 2 | 1 | 22,938 | 7,250 |
| tool_only | A | 3 | 8 | 5 | 89,614 | 15,083 |
| tool_only | B | 3 | 3 | 0 | 33,522 | 7,855 |
| tool_only | C | 3 | 3 | 0 | 33,429 | 5,649 |
| no_exact_carrier | A | 3 | 7 | 4 | 77,997 | 10,209 |
| no_exact_carrier | B | 3 | 6 | 3 | 66,816 | 9,955 |
| no_exact_carrier | C | 3 | 6 | 3 | 66,606 | 11,385 |
| event_heavy_tool_only_batch | A | 3 | 27 | 24 | 334,625 | 33,341 |
| event_heavy_tool_only_batch | B | 3 | 3 | 0 | 38,667 | 4,942 |
| event_heavy_tool_only_batch | C | 3 | 3 | 0 | 37,932 | 5,511 |

All three raw-event burst repetitions completed in nine calls (eight evidence
reads followed by the answer). Each enriched burst trial answered in one call.
The unmodified no-carrier A and C inputs are identical; their observed differences
reflect model/order/cache variability rather than a changed event packet.

## Identity, retained evidence and limits

Qualified source: `cfda0b0b41d950f498abe4aea32b6cb6ed7727d6`.
The prepared-manifest comparison showed only the executable hash, uniform
six-to-twelve cap and derived potential-call sum changing. The potential sum is
468, but the hard global 120-call cap remained authoritative. Raw synthetic trial
records remain privately under `.phoenix/event-eval/live-cfda0b0/` in the evaluation
worktree; no production transcripts, IDs, credentials or DB artifacts are published.

| Artifact | SHA-256 |
| --- | --- |
| Executable | `7ede239b4dfa41fe648e76fdc7ee7069850c425bad47b420f96b543c471ca029` |
| Prepared manifest | `f81fd995c7a7b80d21faf9f60bfdd7c626c32af881ab19f8ab4160f310e1830b` |
| Complete live manifest | `49242b5ae9d957d62e17578407cba915ba33beca91b1968824bede9965f826f9` |
| Analysis | `aaee4be3adb0a7e567fed48d10c4abc78150d8cd349565f6e3e120db545864eb` |
| Corpus | `6a7c73457cc21ae2c0486d698a67d8d1acb88bd6b0a5113e6170d6d1d7601ada` |
| Independent oracle | `0df8c30f5589c83080c791a26849508a2b5c8e4a15d077398f82cf78bacd03c4` |
| Actual 28-tool registry | `30c287e28544588a56cde53d74ec0c80aa7daca84f425fb307d17d2783651b11` |
| Static system prompt | `787109b6cc371b0b51a0a62105c918bfc5a529ab6e3c4af070c4774bc82966f0` |
| Endpoint configuration | `1e44b44ac8908fd78e1e50d1f68a2a0dde267444ecc3c778484e1517da82876d` |

This qualifies the synthetic local component, not production usage, full
foreground admission, compaction, remote enrichment, physical-phone behavior or
all combined Ember features. Provider cache state and service load were not
controlled, and three control repetitions offer limited statistical precision.
Context tokens describe processing, not billed-dollar savings. The 28-tool count
is a frozen experiment fact, not authority for a later assembled surface.
