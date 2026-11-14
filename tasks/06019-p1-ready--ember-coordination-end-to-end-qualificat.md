# Ember phase 6: end-to-end qualification and cutover

Parent: [06013](./06013-p1-ready--ember-consistency-and-durable-tracking.md). Depends on delivered consistency, tracking/continuity and scanner slices; roadmap shutdown is independently verifiable.

## Outcome

Demonstrate the complete user journey through the real assembled Coordinator surface, and prove which portions are shipped rather than confusing branch completion, notification acceptance and useful follow-through.

## Validation journeys

1. User requests two meaningful pieces of work in Ember, then detours into a separate exploratory question. Direct tracking records one request, scanner captures the omitted request and a useful conversational bookmark with exact source evidence. The curiosity remains a bookmark, not another commissioned task. Worker-only promises are outside discovery scope. Returning to the question supplies a short refresher and resumes from the stopping point, as in Issue #1075.
2. Continue Ember; fresh tracker index retains identity and owed step despite a deliberately terse handoff. Exact source evidence remains historical; current worker target follows continuation.
3. Dismiss a stale record while inference is in flight. Stale apply changes neither registry nor cursor; next scan sees retirement. Rediscovering old source cannot reopen that existing retired record; a semantic duplicate under a new identity is measured separately, not incorrectly claimed impossible. Fresh user renewal is an explicit Ember change.
4. Interrupt before/after capture/cursor commit; reopen the same supported DB. No partial committed capture/coverage or duplicate exact application. Model-call repetition is not represented as exactly-once billing.
5. Deliver new tail while the same transcript's backfill is incomplete; verify the gap stays explicit. Include an old request canceled far beyond its historical chunk and verify no action follows unverified historical capture. Make source too large, edit old source, disable the chosen cheap route, and return truncated/invalid responses. Coverage stays honest, limits hold and normal input admission continues.
6. Message accepted/queued, watch event received, reminder due and Goal applied remain distinct from verified completed follow-through. Tracking does not wake/rearm stopped work or bypass user pauses/Authority.
7. Test local discovery/action/observation, selected-peer query/read/watch, and exact continuation using installed definitions and real DB admission. Preserve no-local-fallback and explicitly local-only operations.
8. Retire roadmap automation with workflow/runs/variable/Issue readback, while retained historical evidence can still be retrieved. Reconcile only confirmed live obligations; no blanket import.

## Evidence and harness

Use production constructors and sanitized disposable runtimes. Extend/add a purpose-built isolated QA harness; the existing coordinator-drive-eval uses four fake tools, canned SQL and archived prompts and cannot prove full tool or live continuation behavior.

Record commit, configured model/effort/endpoint, catalog/language, prompt/schema hashes, failed-call/identity-error counts, context bytes or actual tokens with named measurement, scanner coverage/latency/usage and capture-quality review. Synthetic tests prove lifecycle invariants; real-model trials assess behavior separately; user acceptance proves utility. Do not substitute source assertions or server health for those later layers.

Compare native-first/clearer-target guidance to baseline before promoting new discovery helpers or wrappers. Do not require a universal API wrapper to finish.

## Repository and delivery checks

Use ./dev.py check-plan for changed lanes, then required task/spec/allium/Rust/fmt/clippy and presentation checks. Honor existing host execution/resource owners; no unsolicited production restart/deploy. Ensure generated wire files, if changed, are regenerated via dev.py. Independently review concurrency/retention and provider-boundary changes. PR descriptions expose phase dependencies and outstanding mandatory tasks.

Done when each phase has checkable evidence, live deployed revision/controlled acceptance is distinguished from merge, user can recover an owed next step after continuation, and roadmap updater/posting instructions are retired. If a layer is not exercised, retain the explicit gate rather than marking the whole program complete.
