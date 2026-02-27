---
created: 2025-01-28
priority: p2
status: ready
---

# tasks validate: missing duplicate task number check

## Summary

`./dev.py tasks validate` passes silently when multiple task files share the same task number. Currently tasks 016, 017, and 018 each have two files.

## Reproduction

```
$ ls tasks/016* tasks/017* tasks/018*
tasks/016-p1-done-graceful-shutdown-recovery.md
tasks/016-p3-done-add-cancellation-timing-integration-test.md
tasks/017-p3-done-forbid-mod-rs-files.md
tasks/017-p3-ready-fix-clipboard-paste-proptest.md
tasks/018-p1-done-server-error-retry-fix.md
tasks/018-p3-done-api-route-naming-consistency.md

$ ./dev.py tasks validate
✓ 132 task files validated   # ← should have reported 3 duplicate-number errors
```

## Fix

In `cmd_tasks_validate`, after parsing all files, group by task number and report any number that appears more than once.

```python
from collections import defaultdict
number_map = defaultdict(list)
for task in parsed_tasks:
    number_map[task["number"]].append(task["path"].name)
for num, files in sorted(number_map.items()):
    if len(files) > 1:
        errors.append(f"duplicate task number {num}: {', '.join(files)}")
```

Also resolve the existing three duplicate pairs by renumbering the lower-priority/later duplicates.

## Acceptance Criteria

- [ ] `validate` reports an error for each duplicate task number
- [ ] Existing duplicate-numbered tasks (016, 017, 018) are renumbered to unique values
- [ ] `./dev.py check` passes cleanly
