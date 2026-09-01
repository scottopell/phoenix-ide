# Replace stale PR702 with current-main ProductConversation QA fixtures

## Observed journey

- A continued ProductConversation must be inspectable in deterministic QA fixtures on desktop and mobile: Open and History remain distinct, lineage segments render chronologically, and one exact persisted handoff summary appears once between predecessor content and the successor's first message.
- A long transcript must preserve that single handoff marker through scrolling and rerendering. Coordinator must remain a separate fixture surface rather than being classified as Open or History.
- Product-creation recovery may be staged only where already-shipped production UI/contracts provide the presentation surface; the fixture must not invent lifecycle or persistence behavior.
- Fixture screenshots are necessary but insufficient. Deterministic automated journeys must also drive the real shipped ProductConversation UI/API in isolated worker dev instances through creation, continuation, Close confirmations, recovery, and reconnect/reload.
- Work starts from exact base `b20ed69ab2ccf84fd27bc63eb46da538c9f96f86`. PR702 and `origin/task-17003-productconversation-fixture-presentation` are stale/conflicting and must be replaced, not incrementally layered as a new feature PR.

## Verified findings

- Exact base is current `main`/`origin/main` in the proposal checkout.
- PR725 commit `daa2f1507` already merged the production `ProductConversationPage`, ProductConversation fixture directory, Ladle stories, capture script, and `./dev.py qa product-conversation` wiring. These merged files are current-main assets to reuse and extend safely, never delete or replace from the stale branch.
- Current fixtures cover desktop multi-segment/Q&A/work metadata, mobile Open, desktop History, loading, error, and a 110-message transcript. Current capture runs every ProductConversation story at fixed 1440x900 and 390x844 viewports and waits for `data-product-conversation-fixture-ready`.
- Existing fixture data can contain multiple handoffs, but no focused proof asserts one exact handoff text appears exactly once at the required join and remains distinct from the successor's first message after rerender/long scrolling.
- Coordinator already has its own fixture/story/capture surface and responsive matrix; separation should be proved without folding it into ProductConversation navigation.
- Product-creation recovery is shipped on `NewConversationPage` through generated recovery response/row/action types. Any QA staging must consume only that existing contract and stay fixture-local.
- The existing `tests/e2e/run.py` harness starts a real Phoenix binary with a temporary database, isolated `HOME`/XDG/Codex/data directories, mock-model configuration, bounded startup/turn waits, and real HTTP/SSE calls. Existing API tests prove the shipped surfaces needed for aggregate uniqueness, segment/handoff ordering, latest writable identity, clean/busy/dirty Close, creation retry, and canonical identity stability.
- Fixture/Ladle ownership and real-journey ownership do not inherently conflict: fixture assets are under `ui/src/fixtures`, stories, and capture scripts, while a dedicated ProductConversation journey harness can own isolated server/browser/API orchestration under `tests/e2e/`. A second task/worker is therefore not proposed. If implementation discovers unavoidable shared-file ownership with another active worker, stop and propose—but do not spawn—an explicit QA-only split naming exact files before either worker edits the overlap.
- The stale branch tip `c4d9a8827` is 29 commits ahead and 4 behind the exact base, with merge base `d04df8294`; replaying its branch diff would risk overwriting PR725 and later main behavior.
- `phoenix-ladle-fixture` is not discoverable in the proposal environment's current skill registry. Implementation must attempt to invoke it proactively before edits and stop/report if it remains unavailable rather than silently substituting another workflow.

## Inferences and unknowns

- The smallest safe implementation is an intent-level reconstruction on the exact base, using current-main fixtures and shipped generated types as authorities. Fixture-only scenarios that cannot be represented without production changes are documented as unsupported and omitted; however, every commissioned real journey listed below is a mandatory gate and a failure exposes a shipped-product defect/blocker rather than authorizing production changes in this task.
- No product choice is required: fixture-local shells may stage Open/History navigation and Coordinator separation, but they must be pure/stateless and must not masquerade as production routing, stores, persistence, or lifecycle behavior.
- Real journeys must assert persisted/HTTP/UI outcomes from stable IDs and semantic DOM/API state, not screenshot similarity, fixed sleeps, seeded global state, or direct database mutation as a substitute for the user action. Controlled fault injection may use only an existing test/mock boundary in the isolated worker instance.
- Exact GitHub PR702 thread and check state must be queried during implementation because local refs do not include paginated PR metadata.

## Interaction map

- Deterministic scenario data → fixture-local pure/stateless presentation shell or existing current-main page adapter → Ladle story → stable scenario-specific ready marker → shared Ladle capture runner → deterministic desktop/mobile PNG evidence.
- Current shipped ProductConversation snapshot/segment/handoff types → fixture data only; no API, Rust, SSE/codegen, route, store, persistence, or lifecycle changes.
- Current shipped creation-recovery row/action types and existing NewConversation presentation → optional fixture-only recovery scenario; unsupported behavior terminates at the fixture boundary.
- Existing Coordinator fixture → independent Ladle proof/capture; no Open/History classification.
- Isolated temporary worker environment → real Phoenix process and real shipped UI/API/SSE → deterministic user actions → stable aggregate/list/snapshot/lifecycle/recovery assertions → process/browser reconnect or reload → identity assertions → complete teardown with no shared database, ports, HOME, worktree, or browser state.
- Fixture/capture gate + commissioned real-journey gate → candidate eligible for devmbp UAT. Neither gate substitutes for the other.
- Task status and stale PR branch → reviewed exact commit → one guarded lease replacement of `task-17003-productconversation-fixture-presentation` → immutable/fast-forward-only follow-up → PR702 CI, exact-head Codex, and paginated thread closure. Never merge.

## Proposed scope

### Implementation sequence and guardrails

1. Start from exact commit `b20ed69ab2ccf84fd27bc63eb46da538c9f96f86`; verify clean worktree and record the observed old remote branch OID.
2. Proactively invoke `phoenix-ladle-fixture` before implementation. If unavailable, stop and report the missing required skill; do not improvise past the gate.
3. Use `taskmd status 17003 in-progress` when implementation begins and keep the filename status truthful; mark done only after all acceptance and remote review gates pass.
4. Reconstruct the needed fixture proof from current-main contracts and components. Read the stale branch only as historical intent; do not cherry-pick/rebase its implementation and do not delete or wholesale replace PR725 files.
5. Limit edits to QA assets: deterministic scenario data/tests, Ladle stories, fixture-local pure/stateless presentation shells and colocated styles where necessary, stable ready markers, deterministic capture scripts, `./dev.py qa` wiring/tests, package QA command wiring, and QA docs.
6. Add focused fixtures/proofs for:
   - Open desktop and mobile;
   - History desktop and mobile;
   - one exact multi-segment handoff marker, rendered exactly once at the join and distinct from the successor's first message;
   - a long transcript whose rerender/scroll proof retains exactly one marker;
   - Coordinator as a separate surface;
   - creation recovery only if the shipped production contract can be staged without invented behavior.
7. Make readiness semantic and scenario-specific: emit the stable marker only after required visible content and layout have settled. Captures must use fixed desktop/mobile viewports, deterministic data/time, no arbitrary sleeps, no external network, and no browser-storage dependency.
8. Add a dedicated deterministic ProductConversation real-journey harness that starts each journey in an isolated worker dev instance with its own temporary Phoenix database, HOME/config/data directories, repository/worktree fixture, ports, browser context, and mock/provider boundary. Prefer a new ProductConversation-specific file over expanding shared `tests/e2e/run.py`; reuse its isolation/process helpers only where ownership remains clear.
9. Commission and automate these real shipped journeys without production changes:
   - create a ProductConversation through the shipped creation surface, submit the initial objective, and prove that exact objective is the first user objective;
   - prove exactly one aggregate/list row and one stable canonical ProductConversation identity for that work across list and detail surfaces;
   - create or drive multiple transcript members, then prove chronological segment ordering, one exact persisted handoff rendered exactly once and distinct from the successor's first message, and only the latest member writable;
   - Close clean work and prove the same stable aggregate moves from Open to History with read-only chronology;
   - attempt Close while work is busy, prove the exact stop-work confirmation, confirm it, and prove settlement continues without duplicate aggregate/Close state;
   - create an exact known dirty-worktree loss inventory, attempt Close, prove the exact loss confirmation matches that inventory, confirm it, and prove the terminal History result;
   - deterministically inject a shipped creation failure at an existing test boundary, prove recovery discovery and allowed retry action, retry, and prove one published aggregate with no duplicate creation/list row;
   - reload the browser and reconnect/restart the isolated client/server where the shipped contract permits, then prove canonical ProductConversation ID, route, list cardinality, segment order, handoff cardinality, lifecycle classification, and latest writable identity remain stable.
10. Require stable semantic assertions and bounded polling/readiness for every journey. No arbitrary sleeps, external services, shared dev DB, pre-existing user state, or direct persistence fabrication may stand in for a shipped action. Capture logs and failure artifacts sufficient to reproduce a failed journey.
11. Preserve an adapter checklist in QA documentation naming only existing backend fields consumed by the fixtures and clearly distinguishing fixture staging from production integration.

### Candidate surfaces

- `ui/src/fixtures/productConversation/**`
- `ui/src/stories/product-conversation.stories.tsx`
- `ui/scripts/capture-product-conversation.mjs`
- focused QA capture tests and `docs/qa/product-conversation.md`
- existing Coordinator fixture/story/capture files only as needed to prove separation
- existing NewConversation fixture/story/capture files only if creation recovery is representable from shipped contracts
- a dedicated QA-only ProductConversation real-journey harness under `tests/e2e/`, plus harness-local tests/docs/fixtures
- `tests/e2e/run.py` only if a narrowly reusable isolation helper cannot live in the dedicated harness without duplication; do not alter existing scenario semantics
- `dev.py` and `ui/package.json` only for QA command routing/documentation/testing

### Explicit non-goals

- No production `ProductConversationPage` or other production page/component behavior.
- No production API implementation/types, Rust, SSE/codegen, routes, Sidebar, stores, atoms, persistence, SQLite, migrations, ChainProvider behavior, browser storage, lifecycle transitions, Close behavior, creation/recovery mutations, or backend endpoints.
- No aliasing transcript/root/chain identity to ProductConversation identity; no mapping legacy `archived` to History.
- No new feature PR and no PR merge.
- No production behavior changes made in response to a journey failure; report the exact blocker to the owning production workstream.
- No manual-only journey or screenshot assertion presented as automated real-product coverage.
- No deletion/replacement of files merged by PR725.

## Acceptance and validation

### Fixture assertions

- Open appears deterministically on desktop and mobile with one consistent root title and only the latest writable tail exposing the ordinary composer where the shipped contract permits it.
- History preserves chronology on desktop and mobile without composer, Close, Archive, chain-management, or mutation controls.
- The exact chosen handoff string appears once, at the correct segment join, and is not the successor's first user message.
- Long-transcript scrolling and a deterministic rerender neither duplicate nor remove the handoff marker.
- Coordinator has an independent ready marker/story and is not labeled or classified Open/History.
- Any creation-recovery story exposes only shipped statuses/actions and performs no mutation; if contracts do not permit the requested fixture presentation, tests/docs record the bounded omission.
- Fixture startup performs no real network or browser-storage work.
- Automated real-product journeys, each in an isolated worker dev instance, pass for: create + exact initial objective; exactly one aggregate/list row; multi-transcript ordering + exact handoff once + latest writable member; clean Close to History; busy stop-work confirmation; dirty-worktree exact loss confirmation; creation failure discovery/retry without duplication; and reload/reconnect identity stability.
- Journey evidence records the isolated instance identity/config, stable product/transcript IDs, expected and observed list cardinality/handoff cardinality/lifecycle/writable owner, and bounded logs/artifacts without leaking fixture secrets.

### Local validation

- Run focused Vitest for changed fixtures, stories/helpers, and capture routing.
- Run UI TypeScript checking.
- Run deterministic ProductConversation desktop/mobile capture and any touched Coordinator/recovery capture; inspect every generated image and retain paths/checksums or equivalent evidence. Screenshots satisfy only the fixture gate.
- Run every commissioned automated real ProductConversation journey repeatedly enough to demonstrate deterministic isolation, including clean teardown and a second fresh instance that cannot observe the first instance's state. These journeys satisfy the separate real-product gate.
- Run the full applicable `./dev.py check` lanes, including task validation, without broadening into unrelated fixes.
- Proactively invoke `phoenix-adversarial-review` on the exact candidate HEAD, then obtain a separate independent review; resolve all findings and rerun affected validation.

### PR702 reconciliation and remote gates

- After local validation and both reviews, replace `origin/task-17003-productconversation-fixture-presentation` with the reviewed exact HEAD using one explicit guarded `--force-with-lease=<ref>:<observed-old-oid>` push. Never use an unguarded force push.
- Treat that replacement as immutable: do not amend/rewrite the pushed commit; any fixes are new commits and fast-forward pushes, each reviewed and validated at its exact HEAD.
- Keep PR702 as the reconciled PR rather than opening/layering a new feature PR.
- Wait for CI at the final exact HEAD and require all applicable checks green.
- Run/obtain exact-head Codex review after the final push and resolve its findings with new commits as needed.
- Enumerate all PR702 review threads through paginated API queries and require zero unresolved threads at the final exact HEAD.
- Mark task 17003 done with `taskmd` only after local checks, fixture captures, all commissioned isolated-instance journeys, reviews, immutable push, CI, exact-head Codex, and zero-thread gates succeed. Never merge PR702.
- The PR702 candidate must not proceed to devmbp UAT until both the fixture/capture gate and commissioned automated real-journey gate are green at the same exact HEAD. Record that exact HEAD and evidence links in the UAT handoff.
