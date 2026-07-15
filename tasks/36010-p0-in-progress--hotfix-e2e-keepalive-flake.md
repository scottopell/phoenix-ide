# Hotfix the prod-deploy E2E keepalive flake

## Problem

`./dev.py prod deploy` can fail its `text_streaming` E2E scenario on a healthy but heavily loaded machine. The current scenario deliberately injects `[[stall:1,16000]]` so a real conversation remains open past the production 15-second SSE keepalive interval, then requires both that keepalive and terminal turn evidence within a 45-second wall-clock budget. Under the observed load (load average 65 on 10 CPUs), the synthetic 16-second stall plus scheduler starvation consumed the budget; the authoritative snapshot was idle with no product error.

Recent work correctly made the harness hermetic, tied continuation completion to exact user-message identity, recognized literal `ping` payloads, and replaced unrelated fixed sleeps with readiness handshakes. This remaining failure was introduced by `test: require live e2e keepalive completion`: one E2E scenario now conflates two contracts—ordinary text-turn completion and the 15-second keepalive cadence—and proves the latter by waiting in real time.

## Hotfix plan

1. Split the two contracts instead of increasing `SCENARIO_TIMEOUT_SECONDS` again.
   - Keep `scenario_text_streaming` as a real-binary HTTP/SSE boundary test of streamed text, terminal signaling, persistence, and final transcript evidence.
   - Remove the 16-second mock stall and the requirement that this ordinary turn cross a production keepalive before completing.
   - Preserve the existing bounded timeout only as a hang/failure ceiling, not as synchronization.

2. Cover the keepalive wire contract deterministically at the narrowest server boundary.
   - Add a focused Rust test around `api::sse::sse_stream` (or a small extracted keepalive constructor) proving the emitted keepalive is `event: ping` with non-empty `data: ping`.
   - Drive Tokio time virtually (`pause`/`advance`) or inspect the constructed typed event directly; do not sleep 15 real seconds and do not add a production interval override solely for tests.
   - Retain the Python harness self-test proving literal `ping` bypasses JSON decoding, so both server encoding and client parsing are covered without a real-time rendezvous.

3. Keep exact-turn correctness intact.
   - Ensure first-turn completion cannot be satisfied by an unanswered idle `init` snapshot.
   - Require terminal SSE evidence and then verify the persisted assistant text for the created conversation.
   - Do not weaken `_send_chat_and_stream` continuation identity checks.

4. Improve failure evidence if the hotfix touches diagnostics.
   - On timeout, include transcript/message evidence as well as conversation state, since `state='idle'` alone cannot distinguish a completed turn from an unanswered provisioning shell.
   - Keep fail-fast behavior and the captured server log tail.

## Verification

- Run the Python harness self-tests.
- Run the focused SSE keepalive Rust test repeatedly; it must use virtual/no elapsed wall time.
- Run `uv run tests/e2e/run.py` repeatedly under artificial CPU contention and confirm `text_streaming` no longer pays a mandatory 16-second delay.
- Run the E2E lane together with the Rust/clippy workload that reproduces prod-deploy contention.
- Run `./dev.py check`, followed by the production pre-deploy check path.
- Confirm a deliberately broken terminal signal still fails at the bounded scenario ceiling rather than passing from an idle init snapshot.

## Non-goals

- Do not raise the 45-second scenario timeout as the primary fix.
- Do not change the specified production 15-second keepalive cadence or 35-second UI watchdog.
- Do not redesign every E2E wait in this emergency patch; that belongs to the separate audit task.

## Expected outcome

The deploy-blocking scenario completes as soon as observable turn evidence exists, while the typed keepalive remains covered deterministically. High CPU can delay a passing test, but no passing condition depends on winning a 16-to-45-second wall-clock race.
