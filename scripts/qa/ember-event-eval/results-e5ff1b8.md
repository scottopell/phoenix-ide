# First comparison: partial, unaccepted for promotion

The first frozen 39-trial comparison is **partial and unaccepted for promotion**.
All three raw-event eight-source control trials reached the six-call per-trial
limit without a final answer. The frozen rule disallows promotion from partial
evidence; favorable measurements in other cases do not override that rule.
There were no automatic retries, oracle changes or production runtime edits.

Source: `e5ff1b80f545e6f6e7a2334e63023cec696d9bdf`.
Test executable SHA-256:
`71a3e2c8bed081ca3096c3730fbfeb47ee4f8b3b6e5dba6b4777de8962b894d1`.
The configured route was `gpt-6-astra`, high effort, Codex connection, standard
service tier. A, B and C used the same actual 28-definition Coordinator registry
and static prompt. A was the raw event, B raw event plus captured evidence, and C
the production hydrated compact projection. All sources were synthetic and used
production occurrence capture, hydration and bounded local read tools.

## Results

All 39 records were retained: 36 final answers and three capped raw-event burst
trials. The invocation made 82 provider calls and processed 946,180 input tokens
including cache reads/writes, below the hard 120-call and two-million observed
input thresholds. All calls reported usage fields. There were no provider errors,
tool errors or denied effectful/remote actions.

Manual review found no source-attribution, authority or content-oracle failure in
the 36 completed answers. The three unfinished answers cannot count as correct.
Across the four targeted cases, source reads were A: 5, B: 3, C: 3, a 40% reduction
for each enriched variant. Pauses and unavailable historical tails remained
explicit; edited current text did not become occurrence-time evidence or approval.

The complete controls below each had three repetitions per variant. Input totals
are accumulated across those repetitions; latency is the median whole-trial time.

| Control | Variant | Input tokens including cache | Median latency, ms | Input change vs A | Latency change vs A |
| --- | --- | ---: | ---: | ---: | ---: |
| One tool-only ending | A | 78,134 | 13,539 | — | — |
| One tool-only ending | B | 56,184 | 10,466 | −28.09% | −22.70% |
| One tool-only ending | C | 33,432 | 5,887 | −57.21% | −56.52% |
| No exact ending carrier | A | 78,053 | 14,350 | — | — |
| No exact ending carrier | B | 66,831 | 8,378 | −14.38% | −41.62% |
| No exact ending carrier | C | 78,033 | 10,938 | −0.03% | −23.78% |

For the eight-event control, A made six sequential evidence reads in each trial
and had no final answer. B and C each answered every repetition in one call,
without additional reads or worker nudges. The recorded A totals were 219,438 input
tokens and 20,959 ms median time to the cap; B recorded 38,652 and 6,930 ms, and C
37,899 and 5,562 ms. These figures are **capped partial evidence, not an accepted
complete-baseline comparison**. They cannot establish the required control gate.

Provider cache state and service load were not controlled. Three control
repetitions offer limited statistical precision. Context tokens do not establish
billed-dollar savings. This harness does not prove full foreground admission,
compaction, remote enrichment or human UI acceptance.

## Preserved evidence and next iteration

Private ignored artifacts remain under the evaluation worktree's
`.phoenix/event-eval/live-e5ff1b8/`. They contain the manifest, all 39 request/tool/
answer records and `analysis.json`; no raw production messages were used.

- Manifest SHA-256:
  `d2db89314208281ba7fbc4ee591cf7378c090507af9a7b96754a6f95d65d4d18`
- Analysis SHA-256:
  `19d025ceedd13306a3d0d96f222ae6d0a6fcd6287b27263ef8bf4d51ac46f482`
- Corpus SHA-256:
  `6a7c73457cc21ae2c0486d698a67d8d1acb88bd6b0a5113e6170d6d1d7601ada`
- Independent oracle SHA-256:
  `0df8c30f5589c83080c791a26849508a2b5c8e4a15d077398f82cf78bacd03c4`

The explicitly authorized next iteration repeats the full 39-trial design with
only the uniform per-trial cap raised from six to twelve. Eight legitimate
individual source reads plus a final answer require at least nine provider calls
when issued sequentially. The
hard global 120-call and two-million observed-input caps, all other limits,
ordering, route, corpus, oracles and promotion thresholds remain unchanged.
The first invocation's failure is retained; selected successful answers are not
retried or substituted. The new prepared manifest requires independent review
before any new model calls.
