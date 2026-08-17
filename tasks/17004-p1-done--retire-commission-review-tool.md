# Retire the commission-review tool end to end

## User-visible outcome

Phoenix no longer offers or executes `commission_review`. Agents cannot spend review tokens, park a conversation for review approval, or rely on a second review workflow that can distract from ordinary PR review and leave conversations in harmful states. Existing conversations and historical transcripts remain readable, and no persisted pending approval becomes permanently stuck merely because the feature was removed.

## Product decision

The commission-review product capability is retired rather than repaired. Ordinary repository review continues through the normal GitHub/PR review process outside this Phoenix tool. Task 70002, which proposes making commission review more resilient, is superseded and must become `wont-do`.

## Scope

- Remove `commission_review` from every tool registry, provider schema, capability calculation, prompt/tool list, runtime execution path, and approval endpoint so no new request can be represented or executed.
- Remove the internal approved-tool variant and the CommissionReview-specific state-machine request, approval, rejection, stale-decision, cancellation, and execution flow.
- Remove the CommissionReview-specific UI approval surface, result viewer, viewer-slot variant/URL contract, fixtures, stories, notifications, route-focus behavior, API methods/types, CSS, generated carriers, and tests that exist only for this capability.
- Remove the tool implementation and commission-review-only dependencies/helpers/tests after a complete consumer search.
- Update or retire `specs/commission-review` as a retired capability; remove commission-review obligations from other normative specs and executives, including viewer-slot and iOS contracts. Record any compatibility or architecture decision required by the persisted-state migration in the proper ADR/spec authority rather than a code comment.
- Transition task 70002 to `wont-do` with this retirement as the reason.
- Audit prompts, task guidance, skills, tests, and documentation so Phoenix agents are not instructed to invoke the removed tool.

## Persisted-state and historical-data boundary

- Inventory every persisted representation before deleting types: `awaiting_commission_review_approval`, serialized tool inputs/results, state kinds/check constraints, SSE/API state projections, viewer URLs, and any review display payload.
- Existing pending approval rows must have one explicit, tested forward-migration or typed load/recovery outcome that cannot leave the conversation awaiting an endpoint/tool that no longer exists. The migration must preserve transcript/tool-call structural validity and must not fabricate a successful review.
- Preserve historical message content. Old `commission_review` tool-use/result blocks may fall back to the generic historical tool renderer or another explicit read-only compatibility sink, but must not remain executable or retain a specialized writable viewer authority.
- Do not add downgrade, rollback, mixed-version, or permanent legacy-link guarantees unless required by `specs/compatibility/requirements.md`; any such policy change requires an ADR.

## Non-goals

- Do not replace commission review with another LLM review tool, sub-agent persona, automatic reviewer, or new approval workflow.
- Do not alter GitHub PR review, `commission_review`-unrelated task approval, message review/annotation, prose review, or ordinary diff viewing.
- Do not broaden this into a general state-machine cleanup, viewer-slot redesign, or ProductConversation migration.
- Do not retain a hidden or feature-flagged execution path.

## Acceptance evidence

- Tool discovery and provider payloads never expose `commission_review` or an approved/internal alias in any conversation mode.
- Replayed stale tool calls receive a bounded unavailable/unknown-tool outcome and cannot enter an approval state or invoke an LLM review.
- Representative databases with a persisted pending review approval migrate/recover without a stuck state, duplicate provider dispatch, fabricated success, or malformed tool transcript.
- Historical commission-review tool blocks remain readable through the chosen non-executable compatibility sink.
- Approval API calls and old specialized viewer URLs fail or normalize according to the explicit compatibility contract; no writable lifecycle remains.
- Repository-wide search leaves no CommissionReview-specific production execution, state, API, UI, prompt, or tool registration surface, except any narrowly documented historical compatibility sink.
- Task 70002 is `wont-do`; commission-review specs and cross-spec references truthfully describe retirement or are removed according to project authoring rules.
- Focused Rust/UI/migration tests, codegen where required, Allium/spec validation, browser regression for ordinary conversation/task approval/diff/message viewers, and `./dev.py check --all` pass.
- The implementation is committed, pushed, opened as a PR, and receives fresh exact-head review before merge.
