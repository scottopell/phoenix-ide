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
