# Stable skill metadata within a user turn

## Authority and scope

User-approved direction relayed by Global: keep skill catalog stable within a turn instead of rediscovering/rewriting between tool calls; refresh at a deliberate boundary; append authoritative additions/removals where the provider contract supports it, otherwise accept necessary prefix rewrite/cache miss; enforce runtime permission withdrawal immediately; explicitly decide body freshness. Approval does not authorize merge, deployment, public release, paid provider calls or broad provider rewrites. This brief is the Explore-to-Work gate for the bounded implementation, not a claim that every provider recommends the policy.

Related design evidence: `provider-cache-friendly-updates-design.md` and `immutable-conversation-heads-audit.md`. Language simplification and product-wide immutable language remain separately scoped; do not combine them into this patch.

## Intended behavior

An ordinary accepted user turn advertises one resolved skill metadata catalog through its model/tool loop. Editing, adding or deleting skills during that loop must not rewrite its earlier advertised catalog. Resolve refresh at the next accepted user-turn boundary, not every dispatch. Preserve source attribution and instruction authority. Provider-supported appended updates must remain in subsequent rendered history; do not send a transient update that disappears on the next request.

Where an exact provider contract or existing typed representation does not support the required authoritative update, use a correct boundary refresh with the necessary cache miss. Do not invent a protocol, demote instructions into ordinary tool output, or add an instruction-epoch/configuration-version framework. A turn metadata snapshot by itself does not establish all prior history stays append-only; describe the achieved guarantee accurately.

The Coordinator's authenticated embedded catalog and live trusted-result provenance remain separate and unchanged. Runtime authorization, tool/schema admission, WorkScope lifecycle, MCP auth/server readiness and cancellation remain live. Metadata is not a permission grant.

## First implementation steps

1. Read current roadmap and owning skills/provider/runtime/compatibility specifications, inspect exact checkout and request construction, and coordinate changes to shared builder/skill/request files with the identity/goals design owners via Global before editing overlapping work. Preserve others' files. Record exact implementation revision; do not assume earlier audited/deployed revisions still describe current main.
2. Trace the accepted-user-turn identity and model/tool/retry boundaries. Select the smallest existing lifecycle owner for the resolved metadata snapshot, including queued steering, retries and startup recovery. Avoid a new user configuration knob, watchers, catalog framework or unearned restart compatibility promise. State exactly what happens on restart and retain existing generation fencing.
3. Verify exact provider contract before implementing an appended catalog event. Reuse existing supported privileged message/history paths if sufficient. If blocked, ship only the correct explicit boundary refresh/cache-miss behavior for that route; record limitation. Do not broaden into effort/model transitions, native compaction or tool-schema redesign.
4. Define body freshness before consequential changes. Recommendation: retain current body reads at invocation, freeze only advertised metadata within turn, fail clearly on deleted/unavailable skill, and analyze name-shadowing/path changes so a metadata snapshot cannot silently select an unrelated skill. Live bodies can differ from earlier descriptions; this is the concrete tradeoff against new persistence/version machinery. If safe identity binding requires a new product decision, return the narrow choice rather than silently pinning whole bodies.
5. Update owning normative spec and executive to match the bounded guarantee and its limits; preserve design rationale appropriately. Do not rewrite old ADRs or claim provider-universal mandates.

## Starting evidence and code anchors

- `crates/phoenix-ide/src/runtime/executor.rs::dispatch_llm_request` currently rebuilds ordinary system text before every dispatch.
- `crates/phoenix-ide/src/system_prompt.rs::build_system_prompt_with_options` calls live `discover_skills_with_options` and renders names/descriptions/paths.
- `crates/phoenix-skills/src/lib.rs` owns discovery precedence and invocation.
- `crates/phoenix-tools/src/skill.rs::SkillTool::run` independently discovers current ordinary skill bodies.
- `message_expander::expand` slash invocation is accepted user expansion, not the same path as model tool invocation.
- Coordinator uses `AuthenticatedCoordinatorSkillCatalog`; preserve its audience, registration and live-only provenance.
- Existing task-file prompt stability test proves only task-hint stability, not skill metadata mutation behavior.
- Existing task36009 overlaps broader guidance/catalog snapshot and refresh; reconcile this turn-scoped work rather than duplicate or silently fulfill its cross-turn guarantees.

## Acceptance evidence

Focused deterministic tests with fake providers/filesystems as appropriate:

- Edit/add/remove ordinary skill metadata between two tool-loop dispatches: same turn retains exact advertised catalog and attribution.
- Next accepted turn observes the deliberate refresh; additions/removals represented with correct authority where implemented, otherwise correct prefix rebuild explicitly asserted. Subsequent requests retain effective update state.
- Ordinary skill invocation body edit, deletion and name-shadowing/path change follow documented current-versus-pinned policy; no unrelated replacement selected accidentally.
- Current denial/withdrawal still prevents execution immediately despite old advertised metadata. Do not fake a new permission system for ordinary skills; test the actual owning gates and retain existing authority regressions.
- Coordinator authentication/catalog tests remain valid; ordinary filesystem changes cannot alter its advertised authenticated catalog.
- Retry, queued turn/steering and restart behavior match explicitly chosen bounded contract; no snapshot from a different turn reused accidentally.

Do not run heavy local builds without host admission. Inspect available lightweight checks, run only admitted validation, obtain independent adversarial review, and use normal hosted CI for full checks. Report tests actually run versus only inspected.

## Deliverable

A narrowly reviewed implementation and matching spec/test changes, exact commit/diff scope, focused test evidence and hosted CI pointers, independent review findings, and explicit remaining provider limitations. No merge/deploy/release. If a provider-specific appended update is blocked, deliver the smaller correct turn-stability/boundary-refresh change rather than stall for a general provider framework.
