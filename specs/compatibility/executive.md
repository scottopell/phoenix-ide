# Phoenix Compatibility Guarantees — Executive Status

## Current reality

Compatibility remains explicit and feature-owned. Forward SQLite migration does
not promise downgrade compatibility or universal data retention. Database
replacement and older-binary startup require offline operator work and a matching
backup; no general automatic database rollback or live replacement exists.

ADR-229 withdraws macOS exact prepared/current-schema/ordinary interval
qualifications REQ-COMP-012/013/015/017/018/019. Routine launchd deployment lets
the candidate own migrations, runs it once, and retains failed claim/evidence for
manual recovery after disruption. No deployment-side database scan, registry
parser, snapshot, rehearsal, predecessor startup or replay remains. New
REQ-COMP-021 captures that contract; agent-authored requirements alone do not
justify renewed compatibility machinery.

Linux REQ-COMP-014 and REQ-COMP-016 remain unchanged: exact protected systemd
published/prepared candidates retain preserved configuration, pristine backup,
isolated rehearsal, durable live marker, persistent both-unit inhibits and the
specified candidate-only recovery. Ordinary Linux recovery is runtime-artifact
rollback and does not restore a database or promise candidate-mutated data works
with the predecessor. The in-app UI states this limitation explicitly.

## Requirement coverage

| Requirements | Current reality |
| --- | --- |
| REQ-COMP-001–004,021 | Explicit guarantees, forward migration and offline recovery; launchd helper/installer/manual-recovery tests and operator guidance |
| REQ-COMP-005 | Integer Unix microseconds apply to new or structurally changed internal timestamp columns; unchanged historical columns remain outside convergence scope |
| REQ-COMP-006–007 | Legacy Direct authority repairs remain forward-only; accepted source-locator fingerprints/encodings remain authoritative |
| REQ-COMP-008 | Exact Kache 1.2.2 macOS arm64 strict local-only selection remains unchanged; remote caching is outside its contract |
| REQ-COMP-012/013/015/017/018/019 | Withdrawn macOS qualifications; IDs reserved |
| REQ-COMP-014/016 | Existing exact protected Linux systemd qualification remains active |
| REQ-COMP-020 | Goals lineage validation remains candidate-owned; see below |

The retired macOS regression classes and replacement failure/identity coverage
are accounted for explicitly in the launchd executive and ADR-229. No production
operation occurred; production remains `9c5d471d`.

## Public literal chat

REQ-API-031 and ADR-124 retain `/chat` opt-in on a known receiver verified by full
immutable `git_sha` before each use or retry. Transcript-only historical replay is
default-policy-only; literal replay requires durable policy evidence. This adds no
database, downgrade, mixed-version, live-replacement or deployment guarantee.

## Goals schema prerequisite extraction

REQ-COMP-020 and ADR-187 define the extracted migration suffix 132–140 and read-only lineage refusal before bootstrap and numbered migration execution. Main's migrations 130/131 and historical deployment pins remain unchanged. This slice includes only schema, lineage validation, and the existing direct-turn materialization adapter; peer runtime, remote handlers, pollers, Goal repositories/execution, and activation are not included.

The lineage predecessor check follows embedded registry membership, not numeric contiguity: every registered predecessor through the recorded tail is required, and every stored version/name must match its canonical identity. Regression coverage extends through the registered tail, including the 144-to-146 numbering gap, and rejects missing registered predecessors, wrong tail names, and unknown identities without mutation. These corrective regressions are authored; exact-candidate execution is pending. No deployment interval, damaged-ledger repair, or compatibility promise is added.

Tests cover canonical prefixes/idempotence, unsupported and ambiguous lineages with unchanged schema/ledger/data through both entrypoints, migration constraint/history preservation, and the existing repository materialization path. Candidate-specific compilation and execution are pending; combined-tree results are not extraction qualification. Applicable CI, independent review, and local legacy/reference/watch paths remain qualification work. No live database or deployment action is authorized or performed by this extraction.
