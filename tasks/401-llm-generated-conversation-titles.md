---
created: 2025-02-07
priority: p2
status: ready
---

# LLM-Generated Conversation Titles

## Summary

Automatically generate meaningful conversation titles using a cheap/fast LLM call based on the initial user message, replacing the current random slug generation (e.g., "monday-morning-azure-phoenix").

## Context

Currently, new conversations get auto-generated slugs via `generate_slug()` in `src/api/handlers.rs` which produces friendly but meaningless names like "thursday-evening-crystal-wolf". These don't help users identify conversations later.

A cheap LLM call (e.g., `gpt-4o-mini`, `claude-3-haiku`, or a small model via Fireworks) can generate a short, descriptive title based on the user's first message, making conversations much easier to find and recognize.

## Implementation Approach

### 1. Add Title Generation Service

Create a lightweight title generation function that:
- Takes the initial user message text
- Calls a fast/cheap model with a simple prompt
- Returns a short title (max ~50 chars)
- Has a timeout/fallback to the random slug if LLM fails

Example prompt:
```
Generate a very short (3-6 words) title summarizing this request. Output only the title, no quotes or punctuation:

{user_message}
```

### 2. Model Selection Strategy

Options (in order of preference):
1. **Dedicated cheap model**: Configure a specific model for title generation in the registry
2. **Same provider, smaller model**: If using Claude, use Haiku; if using GPT-4, use gpt-4o-mini
3. **Fallback**: Keep current random slug if no cheap model available or on error

### 3. Integration Points

- **`src/api/handlers.rs`**: In `create_conversation`, after creating the conversation, fire off async title generation
- **`src/db.rs`**: Use existing `rename_conversation()` to update the slug once title is generated
- **New module**: `src/title_generator.rs` for the generation logic

### 4. Async/Background Generation

Title generation should NOT block conversation creation:
1. Create conversation with random slug immediately (current behavior)
2. Spawn background task to generate title
3. Update slug via `rename_conversation()` when complete
4. Emit SSE event to update UI

## Acceptance Criteria

- [ ] New conversations get LLM-generated titles based on initial message
- [ ] Title generation is async and doesn't block conversation creation
- [ ] Fallback to random slug on LLM error or timeout (e.g., 5 seconds)
- [ ] UI updates when title changes (via SSE or next poll)
- [ ] Works with any configured LLM provider
- [ ] Title length is reasonable (capped at ~50-60 chars)
- [ ] Cost is minimal (uses cheapest available model)

## Files to Modify

- `src/api/handlers.rs` - Trigger title generation after conversation creation
- `src/db.rs` - Possibly add a `set_title()` or reuse `rename_conversation()`  
- `src/llm/registry.rs` - May need method to get a "cheap" model
- New: `src/title_generator.rs` - Title generation logic
- `ui/` - Handle slug/title updates (may already work via existing refresh)

## Notes

- Consider caching/batching if users create many conversations quickly
- The title prompt should handle code snippets, error messages, and various input types gracefully
- May want to sanitize titles (remove special chars that break URLs if slug is used in URLs)
- Consider making this opt-out via config if users prefer random slugs
