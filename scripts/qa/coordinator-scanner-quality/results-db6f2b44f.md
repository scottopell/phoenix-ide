# Paired scanner qualification: rejected

Root and a fresh independent semantic reviewer rejected the complete paired
37-case run at `db6f2b44f973943312ab2f23b7f41ad04248420a`. Thirty-five cases
passed, but two old requests were recreated despite visible retirement. This run
does not qualify scanner activation. The [first rejected run](results-ec570d0f3.md)
and its raw evidence remain unchanged.

## Frozen comparison

All 37 actual prepared ScanInput payloads were byte-identical to the first run.
The same closed source databases, UUID identities, source aliases, clocks,
candidates, corpus, independently reviewed oracles and thresholds were reused.
Only the general system instructions changed. Oracle/withheld/omitted source
material did not enter inference. The normal host prepared and reserved each
batch, then validated and atomically applied the actual model response.

Actual catalog/outbound model: `gpt-6-luna`; Low effort; Codex; no tools. The run
used one call per case, with no retry, skipped case or fallback. The original
limits remained 37 calls, 1 MiB aggregate logical content, 24 KiB per request,
30 seconds per request, and accepted output at most 2,048 reported tokens and
16 KiB UTF8 text. Codex does not enforce a generation or billing ceiling.

| Artifact | SHA256 |
|---|---|
| Retained executable | `23439600a858fd70dca44075176dc59662dc2e7cb7d241265191d710453fcad5` |
| Approved prepared manifest | `7218f3d8a40528857f4170ac71febbd229a1bc33ef7b7b639600cd9fc54cb984` |
| Completed live manifest | `37547493d66493ee5cb67c3e2a251c694a72a2eed24bb080c0f82351244fc63e` |
| Original seed manifest | `9057badc7ac30f3eab61395569b014d497f2afe24c86e914edf8782daf7e230e` |
| Protocol | `ceaa794b4bb26ccb812eb5e78991edbf9bb4fe5262ad50cf67b598643aeaabc4` |
| Corpus | `869882606a01834cb2e45a3f45490bba5d2f9ca025551c42f94210e5e5af3ea4` |
| Oracles | `d195c43320a31a452abc110d54ab95df71d1547b84f2870b1e1fb61f30edcb14` |
| System instructions | `1b100d3613bbf3c14527f801f810be86854a10b1bbf0558c51c857f60f8a6518` |
| Endpoint fingerprint | `30eeb3ad95185622a2a26c0a9f6de4f542773e3ebffdf38b3ad85542c5002fe7` |

## Independent results

| Gate | Result |
|---|---|
| Requested outcomes, minimum 10/11 | 11/11; pass |
| Bookmark outcomes, minimum 4/5 | 5/5; pass |
| Mandatory target-bound restrictions | 13/13; pass |
| All proposed quotations valid | 33/33; pass |
| False requested-work commissions, required zero | 2/21; fail |
| No-new-item negatives | 15/17 correct; fail |
| Visible duplicates, maximum 1/11 | 2/11; fail |
| Structural reopening of old records | Zero; does not excuse semantic renewal failures |

D08 and D10 created new requested-work records for older source requests whose
matching candidates were visibly retired. D10 additionally asserted that its
request followed retirement despite the supplied source date preceding the
retirement date. Exact quotation validity and unchanged old dispositions do not
make these invented renewals legitimate. Both failures count against the frozen
semantic/source-authority gates.

The other 35 cases passed independent review, including the structured substantive
answer, stop/abandonment evidence, answered question and completed-outcome cases
that failed in the first run. Mandatory restriction evidence was retained on its
proper target. The later explicit renewal remained distinct from its closed old
record; Unknown provenance, the historical outside-window cancellation, genuinely
omitted candidate and partial Unicode source retained their separate judgments.
This is not general reliability or deployment evidence.

All 31 actions applied: 21 new records and ten evidence appends. New records
remained historical and review-required. Old retirement histories/dispositions
were not rewritten. No source-message or foreground-observation count changed.
Raw model proposals and actual accepted host effects remain separately retained;
no rejected or problematic proposal was removed from scoring.

## Accounting and next boundary

The run made 37 calls with zero adapter failures or retries. Observed usage was
52,882 input tokens, zero cache tokens and 4,351 output tokens. Logical inference
content totaled 254,320 bytes, maximum 10,523 per request. The largest output was
362 reported tokens and 1,250 serialized response-content bytes. Summed case
inference time was 118,416 ms, maximum 8,294 ms. Logical bytes are not token or
billing estimates. Original manifests, all case record hashes, raw responses and
before/after databases remain retained unchanged in private ignored artifacts.

A host-derived, typed retirement/source chronology projection is planned so the
cheap model does not have to compare long timestamp integers. It would identify
which supplied fragments are strictly later than each retired candidate;
temporal eligibility alone would establish neither renewal intent nor authority.
That correction is not qualified by this run. It requires source/host tests,
a new exact prepared-input review and explicit approval before another bounded
trial. The corpus, oracles and thresholds remain unchanged; neither recorded
completion nor improvement on other cases constitutes acceptance.
