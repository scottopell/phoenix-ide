# ADR-021: The Coordinator surface is chat-only

- **Status:** Accepted
- **Date:** 2026-07-20
- **Affects:** REQ-GR-001, REQ-GR-004, REQ-GR-005, REQ-GR-010, REQ-GR-011

## Context

The Coordinator surface combines the normal durable conversation with a second browser-facing projection for current attention and open-work filtering. On compact screens, the duplicate page header, view selector, and separate utility substantially reduce the transcript viewport. On desktop, the utility still duplicates work discovery already available through conversation filtering and the Coordinator's deterministic context.

The deterministic open-work model has two agent consumers that remain valuable independently of that browser UI: a bounded capsule attached to every Coordinator turn and a paginated `list_open_work` tool for deeper inspection. The product decision is therefore about presentation and browser adapters, not removal of current-work orientation from the Coordinator agent.

## Options considered

1. **Retain the split surface and tighten mobile CSS** — preserves direct browser filtering, but keeps duplicated navigation and two competing modes while only partially recovering transcript space.
2. **Hide Work only on compact screens** — improves phones, but leaves desktop and mobile as different products and retains the browser endpoint, state, and fixtures for a utility that duplicates existing discovery paths.
3. **Make the surface chat-only while preserving agent context and tools** — removes duplicated UI at every width, maximizes transcript space, and keeps deterministic work awareness available through the conversational interface.
4. **Remove all open-work capabilities** — minimizes code further, but makes the Coordinator less informed and unable to inspect beyond a bounded snapshot.

## Decision

Phoenix chooses option 3. `/global` presents only the shared conversation runtime. A compact `Brief me` action lives in the normal composer action group and submits a read-only prompt through the ordinary durable message path. The separate browser Open Work endpoint, pane, filtering controls, and Conversation/Work selectors are removed.

The deterministic projection remains an internal application service used by `GlobalReadService::current_work_capsule` and the Coordinator-only `list_open_work` tool. Those typed consumers define why the projection exists; an unused browser adapter does not remain as an accidental third contract.

## Consequences

- **Positive:** The Coordinator transcript and composer use the full route at phone, tablet, and desktop widths.
- **Positive:** Coordinator messaging retains the standard streaming, continuation, persistence, cancellation, reconnect, and draft behavior without a parallel UI mode.
- **Positive:** Automatic turn-current orientation and paginated work inspection remain available to the Coordinator agent.
- **Negative:** Users can no longer browse or filter the open-work projection directly on `/global`; they use conversation filtering or ask the Coordinator instead.
- **Neutral:** Stable work references and app-local destinations remain part of agent tool results and Coordinator citations rather than browser Open Work rows.

## References

- `specs/global-recall/requirements.md`
- `specs/global-recall/executive.md`
- `CoordinatorPage`
- `InputArea`
- `GlobalReadService::current_work_capsule`
- `GlobalReadService::open_work_page`
