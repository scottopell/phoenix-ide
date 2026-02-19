---
created: 2026-02-19
priority: p1
status: done
---

# Silent model fallback: wrong model used without any indication

## Summary

When a user requested a model that wasn't available in the registry (e.g.,
`claude-4.5-sonnet` when only OpenAI models were registered), the system
silently used the default model instead. The conversation metadata showed the
requested model, but all LLM calls went to a completely different model. No
error, no warning, no log line — total silent data integrity violation.

## How it was discovered

During testing of the new `--model` flag on `phoenix-client.py`, a conversation
was created with `-m claude-4.5-sonnet`. The conversation appeared to work, and
the conversation record stored `model: "claude-4.5-sonnet"`. But the Phoenix
server log revealed every LLM request actually went to `gpt-5.1-codex` (the
default). The Anthropic models weren't registered because the LLM gateway
wasn't detected at startup time (race condition with ai-proxy), so the registry
only had OpenAI models via a direct `OPENAI_API_KEY` in the environment.

## Root cause: three silent fallback points

The model was resolved at three separate points, each with its own silent
fallback. A requested model could be swapped out at any stage without
indication.

### 1. Runtime startup (`src/runtime.rs` — `get_or_create`)

```rust
let model_id = conv.model.as_deref()
    .unwrap_or(self.llm_registry.default_model_id());
```

When `conv.model` was `None`, this silently fell back to the default. This
fallback is **correct** — it handles "no preference, use default." But the
model was resolved a second time independently for the LLM client (see #2).

### 2. Duplicate resolution (`src/runtime.rs` — `get_or_create`)

```rust
let llm_client = RegistryLlmClient::new(
    self.llm_registry.clone(),
    conv.model.clone()
        .unwrap_or_else(|| self.llm_registry.default_model_id().to_string()),
);
```

The model was resolved **again**, independently from #1. Same logic, but the
duplication meant the model identity existed in two places that could
theoretically diverge. Redundant and fragile.

### 3. Request-time fallback (`src/runtime/traits.rs` — `RegistryLlmClient::complete`)

```rust
let llm = self.registry.get(&self.model_id)
    .or_else(|| self.registry.default())  // <-- THE BIG ONE
    .ok_or_else(|| LlmError::network("No LLM available"))?;
```

This was the most dangerous fallback. Even if a model ID made it through
creation and into the state machine, the actual LLM call would silently swap
to the default if the requested model wasn't in the registry. This is what
caused the observed behavior: conversation metadata said `claude-4.5-sonnet`,
actual API calls hit `gpt-5.1-codex`.

## Fix

### Validate at creation (API layer)

`src/api/handlers.rs` — `create_conversation` now checks the requested model
against the registry before creating the conversation. Returns 400 with the
list of available models if the requested model doesn't exist.

### Error instead of silent swap (LLM client)

`src/runtime/traits.rs` — `RegistryLlmClient::complete` now returns an error
if the model isn't found, instead of silently falling back to the default.

### Single point of resolution (runtime)

`src/runtime.rs` — Model is resolved once into a single `model_id` variable
used by both `ConvContext` and `RegistryLlmClient`, eliminating the duplicate
resolution that could diverge.

## Lesson

Never silently substitute a core parameter. If a user asks for model X and
model X isn't available, that's an error — not an invitation to use model Y.
The "helpful" fallback masked a real configuration problem (missing gateway)
and produced responses from the wrong model with no way to detect it.
