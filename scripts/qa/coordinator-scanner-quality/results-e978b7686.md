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


## Host-only topology follow-up

After the accepted model trial, final review found two host defects: priority-head
inclusion could move discovery past intermediate members, and topology reads
materialized unnecessary runtime/handoff aggregates. The repair advances the
cursor from the eight-member discovery page before adding the head. Scalar
actual-head lineage queries supply at most eight discovery members plus priority
head, exact membership and eight coverage entries plus a lookahead. They do not
decode runtime state or historical handoff blobs. Recursive traversal/count work
still scales with lineage; bounded data returned to scanner/Rust is not a constant
SQL CPU guarantee. SQLite CTE working data may also scale with lineage.

All twenty-five DB tests passed at `6c11894f75b60463b79586e62d840849ea761fcd`.
The subsequent binding-name-only head `74881f401c187ed6b74f576d0eeae8011ac0b8a0`
passed owning DB/IDE all-target Clippy and the actual 37-case host fixture test.
The three added regressions cover fourteen transcripts made by thirteen actual
continuations, enrollment/restart generation revisit of intermediate members,
1 MiB irrelevant malformed runtime state and an actual typed handoff seeded before
corruption, and detached-source CAS refusal after an explicit test-only edge-guard
drop. The DB tests took 23.96 seconds; owning Clippy took 2m24s; the 37-case fixture
test took 54.86 seconds. Independent source review found no additional blocker.

A new offline preparation reused the original `9057badc…` closed seeds and made
zero provider calls. Root and a fresh independent reviewer compared all 111
input/prepared/before files across the 37 cases. Every file matched the accepted
`288329e4…` preparation byte for byte; input hashes and binding IDs also matched.
Prompt, DTO, corpus, oracles, endpoint, limits and 262,621 total logical content
bytes were unchanged; protocol differences
were only source/workspace and executable identities. This is host-input
comparison evidence, not a new paid model trial or a change to the scores above.

| Later host artifact | SHA256 / identity |
|---|---|
| Compiled source and preparation workspace | `74881f401c187ed6b74f576d0eeae8011ac0b8a0` |
| Retained executable | `b90fc537e8ae7b4c019832badcceab717956a5d977f501bb4760c10dee5e511b` |
| Offline prepared manifest | `957a2cfa5ac11d473a79f7d1738a891d849f47d3e44f6bcc7b323b1e319bb99e` |
| Offline protocol | `35593a2ffc8e923f9b08552b1810c89d74c85a80bf6743fd4bee48f0c1fc8f9d` |

The original `e978b7686` paid-trial source, executable, prepared/live hashes,
37-case scores, missed bookmark, invisible duplicate, prior rejected experiments
and provider accounting remain unchanged. Exact delivery review/CI/merge state is
tracked by PR #1098. No additional provider call was made for this follow-up.


## Later host resource and fairness follow-up

A later cumulative-read review found that individual LIMIT32 queries could still
fetch many source rows across one preparation loop. The repair creates one
non-Clone 32-fetched-source-row allowance for the whole preparation, shared across
members and lanes. Rows are charged when fetched, before projection, errors or
backoff; known spending is checked before prose is read. This bounds fetched
source rows per preparation, not SQLite's lineage traversal or internal working
data, and does not change the model input/result contract.

When a blocked pass exhausts the allowance, its Wait durably prefers the opposite
of the last consumed lane, including the missing-route path. Committed non-prose
Progress updates the ordinary lane preference without an inference attempt charge
or clearing a recorded blocker. Ready leaves the existing reserve behavior
unchanged. Regressions cover both lane directions across restart, malformed
source and route absence, continuous non-prose history/live progress, and actual
hourly caps. No additional daily-byte-limit regression is claimed.

At `1824f48d39ef76d6ea07560070b78cfe18bc2907`, all thirty DB tests passed
(35.19 seconds), owning DB/IDE all-target Clippy passed (2m18s), scoped Allium
passed, and the actual 37-case host fixture passed (54.78 seconds). Independent
source review of the cumulative-read/fairness changes found no further blocker.

Offline preparation reused the original closed seeds and made zero provider
calls (3.86 seconds). Root and a fresh independent reviewer compared all 111
input/prepared/before files against the accepted `288329e4…` preparation: every
file was byte-identical. All input/record hashes and binding IDs were recomputed
and matched, and the retained executable hash below was independently verified. Model, effort, endpoint, prompt, DTO,
corpus, oracles, limits and 262,621 logical content bytes were unchanged; the
protocol differed only in source/workspace and executable provenance. This is
host-only evidence, not another model trial.

| Later resource artifact | SHA256 / identity |
|---|---|
| Compiled source and preparation workspace | `1824f48d39ef76d6ea07560070b78cfe18bc2907` |
| Retained executable | `5d422c915b86eeaffcdffd3293d319500b5e526bc8bddc448cb592eebd2002fa` |
| Offline prepared manifest | `8c5d79fd6e20e1db09892b903814440470ffa212b404f53dab5a9be647c345ac` |
| Offline protocol | `dab3f3655167002f6b36cc0c7e7a0daebc2da17c5c9d85097d04cbaff4af0ebf` |

The original e978 paid-trial identity, hashes, scores, failed experiments and
B04/I01 limitations remain unchanged, as does the earlier 7488 topology receipt
above. No additional provider calls were made or used to replace prior evidence.


## Later SQLite engine-size follow-up

Final review found that `length(CAST(content AS BLOB))` could materialize an
entire large TEXT value even when bounded substring output or message-type
exclusion hid that content from Rust. The corrected SELECT nests its type CASE:
excluded message kinds access neither content nor its size; eligible user/agent
content uses direct-column `octet_length` metadata before returning the complete
value only when at most 1 MiB. Oversized eligible content returns NULL and remains
explicitly SourceUnavailable/unprocessed, without coverage advancement.

SQLite's [primary octet_length documentation](https://www.sqlite.org/lang_corefunc.html#octet_length)
explains that direct TEXT/BLOB columns can use stored size metadata without
reading their payload, and that byte counts depend on database encoding.
The pinned bundled SQLite 3.51.3 investigation and actual Rust regression are
scoped to a verified UTF-8 database; no UTF-16 conversion compatibility is added.

The formal file-DB regression inserts 32 MiB tool and user values before lowering
`SQLITE_LIMIT_LENGTH` to 2 MiB on a dedicated detached connection. Both direct
content and the old CAST-size controls fail with SQLITE_TOOBIG (18). The actual
scanner preparation succeeds: excluded tool rows advance as non-prose, oversized
user rows remain SourceUnavailable without advancing their frontier, and a small
Unicode control is returned exactly. The test verifies limit restoration and
connection closure. This establishes the tested per-value engine gate; it is
not a total SQLite CPU, internal-working-memory or process-RSS guarantee.

All thirty-one DB tests passed at `a5680344ac0d37e239600be7f122a1136c93c50b`
(41.15 seconds). Production code remained identical at
`ba39ee48d92141ea100ad33971499ce8ab19d2cd`, which changes only two equivalent
test-lint expressions. That head passed owning DB/IDE Clippy (2m18s) and the actual
37-case host fixture (55.21 seconds); root and fresh source review found no blocker.

Offline preparation reused the original closed seeds and made zero provider
calls (3.82 seconds). Root and a fresh independent reviewer verified all 111
input/prepared/before files byte for byte against the accepted `288329e4…`
preparation, with all 37 input hashes and bindings unchanged. Current database
hashes matched the manifest; binary and prompt/corpus/oracle references matched,
and Clippy/fixture/preparation receipts exited zero. Model, prompt, DTO, corpus,
oracles, endpoint, limits and
262,621 logical content bytes were identical; full protocol differences were
only source/workspace and executable provenance.

| Later engine artifact | SHA256 / identity |
|---|---|
| DB31-tested production correction | `a5680344ac0d37e239600be7f122a1136c93c50b` |
| Compiled/prepared test-lint-only successor | `ba39ee48d92141ea100ad33971499ce8ab19d2cd` |
| Retained executable | `3c9e749cbd48b2cac9713d7d9b183f56ca7cf3788fabf5d9e1594156f301fd7b` |
| Offline prepared manifest | `4df68a102ff5eec03a7e42eced14c9457e2231e7e98fc5fc022ce4459109850f` |
| Offline protocol | `0fb24a6d0f97614985797f6d4b3839e1e10056a5aeddf27e7609b3d029003f56` |

No additional model call occurred. Every earlier e978 paid-trial identity,
configuration, score, failure and limitation, plus 7488 topology and 1824
resource/fairness receipts, remains unchanged above.
