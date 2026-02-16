---
created: 2026-02-07
priority: p3
status: ready
---

# Fix [object Object] rendering in review feedback notes

## Summary

Review feedback notes sometimes render `[object Object]` instead of actual content when displaying line references or requirement IDs.

## Context

Observed when reviewing `specs/bedrock/executive.md`. The feedback tool displayed:

```
> Line 7: `| ,[object Object], Conversation Mode | ❌ Not Started | ...`
```

The actual file content is correct markdown with proper emoji characters (✅, ❌). The bug is in the feedback/review tool's serialization - something is failing to stringify properly when constructing the review note display.

## Acceptance Criteria

- [ ] Identify where review notes are constructed/serialized
- [ ] Find the object that's not being stringified
- [ ] Fix serialization to render actual content
- [ ] Verify emoji characters (✅, ❌) display correctly in review notes

## Notes

- Likely a missing `.toString()` or JSON serialization issue
- May be related to how line content is extracted and interpolated into the note template
