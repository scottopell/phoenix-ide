# First scanner qualification: rejected

Root and a fresh independent reviewer rejected the complete 37-case run at
`ec570d0f3e17fb0c5cc3aaf80ed3c2b40f667f6a`. All calls and host applications
completed, but six cases failed frozen semantic requirements. This run does not
qualify scanner activation. Earlier host durability and foreground usefulness
checks remain separate evidence.

## Frozen configuration

Actual catalog/outbound model `gpt-6-luna`, Low effort, Codex, no tools. One call
per case, no retries, skips or fallback. Limits were 37 calls, 1 MiB accumulated
logical content, 24 KiB per request, 30 seconds per call, 2,048 accepted reported
output tokens and 16 KiB accepted text. Codex does not enforce a generation or
billing ceiling. These gates, corpus and oracles preceded inference.

- Source and workspace: `ec570d0f3e17fb0c5cc3aaf80ed3c2b40f667f6a`.
- Retained binary SHA256: `1cc672ca75078eaf60f0d652418fe8df608d1154923770fe4afe11bc59cc6e38`.
- Approved prepared manifest: `9057badc7ac30f3eab61395569b014d497f2afe24c86e914edf8782daf7e230e`.
- Live manifest: `17724f895429bf454fac66afc4535cdbe9118dedbc57a4adcfb5655bd3ff7cfd`.
- Corpus: `869882606a01834cb2e45a3f45490bba5d2f9ca025551c42f94210e5e5af3ea4`.
- Oracles: `d195c43320a31a452abc110d54ab95df71d1547b84f2870b1e1fb61f30edcb14`.
- System instructions: `ad9671d8d1596dee15a3cb0f52203a3965ea6f4cf4d1bc843c22d719f4dda7b5`.
- Endpoint fingerprint: `30eeb3ad95185622a2a26c0a9f6de4f542773e3ebffdf38b3ad85542c5002fe7`.

A preflight manifest with category-labelled source IDs was blocked before any
calls. This actual trial used UUIDs generated at initial admission; logical
aliases stayed in hash-bound host evidence. All 37 prepared inputs were reviewed
independently. Only actual DB-prepared ScanInput entered inference; no oracle,
withheld cancellation or omitted-candidate content was supplied.

## Independent result

| Gate | Result |
|---|---|
| Requested outcomes, minimum 10/11 |10/11; pass |
| Bookmark outcomes, minimum 4/5 |5/5; pass |
| False requested-work commissions, required 0 |2/21; fail |
| No-new-item negatives |14/17 correct; fail |
| Mandatory target-bound restrictions |10/13; fail |
| Visible duplicates, maximum 1/11 |1/11; pass, with independent D10 failure |
| All proposed quotations valid |29/29; pass |
| Unknown apparent request / machine negative |L01 provisional 1/1; L02 no capture; pass |

R04 discarded an explicit structured implementation answer and its read-only
restriction. N03 and N05 discarded rejection/stop evidence for existing items,
leaving their old reviewed stopping points intact while advancing coverage.
N06 captured an already answered question. N08 recreated an explicitly completed
and verified outcome. D10 invented a later renewal even though its request
preceded retirement, creating a prohibited active duplicate.

All 27 actions applied: 21 new records and six evidence appends. Every new record
remained historical and review-required, with null review time. Old retirement
history/dispositions stayed unchanged: there were no structural reopen writes,
but D10's invented renewal still failed source/authority semantics. No message
or foreground-observation count changed.

R08 correctly created a distinct provisional renewal without reopening the old
row. H01 separately captured historical work without inventing knowledge of its
withheld cancellation. I01 created one separately reported invisible duplicate;
its matching candidate was genuinely omitted. U01 retained incomplete coverage
at UTF8 offset 4232 and exact request evidence at bytes 1025–1094.

## Accounting and correction

37 calls; zero adapter failures or retries. Observed usage: 43,225 input tokens,
zero cache tokens and 3,895 output tokens. Logical content totaled 196,341 bytes,
maximum 8,956 per request. Maximum output was 313 reported tokens and 1,213
serialized response-content bytes. Summed case inference time was 115,126 ms;
maximum 11,459 ms. The complete test took 120.13 seconds. Private raw responses,
proposals, all 37 record hashes and before/after databases remain retained.

The proposed general instruction correction distinguishes new intent from
mandatory changed evidence, recognizes substantive answers, avoids answered or
completed work, and compares source dates against retirement dates. It requires
a newly approved paired run using the same closed seeds, sources, candidates,
oracles and thresholds. This report and the unsuccessful run remain unchanged;
no improvement is claimed before that qualification.
