# Direct-turn authoritative materialization boundary

Repair the production runtime integration for already-durable direct turns. This is a narrow prerequisite/regression against completed task 78007: accepted turns must cross authoritative user-message materialization without provisional live state, SSE cursor holes, or ambiguous claim settlement. Preserve the exact accepted-turn identity and generation settlement completed under task 24705.

Task 78002 remains separate and unchanged: it owns provider-response durability after provider dispatch and should build on this repaired pre-provider materialization boundary rather than absorb it.

## Acceptance

- `LlmRequesting` is not published to live state watchers before authoritative materialization succeeds.
- Every reserved persisted SSE sequence is filled on success, exact replay, stale authority/claim loss, and materialization error.
- Direct-turn dispatch is acknowledged through materialization: pre-materialization failure releases the live claim, claim loss does not stale-release, and post-materialization failure does not release a materialized claim and enters typed failure recovery.
- Exact accepted-turn and generation identity remain the settlement authority.
- Focused regressions cover cursor continuity, watcher timing, claim settlement, and post-materialization failure.
- No request drain, managed-task/shutdown lifecycle, creation/poller/browser/MCP lifecycle, Repository Cutover, repository openers, or `drive_turn` change enters this task.
