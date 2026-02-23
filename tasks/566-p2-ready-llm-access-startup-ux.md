---
created: 2026-02-23
id: 566
priority: p2
status: ready
slug: llm-access-startup-ux
title: "Surface clear UX for all LLM access states at startup"
---

## Problem

Phoenix currently silently falls back to hardcoded models in all failure modes,
which means the UI can show a full model list while calls will fail at runtime.
The user gets no actionable signal until they try to send a message.

## Permutations to handle

| Gateway configured | Gateway reachable | Direct API key(s) | Discovery result | Current behaviour | Desired behaviour |
|--------------------|-------------------|-------------------|-----------------|-------------------|-------------------|
| No                 | —                 | Yes               | hardcoded       | ✓ works           | ✓ no change needed |
| No                 | —                 | No                | empty           | UI shows no models, silent | Show "no LLM configured" onboarding prompt |
| Yes                | Yes               | —                 | discovered      | ✓ works (after proxy fix) | ✓ no change needed |
| Yes                | Down              | No                | falls back to hardcoded w/ implicit key | UI shows models, first call fails | Show "gateway unreachable" warning banner |
| Yes                | Down              | Yes               | falls back to hardcoded w/ real key | ✓ works (direct keys used as fallback) | ✓ no change needed |
| Yes                | Up, bad format    | No                | defensive guard triggers, falls back w/ implicit key | UI shows models, first call fails | Same as "gateway down" case |

## Root cause of the bad cases

`try_create_model` always succeeds in gateway mode (uses `"implicit"` as the API
key unconditionally), so models appear registered even when the gateway is
unreachable. The registry has no way to probe actual reachability at startup.

## Proposed approach

1. **Detect at startup**: after `new_with_discovery`, if gateway is configured
   but discovery failed (returned empty or triggered the fallback guard), attempt
   a lightweight health probe (`GET {gateway}/health` or similar) to distinguish
   "gateway configured and healthy" from "gateway configured but unreachable".

2. **Expose reachability state** on `ModelRegistry` (e.g.
   `gateway_status: GatewayStatus { Healthy, Unreachable, NotConfigured }`).

3. **API response**: include gateway/LLM status in `GET /api/models` so the
   frontend can act on it.

4. **UI**: show an inline warning banner when the gateway is configured but
   unreachable, with a hint ("start your LLM gateway and refresh"). Show an
   onboarding prompt when no LLM is configured at all (no gateway, no API keys).

## Out of scope

- Retrying the gateway on every request (that's runtime concern, not startup UX)
- Changing model routing or fallback logic
