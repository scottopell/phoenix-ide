# Scanner qualification: accepted synthetic corpus

Root and a fresh independent reviewer accepted the complete frozen 37-case run
at `e978b7686ef883f0f292cfae0db621356acde1f2` after inspecting all raw proposals,
exact quotations and actual SQLite effects. Every preregistered gate passed.
The implicit Ruby bookmark was missed, within the allowed bookmark threshold;
a separately measured duplicate was created when its matching candidate was
outside the supplied candidate window. Neither limitation is hidden by acceptance.

This is bounded synthetic evidence, not a general production accuracy estimate,
deployment result or physical-device test. Host durability, foreground usefulness,
final CI/review and merge are separate gates.

## Frozen configuration and artifacts

Actual catalog/outbound model `gpt-6-luna`, Low effort, Codex; no tools. One call
per case, no retries, skips or fallback. Limits remained 37 calls, 1 MiB accumulated
logical content, 24 KiB per request, 30 seconds per call, 2,048 accepted reported
output tokens and 16 KiB accepted text. Codex does not enforce a generation or
billing ceiling. Unknown usage would remain unknown rather than become zero.

| Artifact | SHA256 / identity |
|---|---|
| Compiled source and preparation workspace | `e978b7686ef883f0f292cfae0db621356acde1f2` |
| Retained executable | `9e86b076a1d1d487967530512262e254c15d9d695a3181b867faa2e4496b1b51` |
| Original prepared seed manifest | `9057badc7ac30f3eab61395569b014d497f2afe24c86e914edf8782daf7e230e` |
| Independently approved prepared manifest | `288329e42dd2dc4c9e28a4a765de4c48a8e7cb9c49349dfa8822aed5004c7347` |
| Completed live manifest | `6f190a5383df00445282e3df1c5fd7fdd117bc12ece5601a38ba9eaeafcf5ed9` |
| Protocol | `404e6b8c89c374637a7f34eab7b192cebe9fa6e573bb45051029c3263142b628` |
| System instructions | `349c2361920146db802878c7e7e080a5578ce58ea48466116431dbc49448fe1d` |
| Input/result DTO source file | `02b6da56c2b55e79accb3900662d642c6497442b321de3a5fe4c6cd6cee63aff` |
| Corpus | `869882606a01834cb2e45a3f45490bba5d2f9ca025551c42f94210e5e5af3ea4` |
| Oracles | `d195c43320a31a452abc110d54ab95df71d1547b84f2870b1e1fb61f30edcb14` |
| Endpoint fingerprint | `30eeb3ad95185622a2a26c0a9f6de4f542773e3ebffdf38b3ad85542c5002fe7` |

All 37 original closed seed databases were copied, with zero scanner attempts.
Root compared every table row against the original seeds: source prose, roles,
origins, IDs, native candidates, retirement history, aliases and clocks were
unchanged. Physical SQLite file hashes may differ while table rows remain equal;
each actual copied file is bound by the new manifest.

The only model-input transformation replaced candidate `disposition` and raw
`retired_at_us` with a tagged state. Retired state lists the supplied fragments
whose source times are strictly later than retirement. Equality is excluded;
empty means none. This is a chronology fact, not renewal intent, semantic matching
or authority. The complete new input was checked against that exact deterministic
transformation of the original artifact. All other input fields were unchanged.
D08 and D10 had empty later-source lists; R08 retained its genuinely later request.

Actual DB preparation produced every input. Root and a fresh reviewer approved
all 37 payloads before inference. UUIDs were assigned at initial admission;
logical case/source aliases remained only in hash-bound host review records.
Oracle fields, withheld cancellation and omitted-candidate content were excluded.
The original prepared databases were not mutated by inference/application.

## Independent result

| Frozen gate | Accepted result |
|---|---|
| Primary requested outcomes, minimum 10/11 | 11/11 |
| Bookmark outcomes, minimum 4/5 | 4/5; B04 missed |
| False requested-work commissions, required zero | 0/21 |
| No-new-item negatives | 17/17 correct |
| Mandatory target-bound restrictions | 13/13 |
| Visible duplicates, maximum 1/11 | 0/11 |
| All proposed quotations host-valid | 29/29 |
| Authority violations / structural reopens | Zero / zero |
| Unknown apparent request / machine negative | L01 provisional capture; L02 no capture |
| Outside-window cancellation | H01 historical/unverified capture; no invented later knowledge |
| Omitted matching candidate | I01 duplicate 1/1, reported separately |

All 27 actions applied: 18 new records and nine evidence appends. New records
remained historical and review-required, with null review times. Appended
restrictions invalidated the prior reviewed stopping point through the evidence
watermark; existing note text, review watermarks, dispositions and retirement
history were not overwritten. The R08 renewal created a distinct unreviewed
record, leaving its older dismissed record closed.

Every case had one ordinal-one attempt. No source message or foreground
observation count changed. Scanner capture did not admit a foreground turn or
execute requested work. These state checks are distinct from the model's prose.

## All 37 cases

| Case | Observed behavior |
|---|---|
| R01 | Captured two separate Copper CSV and Orchid tax requests. |
| R02 | Captured separate Lapis onboarding and Quartz shortcut requests. |
| R03 | Captured Nectar audit work with the owed event-types question before changes. |
| R04 | Captured the substantive structured Slate implementation answer and read-only scope. |
| R05 | Captured Topaz's remaining blocker while retaining its existing owner. |
| R06 | Captured the broader Umber flight-ready outcome despite a merged PR; retained the after-detour condition. |
| R07 | Captured Willow drafting with approval required before publication. |
| R08 | Captured explicit later Zen renewal separately; retained the design-question-before-code restriction and closed predecessor. |
| B01 | Captured both Saffron and Juniper questions as bookmarks. |
| B02 | Captured Opal ownership as a bookmark without commissioning changes. |
| B03 | Captured Peridot discussion after lunch as a bookmark. |
| B04 | Missed the unfinished Ruby question after a detour; no item created. |
| N01 | Ignored an assistant-only promise/idea. |
| N02 | Did not commission work from worker/event prose alone. |
| N03 | Appended the user's rejection/drop evidence to Amber; no new item. |
| N04 | Appended the structured Silver pause-until-explicit-resume restriction. |
| N05 | Appended Bronze abandonment evidence; no retirement or new item. |
| N06 | Did not bookmark an already answered question. |
| N07 | Did not turn a quoted instruction into work. |
| D01 | Appended Copper evidence to the visible matching item. |
| D02 | Appended Orchid's read-only totals restriction to its matching item. |
| D03 | Appended evidence to the existing Lapis item. |
| D04 | Appended Quartz pause evidence; no duplicate or renewal. |
| D05 | Appended the Nectar replacement value, Tuesday rather than Monday. |
| D06 | Left retired Nimbus work closed; no new item. |
| D07 | Left retired Violet work closed; no new item. |
| D08 | Did not revive the older dismissed Moss request. |
| D09 | Left retired Sierra work closed; no new item. |
| D10 | Did not recreate the older resolved Talon request. |
| H01 | Captured historical Ocher work without inventing its withheld later cancellation. |
| N08 | Did not recreate an explicitly completed and verified outcome. |
| N09 | Respected an in-window later cancellation; no new item. |
| U01 | Captured the exact Unicode Café request and no-sending restriction from the genuine partial source. |
| I01 | Created an Azure record while its older matching item was genuinely omitted; separately measured duplicate. |
| D11 | Matched the visible Cobalt paraphrase and appended instead of duplicating it. |
| L01 | Captured apparent Indigo intent provisionally while preserving UnknownHistorical provenance. |
| L02 | Did not commission work from unknown-origin machine/quoted framing. |

The Unicode source remained incomplete at UTF8 offset 4232; its unread suffix
was not marked covered. I01 exposed sixteen actual candidates and one honest
omission. This does not prove perfect semantic deduplication across an unbounded
registry. B04 shows that implicit conversational recall remains imperfect.

## Accounting and retained failures

37 calls; zero adapter failures, retries, skips or fallback. Observed usage:
54,505 input tokens, zero cache-read/creation tokens and 3,771 output tokens;
all usage fields were reported. Logical content totaled 262,621 bytes, maximum
10,752 per request. Maximum output was 313 reported tokens and 1,207 serialized
response-content bytes. Summed inference time was 114,792 ms; maximum 11,515 ms.
The complete test took 118.99 seconds. No dollar-cost or Codex generation/billing
ceiling is inferred from these observations.

The [first trial](results-ec570d0f3.md) and
[paired prompt trial](results-db6f2b44f.md) remain rejected. The first lost
restrictions/structured intent and captured completed/stale work. The second
passed recall/restriction gates but invented two retired renewals. Their raw
artifacts, fixed oracles and failure reports were preserved; no thresholds or
case semantics were relaxed. Across the three complete trials, 111 calls reported
150,612 input and 12,017 output tokens.

Private retained artifacts include the exact executable, approved prepared
manifest and all 37 seed databases, raw response blocks, proposals before host
application, rejected-proposal audits, every before/after state, record hashes,
live manifest, and durable command log/exit status. Acceptance followed independent
semantic and SQL review, not recorder exit status. No additional provider run is
needed or authorized by this report.
