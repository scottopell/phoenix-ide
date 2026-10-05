# ADR-075: GPT-6.1 Sol is the ordinary product default

- **Status:** Accepted
- **Date:** 2026-10-05
- **Affects:** REQ-LLM-003, REQ-CCR-011, REQ-AG-005/010/011

## Context

The user selected GPT-6.1 Sol Standard as Phoenix's ordinary default. The registry previously preferred other models, and unspecified managed creation selected a cheap auxiliary model independently of the advertised default. Named coding workers are user configuration, not built-in repository personas.

## Options considered

1. **Operational pin only** — can select the model on one host but leaves fresh installations and omitted managed creation using other defaults.
2. **Rewrite all selections** — would discard remembered user choices and specialized worker intent.
3. **Change product fallback and ordinary creation only** — applies the user mandate while preserving explicit selections and existing operational configuration precedence.

## Decision

Prefer registered exact `gpt-6.1-sol` for the product default, while preserving available explicit `DEFAULT_MODEL` overrides and existing provider fallback order when GPT-6.1 Sol is unavailable. Ordinary creation with no model selection uses that registry default independently of workspace provisioning mode.

Standard remains the existing default request speed. No new reasoning-effort override is introduced: omitted effort retains model-native semantics, and supported explicit effort is preserved. Anonymous workers continue exact parent inheritance; specialized persona/task selections and cheap auxiliary work keep their own contracts.

The ordinary configured `coding_worker` preference is to use `gpt-6.1-sol` through its existing Codex connection, preserving its explicit `high` effort. Applying host configuration and activation requires the actual executor's filesystem and operational authorization; a source PR does not mutate running conversations or installed configuration.

## Consequences

Fresh unselected ordinary conversations follow the advertised product default. Remembered valid browser choices, pending native attempts, active conversation identities, and specialized review routes do not change. Provider execution errors do not cause model or billing-route substitution. Deployment configuration may still explicitly override the source default, so operational readback remains necessary.
