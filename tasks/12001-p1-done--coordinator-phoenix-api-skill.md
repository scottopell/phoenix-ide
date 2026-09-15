# Add a Coordinator-only Phoenix API skill and correct generated prompts

Portfolio task **12001** reconciles historical artifact task **76005**. Commit `4cca18ffb79d1b88c09612aaf9201a5a5845f908` and approved-plan conversation `c4fa51ec` remain provenance for the previously approved Coordinator operator architecture. The user has now explicitly superseded that architecture for this delivery with a narrower design: a Coordinator-only built-in API reference invoked through the existing skill tool, plus user-authorized scoped Bash/curl against supported Phoenix HTTP APIs.

Implementation must start from exact `origin/main` commit `a56928576440ccfb39344d8e16964aaf4c6a2d8e`, not the newer local `main` currently visible in the planning worktree. No current-session bypass is possible or desired; behavior takes effect only after merge, deploy, and prompt reload. This commission does not merge or deploy.

## Observed journey

- The generated Global Coordinator prompt says it has no dedicated lifecycle mutation tools **and cannot create conversations**, although current Phoenix exposes authenticated HTTP APIs for ProductConversation creation and transcript-level lifecycle actions and grants the Coordinator explicitly WorkScope-scoped Bash.
- The Coordinator prompt deliberately omits the ordinary skill catalog and the Coordinator registry lacks `skill`, so a built-in API reference cannot currently be discovered or invoked there.
- The Coordinator continuation-summary system prompt independently repeats `cannot create conversations`, so correcting only AGENTS guidance or only the initial prompt leaves contradictory generated instructions.
- The desired journey is: user authorizes a lifecycle action → Coordinator discovers/invokes its built-in Phoenix API skill → resolves the current aggregate/transcript target → uses scoped Bash/curl with normal Phoenix authentication → checks the HTTP response → re-reads resulting state → reports acceptance separately from observed execution.

## Verified current-main findings

- `crates/phoenix-core/src/llm_language.rs::coordinator_prompt` contains the categorical creation prohibition; `COORDINATOR_CONTINUATION_SYSTEM_PROMPT` repeats it.
- `crates/phoenix-ide/src/system_prompt.rs::build_coordinator_system_prompt` builds a special prompt with no skill discovery/catalog. Its tests currently assert `available_skills` is absent.
- `crates/phoenix-tools/src/lib.rs::ToolRegistry::coordinator` accepts only supplied global tools plus `think`; the supplied current set is search/read/query/resolve/send/scoped Bash. `SkillTool` already exists for other top-level registries.
- Built-ins are embedded from `crates/phoenix-skills/src/builtin/`, extracted by `builtin::extract_to`, then discovered from disk. Current skill metadata has no audience field, so Coordinator-only exposure needs a small structural discovery/invocation filter rather than prompt convention alone.
- Current `origin/main` routes include ProductConversation inspection and creation (`GET /api/product-conversations`, `GET /api/product-conversations/:reference`, `GET .../:reference/route`, `POST /api/product-conversations/new`, creation recovery/cancel/retry-delivery routes), plus transcript chat/cancel/continue and state/message reads. Creation accepts a client UUID `request_id`; chat and continuation accept client `message_id`; response types distinguish creation identity, chat persisted/queued/steering acceptance, continuation accepted/dispatch-failed/already-exists, and cancel action/no-op.
- `/api/auth/status` is auth-exempt. Protected APIs preserve normal auth. Non-browser Bearer auth uses the configured password; the opaque `phoenix-auth` cookie is a session token, not the password. Documentation must teach discovery and use without printing, logging, or persisting credentials.
- ProductConversation reads expose `writable_transcript_row_id` and `latest_transcript_row_id`, allowing aggregate reference resolution before transcript-targeted mutations.
- Current gaps are real and must be stated honestly: there is no general operator endpoint, no uniform command/receipt envelope across lifecycle actions, no general conversation retry endpoint, cancel has no caller idempotency key, and several mutations target transcript rows rather than ProductConversation references. These gaps justify separate follow-up proposals when required; they do not expand this task.
- The historical artifact object is absent from the local clone and its public commit URL was unavailable during planning. Its identity and user-supplied provenance are preserved, but no unverified file content is claimed.

## Exact reduced file scope

### New built-in content

1. `crates/phoenix-skills/src/builtin/phoenix-api/SKILL.md`
   - Add a built-in skill marked `audience: global-coordinator`.
   - Direct the Coordinator to use only documented current endpoints and only for explicit user-authorized actions.
   - Require current target resolution, normal authorization, response verification, resulting-state verification, and acceptance-versus-observed-execution wording.
2. `crates/phoenix-skills/src/builtin/phoenix-api/references/api-reference.md`
   - Document supported current-main inspection, creation, chat/steer, continuation, cancel, creation-recovery, and verification routes with exact request/response fields needed for safe use.
   - Document auth-status discovery and non-disclosing credential handling.
   - Require reuse of the same request/message identity on retries only where the endpoint supplies that contract.
   - Name endpoint gaps without inventing aggregate targeting, idempotency, receipts, audit, or execution guarantees.

### Minimal discovery/invocation wiring

3. `crates/phoenix-skills/src/lib.rs`
   - Parse optional audience metadata into a type that prevents a Coordinator-only skill from entering an ordinary catalog.
   - Add audience-filtered discovery/invocation entry points while preserving existing behavior for audience-neutral skills.
4. `crates/phoenix-skills/src/builtin.rs`
   - Update focused inventory/extraction tests for the new skill and companion reference.
5. `crates/phoenix-tools/src/skill.rs`
   - Make the existing skill tool audience-bound so invocation enforces the same boundary as discovery; do not rely on the prompt alone.
6. `crates/phoenix-tools/src/lib.rs`
   - Add only the existing `skill` tool, configured for `global-coordinator`, to the Coordinator registry. Do not add a new tool or dependency.

### Generated prompts and actual request tests

7. `crates/phoenix-ide/src/system_prompt.rs`
   - Discover/render the Coordinator skill catalog only when an eligible built-in actually exists.
   - Add exactly: “No dedicated lifecycle tools are provided. Use documented Phoenix APIs through scoped Bash for user-authorized lifecycle actions; preserve normal authorization and verify results.”
   - Preserve separate scoped-Bash, untrusted-data, no-background-monitoring, and message-acceptance safety rules.
   - Test actual generated initial Coordinator prompt with and without discoverable built-ins and ensure no contradictory prohibition remains.
8. `crates/phoenix-core/src/llm_language.rs`
   - Remove categorical `cannot create conversations` wording from native and Caveman Coordinator guidance and from the Coordinator continuation-summary system prompt.
   - Do not broaden ambient authority or weaken independent safety restrictions.
9. `crates/phoenix-ide/src/runtime/executor.rs`
   - Extend the existing recorded-request continuation tests to assert the actual Coordinator continuation-summary request is contradiction-free.
   - Exercise the continued Coordinator’s normal generated prompt path (the successor retains `RuntimeRole::Coordinator`) rather than testing AGENTS content or a detached string alone.

### Normative/current-reality reconciliation

10. `specs/global-recall/requirements.md`
    - Preserve the absence of dedicated lifecycle tools while allowing explicitly user-authorized, normally authenticated HTTP API use through scoped Bash and an audience-bound skill.
11. `specs/global-recall/executive.md`
    - Reflect the narrowed shipped behavior and current endpoint gaps.
12. `specs/skills/requirements.md` and `specs/skills/skills.allium`
    - Specify audience-filtered catalog and invocation behavior.
13. `specs/builtin-skills/builtin-skills.allium` and `specs/builtin-skills/executive.md`
    - Specify/document the Coordinator-only built-in and its embedded companion reference.
14. This task file
    - Preserve the portfolio/artifact identity mapping and supersession provenance.

Run the `specs/AUTHORING.md` pre-flight before pushing any spec changes.

## Artifact reuse policy

Do not cherry-pick `4cca18ffb79d1b88c09612aaf9201a5a5845f908` wholesale. No artifact source bytes are approved for reuse until independently inspected and reconciled against current main. Useful concepts may be recovered: Coordinator-only discovery, structured API documentation, safe auth handling, current target/continuation resolution, endpoint-specific idempotency, and acceptance-versus-observation language.

The old operator service/tool/facade architecture was approved in its original context, not unilateral scope growth. It is now superseded by explicit user direction for this delivery. Preserve its commit/conversation provenance; do not resurrect or delete it wholesale.

## Acceptance evidence

- Built-in inventory and extraction tests cover `phoenix-api/SKILL.md` and its companion reference.
- Discovery and invocation tests prove `phoenix-api` is available to `global-coordinator` and unavailable to ordinary conversations.
- Coordinator registry tests prove the only added callable is the existing audience-bound `skill`; `phoenix_operator` is absent.
- Actual generated initial Coordinator prompt:
  - contains the exact replacement guidance;
  - references/inventories `phoenix-api` only when that built-in is discoverable;
  - retains separate security/scoping rules;
  - contains neither categorical `cannot create conversations` nor an API-through-Bash prohibition.
- Actual recorded Coordinator continuation-summary request and the continued Coordinator’s normal generated prompt are checked for the same contradiction removal; tests are not limited to AGENTS content.
- Skill-content tests check safe auth discovery without credential disclosure, current target resolution, endpoint-specific identity reuse, response plus resulting-state verification, and accepted-versus-observed execution.
- Lightweight textual/spec validation may run while devmbp’s sole heavy slot is occupied. Do not run Cargo, rustc, `./dev.py check`, Playwright, Vitest, or xcodebuild until that slot is released. After release, run focused Rust tests and the required repository/spec checks.

## Explicit non-goals and forbidden expansion

- No `phoenix_operator` tool, crate dependency, service, facade, or route.
- No receipt/audit tables, migrations, generalized lifecycle/admin framework, or invented guarantees.
- No blanket prohibition on Phoenix HTTP APIs through scoped Bash.
- No wholesale cherry-pick or wholesale deletion of the historical artifact.
- No auto-continue behavior.
- No new server endpoint in this task. A missing required endpoint becomes a separately justified small follow-up.
- No UI, iOS, or `phoenix-client.py` expansion.
- No `AGENTS.md` workaround.
- No changes to existing auth policy, WorkScope selection, Bash audit/bounds, untrusted-data handling, background-monitor prohibition, or cross-conversation message semantics.
- No current-session bypass, merge, or deploy. Loaded Coordinator prompts remain unchanged until normal rollout and prompt reload.
