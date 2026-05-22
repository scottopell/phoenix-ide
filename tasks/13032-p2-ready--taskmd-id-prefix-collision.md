Report a task-ID allocation defect to upstream `taskmd` and track the follow-up.

## Problem

`taskmd new` allocates IDs as `{prefix}{seq}`:

- `prefix` — 2 digits, derived by hashing the tasks-dir path (confirmed by
  probing `taskmd._core`'s `prefix_for`). The Rust extension also links the
  `gethostname` crate, so the hostname is very likely mixed into the hash too.
- `seq` — sequential: the next free value *within that prefix bucket*.

The prefix is meant to namespace allocations per checkout so concurrent
branches land in different buckets. That only works if the hash inputs
(hostname + path) actually differ between checkouts.

## Failure mode

When two checkouts share the same hostname and tasks-dir path they hash to the
same prefix bucket. `next_id` is sequential within a bucket and each branch
sees the same committed history, so two independent sessions branching from the
same commit mint the *same* ID. The collision is invisible on either branch
alone — it surfaces only when the branches are merged (or when CI validates a
PR merge commit).

Observed on PR #117: `main` and the feature branch both minted `13027`.
`taskmd.validate()` passed on each branch alone but failed on the PR merge
commit. Resolved by renumbering `13027` -> `13031`.

## Concerns unique to this environment

Claude Code on the web runs every session in a fresh, ephemeral container, and
both prefix-hash inputs are *constant* across all of them:

- Hostname is a generic constant — `vm`.
- The repo is always cloned to the same fixed path — `/home/user/phoenix-ide`
  (so the tasks dir is always `/home/user/phoenix-ide/tasks`).

The per-checkout entropy the prefix relies on therefore does not exist here:
every cloud session hashes into the *same* bucket, and within-bucket sequential
allocation then *guarantees* a collision whenever two cloud sessions branch
from the same commit. For this repo that is the normal workflow, not an edge
case — multiple concurrent agent PRs are expected.

## Suggested upstream fixes

- Mix real per-checkout/per-machine entropy into the prefix hash: `/etc/machine-id`,
  a random nonce persisted in the tasks dir (committed once), or the git branch name.
- Or allocate `seq` randomly within a large space instead of sequential
  next-free, so same-bucket checkouts rarely collide.
- Or check candidate IDs against the merge target / remote at `new` time.

## Workarounds until an upstream fix lands

- `./dev.py tasks fix` renumbers duplicate IDs.
- Rebase onto `main` before running `taskmd new`.
- `./dev.py check`'s `task validation` lane now prints the offending IDs and the
  resolved taskmd version (the error list was previously swallowed), so a future
  collision is at least diagnosable from CI logs.
