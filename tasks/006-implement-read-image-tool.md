---
created: 2026-01-29
priority: p2
status: done
---

# Implement read_image Tool

## Summary

Implement the `read_image` tool that was defined in the typed input system but has no actual implementation.

## Context

In Phase 1, we added `ReadImageInput` to the `ToolInput` enum:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadImageInput {
    pub path: String,
}
```

However, there's no corresponding tool implementation in `src/tools/`. The LLM may try to use this tool based on the schema, which would fail.

## Acceptance Criteria

- [ ] Create `src/tools/read_image.rs` with `ReadImageTool` implementation
- [ ] Tool reads image file from path and returns base64-encoded content
- [ ] Tool validates file exists and is a supported image format (png, jpg, gif, webp)
- [ ] Tool returns appropriate error for invalid paths or unsupported formats
- [ ] Register tool in `ToolRegistry::new()`
- [ ] Add unit tests

## Notes

This tool enables the LLM to examine images in the workspace, useful for:
- Reviewing screenshots
- Analyzing diagrams
- Checking generated visualizations

Consider size limits to avoid sending huge images to the LLM.
