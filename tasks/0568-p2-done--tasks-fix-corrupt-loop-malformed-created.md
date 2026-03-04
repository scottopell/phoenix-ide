---
created: 2025-01-28
priority: p2
status: done
---

# tasks fix: corrupt-loop on malformed `created` field

## Summary

`./dev.py tasks fix` corrupts task files that have a malformed-but-present `created:` value (e.g. the template placeholder `YYYY-MM-DD`). It inserts a second `created:` line without removing the bad one. Because the frontmatter parser is last-wins for duplicate keys, the bad value still wins after the fix — so every subsequent `fix` run inserts yet another `created:` line.

## Reproduction

Give any task file `created: YYYY-MM-DD` in its frontmatter, then run `./dev.py tasks fix` twice:

```
# After first fix run:
---
created: 2024-01-15    # inserted by fix
created: YYYY-MM-DD    # old value, not removed
...

# Parser reads last-wins → sees "YYYY-MM-DD" → validate still fails
# Second fix run inserts a third created: line, and so on
```

## Root Causes

1. **`cmd_tasks_fix`** — when `created` is already in frontmatter but has an invalid format, the fix inserts a new `created:` line without deleting the old one.
2. **`parse_task_file`** — the frontmatter loop is last-wins for duplicate keys, so the newly inserted (correct) value is silently overwritten by the surviving bad value.

## Fix

In `cmd_tasks_fix`, when the `created` field exists but is malformed, replace the existing line instead of prepending a new one:

```python
if re.search(r'^created:.*$', content, re.MULTILINE):
    content = re.sub(r'^created:.*$', f'created: {created}', content, count=1, flags=re.MULTILINE)
else:
    content = content.replace('---\n', f'---\ncreated: {created}\n', 1)
```

Optionally, also make `parse_task_file` error on duplicate frontmatter keys.

## Acceptance Criteria

- [ ] Running `tasks fix` on a file with `created: YYYY-MM-DD` replaces the bad value rather than inserting a second `created:` line
- [ ] Running `tasks fix` a second time on the same file is idempotent (no further changes)
- [ ] `./dev.py check` passes cleanly
