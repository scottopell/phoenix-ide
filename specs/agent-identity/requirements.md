# Agent Identity and Credentials

## User Story

As a developer using Phoenix IDE, I need agent commits to be clearly attributed to Phoenix and to not be blocked by my existing git signing configuration (e.g. 1Password SSH signing, GPG) so that the agent can commit freely to the task branch and I can see at a glance which commits came from the agent.

## Context

Work conversations run git commands via the bash tool in a dedicated worktree on a task branch. Two problems arise:

1. **Signing prompts block agent commits.** Users commonly have `commit.gpgsign = true` in their global `~/.gitconfig`, backed by 1Password SSH agent or GPG. These require user interaction that cannot be automated — agent commits hang indefinitely.

2. **No attribution without action.** Without any marker, `git log` on the task branch shows all commits as coming from the user's own identity, making it impossible to distinguish agent work from manual edits during the task.

Agent commits live on the task branch and reach main when the user merges the branch's pull request through their normal workflow (work-lifecycle REQ-WL-002); Phoenix never squash-merges or pushes on the user's behalf. The `Co-authored-by` trailer carries agent attribution into main's history, and whether those commits are signed is the user's repository policy, applied at merge time. Phoenix does not manage push authentication, remote credentials, or commit signing on behalf of the user.

---

## Requirements

### REQ-AID-001: Git Signing Bypass

WHEN the bash tool executes commands in a Work conversation, Work sub-agent, or Direct conversation
THE SYSTEM SHALL inject git configuration environment variables that disable commit signing and tag signing
SO THAT `git commit` operations complete without prompting for signing credentials

WHEN the bash tool executes commands in an Explore conversation or Explore sub-agent
THE SYSTEM SHALL NOT inject signing bypass variables

**Rationale:** Users should not need to reconfigure their global git signing setup to use Phoenix. The bypass exists so signing prompts (1Password SSH, GPG) cannot hang an automated agent commit — not because the commits are throwaway; they persist and reach main through the user's PR merge. Whether they are signed is the user's repository policy, applied at merge time, not Phoenix's concern. The bypass operates via process environment only; no config files are modified.

---

### REQ-AID-002: Co-Authored-By Trailer

WHEN an agent makes a git commit in a Work conversation, Work sub-agent, or Direct conversation
THE SYSTEM SHALL instruct the agent via system prompt to append a `Co-authored-by` trailer to every commit message
AND the trailer SHALL identify Phoenix IDE as the co-author

Trailer format:
```
Co-authored-by: phoenix-ide <phoenix-ide@noreply.local>
```

**Rationale:** `git log` on the task branch should make agent authorship visible at a glance, consistent with how Claude Code and other AI coding tools attribute their commits. The trailer is a git-standard mechanism that survives rebase and is rendered by GitHub, Forgejo, and other hosts. Instruction via system prompt is sufficient — no hook infrastructure needed.
