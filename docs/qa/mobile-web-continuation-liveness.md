# Mobile-web continuation liveness — qualification record

## Scope and disposition

Draft PR [#1080](https://github.com/scottopell/phoenix-ide/pull/1080), task 46009. This record is an evidence ledger, **not a merge-ready or deployment approval**. No local build, test runtime, dependency installation, browser, shared-service restart, production mutation, or real-conversation exhaustion was used. Hosted CI and synthetic conversations are the qualification surface. Source inspection is not an executed pass.

Integration base: `bc92612f1c2b39fdbffd0499188c7b5192513142`. The RC5 source is `4da5f81c60d65d548254264003540bc7d462d2b0`; `fae542129…` identifies its annotated tag object, not source. This source comparison does not independently establish what production was running at incident time.

## Failure model and repair boundaries

Persistence records `continued_in_conv_id`, but the original automatic/manual/native service paths did not publish the committed edge to the existing predecessor broadcaster. A connected page could therefore retain a predecessor until reconnect/init or reload supplied the missing metadata. The repair publishes an admitted metadata invalidation after committed successor discovery, including recovered reservations. It does not synthesize readiness, resume a stopped turn, or change the URL to a transcript route.

The product page refreshes authoritative product/Global state from the observed link before adopting the latest member. Transport subscription and send destinations remain transcript IDs. Composer ownership is product-scoped, including draft/annotation storage and upload epochs; upload requests separately carry the actual transcript ID. Existing-conversation send validation accepts stored files only from the persisted continuation prefix ending at the target member. It does not authorize future members, unrelated chains, or subagents merely sharing a product.

## Actual qualification findings

- Independent review of `5be5e5826…` rejected the initial attachment implementation: stable UI scope was incorrectly used as an upload endpoint, and predecessor uploads were rejected on successor send. Both repair paths have source regressions. Subsequent static review at `8f7d47f232b79dc037193532e141eba785eced61` found no confirmed authorization escape but required DB-backed direction coverage; that regression was added afterward.
- Hosted run [43630693229](https://github.com/scottopell/phoenix-ide/actions/runs/43630693229) failed compilation and client qualification. These failures are not passes or baseline reproduction of the production incident.
- Hosted run [43632585468](https://github.com/scottopell/phoenix-ide/actions/runs/43632585468) exposed missing DB-error conversion and malformed delegated TS syntax. Later source corrections require fresh checks.
- Independent tracing found the synthetic continuation update had sequence 2 while init delivered messages through sequence 3. The real reducer correctly discarded that replay. Fixture events now use coherent per-member cursors; reducer replay guards were not weakened.
- The workflow contains an isolated mutation witness: remove native link publication, require the pending-opening regression to fail, restore source, and rerun. **Authored wiring alone is not failing-before/passing-after evidence.** The outcome must be read from the hosted build job.
- Mobile captures are wired for both ordinary and Global synthetic product surfaces. Screenshot existence, visual inspection, and exact-source run success must be recorded separately; no physical mobile/PWA keyboard claim follows from emulation.

## Bounded lifecycle audit

All entries below describe source-present coverage unless an exact hosted result is separately cited.

| Boundary | Source witnesses | Qualification limit |
| --- | --- | --- |
| Committed link → delivery → follow | `RuntimeManager::publish_continuation_link`; `conversation_update_serializes_continuation_link`; `fresh_concurrent_requests_dispatch_one_opening_through_exact_turn_settlement`; fixture `follows a published %s continuation through the real stream and resumes after disconnect` | Real React/connection/store composition, synthetic EventSource/API; no single Rust→HTTP→browser witness. |
| Reserved runtime publication race | `existing_broadcaster_lookup_holds_materialization_fence`; manager-owned broadcaster regression | Deterministic lock/channel source assertions; requires executed Rust result. |
| Automatic recovery / pending opening | Publication outside reservation branch; `pending_replay_requires_current_successor_capacity_and_retains_winning_intent` | Native retry covered in source; dedicated automatic `SuccessorReserved` recovery regression remains absent. |
| Manual failure/retry | `keeps explicit retry for a transient continuation failure`; `dispatch_failed keeps the edit on the parent for durable retry`; reviewed-input/model-guidance test | Action-level mocks, not dual-route manual browser journeys. |
| Readiness / running / idle / error | Product opening-authority guards; `does not overwrite authoritative idle when SSE completes before the chat POST response`; InputArea continuation-progress/recovery-gate tests | New journey jumps to a usable successor; delayed acceptance and successor POST not proven by that journey. |
| Question / task approval | `does not expose note sending while the latest row shows a question-only response panel`; `keeps task approval ownership on the aggregate route while suppressing the embedded transcript owner`; `gates task approval mutations behind liveControlsEnabled` | Ordinary authority source coverage; continuation directly into Global approval/resolution not newly exercised. |
| Stop versus completion | Read-only/archive guards in cancellation; `keeps Stop and steering Queue independently available while working`; `does not expose a cancellation control while awaiting continuation` | No new Stop-versus-handoff browser race or stopped-successor non-resume witness. |
| Close / History / intentional reading | `preserves loaded history across active Close refreshes`; `keeps a History aggregate read-only after a failed background refresh despite unarchived drift`; exact Global pin and reader-anchor tests | Preserve separate reading/composer authority; no real-conversation Close experiment. |
| Disconnect / duplicates / out-of-order | `replaces a failed stream and requires the replacement init to complete reconnect`; `drops a stale-EventSource event after slug change (cross-conversation contamination guard)`; atom overlapping-message/dedup/out-of-order tests | New journey adds synthetic successor reconnect and message uniqueness, not every metadata race. |
| Foreground / online / failed readback | Visible/online recovery effects; inline non-destructive Retry live view; fixture `retains the %s composer after failed continuation readback and retries without reload` | No physical suspension test. A stale successful snapshot still requires another invalidation, reconnect, foreground/online, or retry; no timer-based eventual-recovery guarantee. |
| Draft / file / annotation ownership | `migrates a transcript draft once and preserves an explicit clear over a surviving legacy key`; deferred upload scope tests; `continuation_attachment_authorization_uses_persisted_chain_prefix`; filesystem symlink test | Pending upload UI and backend validation are separate witnesses; full-composition attachment POST remains unproven. |
| Focus / reading / mobile layout | Failed-readback composer identity/focus assertion; existing deep-link and aggregate-reader anchor tests; before/followed/reconnected capture wiring | Handoff focus/selection/keyboard and scrolled-reader preservation need inspected interaction evidence; don't infer from draft retention. |

## Final gates

Before a merge-ready packet: record exact final HEAD and merge base; inspect hosted compile/fmt/clippy/codegen/UI/test results; read the mutation-witness outcome; download and inspect ordinary/Global screenshots; disposition exact-target independent review; classify any remaining lifecycle gaps explicitly. Until then PR stays draft, task stays in progress, and no merge/deploy is authorized by this record.
