---
created: 2025-01-28
priority: p0
status: ready
---

# Update recommended model list for claude-opus-4-6, claude-sonnet-4-6, gpt-5.2-codex

## Summary

Three new models have shipped and need to be added/promoted in `src/llm/models.rs`:

| Model | Status | Action |
|---|---|---|
| `claude-opus-4-6` | New release | Add entry, mark `recommended: true` |
| `claude-sonnet-4-6` | New release | Add entry, mark `recommended: true` |
| `gpt-5.2-codex` | Already in list | Flip `recommended` to `true` |

The 4-5 Anthropic variants (`claude-opus-4-5`, `claude-sonnet-4-5`) should be demoted to `recommended: false` (legacy) once the 4-6 entries are in place.

## Location

`src/llm/models.rs` — `all_models()` function. Verify the exact `api_name` strings against Anthropic and OpenAI API docs before committing.

## Acceptance Criteria

- [ ] `claude-opus-4-6` added with correct `api_name`, `recommended: true`
- [ ] `claude-sonnet-4-6` added with correct `api_name`, `recommended: true`
- [ ] `gpt-5.2-codex` flipped to `recommended: true`
- [ ] Superseded 4-5 Anthropic entries demoted to `recommended: false` with `(legacy)` in description
- [ ] `default_model()` updated to point at `claude-sonnet-4-6` (index or ID lookup)
- [ ] `./dev.py check` passes
