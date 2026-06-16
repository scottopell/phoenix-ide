---
title: Managed lifecycle states
summary: Every state of the two stateful surfaces in the managed lifecycle — the task-approval reader and the Done? action bar.
category: reference
keywords: [approve, propose_task, done, work actions, mark as merged, abandon, pr, gh, continuation]
related: [howto/run-a-managed-task.md, concepts/modes.md, concepts/tasks.md, reference/glossary.md]
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
**and** the phase is `idle`, `error`, or `context_exhausted` — hidden while the
agent runs or awaits. (It stays up while errored / out of context so a stuck
conversation is still disposable.) Exactly **one** button glows as the *primary*
at a time. The bar has three zones.

**Review** — always shows **`View Diff`**.

**Resolve** — the "push it forward" zone, present only on an idle, open PR.
Phoenix has no merge API, so Merge/Open are honest GitHub link-outs:

| Verb (verbatim) | When | Does |
|-----------------|------|------|
| `Address feedback` (→ `Capturing...`) | an open PR Phoenix can post to | re-drives the agent against CI + review feedback; carries a freshness label and a `⚠` coverage marker |
| `Merge on GitHub #N ↗` | checks confirmed passing | opens the PR on GitHub — rides as a non-glowing secondary beside `Address feedback` when green |
| `Open PR #N ↗` | draft, not green, or stale status | opens the PR on GitHub to verify |

**Finish** — the terminal verbs, each with an `ⓘ` hover hint:

| Verb (verbatim) | `ⓘ` hint (Work mode, verbatim) |
|-----------------|-------------------------------|
| `Clean up` (→ `Cleaning...`) | *"Mark as merged. Deletes the worktree and the task branch Phoenix created. No confirmation — use Abandon if you want a diff snapshot first."* |
| `Abandon` (→ `Abandoning...`) | *"Captures a diff snapshot, then deletes the worktree and the task branch. Asks for confirmation."* |

In Branch mode both hints instead read *"…your branch is kept."* `Abandon` also
asks to confirm, differing by mode:

| Mode | Confirmation (verbatim) | Branch kept? |
|------|-------------------------|--------------|
| Work | *"Abandon this task? The worktree and task branch will be deleted."* | no |
| Branch | *"Abandon this conversation? The worktree will be deleted but your branch will be kept."* | yes |

### Which disposition, when

One row matches; first match wins. `Abandon` shows in every disposition **except
continued**; `Clean up` shows where marked.

| Situation | Primary glow | Resolve | Finish | Note (verbatim) |
|-----------|--------------|---------|--------|-----------------|
| Continued into another conversation | none | — | none | *"Continued — actions belong on the continuation."* |
| PR status loading | `Abandon` | — | Abandon | *"Checking PR…"* |
| Stuck¹, PR merged | `Clean up` | — | Clean up + Abandon | — |
| Stuck¹, PR closed | `Abandon` | — | Clean up + Abandon | *"PR #N is closed without merge. Use Abandon to clean up."* |
| Stuck¹, PR open/draft | `Abandon` | — | Clean up + Abandon | *"PR #N still open — merge on GitHub, or abandon."* |
| Stuck¹, gh unavailable | `Clean up` | — | Clean up + Abandon | *"gh unavailable — manual cleanup."* |
| idle, PR open/draft | `Address feedback` / `Open PR ↗` | yes | Abandon | — |
| idle, PR merged | `Clean up` | — | Clean up + Abandon | — |
| idle, PR closed | `Abandon` | — | Abandon | *"PR #N is closed without merge. Use Abandon to clean up."* |
| idle, gh unavailable | `Clean up` | — | Clean up + Abandon | *"gh unavailable — manual cleanup."* |
| idle, no PR found | `Clean up` | — | Clean up + Abandon | — |

¹*Stuck* = the `error` or `context_exhausted` phase; the Resolve zone is always
suppressed there, and a continued conversation's successor owns disposal.

## Related

- [Run a managed task](../howto/run-a-managed-task.md) — the happy-path procedure
- [Modes](../concepts/modes.md) — Explore, Work, Branch
- [Tasks](../concepts/tasks.md) — the plan, and how approval makes the branch
- [Glossary](glossary.md) — canonical terms
