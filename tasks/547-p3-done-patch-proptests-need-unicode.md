---
created: 2026-02-15
priority: p3
status: done
---

# Patch Proptests Need Unicode Content Generation

## Summary

The patch tool proptests only generate ASCII content, missing the UTF-8 truncation bug that caused a panic in production.

## Context

A panic occurred in `patch/planner.rs:247` when truncating content at byte 2000:

```rust
let header = &content[..content.len().min(2000)];
```

This panics if byte 2000 falls inside a multi-byte UTF-8 character (e.g., box-drawing char is 3 bytes).

### Why Proptests Missed It

The arbitrary content generators are ASCII-only:

```rust
fn arb_content() -> impl Strategy<Value = String> {
    // Generate printable ASCII strings, avoiding edge cases with control chars
    "[a-zA-Z0-9 \n\t]{1,200}"
}

fn arb_unique_substring() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_]{3,30}"
}
```

No multi-byte characters are ever generated, so the truncation bug could never be triggered.

## Acceptance Criteria

- [ ] Add `arb_unicode_content()` strategy that includes multi-byte UTF-8 characters
- [ ] Use it in at least one proptest exercising content truncation paths
- [ ] Verify the fixed truncation code handles edge cases:
  - Content exactly at boundary
  - Content ending mid-character at boundary
  - Content shorter than truncation limit
  - Content with emoji, CJK, box drawing, etc.

## Implementation Notes

### Unicode Strategy

Use `proptest`'s built-in `any::<String>()` which generates arbitrary valid UTF-8, but filter for printable and truncate safely:

```rust
fn arb_unicode_content() -> impl Strategy<Value = String> {
    any::<String>()
        .prop_filter("printable unicode", |s| {
            s.chars().all(|c| !c.is_control() || c == '\n' || c == '\t')
        })
        .prop_map(|s| truncate_to_char_boundary(&s, 200))
}

fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}
```

### Test the Truncation Path

The `is_generated_file` function does the truncation. Add a proptest that:
1. Generates content with multi-byte chars near the 2000-byte mark
2. Calls `is_generated_file` (or the function that calls it)
3. Asserts no panic

```rust
#[test]
fn prop_is_generated_handles_unicode(content in arb_unicode_content_long()) {
    // Should not panic regardless of where multi-byte chars fall
    let _ = is_generated_file(Path::new("test.rs"), &content);
}
```

## Related

- Fix commit: `1a6b032` - fix(patch): handle UTF-8 char boundaries when truncating content
- Task 545: Documents the broader incident
