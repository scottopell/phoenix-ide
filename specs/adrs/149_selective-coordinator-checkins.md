# ADR-149: Selective intervention during coordinator check-ins

- **Status:** Accepted
- **Date:** 2026-10-26
- **Affects:** REQ-GR-011

## Context

ADR-145 established active coordination within commissioned scope. The user described returning every two or three hours without reading the full feed, wanting to discover decisions blocking work while keeping delivery moving. They identified the primary risk as mistaking “drive” for “intervene everywhere”: redundant nudges, replanning and activity without delivery.

## Decision

Refine the existing system prompts in both language variants and the check-in action together. Inspect commissioned work broadly, but intervene only where useful; leave healthy progressing work alone and do not infer a stall from idle state alone. Continue routine authorized follow-through through existing owners. Repeated check-ins do not expand authority or reset decisions, corrections, pauses or gates.

Recover unanswered user decisions from history and verify they remain unresolved and material. Advance independent authorized work before asking. For check-in briefings, put still-needed input first with context and a recommendation, then explain material changes as problem, intervention and direction. Use the existing question control for genuine choices, without imposing a status-report template or announcements for routine watch events.

## Options considered

- Keep active coordination wording alone: leaves the incentive to generate visible activity ambiguous.
- Make check-ins observational only: fails the user's goal of moving authorized work forward.
- Add a cooldown or automatic repeated check: addresses scheduling rather than intervention judgment and is outside this prompt refinement.

## Consequences

This is a prompt policy, not a scheduler, delivery guarantee or new authority. Existing runtime, tools and ownership remain unchanged. Prompt and UI regressions can verify the instructions reach the model; they cannot prove live judgment. Archived evaluation artifacts remain frozen and no additional paid evaluation is part of this change.
