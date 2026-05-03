---
created: 2026-05-03
priority: p2
status: ready
artifact: src/llm/codex_credential.rs
---

Codex-backed OpenAI auth currently refreshes access tokens inline inside CredentialSource::get(), which violates the trait expectation that get() is an immediate lookup. Move Codex refresh toward the same recovery shape as CredentialHelper: return cached credentials immediately when valid, expose in-progress recovery through is_recovering(), and perform OAuth refresh in a background/single-flight path so request setup does not block on filesystem/network work.

This should be designed alongside planned first-class auth support inspired by the Pi coding agent. The goal is a shared auth lifecycle abstraction that can support Codex auth.json, future Phoenix-owned OAuth/device flows, and clear user-facing recovery states without baking provider-specific blocking behavior into LlmAuth.
