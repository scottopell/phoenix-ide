---
created: 2026-02-17
priority: p1
status: done
---

# Remove AI Gateway mode entirely

## Summary

Remove all AI Gateway code (ddtool auth, AI_GATEWAY_* config, OAuth UI).
The exe-dev gateway proxy (`ai-proxy.py`) now encapsulates AI Gateway details,
so phoenix-ide only needs two LLM modes: exe-dev gateway and direct API keys.

## Context

AI Gateway mode was a separate LLM backend that authenticated via ddtool JWT
tokens and spoke OpenAI-compatible format to `ai-gateway.us1.staging.dog`.
This is now handled externally by `ai-proxy.py`, which accepts exe-dev gateway
requests (native provider formats, `implicit` API keys) and translates them
for AI Gateway. Phoenix-ide no longer needs to know about ddtool, AI Gateway
auth, or OpenAI format translation.

After removal, the registry simplifies from 3 modes to 2:

```
Current:  ai_gateway_enabled → AIGatewayService | gateway → implicit keys | else → direct keys
After:    gateway → implicit keys | else → direct keys
```

## Scope

### Delete entirely (~1,170 lines)

- `src/llm/ai_gateway.rs` — AIGatewayService, ddtool token management, OpenAI translation (510 lines)
- `src/api/ai_gateway_auth.rs` — AuthState, DdtoolProcess, OAuth flow handlers (553 lines)
- `ui/src/hooks/useAIGatewayAuth.ts` — auth polling hook (55 lines)
- `ui/src/components/AuthBanner.tsx` — OAuth/device-code banner (67 lines)

### Modify — Rust

- `src/llm.rs` — remove `mod ai_gateway` and `pub use AIGatewayService`
- `src/llm/registry.rs` — remove `ai_gateway_enabled`, `ai_gateway_source`,
  `ai_gateway_org_id` from LlmConfig; remove `create_ai_gateway_service()`;
  remove the `if config.ai_gateway_enabled` branch in `try_create_model()`
- `src/api.rs` — remove `pub mod ai_gateway_auth`, remove `auth_state` field
  from AppState, remove `init_auth_state()` call
- `src/api/handlers.rs` — remove 3 `/api/ai-gateway/*` routes and their import
- `src/llm/proptests.rs` — remove all `ai_gateway::test_helpers` tests

### Modify — Python (dev.py)

- Delete functions: `get_ai_gateway_config()`, `check_ai_gateway_auth()`,
  `prompt_ai_gateway_auth()`
- Remove `--ai-gateway` CLI flag and `ai_gateway` parameter from
  `cmd_prod_deploy()`, `native_prod_deploy()`, `prod_daemon_deploy()`,
  `lima_prod_deploy()`
- Remove AI_GATEWAY_* env var blocks from `generate_systemd_service()`

### Modify — Frontend

- `ui/src/App.tsx` — remove `useAIGatewayAuth` hook usage and auth banner rendering
- `ui/src/api.ts` — remove `checkAIGatewayAuthStatus()`, `initiateAIGatewayAuth()`,
  `pollAIGatewayAuth()` methods
- `ui/src/index.css` — remove `.auth-prompt-banner`, `.auth-banner` and related styles

### Remove env vars

`AI_GATEWAY_ENABLED`, `AI_GATEWAY_URL`, `AI_GATEWAY_DATACENTER`,
`AI_GATEWAY_SERVICE`, `AI_GATEWAY_SOURCE`, `AI_GATEWAY_ORG_ID`

## Acceptance Criteria

- [ ] `cargo build` succeeds with no AI Gateway references
- [ ] `cargo test` passes (proptests updated)
- [ ] `./dev.py up` works with `LLM_GATEWAY=...` (exe-dev gateway mode)
- [ ] `./dev.py up` works with `ANTHROPIC_API_KEY=...` (direct mode)
- [ ] No `ai_gateway`, `ddtool`, `AI_GATEWAY` strings remain in src/ or ui/src/
- [ ] `--ai-gateway` flag removed from `./dev.py prod-deploy`
- [ ] Frontend has no auth banner or auth polling code

## Notes

- ~1,500 lines removed total across Rust, Python, TypeScript, CSS
- No Cargo.toml dependency changes needed (ddtool was called via subprocess)
- The exe-dev gateway contract is documented in `EXE_DEV_AI_GATEWAY_SPEC.md`
