# Remove dead LLM breadcrumb trail plumbing end-to-end

The old `BreadcrumbBar` / SharePage step-trail UI has been removed, but the LLM/tool/sub-agent breadcrumb trail still exists as dead protocol and reducer plumbing.

## Problem

The remaining breadcrumb trail machinery no longer has a product UI consumer. It still:

- builds `SseBreadcrumb` values on the backend;
- includes `breadcrumbs` in `init` SSE payloads and generated wire types;
- validates/transforms breadcrumbs on the frontend;
- stores `breadcrumbs` and `breadcrumbSequenceIds` in `ConversationAtom`;
- derives breadcrumb entries from phase transitions;
- updates breadcrumb result summaries from tool results;
- keeps tests and specs asserting the now-dead trail behavior.

This is not the live seed/parent conversation breadcrumb, project provenance breadcrumb, or QuestionPanel breadcrumb UI. Those are separate live concepts and must remain.

## Proposed removal

Remove the dead LLM/tool/sub-agent breadcrumb trail contract end-to-end:

- Delete backend `SseBreadcrumb` and remove `breadcrumbs` from `SseEvent::Init` / typed wire structs.
- Delete backend breadcrumb reconstruction helpers such as `extract_breadcrumbs` and `flush_subagents` if no remaining caller needs them.
- Regenerate TypeScript wire types.
- Remove `SseBreadcrumbSchema`, `SseBreadcrumb` exports/imports, and `breadcrumbs` from frontend init schemas.
- Remove frontend `Breadcrumb` type if it becomes unused.
- Remove `breadcrumbs` and `breadcrumbSequenceIds` from `ConversationAtom`, `InitPayload`, and related selectors.
- Delete breadcrumb derivation/update code (`breadcrumbFromPhase`, `applyBreadcrumb`, result-summary breadcrumb updates) unless a live non-trail consumer is found.
- Update or delete tests that only preserve the dead trail contract.
- Update `specs/conversation-ui` to remove stale claims that `BreadcrumbBar` / SharePage step trail exists.

## Guardrails

Do not remove unrelated live breadcrumb/provenance concepts:

- seeded conversation parent breadcrumb (`seed_parent_id`, `seed_parent_slug`, `.conversation-seed-breadcrumb`);
- project/fork provenance breadcrumb (`spawned_from_conversation_id` and related audit/provenance fields);
- QuestionPanel question breadcrumbs.

## Acceptance criteria

- No `breadcrumbs` field remains in the SSE init payload for the dead LLM/tool/sub-agent trail.
- No frontend atom state is dedicated to the dead breadcrumb trail.
- No code derives breadcrumb trail entries from `tool_executing`, `llm_requesting`, or `awaiting_sub_agents` phases.
- Generated TypeScript wire types and schemas match the Rust wire contract.
- Stale conversation-ui spec references to `BreadcrumbBar` / SharePage step trail are removed or rewritten.
- Existing live seed/provenance/question breadcrumb behavior remains intact.
- `./dev.py check` or appropriately scoped equivalent checks pass.
