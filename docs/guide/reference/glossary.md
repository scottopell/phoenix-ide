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
| **conversation** | One thread with the agent, tied to a working directory and a mode. | [Conversations](../concepts/conversations.md) |
| **mode** | A conversation's freedom level: one of Direct, Explore, Work, Branch. Lowercase except the four proper names. | [Modes](../concepts/modes.md) |
| **Direct / Explore / Work / Branch** | The four modes, capitalized. Match the badge text exactly. | [Modes](../concepts/modes.md) |
| **managed** | The Explore → Work *lifecycle*, not a mode. Lowercase. There is no "Managed" control in the UI. | [Modes](../concepts/modes.md) |
| **Workflow card** | A choice on the new-conversation screen (e.g. *"Chat in a fresh worktree"*) that selects a mode. Never "mode picker". | [Run a managed task](../howto/run-a-managed-task.md) |
| **worktree** | An isolated git checkout keyed to the conversation. One word, lowercase. | [Workspace](../concepts/workspace.md) |
| **task branch** | The branch a managed task runs on, renamed from `task-pending-{id}` on approval. Exact name depends on the plan form — see [Tasks](../concepts/tasks.md#from-plan-to-branch). | [Tasks](../concepts/tasks.md) |
| **task file** | The `tasks/NNNNN-pX-status--slug.md` file that holds the plan; a living contract. | [Tasks](../concepts/tasks.md) |
| **Done? bar** | The Work/Branch action bar (label `Done?`) with View Diff, completion, and Abandon. | [Managed lifecycle states](managed-lifecycle-states.md) |
| **continuation** | Moving work to a successor conversation. Continuations link conversations into a chain, and a continuation also locks the original's terminal actions. | [Chains](../concepts/chains.md) |
| **chain** | A run of conversations linked by continuation, named and queryable as one unit. | [Chains](../concepts/chains.md) |
| **chain Q&A** | Recall questions answered by a read-only agent scoped to one chain. | [Chains](../concepts/chains.md) |
| **sub-agent** | A spawned child conversation running a delegated task. | [Sub-agents](../concepts/sub-agents.md) |
| **skill** | A reusable instruction set invoked as `/name`. Lowercase. | [Skills](../concepts/skills.md) |
| **permissions** | The deny layer that gates consequential tool calls before they run. | [Permissions](../concepts/permissions.md) |
| **handle** | A backgrounded `bash` command you can peek/wait/kill. | [bash](tools/bash.md) |
| **WorkScope** | The owner of a conversation's resources (shells, browser, tmux). One word, this casing. | [Workspace](../concepts/workspace.md) |

Unlinked owners are concept pages not yet written; link them here when they land.

## Related

- [Modes](../concepts/modes.md)
- [Run a managed task](../howto/run-a-managed-task.md)
- [Managed lifecycle states](managed-lifecycle-states.md)
