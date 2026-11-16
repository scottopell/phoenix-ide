# Coordinator scanner status

Contract/corpus preparation only. No migration166, inference adapter, application actor, runner or automatic capture exists on this branch. The manual full-trace usefulness gate and main integration/migration order remain prerequisites for activation. Event165 source must be present before any fresh166 database is created; no compatibility gap or filler migration is allowed.

| Requirements | Status | Evidence |
|---|---|---|
| REQ-SCAN-001–008 | Planned | Approved architecture direction in ADR263; implementation and lifecycle qualification pending |

The independently reviewed version2 synthetic corpus has35 cases:11 primary requested units (minimum10),5 bookmarks (minimum4),16 no-new-item negatives,20 cases forbidding new requested_work,11 visible-candidate duplicate opportunities (maximum1),13 target-bound hard restrictions, one separately reported outside-window cancellation and one genuinely omitted-candidate opportunity. Actual primary human-source evidence is required; assistants/events support context only. Exact labels/denominators live in scanner-corpus.json and corpus-oracles.md.

Independent source/oracle review found no remaining actionable issues at JSON SHA2564373e702b8862f217db65db1edf2c2c37c03921a0f9d03df62cfb9a18ca2e546 and oracle SHA256ab4a74af62f707817339ac1a6ae24fd9676381b678eab23ac220d1cd62e336a0. No cheap-model call has occurred. Actual prepared host batches/prompt/route still require review before calls; all corpus inputs use historical/unverified metadata, while live/frontier/member/coverage/replay and renewal visibility remain mandatory host tests.

REQ-SCAN-001–008, ADR263 and the focused Allium lifecycle are authored before behavior. No migration166 or runner is activated. Root accepted the manual usefulness gate separately; this branch must inherit qualified manual164 plus event165 source before creating fresh166 databases.
