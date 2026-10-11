# Target conversation replay: dependency map and implementation seams

## Approved objective and boundary

The normal Phoenix binary, entry point, route table and production UI hydrate target ProductConversation `7c2d9446-46a1-42b4-bbad-c8ddfbfddf7f` from an immutable sanitized fixture without constructing execution authority. Calibrate against local production before agreeing repeated baseline samples. No optimization, continuation/recovery, mock-provider execution, durable-wake removal, general read-only coverage or Nono integration.

Standing rules: partial capability coverage is acceptable; parallel application behavior is not. Observation does not imply activation. Expected capability absence is ordinary and explicit. Commands cannot report successful effects that did not occur. Individual callers use specific capabilities, not a global read-only flag.

## Verified journey

```mermaid
flowchart TD
    List["List click"] --> Alias["App alias resolution: coordinator identity and small product snapshot"]
    Alias --> Product["ProductConversationPage: initial page snapshot"]
    Product --> Transcript["Historical plus latest transcript projection"]
    Product --> Embedded["EmbeddedConversationPage: composer-only"]
    Embedded --> Route["Member ID or slug resolution"]
    Route --> Stream["useConnection and stream initialization"]
    Stream --> Hydrate["Validated init hydrates conversation atom"]
    Stream --> Materialize["Nonterminal runtime materialization"]
    Stream --> Normalize["Possible persisted Fast-tier correction"]
    Embedded --> Ancillary["Models, credentials, PR, prompt and resource owners"]
    Product --> Telemetry["Open telemetry command"]
```

`showTranscript=false` is a rendering choice, not absence of live dependencies. `mutationEnabled=false` suppresses the composer but does not prevent those dependencies from mounting. Aggregate Open lifecycle is not proof of execution capability.

## Dependency ledger

| Operation | Current owner | Classification | Required replay treatment |
|---|---|---|---|
| Auth/bootstrap status | App + auth handlers | Host observation; login writes persistent sessions | Truthful composition facts; no fake provider credentials; isolate/reject persistent session authority |
| Product/member/coordinator alias resolution | App, ConversationPage, route handlers | DB observation | Shared `ConversationRouteObservations`, preserve namespaces/errors/coordinator continuation membership |
| Initial/older product snapshots | ProductConversationPage + product_conversations helpers | DB read/projection | Same production SQL, cursors, metadata and serialization; retain request multiplicity for baseline |
| Member stream transcript selection | stream_conversation + stable_transcript_read | DB observation plus process-local cursor evidence | Extract shared initialization; do not substitute static init bytes |
| Live broadcaster/replay ring | RuntimeManager + SSE pipeline | Process-local observation | Preserve honest completeness/cursor semantics; DB-only capture cannot reproduce in-flight token evidence |
| Runtime materialization | stream_conversation calls get_or_create for nonterminal rows | Execution admission | Separate activation from observation; absent activation authority is never constructed/invoked |
| Fast-tier correction | stream_conversation | Persisted command | Keep normal path semantics; do not run in observation composition |
| Conversation enrichment | enrich_conversation_with_seed/runtime | Persisted metadata plus live browser/tmux/resource facts | Shared persisted projection with explicit optional observation sources; no invented defaults implying live success |
| Legacy active/archived lists | refresh driver, DesktopLayout, Sidebar | DB observation plus repeated live refresh | Preserve list workload for calibration; one-shot read and subscription/polling capabilities distinct |
| Models/helper/login | useModels, useAutoAuth, CredentialHelperPanel, Sidebar | Provider observation and credential execution | Not needed to display history; capability-aware owners without fabricated valid model |
| PR refresh/system prompt/work inventory | ConversationPage hooks | Persisted seed plus external/runtime observations | Retain persisted seed; unsupported observation explicitly unavailable; no git/provider/process work |
| Terminal/browser/file viewers | conditional/restored UI owners | Host/resource reads and execution | Specific capabilities at owners; restored UI cannot bypass absence |
| Notification settings | shell/settings owners | Observation plus mutation/browser effects | Specific settings/subscription capabilities; no accidental migration writes |
| Telemetry | Product open reporting and conversationOpenTelemetry | Optional command | Specific recording capability; local timing observers remain available |
| Send/cancel/approval/model/file actions | composer/WorkActions/task owners | Commands | UI unavailable and server admission rejects independently |
| Durable wake | optional obsolete status UI and startup worker | Separate producerless subsystem | Not part of replay journey; exclude execution machinery without extending/removing it here |

## Startup hazards

`run_server` performs work before `AppState::new`: writable migrations/settlement, worktree and project reconciliation, skills/companion writes, provider discovery, MCP setup and host probes. `AppState::new` performs close/continuation/wake settlement and starts workers, PR polling, attachment cleanup and retrieval reconciliation. Replay selection must precede these, not merely omit API keys or disable one runtime flag.

## Adversarial review outcome

An enum around only RuntimeManager is insufficient: AppState also owns providers, MCP, terminals, chains, credentials and writable DB. Making all those fields optional is rejected. The proposed next seam is an application envelope with shared observation services and a typed live dependency bundle, with fallible request extraction that rejects unavailable execution before body/upload/WebSocket handling. Keep a single route table. No Deref fallback that silently grants live authority in replay.

The request-composition seam is implemented: `ApplicationState` owns either the unchanged live `AppState` bundle or bounded observation services; `LiveState` rejects unavailable live authority before request bodies/upgrades. The normal route table is shared. Shared observations expose bounded methods, not writable pool/DB accessors. Auth middleware preserves the live path and admits observation only for direct loopback peers with no forwarding headers; no observation session store exists. Startup selection, shutdown and transcript service extraction remain pending; GET alone is not an observation classification.

## Implementation status

1. Route observations extracted from production handlers; immutable fixture tests plus normal HTTP route tests establish the initial read seam.
2. Request composition/admission implemented and tested without replay startup: immutable same-router reads, pre-body/multipart/unsafe-GET/WebSocket denial, peer/forwarding policy, unchanged fixture, plus normal password/cookie/bearer-lockout regressions. Early normal-binary startup selection is implemented and independently reviewed; shared transcript services remain next. The replay branch does not construct the live bundle.
3. Then: shared transcript initialization and persisted enrichment, preserving normal ring/cursor/subscribe-before-read semantics.
4. Coordinate UI capability callers with the current ProductConversationPage streaming-render owner before changing its interfaces.
5. Delete separate fake benchmark server and auxiliary responses only when replacement works through real application.
6. Correctness pilot, calibration report, then sample/reset/cache/budget check-in before baseline collection.

## Open verification work (engineering, not product choices)

- Validate frozen fixture init completeness without pretending in-flight runtime evidence is present.
- Inspect target state and fixture's normalization losses; no new production-derived copy without specific authority.
- Preserve large legacy list interference and account for changed DB physical layout/compression/backend profile.
- Cover full normal router's unsafe GETs, multipart and WebSocket admission; immutable DB failure alone is not an isolation proof.
- Cross-check normal stream regressions and token-render isolation; do not absorb peer UI fixes.

## Coordination

Global handed off shepherding with UI owner overlap at ProductConversationPage. Reply tools rejected both provided Global addresses (stable target not found; exact transcript prohibited as Coordinator chain). Report milestones in this conversation until a permitted routing target exists; this delivery failure is not a reason to stop approved work. Durable-wake removal belongs to separate user-driven work.

## Validation milestone: composition foundation

- Route observation commit: `76023ec169b42ee1e1568f694d4124a2601a3968`; two immutable observation tests and nine normal route tests passed, focused fmt/clippy passed, independent source review found no blocker.
- Asset containment correction: `7116d7a73`; eight asset tests passed, including raw traversal and symlink escapes. This inherited issue was a blocker for treating public assets as bounded observation reads. Canonical containment assumes a trusted owned UI directory, not hostile concurrent filesystem replacement.
- Request composition: six full-router tests passed (three immutable observation/admission tests and three live-auth regressions); fmt/clippy passed. The broader API run passed 676 tests and failed only the new test's incorrect share URL; corrected admission suite then passed. Run normal live fixture tests with `PHOENIX_DISCOVERY_ENABLED=false` to avoid unrelated host service probes. AppState workers in those live regressions operate on disposable in-memory state and isolated runtime directories.
- Both ast-grep rules accept current source and reject deliberately injected authority bypasses. They remain syntactic guardrails, not isolation proofs.
- Replay startup/browser hydration has NOT been delivered by this milestone. No new fixture capture, production mutation or optimization occurred. Old fixture-server removal is pending replacement.

## Resumable next slice at HEAD 364b86986

Approved implementation continues; no new design approval is required. Startup review identifies an additional early boundary: `main::parse_invocation` currently rejects unknown server flags, and `main` installs a fatal diagnostic hook before `run_server`. Replay selection must happen before inherited fatal-log configuration can write on startup errors, not merely before AppState. Prefer one explicit CLI form (`--observe-fixture ABSOLUTE_DB --target REFERENCE [--port PORT]`), no parallel environment authority and no fallback to Live on incomplete arguments.

Next bounded implementation:

1. Typed invocation selection; preserve existing live/migrate/suggest/sandbox/build-identity dispatch.
2. Move the existing standalone fixture validator out of load_benchmark via clipboard into a shared owner (do not duplicate). Read-only/regular/absolute/no-hardlink/no-sidecar checks remain. No new fixture capture.
3. Add validated typed target scope to bounded observations, using the actual ordinary product resolver and aggregate topology; do not cache a duplicate member set. Preserve ID versus slug namespaces; reject off-target/child/disconnected rows.
4. Normal-binary observation branch before logging/provider config/probes/recovery, using the SAME create_router. Supply ConnectInfo from a real loopback listener. No normal AppState construction, no dummy live dependencies.
5. Validate schema/read compatibility by the needed shared reads; fixture builder omits migration ledger rows, so MAX(version) is not a valid compatibility test. Do not migrate or promise historical compatibility.
6. Observation shutdown owns only listener/read services, not live runtime retirement or bash kill-tree; bounded SIGINT/SIGTERM drain, close DB after transport release.
7. Actual executable tests with marker-writing credential-helper and local network sentinel configurations verify that live initialization is never invoked; unchanged fixture/hash/sidecars, scope matrix and real ConnectInfo are required.

This intermediate server will still reject product snapshots, lists and streams until their shared observation service extraction lands. Do NOT claim browser hydration, delete the prior scaffold prematurely, or collect a benchmark from this intermediate milestone. Follow with shared transcript initialization/persisted enrichment, then coordinated UI capability callers, replacement/scaffold deletion and calibration check-in.

Review provenance: final corrected composition review approved tracked diff SHA-256 `7a0633bf43f8ca2cb2e5e09aa04d284ede1adfb60554f38ba2706355624b1454` against asset-fix HEAD `7116d7a73`; code committed as `4af6894f1`, contract/status committed as `364b86986`. The review explicitly excludes startup/SSE/UI/calibration claims.

## Functional milestone: normal-binary observation startup

The normal executable accepts one explicit intent: `--observe-fixture ABSOLUTE_DB --target REFERENCE [--port PORT]`, default port 8043. Selection happens before fatal diagnostic hooks and all live startup. The live `run_server` body is untouched. Observation opens a validated immutable file, preflights the actual scoped read services, serves the existing route table with real loopback ConnectInfo/compression, and performs bounded signal shutdown without live retirement/cleanup. It consumes no inherited live provider, password, TLS, bind, database or diagnostic configuration.

Route scope is a typed canonical ProductConversation identity, not a cached member set. Exact product-ID references use the aggregate canonical root in observation only; exact transcript/slug references must belong to the root's forward chain. This avoids admitting children/disconnected rows and avoids depending on the legacy resolver's lexical candidate selection. Normal resolution behavior is unchanged.

Current proof: 2 actual-normal-executable tests, 3 immutable observation tests, 4 invocation/parser tests, 6 full-router admission/live-auth regressions, 3 normal route regressions and 8 asset regressions passed; focused fmt/clippy passed. Executable tests use disposable synthetic fixtures without a migration ledger and intentionally enabled provider/helper/discovery/logging traps, checking no sentinel activity, immutable bytes/no sidecars, unrelated-reference denial, unavailable command/read admission, startup errors without live fallback and SIGINT/SIGTERM shutdown. Negative corrupted-topology fixtures explicitly remove relevant schema guards only during disposable test setup. The startup guardrail accepts the current source and rejects an injected `run_server` call.

The independent exact-source review approved the corrected startup slice. It did not independently run tests or approve hydration/UI/calibration. Source has a 5-second HTTP drain plus a separately bounded 5-second pool close; hanging-request stress coverage remains a limitation. A same-host proxy stripping all forwarding headers is outside the direct-loopback detection guarantee.

This is a real functional startup milestone, NOT a working conversation viewer: product snapshots/lists/member stream still return capability-unavailable until shared hydration is extracted. Do not point users at it as a benchmark. Old fake-server tooling remains only until replacement can load the target. No new fixture capture, production mutation/deployment or calibration choice occurred.

Next: promote shared product read/projection and persisted member hydration into bounded observations; separate stream activation/normalization from shared init and preserve subscribe-before-read, generation/cursor and final-message identity semantics. Existing fixture was normalized to Idle and omitted runtime obligations; prove sufficient settled observation evidence before claiming faithful stream replay. No fake process metadata, model availability or static SSE bytes. Then coordinate specific-capability UI callers and remove scaffold.

UI overlap warning: supplied peer hash `e8090e7ae61a3801979373738da3c4a0db68fb18` resolves in this checkout to Markdown disclosure-recovery changes, not the described coherent-transcript-finalization candidate. Obtain the actual interface/source identity through shepherding before editing ProductConversationPage; no UI changes have been made here.

## Shared hydration milestone: product detail and persisted transcript owner

- `cd2e80875`: the normal product-detail route uses `ConversationRouteObservations` in both compositions and delegates to the exact `read_product_conversation_snapshot` reader/projection. Live forwards the original reference/query unchanged. Scoped observation validates target and reachable member, preserving requested-member/cursor semantics; real snapshots now work over the actual normal binary. Product list remains live-only; no canned responses were introduced.
- Validation: 5 observation/parity/pagination tests, 28 normal product tests, 6 admission/live-auth tests and 2 normal-executable tests passed; fmt/clippy passed. Independent exact-source review approved three-file diff SHA-256 `e0b3f9178231b5e3405adc4861a6f0c65b4b14f4f97037b9bdbe03fba37317c7` against `89f74d0c4`.
- Calibration caveat: scoped validation performs additional resolver/topology reads before the production snapshot helper. This is real scope admission overhead absent from Live, not an optimization. Its request time must be separated from helper stage timing during calibration; do not infer production parity from replay wall time.
- `7cf659a68`: generation-stable transcript reading, render-unit alignment, bounded latest/before selection, stream DB selection/coverage and test generation-race hooks moved to a shared persisted observation owner. Latest/before bounded methods privately reference Database and enforce positive/clamped limits. Normal stream reservation/subscription, ring-before-durable read, generation retry, cursor selection and init floor ordering remain unchanged. No fake SSE events or metadata were added.
- Validation: 38 transcript tests, 15 stream tests, 4 message-history tests, fmt/clippy passed. Independent review approved behavior on source hashes `handlers.rs=166ed39a20c9bd2e12b95cef8174e6f7373bbacc39fea16b169f5df98dfc491c` and `transcript_observation.rs=14502f2335011ab55939ee07eabc0c8715591db92513ffdf0a3859da8417cab6`. Guardrail coverage was then extended to the new owner and tested: `TranscriptCoverage` data import allowed, injected `RuntimeManager` import rejected. The review's initially supplied abbreviated baseline was mistaken; actual baseline was `cd2e80875`, verified before commit.

Remaining stream-init seam: handlers still own activation/service-tier normalization, broadcaster/ring evidence, process-derived browser/tmux observations, conversation metadata and typed Init assembly. Next extract persisted enrichment and typed initialization with explicit process-evidence input, not a Runtime trait or fake live bundle. Keep normal subscribe-before-read and checkpoint/finality invariants; DB-only fixture normalization to Idle is not proof that all live in-flight evidence was captured. Use existing private fixture only; no new capture has been authorized. No unsupported metadata should be silently encoded as healthy/valid/live.

Peer UI clarification received: `e8090e7ae` is the tip of a four-commit candidate, not the single coherent-snapshot commit. The full `c5c33a53..e8090e7ae` range was inspected: early `18707e7cc` and `9365b3190` introduce coherent structural snapshots and request-matched streaming leaves/pending acknowledgment filtering; later Markdown disclosure corrections are expected. No cherry-pick or UI edits were made. Coordinate the candidate's publication/actual integrated source before touching ProductConversationPage; preserve leaf token isolation and final-message dedup contracts.

No new fixture capture, production changes, provider/tool execution, baseline sample choices or optimization occurred. The fake HTTP scaffold remains pending a correct shared member initialization/UI replacement. These are hydration-building milestones, not a calibrated baseline or completed replay.
