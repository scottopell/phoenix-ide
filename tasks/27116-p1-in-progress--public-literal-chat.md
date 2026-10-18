Expose bounded literal_text public chat policy and explicit version capability; repair embedded authenticated fail-closed API guidance, regression coverage, and normative contracts.

Worker constraints: no commit/push, production API calls, deployment, or heavy local build. The commit/push restriction applies only to workers; the parent owns integration and hosted Rust/E2E qualification. Production remains unchanged and no deployment is authorized by this task.

Validation evidence:
- Offline guidance/source checks: 8 passed (`uv run --offline python -m unittest discover -s tests/devpy -p test_phoenix_api_guidance.py`).
- Offline E2E harness self-tests: 35 passed (`uv run --offline tests/e2e/run.py --self-test`); this is not real-binary E2E acceptance.
- `rustfmt --edition 2021 --config skip_children=true crates/phoenix-ide/src/api/handlers.rs` and `git diff --check` passed.
- Manual-opening regression fixture extracted without dropping assertions. Test and helper are 86 and 43 nonblank lines; all newly added Rust tests inspected are at most 86 nonblank lines, with no blanket lint allowances.
- Added valid owner-Bearer admission coverage alongside existing unauthenticated and invalid-file rejection coverage. Valid uploaded-file preservation coverage was omitted because no reusable upload test helper was found; existing literal payload coverage retains images, and hosted attachment validation remains required.

Review corrections represented in the shared candidate:
- Reserved manual-opening IDs (raw and successor-namespaced) compare literal text and user agent before canonicalization; mismatches preserve pending intent and create no turn, queue entry, or message.
- Transcript-only replay cannot prove nondefault expansion policy; exact policy-bearing durable evidence is required for literal replay.
- Running-message E2E subscribes before POST and consumes the matching live queue event, avoiding the subscribe-after-send race; harness self-tests cover event absence and ordering.

Rust compilation/tests and real-binary E2E were not run locally. Parent must qualify the integrated candidate on hosted infrastructure before claiming runtime acceptance.

## User scope correction and first hosted qualification

User: “Don’t go overboard with destination capability check, I dgaf.” Removed the dedicated route and version capability flag; retained only the opt-in `literal_text` on existing `/chat`. Five-step bootstrap requires an independently verified exact receiving commit, immediate same-destination version check, one harmless endpoint/materialization smoke and no fallback or expandable probes when support is unknown. Existing auth/admission/idempotency remain. ADR118 in this unmerged PR revised accordingly.

First head72318 CI39082259710 failed compilation: missing ChatExpansionPolicy imports and private ClientTurnKey tuple construction. Corrected by scoped test import/public constructor; production missing import disappears with removed extra handler. First hosted review found no findings, but this does not qualify the simplified successor. Intermediate failed evidence retained in PR. Eight offline guidance tests and35 harness self-tests pass; successor requires clean committed real-binary fixture and rejects dirty/unknown identity. No local Rust build, production deploy, RC4 repin, live instruction replacement or remote steering. Parent will commit/push and qualify exact successor in hosted CI.
