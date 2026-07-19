# Production paper-cut hunt

## Scope and method

This inventory targets small, independently shippable latency and UX improvements. Durable-workflow, inbox/outbox, and endpoint-architecture migrations are intentionally out of scope.

Production evidence came from bounded VictoriaTraces queries for `phoenix-ide`: a maximum seven-day window ending 2026-07-18, at most 1,000 results per route, and full traces fetched only for selected exemplars. Counts of `1,000+` hit the result cap. Percentiles describe returned samples, not complete service-level metrics.

Evidence labels:

- **M** — measured in production traces.
- **C** — directly established from code behavior.
- **H** — plausible hypothesis requiring measurement or UX confirmation.

Effort is **S** (hours), **M** (roughly a day or two), or **L** (larger than a paper cut and included only when it can be sliced).

## Highest-return shortlist

| Rank | Paper cut | Evidence | Why first | Small next step |
|---:|---|---|---|---|
| 1 | Branch picker takes seconds | M+C | 10 samples; p50 4.37 s, max 9.96 s, violating the “instant” local-list intent | Replace per-branch remote-existence subprocesses with one remote-ref inventory, then measure remaining behind-count cost |
| 2 | PR status poll is routinely multi-second | M+C | 1,000+ samples; p50 2.97 s, p95 4.09 s, max 11.07 s | Coalesce in-flight refreshes and apply a short freshness budget before running another refresh |
| 3 | Mark-merged gives no feedback for tens of seconds | M | 7 samples; p50 14.2 s, max 29.0 s | Add phase progress copy and disable duplicate submission; profile PR refresh vs cleanup separately |
| 4 | Skills listing has a CPU-heavy tail | M | 11 samples; p95/max 686 ms, almost entirely busy time | Cache discovery by resolution root and invalidate on relevant filesystem changes or short TTL |
| 5 | Work-scope inventory polling has a visible tail | M+C | 1,000+ samples; p95 263 ms, max 562 ms | Do not overlap polls; skip expensive resource sampling when the panel does not display it |
| 6 | Conversation list common path is not cheap | M | 1,000+ samples; p50 38 ms, p95 142 ms, max 667 ms | Measure enrichment phases and avoid full-list refreshes after local mutations |
| 7 | Chat acknowledgement has rare multi-second stalls | M+C | 156 samples; p50 3 ms but max 3.42 s, mostly busy time | Add phase timings for lock wait, expansion, runtime lookup, and dispatch |
| 8 | File search blocks command-palette results | M+C | 7 samples; p50 98 ms, p95 119 ms plus client debounce | Show stale/cached results while refreshing and share identical in-flight searches |
| 9 | Message-latest reads have an avoidable tail | M+C | 267 samples; p50 13 ms, p95 47 ms, max 217 ms | Run independent tail metadata read concurrently or return it from the aligned-message query |
| 10 | Directory picker performs multiple checks per edit | C | List and validation debounces can produce two or three requests and flickering state | Share one normalized-path request state and validate parent only when needed |

## Catalog: 100 paper cuts

### Production-measured request cuts

| # | Surface and evidence | Quick win | Impact | Effort |
|---:|---|---|---|---|
| 1 | **M+C** `GET /api/git/branches`: n=10, p50 4.37 s, max 9.96 s. `list_local_branches` runs remote verification per branch (`api/git_handlers.rs`). | Inventory `refs/remotes/origin` once; eliminate one subprocess per branch. | High | S |
| 2 | **M+C** Same route: each tracked branch runs a separate behind-count subprocess. | Cap behind-count work to visible/recent branches, or compute lazily after the initial branch list renders. | High | M |
| 3 | **M+C** `build_branch_conflict_map` runs synchronous `git worktree list` before entering `spawn_blocking`. | Move the Git call off the async request worker. | Medium | S |
| 4 | **M+C** `GET /api/conversations/:id/pr-status`: n=1,000+, p50 2.97 s, p95 4.09 s, max 11.07 s. | Add per-work-scope in-flight request coalescing. | High | S |
| 5 | **M+C** PR status can sequentially query branch, active PR, then retargeted PR. | Use the new phase spans to skip a direct lookup when persisted identity is already fresh. | High | M |
| 6 | **M+C** Every mounted PR status hook polls at 60 s and refreshes on visibility. | Add a freshness timestamp and suppress back-to-back visibility/poll refreshes. | High | S |
| 7 | **M** `POST /api/conversations/:id/mark-merged`: n=7, p50 14.2 s, max 29.0 s; exemplar is almost entirely idle/wait time. | Show explicit “checking PR / cleaning workspace / finishing” progress instead of one undifferentiated spinner. | High | S |
| 8 | **M+C** Mark-merged can refresh PR status before acting. | Reuse a sufficiently fresh PR snapshot instead of unconditionally paying for another external lookup. | High | S |
| 9 | **M** `POST /api/conversations/:id/archive`: n=11, p50 90 ms, max 3.71 s. | Show immediate cleanup progress and preserve the row with an “archiving” treatment until completion. | Medium | S |
| 10 | **M+C** `POST /api/conversations/:id/chat`: n=156, p50 3 ms, p95 203 ms, max 3.42 s. | Instrument lock wait, reference expansion, runtime lookup, and dispatch to isolate the rare stall. | High | S |
| 11 | **M+C** Chat acceptance holds the shared acceptance lock across DB reads and expansion. | Narrow the lock to receipt/idempotency bookkeeping where correctness permits. | High | M |
| 12 | **M** `GET /api/skills`: n=11, p95/max 686 ms and 686 ms busy. | Cache skill discovery per resolution root for a short TTL. | Medium | S |
| 13 | **M+C** Skills are loaded independently by composer/reference surfaces. | Share a root-keyed skill cache and in-flight promise in the UI. | Medium | S |
| 14 | **M** `GET /api/work-scope/:scope_key/inventory`: n=1,000+, p95 263 ms, max 562 ms. | Prevent overlapping inventory fetches and ignore poll ticks while one is active. | High | S |
| 15 | **M+C** Inventory conditionally takes process resource samples. | Request or compute resource samples only when the expanded UI consumes them. | Medium | M |
| 16 | **M** `GET /api/work-scope/.../inspect`: n=3, p50 225 ms. | Keep the previous inspection visible while refreshing rather than showing a loading replacement. | Medium | S |
| 17 | **M+C** Process CPU sampling requires two refreshes separated by a minimum interval. | Split “fast process facts” from optional CPU sampling in the inspector response. | Medium | M |
| 18 | **M** `GET /api/conversations`: n=1,000+, p50 38 ms, p95 142 ms, max 667 ms. | Add bounded phase timings for base query vs enrichment before optimizing. | Medium | S |
| 19 | **M+C** List mutations trigger full conversation-list refreshes. | Optimistically update archive/delete/rename rows and reconcile in the background. | High | M |
| 20 | **M** `GET /api/conversations/archived`: n=1,000+, p50 12 ms, p95 75 ms, max 335 ms. | Fetch archived conversations only when the archived view is opened. | Medium | S |
| 21 | **M+C** `GET /api/conversations/:id/messages/latest`: n=267, p50 13 ms, max 217 ms. | Run message slice and server-tail metadata reads concurrently. | Medium | S |
| 22 | **M+C** Other message range/around paths repeat the serialized tail read. | Introduce one query/helper returning rows and tail metadata together. | Medium | M |
| 23 | **M** `GET /api/files/search`: n=7, p50 98 ms, p95 119 ms. | Retain previous results while a debounced search refreshes. | Medium | S |
| 24 | **M+C** Inline file references debounce at 80 ms and command palette at 120 ms independently. | Share query results/in-flight work by root + query. | Medium | M |
| 25 | **M** `GET /api/files/list`: n=1,000+, p50 13 ms, p95 42 ms. | Skip root polling when the file explorer is hidden or the tab is not visible. | Medium | S |

### Frontend request and state cuts

| # | Surface and code evidence | Quick win | Impact | Effort |
|---:|---|---|---|---|
| 26 | **C** `DirectoryPicker`: list at 150 ms, validate at 300 ms, then possible parent validation. | Normalize once and share one debounced state machine; avoid validating both child and parent by default. | High | M |
| 27 | **C** `DirectoryPicker` sets loading before the request starts. | Set “loading” only when the debounce fires; retain cached suggestions meanwhile. | Low | S |
| 28 | **C** Picker blur uses a fixed delay so suggestion clicks can race closure. | Use pointer-down selection or focus-within semantics instead of a blur timer. | Medium | S |
| 29 | **C** `ConversationSettings` invokes task loading from both click and change handlers. | Route both through one guarded `ensureTasksLoaded`. | Medium | S |
| 30 | **C** Task picker says “Loading” before lazy loading has started. | Distinguish “open to load,” “loading,” “empty,” and “failed.” | Medium | S |
| 31 | **C** Branch search marks loading during its 300 ms debounce. | Show “searching” only once the request starts; keep local results during debounce. | Low | S |
| 32 | **C** Sidebar refetches projects whenever conversation count changes. | Refresh projects only on project-affecting events. | Medium | S |
| 33 | **C** Sidebar project fetch has no stale-result guard. | Add request sequencing or abort. | Medium | S |
| 34 | **C** Sidebar, list page, and login panel independently call Codex preflight. | Use one short-lived cached/in-flight preflight hook. | Medium | M |
| 35 | **C** Desktop layout polls global coordinator every 5 s without an explicit in-flight latch. | Skip ticks while the prior request is active. | Medium | S |
| 36 | **C** `useConversationPrStatus` visibility refresh can overlap scheduled polling. | Add one in-flight promise and minimum refresh interval. | High | S |
| 37 | **C** PR pin/resume mutations always block on a full status refresh. | Apply returned/known selection state immediately, then refresh in background. | Medium | M |
| 38 | **C** WorkActions capture-feedback flow waits for send, then waits for PR refresh. | Let send completion unblock the action; refresh status independently. | Medium | S |
| 39 | **C** WorkActions safety flow refreshes PR status before destructive actions. | Reuse fresh state and refresh only when stale or ambiguous. | High | S |
| 40 | **C** Conversation-list mutations await server then full `refresh()`. | Patch local store from mutation response or known result. | High | M |
| 41 | **C** Opening chain-delete confirmation fetches the chain first. | Reuse list/root metadata and fetch details only when truly missing. | Medium | S |
| 42 | **C** Provisioning delete waits for IndexedDB deletion before navigation. | Navigate after server success; clean local cache in the background. | Medium | S |
| 43 | **C** Continue-conversation logic is duplicated across two branches. | One pending-guarded handler prevents double clicks and divergent errors. | Medium | S |
| 44 | **C** Route resolution repeatedly tries ID then slug fallback. | Cache resolved route identity for the session. | Medium | M |
| 45 | **C** Transcript alignment recovery can repeatedly fetch the full conversation. | Cache one fallback per transcript generation. | Medium | M |
| 46 | **C** Usage panel fetches every time it opens. | Add a short per-conversation TTL and stale-while-revalidate display. | Low | S |
| 47 | **C** Approval context-window fetch can land after the panel closes/reopens. | Abort or sequence requests by panel generation. | Low | S |
| 48 | **C** Work-scope inventory requests are ignored after scope change but not aborted. | Abort obsolete requests to reduce wasted backend work. | Medium | S |
| 49 | **C** Slug lookup is repeated from multiple message/fork affordances. | Memoize immutable conversation-id → slug results. | Low | S |
| 50 | **C** MCP panel refetches status after reload and every toggle. | Use mutation responses when complete; dedupe the fallback status refresh. | Medium | S |
| 51 | **C** MCP reload uses polling plus a fixed 5 s spinner timeout. | Model explicit success/failure/timeout states and stop polling on terminal response. | Medium | S |
| 52 | **C** Local services poll every 15 s whenever mounted. | Pause when collapsed/hidden; refresh immediately on open. | Low | S |
| 53 | **C** File tree polls root every ~10 s while visible even when unchanged. | Pause on hidden tab and after repeated unchanged responses. | Medium | S |
| 54 | **C** Expanded directory loads can duplicate during rapid restore/click. | Store one in-flight promise per directory. | Medium | S |
| 55 | **C** Process inspector polls every second and can keep retrying seed requests. | Stop/reduce polling when hidden and add bounded backoff after failures. | Medium | S |
| 56 | **C** Every process inspector owns a 1 s elapsed timer. | Derive elapsed display from one shared clock. | Low | S |
| 57 | **C** StateBar owns separate 1 s elapsed and heartbeat timers. | Use one shared tick and memoize unaffected children. | Low | S |
| 58 | **C** Terminal command HUD ticks every 100 ms. | Reduce to 250–500 ms or update only when the formatted value changes. | Low | S |
| 59 | **C** Terminal unread state flushes on a 200 ms interval. | Schedule one animation-frame/microtask update when the ref changes. | Low | S |
| 60 | **C** Browser view always waits 1.5 s before reconnect. | Retry immediately once, then back off; show next retry timing. | Medium | S |
| 61 | **C** Browser connecting overlay is mostly blank status text. | Preserve the last frame and add reconnect/cancel guidance. | Medium | S |
| 62 | **C** Command palette waits 120 ms before async search and shows stale results without a searching cue. | Keep results but mark them stale/searching; bypass debounce for pasted/full paths. | Medium | S |
| 63 | **C** File search hides opaque results entirely. | Show disabled “cannot preview” matches rather than making files disappear. | Medium | S |
| 64 | **C** File source gives no hint for an empty query. | Show “type to search files” instead of an empty result area. | Low | S |
| 65 | **C** `TaskViewer` conflates not loaded and failed, leaving Start disabled. | Give loading/error/empty distinct states and a retry control. | Medium | S |
| 66 | **C** `SkillViewer` and task viewer collapse file errors to generic/raw text. | Map size, missing, binary, and confinement failures to actionable copy. | Low | S |
| 67 | **C** TasksPanel count is not invalidated after task changes. | Patch/invalidate counts after relevant actions or SSE changes. | Medium | S |
| 68 | **C** TasksPanel displays “loading…” for absent, pending, and failed counts. | Render explicit unavailable/retry state. | Medium | S |
| 69 | **C** Conversation diff retry immediately repeats the same fetch. | Disable while pending and add a short retry backoff/status reason. | Low | S |
| 70 | **C** Diff viewer lacks a stale-result sequence guard across rapid prop changes. | Add request generation/abort and retain the prior diff until replacement arrives. | Medium | S |

### Rendering, focus, and timer cuts

| # | Surface and code evidence | Quick win | Impact | Effort |
|---:|---|---|---|---|
| 71 | **C** Message find highlight probes the DOM at three fixed delays. | Observe row mount/virtualizer completion and highlight once. | Medium | M |
| 72 | **C** MetaViewer jump-to-line waits a fixed 100 ms. | Scroll from a layout effect or viewer-ready callback. | Medium | S |
| 73 | **C** MetaViewer focus/jump uses another fixed 100 ms delay. | Consolidate with the same readiness signal. | Medium | S |
| 74 | **C** Phoenix code/diff views defer open/scroll actions with timers. | Expose a rendered callback and queue one pending target. | Medium | M |
| 75 | **C** QuestionPanel uses several zero/200 ms focus and advance timers. | Move focus in layout effects keyed by active question/step. | Medium | M |
| 76 | **C** QuestionPanel initial focus is delayed and can miss during rapid input. | Use a ref callback or layout effect when the input becomes enabled. | Low | S |
| 77 | **C** FileTree context menu installs click-outside handling one tick late. | Stop propagation on the opening event and install listeners immediately. | Medium | S |
| 78 | **C** Context menu renders then clamps, causing positional jump. | Calculate dimensions offscreen or use CSS/positioning middleware before paint. | Low | S |
| 79 | **C** Message review highlight disappears after a fixed 2 s. | Clear on user navigation or provide a longer/accessibility-aware duration. | Low | S |
| 80 | **C** Toast and copy acknowledgements use inconsistent fixed timers. | Centralize duration policy and pause on hover/focus. | Low | S |
| 81 | **C** ContextIndicator opens into a blocking blank/loading state. | Render cached usage immediately and refresh unobtrusively. | Medium | S |
| 82 | **C** Task/Skill viewers show generic blank loading text. | Keep the surrounding metadata shell and use a content skeleton. | Low | S |
| 83 | **C** Draft persistence waits 300 ms and relies on switch-time flushing. | Flush on page visibility loss and before route transitions. | Medium | S |
| 84 | **C** Local-services stale rows use an ambiguous ellipsis. | Use explicit stale/error/refreshing symbols and last-updated time. | Low | S |
| 85 | **C** Terminal integration detection can show an unexplained placeholder for 5 s. | Show what is being detected and allow manual fallback sooner. | Low | S |

### Backend computation and I/O cuts

| # | Surface and code evidence | Quick win | Impact | Effort |
|---:|---|---|---|---|
| 86 | **C** `reconcile_accepted_messages` checks message IDs serially. | Add a batch lookup or bounded concurrent reads. | Medium | M |
| 87 | **C** `get_conversation_slug` loads a full conversation row. | Add/select a slug-only projection. | Low | S |
| 88 | **C** Cancel-provisioning writes, then rereads the conversation for SSE. | Return the updated row/state from the DB mutation. | Medium | M |
| 89 | **C** Rename persists, then reloads the conversation for its response. | Make rename return the updated row. | Low | M |
| 90 | **C** System-prompt handler serially reads conversation, persona, and coordinator status. | Fetch independent metadata concurrently or with one projection. | Medium | M |
| 91 | **C** Stream initialization rereads conversation generation alongside messages/tail. | Return generation in the stable transcript projection. | Medium | M |
| 92 | **C** `/api/list-directory`, mkdir, and file reads use blocking std filesystem calls on async workers. | Wrap bounded filesystem scans/reads in `spawn_blocking`. | Medium | M |
| 93 | **C** File root allowlists repeatedly canonicalize static task/skill roots. | Cache canonical static roots; keep cwd-specific roots per request. | Medium | S |
| 94 | **C** Preview allowlist canonicalizes every skill root on every request. | Reuse the same canonical-root cache. | Medium | S |
| 95 | **C** `task_entries_for_cwd` loads all conversations/projects, then filters in memory. | Query only matching cwd/project enrichment rows. | Medium | M |
| 96 | **C** `paginate_open_work` flattens all groups before slicing. | Paginate with an index walk without a second full vector. | Low | S |
| 97 | **C** Rebuilding paginated groups linearly searches groups for every item. | Keep a project-id → output-index map. | Low | S |
| 98 | **C** Task status discovery can scan task filenames repeatedly per conversation. | Build one per-request task-status index. | Medium | S |
| 99 | **C** Remote branch search repeatedly lowercases names while filtering/sorting. | Precompute one normalized key per result. | Low | S |
| 100 | **C** PR selection clones the complete PR list before choosing one. | Select by reference, clone only the chosen PR, then consume observations. | Low | S |

## Observability cuts that support the hunt

These are enablers rather than additional catalog items:

1. Add child spans to branch listing for `conflict_map`, `local_refs`, `remote_presence`, `behind_counts`, and `default_branch` using bounded phase names only.
2. Add chat-acceptance child spans for `acceptance_lock`, `idempotency`, `attachment_validation`, `reference_expansion`, `runtime_lookup`, and `dispatch`.
3. Add mark-merged phase spans for PR observation and each cleanup class without IDs or paths.
4. Add inventory phase spans for DB projection, live-handle inspection, and process sampling.
5. Add skills-list phase spans for discovery roots, filesystem scan, and merge/dedup.
6. Record a bounded HTTP `status_family` field to simplify error slicing.
7. Record tool outcome (`completed`, `cancelled`, `unknown_tool`) on the existing `tool.execute` span.
8. Record whether `conversation.turn` received a valid trigger link, without exporting trigger identity.
9. Add exported-span allowlist tests for every newly designated span.
10. Keep IDs, paths, branch names, PR identities, prompts, file contents, and tool arguments out of exported attributes.

## Suggested slicing

- **Batch A — request suppression:** items 4, 6, 14, 19, 25, 32–36, 48, 52–55.
- **Batch B — instant branch picker:** items 1–3 and 99, with before/after production samples.
- **Batch C — stale-while-revalidate UI:** items 16, 23, 46, 60–62, 70, 81–82.
- **Batch D — remove fixed-delay coordination:** items 28, 71–78.
- **Batch E — cheap backend reads:** items 21–22 and 86–100, selecting only changes supported by route frequency.

The target of 100 is an exploration device, not a mandate to implement all 100. Items should graduate to work only after a quick reproduction or measurement confirms user impact; low-impact allocation cleanup should not displace a measured latency or obvious interaction defect.
