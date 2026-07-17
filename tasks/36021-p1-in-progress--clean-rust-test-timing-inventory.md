# Clean the existing Rust test timing inventory

Classify every finding from `scripts/check_rust_test_timing.py --all crates/` as a removable timing bet, an intentional behavior driver, an already-bounded false positive, or an architectural synchronization gap. Land deterministic fixes in small subsystem tranches, use local `test-timing-allow` reasons only when elapsed time itself is the tested behavior, and file explicit follow-ups for concurrency seams that require production readiness signals. Completion requires an empty `--all` inventory without broad allowlists.

## Progress

- Initial inventory: 58 findings.
- Removed redundant skills/DB timing sleeps and replaced the skills idempotence check with a direct no-rewrite proof.
- Removed a redundant post-readiness cancellation delay, bounded the uncooperative release mock directly, and deleted a vacuous buffering smoke test with no postcondition.
- Added local rationales to explicit LLM/MCP scripted latency and retry-interval behavior drivers.
- Removed all 12 MCP findings with request/publication watches, tracked background-task completion, identity-bound scripted-transport witnesses, and direct polling of deliberately parked futures.
- Current inventory: 34 findings.
- Remaining clusters include terminal relay ownership, process/credential settlement, and browser/tool protocol fixtures; these require subsystem-specific deterministic witnesses or narrowly justified behavior drivers.
