---
created: 2026-02-07
priority: p3
status: done
artifact: completed
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

- [x] Identify where review notes are constructed/serialized
- [x] Find the object that's not being stringified
- [x] Fix serialization to render actual content
- [x] Verify emoji characters (✅, ❌) display correctly in review notes

## Notes

- Likely a missing `.toString()` or JSON serialization issue
- May be related to how line content is extracted and interpolated into the note template

## Resolution

The bug was in `ui/src/components/ProseReader.tsx` in the `annotatable` factory function inside `renderMarkdown`. Line 454 used `String(children).slice(0, 200)` to populate the `lineContent` field. When `children` is a React element (e.g., for table cells `<td>` containing inline markdown like emoji, bold text, or other inline elements), `String()` on an object returns `[object Object]`.

Fix: replaced `String(children)` with raw source extraction using the HAST `node.position` data that `react-markdown` already provides. The raw file content is pre-split into lines (`rawLines`), and the annotatable factory slices the relevant source lines using `node.position.start.line` and `node.position.end.line`. This gives the actual markdown source text for the annotated block, which correctly includes emoji characters and all other raw content.
