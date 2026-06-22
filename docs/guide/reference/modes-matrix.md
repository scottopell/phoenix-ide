---
title: Modes matrix
summary: What each mode — Direct, Explore, Work, Branch — can and can't do, at a glance.
category: reference
keywords: [modes, matrix, capabilities, direct, explore, work, branch]
related: [concepts/modes.md, howto/run-a-managed-task.md, reference/glossary.md]
---

# Modes matrix

> **At a glance:** the capability grid for the four [modes](../concepts/modes.md).
> The dividing line is **write access**: only Explore is read-only.

| Capability | Direct | Explore | Work | Branch |
|------------|--------|---------|------|--------|
| **Worktree** | none — your checkout | temporary, read-only | the task branch | your chosen branch |
| **Write** (patch/bash) | ✓ | ✗ — `patch` only in `tasks/` | ✓ | ✓ |
| **Read & search** | ✓ | ✓ | ✓ | ✓ |
| **bash** | ✓ | sandboxed read-only | ✓ | ✓ |
| **Terminal / tmux / browser** | ✓ | ✓ | ✓ | ✓ |
| **propose_task** | only in a git repo (fork) | ✓ — the Explore→Work gate | ✓ (fork) | ✓ (fork) |
| **Push / open a PR** | ✓ | ✗ | ✓ | ✓ |

## Which Workflow card creates which mode

| Workflow card | Mode |
|---------------|------|
| **Chat in a fresh worktree** | Explore (managed) → Work on approval |
| **Start from a task** | Explore (managed), seeded to propose that task |
| **Chat in a specific branch** | Branch |
| **Work in this folder** | Direct |

## Related

- [Modes](../concepts/modes.md) — the concept, with the autonomy spectrum
- [Run a managed task](../howto/run-a-managed-task.md) — Explore → Work in practice
- [Glossary](glossary.md) — canonical terms
