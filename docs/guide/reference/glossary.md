---
title: Glossary
summary: The canonical term registry for the guide — the one spelling and meaning every page must use.
category: reference
keywords: [glossary, terms, vocabulary, definitions]
related: [concepts/modes.md, howto/run-a-managed-task.md, reference/managed-lifecycle-states.md]
---

# Glossary

This is the **canonical term registry**. When a term appears here, every page
uses *this* spelling, casing, and meaning — no synonyms, no re-coining. The
`phoenix-guide-sync` skill greps for known variants and flags drift. Add a term
here the first time the guide needs it; link the definition to the concept that
owns it once that page exists.

| Term | Means | Owner |
|------|-------|-------|
| **conversation** | One thread with the agent, tied to a working directory and a mode. | Conversations |
| **mode** | A conversation's freedom level: one of Direct, Explore, Work, Branch. Lowercase except the four proper names. | [Modes](../concepts/modes.md) |
| **Direct / Explore / Work / Branch** | The four modes, capitalized. Match the badge text exactly. | [Modes](../concepts/modes.md) |
| **managed** | The Explore → Work *lifecycle*, not a mode. Lowercase. There is no "Managed" control in the UI. | [Modes](../concepts/modes.md) |
| **Workflow card** | A choice on the new-conversation screen (e.g. *"Chat in a fresh worktree"*) that selects a mode. Never "mode picker". | [Run a managed task](../howto/run-a-managed-task.md) |
| **worktree** | An isolated git checkout keyed to the conversation. One word, lowercase. | Workspace |
| **task branch** | The branch a managed task runs on, created on approval. | Tasks |
| **task file** | The `tasks/NNNNN-…md` file that holds the plan; a living contract. | Tasks |
| **Done? bar** | The Work/Branch action bar (label `Done?`) with View Diff, completion, and Abandon. | [Managed lifecycle states](managed-lifecycle-states.md) |
| **continuation** | A successor conversation a thread was continued into; locks the original's terminal actions. | [Managed lifecycle states](managed-lifecycle-states.md) |
| **sub-agent** | A spawned child conversation running a delegated task. | Sub-agents |
| **skill** | A reusable instruction set invoked as `/name`. Lowercase. | Skills |
| **handle** | A backgrounded `bash` command you can peek/wait/kill. | [bash](tools/bash.md) |
| **WorkScope** | The owner of a conversation's resources (shells, browser, tmux). One word, this casing. | Workspace |

Unlinked owners are concept pages not yet written; link them here when they land.

## Related

- [Modes](../concepts/modes.md)
- [Run a managed task](../howto/run-a-managed-task.md)
- [Managed lifecycle states](managed-lifecycle-states.md)
