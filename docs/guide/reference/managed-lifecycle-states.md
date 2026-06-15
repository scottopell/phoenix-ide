---
title: Managed lifecycle states
summary: Every state of the two stateful surfaces in the managed lifecycle — the task-approval reader and the Done? action bar.
category: reference
keywords: [approve, propose_task, done, work actions, mark as merged, abandon, pr, gh, continuation]
related: [howto/run-a-managed-task.md, concepts/modes.md, reference/glossary.md]
---

# Managed lifecycle states

> **At a glance:** the dense companion to [Run a managed task](../howto/run-a-managed-task.md).
> Two surfaces are stateful — the **task-approval reader** (the Explore → Work
> gate) and the **Done?** bar (Work/Branch completion). Every reachable state of
> each is below.

## Task-approval reader

Opens when the agent calls `propose_task` in Explore. It renders over the plan
and has **no dismiss control** — no Esc, back, or click-away on the plan itself.
Your only exits are the three toolbar buttons. (The per-line annotation dialog
*does* close on click-away.)

| Control | State | Label (verbatim) | Enabled |
|---------|-------|------------------|---------|
| Discard | always | `Discard` | yes — confirms *"Discard this task? The agent will be informed the task was rejected."* |
| Send Feedback | no annotations | `Send Feedback (0)` | **no** — title *"Add annotations to the plan before sending feedback"* |
| Send Feedback | ≥1 annotation | `Send Feedback (N)` | yes — title *"Send N notes as feedback"* |
| Approve | no unsent notes | `Approve` | yes |
| Approve | unsent notes pending | `Approve without sending feedback` | yes — **your notes are discarded, not sent** |
| Approve | submitting | `Approving...` | no |

**Annotate** a line to leave feedback: the dialog header reads `Line N`, the box
prompts `Add your note...`. With notes pending, a cue appears: *"You have N notes
of unsent feedback. Send feedback, or approve without sending those notes."*

## The Done? bar

Labelled `Done?`. Visible **only** when the mode badge is `Work` or `Branch`
**and** the phase is `idle`, `error`, or `context_exhausted` — it is hidden
while the agent is running or awaiting. (It stays up in `error`/`context_exhausted`
so a stuck conversation is still disposable.)

Always present: **`View Diff`**. **`View Browser`** (title *"Show the live
browser view"*) appears when a browser session is live.

### Address CI & comments

The `Address CI & comments` button (→ `Capturing...` while working) appears only
in the **`idle`** phase and only when the PR is **found and open**. It re-drives
the agent against the PR's CI and review feedback. Disabled when PR refresh is
unavailable or conversation input is unavailable; carries a `new`/`updated`
freshness marker and a `⚠` coverage marker when relevant.

### Completion button — six states

One button. Its label is computed from PR state and `gh` availability:

| Label (verbatim) | When | Clickable | Note shown |
|------------------|------|-----------|------------|
| `Checking PR…` | PR status loading | no | — |
| `Clean up merged PR` | PR found, merged | yes → cleans up local state | — |
| `PR closed without merge` | PR found, closed unmerged | no¹ | *"PR #N is closed without merge. Use Abandon to clean up local Phoenix state."* |
| `Waiting for PR merge` | PR found, open, blocks cleanup | no¹ | *"PR #N is {state}; cleanup unlocks after GitHub reports merged."* |
| `Use manual fallback` | `gh` unavailable, fallback off | yes → **first click only enables fallback** | — |
| `Mark as Merged` | manual-fallback path | yes → asserts merged, cleans up | *"gh unavailable — manual cleanup fallback enabled."* |

While acting the label is `Cleaning...`. ¹Blocked states become clickable once
the manual fallback is enabled. The `Use manual fallback` → `Mark as Merged`
path is two clicks: the first reveals the fallback, the second marks merged.

### Abandon

`Abandon` (→ `Abandoning...`). The confirmation differs by mode:

| Mode | Confirmation (verbatim) | Branch kept? |
|------|-------------------------|--------------|
| Work | *"Abandon this task? The worktree and task branch will be deleted."* | no |
| Branch | *"Abandon this conversation? The worktree will be deleted but your branch will be kept."* | yes |

### Continuation lock

If the conversation was continued into another, **both** the completion button
and `Abandon` are disabled, with the note *"Continued — actions belong on the
continuation."* (tooltip: *"This conversation has been continued. Abandon the
continuation instead."*). Act on the continuation instead.

## Related

- [Run a managed task](../howto/run-a-managed-task.md) — the happy-path procedure
- [Modes](../concepts/modes.md) — Explore, Work, Branch
- [Glossary](glossary.md) — canonical terms
