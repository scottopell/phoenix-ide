# Adopt a Durable Workflow for Address Feedback

## Goal

Model the user-triggered Address Feedback flow as a durable vertical workflow rather than an API call followed by an ordinary user-message post.

The workflow must capture a fresh, exact-PR-head actionable feedback snapshot; bind that snapshot and its handoff baseline to one idempotent request; generate the typed autofix context; enqueue or dispatch it exactly once according to conversation capability; survive restart; and finish only with a durable product outcome.

## Required design work

- Define stable identity from conversation, repository/PR identity, PR head, and user-request idempotency key.
- Separate GitHub observation evidence from the persisted actionable-feedback snapshot.
- Define freshness races when the PR head or review set changes during capture or dispatch.
- Define idle dispatch, busy queueing, cancellation, retry, and continuation ownership.
- Fence duplicate clicks and duplicate agent dispatch.
- Define completion outcomes: handed off, superseded by newer PR head/feedback, cancelled, failed, and agent run completed pending refreshed GitHub verification.
- Preserve GitHub as source of truth; Phoenix stores only the compact agent-actionable snapshot and handoff baseline.
- Add shadow/parity evidence before replacing the existing `pr-auto-fix-context` plus `UserMessage` orchestration.

## Scope boundary

This is a child migration of task 40011. It does not make PR monitoring itself authoritative for GitHub state, and it does not imply that every background refresh must become a workflow.

## Acceptance criteria

- [ ] Normative workflow profile, states, effects, receipts, and ambiguity policies exist.
- [ ] Exact-head snapshot and handoff baseline commit atomically before dispatch.
- [ ] Duplicate user action cannot launch duplicate agent work.
- [ ] Busy/idle/restart schedules produce one durable handoff outcome.
- [ ] New feedback or a new PR head supersedes stale work explicitly.
- [ ] Deterministic crash schedules and production shadow parity pass.
- [ ] Legacy endpoint/message choreography drains before retirement.
