# Agent Identity - Design Document

## Overview

This document describes the technical design for REQ-AID-001 and REQ-AID-002. Both are small, targeted changes to two existing subsystems: the bash tool (signing bypass) and the system prompt (co-authored-by instruction).

---

## REQ-AID-001: Git Signing Bypass

### Mechanism

Git supports programmatic config overrides via `GIT_CONFIG_COUNT` / `GIT_CONFIG_KEY_N` / `GIT_CONFIG_VALUE_N` environment variables (introduced in git 2.32, released 2021). These take precedence over all config files (system, global, local, worktree).

When the bash tool runs in a context with write access, inject into the process environment:

```
GIT_CONFIG_COUNT=2
GIT_CONFIG_KEY_0=commit.gpgsign
GIT_CONFIG_VALUE_0=false
GIT_CONFIG_KEY_1=tag.gpgsign
GIT_CONFIG_VALUE_1=false
```

### Contexts That Receive the Bypass

| Context | Bypass injected |
|---------|----------------|
| Work conversation (parent) | Yes |
| Work sub-agent | Yes |
| Direct conversation | Yes |
| Explore conversation | No |
| Explore sub-agent | No |

### Scope

- Applies only to the bash tool's child process environment
- Does not modify `~/.gitconfig`, `.git/config`, or any file on disk
- Does not affect push authentication — the user's own SSH keys / HTTPS credentials are used as normal
- Does not affect git operations run outside Phoenix

### Minimum Git Version

`GIT_CONFIG_COUNT` requires git ≥ 2.32 (June 2021). Versions older than this are not supported; the bash tool logs a warning if an older git is detected at startup.

---

## REQ-AID-002: Co-Authored-By Trailer

### Mechanism

The agent is instructed via system prompt to append the trailer to every commit message. No hook, wrapper, or post-processing is required — this is consistent with how Claude Code and other AI coding tools handle attribution.

### System Prompt Instruction

Included in the system prompt for Work conversations, Work sub-agents, and Direct conversations:

```
When making git commits, always append the following trailer to the commit message
(separated from the body by a blank line):

    Co-authored-by: phoenix-ide <phoenix-ide@noreply.local>

Example:

    fix: correct off-by-one in token counter

    Co-authored-by: phoenix-ide <phoenix-ide@noreply.local>
```

### Trailer Format

```
Co-authored-by: phoenix-ide <phoenix-ide@noreply.local>
```

- `Co-authored-by` is the git-standard trailer key recognised by GitHub, Forgejo, GitLab, and most other hosts
- `phoenix-ide@noreply.local` is a non-routable address that makes the source identifiable without implying a real email account
- Capitalisation follows the `Co-authored-by` convention used in this codebase (see existing commits)

### Contexts That Receive the Instruction

| Context | Trailer instruction included |
|---------|-----------------------------|
| Work conversation (parent) | Yes |
| Work sub-agent | Yes |
| Direct conversation | Yes |
| Explore conversation | No (no commits possible) |
| Explore sub-agent | No (no commits possible) |

### Compatibility

- Survives `git rebase -i` (trailers are preserved as commit message content)
- Visible in `git log`, `git show`, and all major git hosts
- Compatible with user re-signing — the squash commit on main (REQ-PROJ-009) is authored by the user and can be signed normally; the agent commits with their trailers are squashed away

---

## Dependencies

- REQ-BED-027 (ConvMode) — needed to determine which contexts receive bypass and instruction
- No new crates or system binaries required
