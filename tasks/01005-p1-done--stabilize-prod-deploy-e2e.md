# Stabilize prod-deploy E2E checks under local contention

## Problem

A fresh `./dev.py prod deploy` can fail during its pre-deploy E2E lane even though the product checks and earlier mock scenarios pass. The observed run had three 15-second SSE watchdog failures and a `perf_stream` JSON decode failure while `cargo test` and clippy were consuming the same host.

Root-cause findings:

1. The mock-only E2E server is not hermetic. `tests/e2e/run.py` copies the caller environment and launches Phoenix with the caller's real home directory, so Phoenix discovers `~/.claude.json`, starts Chrome DevTools MCP, and attempts Atlassian OAuth. The supplied server log proves both happened. This adds unrelated processes, network handshakes, and contention to a deterministic mock test.
2. The conversation SSE client JSON-decodes every event before inspecting its type. Phoenix deliberately emits `event: ping` with literal `data: ping` every 15 seconds, so any scenario that remains active long enough under contention fails with `JSONDecodeError` instead of treating the keepalive as transport liveness. This explains the `perf_stream` failure near the first keepalive interval.
3. Several scenarios use a 15-second absolute watchdog. The check ran the E2E server alongside a nine-thread Rust suite on a 10-core host with load already above core count; clippy also overlapped the scenario phase. Normally short mock turns crossed that deadline under contention. The watchdog's cross-thread `client.close()` is only observed at the 20-second HTTP read timeout, explaining the approximately 20-second scenario durations.
4. The harness continues after a timed-out scenario on the same server. A failed scenario can leave work in flight and contaminate later results, turning one timing failure into a cascade.
5. The server warning `LLM task dropped its outcome sender (panic/abort)` is expected for the successful `mid_stream_cancel` scenario: that scenario intentionally invokes `Effect::AbortLlm`. It should not be treated as the initiating failure.

## Implementation plan

1. Make the E2E server environment hermetic.
   - Build the child environment through a testable helper.
   - Point the child at an isolated temporary home/config root (or add and use an explicit no-MCP startup control if repository specifications require preserving the real home).
   - Continue stripping real provider credentials and preserve only the minimum environment needed to execute the binary and tools.
   - Add a harness self-test proving caller MCP configuration cannot be discovered by the E2E server.

2. Correct the SSE barrier.
   - Inspect the event type before parsing its body.
   - Treat `ping` as a recognized keepalive with an intentionally opaque/non-JSON payload.
   - Continue requiring JSON for Phoenix wire events and retain actionable malformed-event errors.
   - Add focused self-tests for a literal ping, a valid terminal event, and malformed JSON on a typed wire event.

3. Make scenario deadlines robust without hiding hangs.
   - Separate transport read timeout from a documented scenario deadline.
   - Give deterministic scenarios enough bounded headroom for the check's supported parallel-load conditions, while keeping the outer check timeout as a hard bound.
   - Improve timeout diagnostics with the final authoritative conversation state and recent server log context.
   - Review check-lane scheduling/resource reservation; if isolation plus keepalive handling is insufficient in repeated stress runs, prevent the CPU-heavy Rust/clippy phase from oversubscribing the E2E scenario phase rather than continually inflating timeouts.

4. Prevent cascading failures.
   - On scenario failure, cancel/settle the conversation when its ID is available, or fail fast and restart with a clean server for subsequent diagnostics.
   - Ensure a timed-out scenario cannot leave background work that changes later scenario timing or errors.

5. Verify the actual regression path.
   - Run the harness self-tests.
   - Run the E2E lane repeatedly with elevated load and `E2E_RUST_LOG=debug`, confirming no real MCP processes or OAuth flows start, pings do not fail parsing, and all scenarios terminate.
   - Run `./dev.py check` using the project workflow, then re-run the prod pre-deploy checks.
   - Confirm genuine broken terminal signaling still trips a bounded watchdog, so the stabilization does not weaken hang detection.

## Expected outcome

Prod deployment checks remain deterministic on a busy local machine; the mock E2E process has no dependency on personal MCP configuration; typed keepalives are accepted correctly; and a single scenario failure produces one actionable root failure rather than a cascade.
