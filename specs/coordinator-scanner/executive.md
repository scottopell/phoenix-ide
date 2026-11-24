# Coordinator scanner status

Scanner migration 166, bounded host preparation/reservation/application, coverage
inspection and the supervised background runner are implemented and qualified.
Delivery is tracked in [PR #1098](https://github.com/scottopell/phoenix-ide/pull/1098),
which is the authority for exact review, CI and merge state. Independent review
accepted the third complete 37-case real-model trial at `e978b7686`; both earlier
rejected trials remain preserved. No production deployment or candidate-server
activation is claimed.

| Requirements | Qualification evidence | Scope |
|---|---|---|
| REQ-SCAN-001–005: Scope, coverage, source validity, authority and recovery | Thirty-one actual DB tests passed, including strict typed retirement/source chronology, old-prose generation-only restart, two continuations, exact deduplication, retirement races, live gaps and contention recovery | Deterministic production-host lifecycle evidence |
| REQ-SCAN-006: Bounded resource use | Eighteen runner/kick tests passed; strict catalog/outbound Luna/Low, accepted-output bounds, durable spending/retry limits and no foreground wake are covered | Codex generation/billing is not capped by accepted-output checks |
| REQ-SCAN-007: Bounded inspectability | Native coverage inspection and accepted eight-variant assembled inventory include the closed tracking coverage operation | Synthetic assembled configuration, not production inventory |
| REQ-SCAN-008: Historical recovery and qualification | Manual/foreground usefulness, actual 37-case host fixtures, owning all-target Clippy and fresh source reviews passed; third semantic trial independently accepted | Small synthetic corpus; bookmark miss and invisible duplicate disclosed below |

The production-runtime composition test passed with the runner/kick suite: an
idle capture adds no foreground turn/messages; the next actual admitted input
sees a fresh bounded index; appended pause evidence invalidates the old stopping
point through two real continuations. This synthetic inference proves lifecycle
composition, not cheap-model interpretation or general user utility.

The thirty-one DB tests preserve original/retired evidence through supported file
restart. The old sequence-5/covered-through-100 case first proves metadata-only
invalidation and unchanged exact deduplication, then atomically changes valid
old prose and its generation without appending a row, closes/reopens and recovers
new exact evidence. That latter fixture models externally changed source; it
adds no user-edit API or UI capability. Normal history/live fairness, explicit
source gaps, reserve-crash retry accounting, later recovery and bounded
contention remain separate assertions.

Final topology review found and corrected two host defects: a priority-head append
could advance discovery past intermediate members, and full topology loading
unnecessarily decoded runtime/handoff blobs. The cursor now advances from the
eight-member discovery page before adding the head. Scalar actual-head lineage
queries return at most eight discovery members plus the head, exact membership,
and eight coverage entries plus one lookahead, without decoding runtime/handoff
aggregates. Recursive traversal and counts still scale with lineage; this bounds
data returned to scanner/Rust, not SQL CPU independently of history size.

All twenty-five DB tests passed, including fourteen transcripts formed by thirteen
actual continuations, middle-member enrollment/restart generation revisit,
1 MiB irrelevant malformed runtime state and a real typed handoff seeded before
corruption, and detached-source CAS refusal after an explicit test-only edge-guard
drop. Owning DB/IDE all-target Clippy and all 37 host fixtures passed. Offline
preparation at `74881f401` made zero provider calls; all 37 inputs, prepared records,
prior states, input hashes and bindings match the accepted trial byte for byte.
The [accepted report's host follow-up](../../scripts/qa/coordinator-scanner-quality/results-e978b7686.md#host-only-topology-follow-up)
records new executable/manifest hashes without changing its original model trial.

## Frozen quality evidence

The independently reviewed corpus has 37 synthetic cases: eleven primary
requested units (minimum ten), five bookmark units (minimum four), seventeen
no-new-item negatives, twenty-one cases forbidding new requested work, eleven
visible duplicate opportunities (maximum one), thirteen target-bound restrictions,
one outside-window cancellation and one genuinely omitted matching candidate.
Unknown apparent intent and machine/quoted framing are scored separately; original
provenance remains intact and every new capture is historical and unreviewed.

Corpus SHA256:
`869882606a01834cb2e45a3f45490bba5d2f9ca025551c42f94210e5e5af3ea4`.
Oracle SHA256:
`d195c43320a31a452abc110d54ab95df71d1547b84f2870b1e1fb61f30edcb14`.
The Unicode source is genuinely partial under normal host bounds, the omitted
candidate is outside sixteen actual native records, and scanner-source IDs are
UUIDs admitted initially with logical aliases retained only in host review data.
No oracle, withheld cancellation or hidden candidate text enters inference.

The [first trial](../../scripts/qa/coordinator-scanner-quality/results-ec570d0f3.md)
made 37 calls and was rejected for six semantic cases despite valid quotations.
The [paired prompt correction](../../scripts/qa/coordinator-scanner-quality/results-db6f2b44f.md)
reused all 37 byte-identical source payloads and made 37 calls: eleven requested
units, five bookmarks, all thirteen restrictions and all 33 quotes passed, but
two stale retired requests became new active records. Both runs remain rejected;
zero structural reopen writes does not excuse invented semantic renewal.

The typed input projection supplies host-derived fragments strictly later than
a retired candidate. Inference uses that temporal relation rather than comparing
long timestamp integers; eligibility is not renewal intent or permission. The
same original sources, candidate facts, UUIDs, corpus, oracles and thresholds were
retained, with the expected projection difference independently reviewed before
calls. DB tests cover strict equality/order and per-candidate chronology.

The [accepted third trial](../../scripts/qa/coordinator-scanner-quality/results-e978b7686.md)
recovered all eleven requested units, four of five bookmarks and all thirteen
restrictions, with 29/29 exact valid quotes. It produced zero false requested-work
commissions across 21 opportunities, no new items in all seventeen negatives,
zero visible duplicates across eleven opportunities and no authority/reopen
violations. Root and a fresh independent reviewer inspected every raw result,
record hash and host effect before acceptance.

The missed bookmark is B04, within the predeclared four-of-five threshold.
Unknown apparent-request/machine-negative cases passed. H01 remains historical
without knowledge of withheld cancellation; I01 produced one separately reported
invisible duplicate because the matching record was genuinely omitted. Perfect
recall or semantic deduplication is not claimed. Exact configuration, usage,
latency, original failures and all artifact hashes are in the reports.

## Bounds and limits

Host source projection is bounded to 32 physical rows/pass and 1 MiB serialized
user/assistant content per row. Larger or malformed rows remain explicitly
SourceUnavailable/unprocessed. Ordinary prose uses an 8 KiB encoded source window,
registry context 8 KiB, and total inference content 24 KiB, with honest clipping
and omissions. These limits do not claim blocked or unread history is covered.

The eligible service must expose exact catalog and outbound `gpt-6-luna` at Low
through Codex or official direct Responses, without an expensive/default fallback.
Accepted output requires known usage at most 2,048 tokens and 16 KiB UTF8 text;
nontext, incomplete, unknown-usage and oversized output is rejected before apply.
Official direct requests the generation limit; Codex has no enforced generation
or billing ceiling. Timeout, durable attempt/hour/day/input budgets and independently
observed usage fields remain separate. Scanning grants no authority and creates no foreground
turn or new review wakeup. Physical-device and deployed-backfill results are not
claimed by these local synthetic qualifications.


## Later resource and fairness qualification

At `1824f48d3`, thirty DB tests, owning DB/IDE all-target Clippy, scoped Allium
and all 37 host fixtures passed. One non-Clone allowance now caps the entire
preparation at 32 fetched source rows across members and lanes, charged before
projection, errors or backoff; known spending is checked before reading prose.
An exhausted blocked Wait durably prefers the opposite of the last consumed lane,
including when the model route is absent. Committed non-prose Progress applies
the normal lane preference without an inference charge or clearing a blocker;
Ready retains the existing reserve behavior. Tests cover restart in both
directions, route absence, continuous non-prose progress and actual hourly caps.

Offline preparation made zero provider calls and reproduced all 111 accepted
input/prepared/before files byte for byte, with identical 37 hashes and bindings.
The [separate resource follow-up](../../scripts/qa/coordinator-scanner-quality/results-e978b7686.md#later-host-resource-and-fairness-follow-up)
records exact receipts. The original paid model trial and earlier topology
qualification remain unchanged; SQLite traversal/working data can still scale
with lineage.


## Later SQLite engine-size qualification

The engine-size gate passed thirty-one DB tests at `a5680344a`; the later
`ba39ee48d` changes only equivalent test-lint syntax and passed owning Clippy and
all 37 host fixtures. Nested type CASE avoids accessing excluded payloads or their
sizes. Direct-column `octet_length` checks metadata before returning full eligible
user/agent content at most 1 MiB; oversized eligible prose returns SourceUnavailable
without advancing coverage.

An actual UTF-8 file-database test inserts 32 MiB tool/user values, then lowers a
dedicated detached connection's per-value limit to 2 MiB. Direct/old-CAST controls
fail with SQLITE_TOOBIG, while the real preparation excludes tool content, preserves
the oversized user gap and returns small Unicode exactly. Limit restoration and
connection closure are checked. This is not a total CPU/process-RSS bound or a new
UTF-16 conversion compatibility claim. [Later engine receipts](../../scripts/qa/coordinator-scanner-quality/results-e978b7686.md#later-sqlite-engine-size-follow-up)
record zero-call offline preparation with all 111 accepted files and 37 bindings
unchanged; prior model/topology/quota receipts remain historical evidence.
