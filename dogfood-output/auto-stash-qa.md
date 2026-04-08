# Auto-Stash and Merge QA Report

Date: 2026-04-03
Tester: Claude (automated API testing)
Server: localhost:8033 (dev, worktree hash 79c939b7)

## Summary

Three bugs found, one critical. The "Stash and merge" button in the UI
can never succeed due to a control flow issue, and the overlap detection
in `check_auto_stash_safe` is broken for all cases.

## Test Setup

Created a temporary git repo at `/tmp/phoenix-stash-test` with:
- Main branch with `main-file.txt`
- Work branches with separate files (`branch-file.txt`, etc.)
- Git worktrees for each work branch
- Injected Work-mode conversations directly into the Phoenix SQLite DB

Tested against `POST /api/conversations/{id}/complete-task` and
`POST /api/conversations/{id}/confirm-complete`.

---

## Scenarios Tested

### Scenario 1: Clean main checkout -- PASS

```
POST /api/conversations/{id}/complete-task
HTTP 200
{"success":true,"commit_message":"feat: add branch file with initial content"}
```

Correctly proceeds to LLM commit message generation.

### Scenario 2: Dirty main, tracked file, no overlap -- PASS (with caveat)

Dirty file: `main-file.txt` (tracked, modified)
Branch changes: `branch-file.txt` (different file)

```
POST /api/conversations/{id}/complete-task
HTTP 409
{
  "error": "Main checkout has uncommitted changes.",
  "error_type": "dirty_main_checkout",
  "dirty_files": ["M main-file.txt"],
  "can_auto_stash": true
}
```

Returns `can_auto_stash: true` -- correct answer, but for the wrong
reason (see Bug 1).

### Scenario 3: Dirty main, tracked file, WITH overlap -- FAIL

Dirty file: `branch-file.txt` (staged)
Branch changes: `branch-file.txt` (same file -- overlap!)

```
POST /api/conversations/{id}/complete-task
HTTP 409
{
  "error": "Main checkout has uncommitted changes.",
  "error_type": "dirty_main_checkout",
  "dirty_files": ["A  branch-file.txt"],
  "can_auto_stash": true   <-- WRONG, should be false
}
```

**Expected:** `can_auto_stash: false` because `branch-file.txt` is
dirty in main AND modified by the merge branch.

### Scenario 4: Dirty main, untracked files only -- FAIL

Dirty file: `unrelated-untracked.txt` (untracked, no overlap)

```
POST /api/conversations/{id}/complete-task
HTTP 409
{
  "error": "Main checkout has uncommitted changes.",
  "error_type": "dirty_main_checkout",
  "dirty_files": ["?? unrelated-untracked.txt"]
}
```

`can_auto_stash` is absent (false). **Expected:** `true`, since the
untracked file doesn't overlap with the merge. The actual `stash push
--include-untracked` at confirm time would handle this fine.

### Scenario 5: confirm-complete with auto_stash=true, no overlap -- PASS

```
POST /api/conversations/{id}/confirm-complete
{"commit_message":"feat: add branch file","auto_stash":true}
HTTP 200
{"success":true,"commit_sha":"88c4fdb"}
```

Post-merge state verified:
- Merge commit created correctly
- Dirty file still present in working tree (stash popped)
- Stash list empty
- Worktree and branch cleaned up

### Scenario 6: confirm-complete with auto_stash=false, dirty main -- PASS

```
POST /api/conversations/{id}/confirm-complete
{"commit_message":"feat: should be rejected","auto_stash":false}
HTTP 409
{"error":"Main checkout has uncommitted changes...","error_type":"dirty_main_checkout"}
```

Correctly rejects.

### Scenario 7: confirm-complete with auto_stash=true AND overlapping files -- FAIL

```
POST /api/conversations/{id}/confirm-complete
{"commit_message":"feat: add overlap-file","auto_stash":true}
HTTP 200
{"success":true,"commit_sha":"a59fc13"}
```

Post-merge state: **conflict markers in working tree**.

```
<<<<<<< Updated upstream
branch4 overlap content
||||||| Stash base
=======
main dirty version
>>>>>>> Stashed changes
```

Stash was NOT dropped (still in `git stash list`). The API returned 200
as if everything succeeded. User's dirty changes are recoverable from
the stash but the working tree is left in a conflict state with no
warning.

### Scenario 8: UI "Stash and merge" button flow -- FAIL

Simulated the UI flow:
1. User clicks "Merge to main" -> `POST complete-task` -> 409 with `can_auto_stash: true`
2. User clicks "Stash and merge" -> `POST complete-task` again -> 409 again

The button calls `completeTask()` which always fails when main is dirty.
The stash only happens in `confirmComplete()`. The button can never
reach the commit modal.

---

## Bugs Found

### Bug 1 (Critical): `check_auto_stash_safe` never detects overlap

**File:** `src/api/handlers.rs:2633-2664`

The function runs `git diff --name-only main...HEAD` from the repo root
(main checkout). Since `HEAD == main` on the main checkout, this is
`main...main`, which always produces an empty diff. The merge files set
is always empty, so the intersection with stash files is always empty,
and the function always returns `true`.

**Fix:** Use the branch name instead of HEAD:
```rust
// Before (broken):
&["diff", "--name-only", &format!("{base_branch}...HEAD")]

// After:
&["diff", "--name-only", &format!("{base_branch}...{branch_name}")]
```

The function needs the branch name passed as an additional parameter.

### Bug 2 (Medium): `git stash create` doesn't capture untracked files

**File:** `src/api/handlers.rs:2635`

`git stash create` does not include untracked files, so when dirty state
is only untracked files, it returns empty string and the function
returns `false`. But the actual stash at confirm time uses `stash push
--include-untracked`, which would handle untracked files fine.

**Fix:** For untracked-only dirty states, `check_auto_stash_safe`
should return `true` since untracked files can never conflict with a
merge (the merge only touches tracked files). Alternatively, use `git
stash create --include-untracked` if your git version supports it (added
in Git 2.35).

### Bug 3 (Critical): "Stash and merge" button is a dead loop

**File:** `ui/src/components/WorkActions.tsx:114-131`

The "Stash and merge" button calls `api.completeTask()` (line 119),
which is the pre-check endpoint that always returns 409 when main is
dirty. The stash only happens during `confirmComplete()`. So the button
can never succeed.

**Fix options:**
1. Have the button skip `completeTask()` and go directly to the confirm
   modal using the commit message from the first `completeTask()` call
   (which was already returned before the error, but actually it wasn't
   -- the 409 is returned before the commit message is generated).
2. Add a query parameter or request body to `complete-task` that tells
   it to skip the dirty-main check (e.g., `?auto_stash=true`), so the
   endpoint generates the commit message even though main is dirty.
3. Have the button call `confirmComplete()` directly with a fallback
   commit message, but this skips the user's chance to edit the message.

Recommended: Option 2. Add `auto_stash` flag to the `complete-task`
request. When true, skip the dirty-main check (the confirm step will
handle the stash). This keeps the two-step flow (preview commit message
-> confirm) intact.

---

## Verdict

The auto-stash feature's backend merge mechanics work correctly (stash
push/pop during `confirm-complete` is solid). But the safety check
(`check_auto_stash_safe`) is broken and always returns `true` for
tracked files, and the UI flow for triggering auto-stash is a dead loop.
The feature is not functional end-to-end.
