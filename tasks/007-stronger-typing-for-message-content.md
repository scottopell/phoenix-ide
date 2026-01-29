---
created: 2026-01-29
priority: p3
status: ready
---

# Stronger Typing for Message Content (Phase 4)

## Summary

Replace `serde_json::Value` with typed enums for message content throughout the system.

## Context

This was Phase 4 of the state machine fix plan, marked as optional/deferred. Currently, message content is stored as `Value` (JSON blob), which allows invalid data to be represented.

## Acceptance Criteria

- [ ] Define `MessageContent` enum with variants for each message type
- [ ] Update `Message` struct to use typed content
- [ ] Update database serialization/deserialization
- [ ] Update all code that constructs or reads message content
- [ ] All tests pass

## Notes

Proposed type:

```rust
enum MessageContent {
    User {
        text: String,
        images: Vec<ImageData>,
    },
    Agent {
        blocks: Vec<ContentBlock>,
    },
    Tool {
        tool_use_id: String,
        output: String,
        is_error: bool,
    },
}
```

This is lower priority because:
- Current loose typing works
- Main benefit is documentation/safety
- Significant refactoring effort

Consider doing this incrementally, starting with the most error-prone areas.
