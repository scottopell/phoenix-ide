---
created: 2026-04-12
priority: p2
status: in-progress
artifact: pending
---

# rewrite-agent-identity-specs

## Plan

# Rewrite Agent Identity Specs — Explore/Work Model

## Summary

The three `specs/agent-identity/` files (`requirements.md`, `design.md`, `executive.md`) were written against the deprecated Restricted/Unrestricted mode model (REQ-BED-014–016). Bring them fully up to date with the Explore/Work model, sub-agent advances, and all decisions made in review.

---

## Decisions Made (do not re-open)

| Question | Decision |
|----------|----------|
| Sub-agent commit identity | Work sub-agents get `GIT_AUTHOR_*` / `GIT_COMMITTER_*` injected (same as parent Work conv), but NO `SSH_AUTH_SOCK` / `GIT_SSH_COMMAND`. Commit works; push fails with auth error. |
| Agent transparency | System prompt baseline for every conversation explains credential status and capabilities. Synthetic messages injected on key events: credential minted (task approval), credential revoked (Terminal). Sub-agents get a matching system prompt block explaining their own level. |
| Credential expiry | Drop the 30-minute TTL and all application-level expiry timers/events/notifications. Cert TTL = configurable conversation inactivity timeout (default **7 days**). This is a dead-man's switch against orphaned ssh-agents after server crash — not a rotation mechanism. No REQ-AID-006 style re-mint flow. |
| Standalone mode | Credentials ARE available in Standalone mode. Cert principal uses `conversation_id` as scope (no task ID). Audit log records it as a standalone session. |
| Sub-agent push | Work sub-agents can commit (using parent identity env vars), cannot push (no SSH auth). Explore sub-agents get nothing. |

---

## Context

**Current state of dependencies:**
- REQ-BED-027/028/029 (Explore/Work/Terminal modes) — ❌ Not Started but fully spec'd; agent-identity rewrite should target these
- REQ-PROJ-003/004 (propose_plan, task approval) — ✅ Complete
- REQ-PROJ-009/010 (complete/abandon) — ✅ Complete
- REQ-PROJ-008 (Work sub-agent mode params) — 🔄 Partial

**What changes in the model:**
- "Restricted mode" → "Explore mode" (read-only, no worktrees)
- "Unrestricted mode" → "Work mode" (task-scoped git worktree, full write)
- "Mode upgrade approval" → "Task approval via `propose_plan`" (REQ-BED-028)
- "Mode downgrade" → eliminated; replaced by task completion (squash merge) or abandonment → Terminal state (REQ-BED-029)
- Standalone mode is a new third mode (non-git dir, full write, no worktrees, no task system)

---

## What to Change

### `requirements.md`

**Context section:** Replace Restricted/Unrestricted framing with Explore/Work. Add Standalone as a third mode. Describe the two-level credential structure (SSH auth for push = parent Work only; git identity for commits = Work + Work sub-agents; Standalone = SSH auth scoped to conversation ID).

**REQ-AID-001:** Terminology update only.

**REQ-AID-002:** 
- "Mode upgrade to Unrestricted" → "Task approval creating a Work conversation" (REQ-BED-028)  
- Cert TTL default: change from 30 minutes to 7 days (conversation inactivity timeout)  
- Rationale: TTL is a dead-man's switch, not a rotation mechanism

**REQ-AID-003:** 
- "Unrestricted mode with active credentials" → "Work conversation with active credentials OR Standalone conversation"
- Add: Standalone conversations get SSH auth scoped to conversation ID

**REQ-AID-004:** Terminology update. Clarify that git identity env vars are injected for Work conversations AND Work sub-agents (not Explore sub-agents).

**REQ-AID-005 (Credential Revocation — rewrite):**
- Old: triggered by mode downgrade
- New: triggered by conversation reaching Terminal state (complete or abandon — REQ-BED-029), OR by explicit user action (optional UI affordance)
- Remove all references to downgrade

**REQ-AID-006 (DROPPED):** Remove the credential expiry notification requirement. Replace with a note that cert TTL is handled in REQ-AID-002 and REQ-AID-009 as a dead-man's switch — no application-level timer or notification needed.

**REQ-AID-007 (Audit Trail):** Update `approved_by` → `task_id` as the primary trace anchor (for Work convs); for Standalone, use `conversation_id`. Update reason enum to remove `Downgrade`, add `TaskComplete` and `TaskAbandoned`.

**REQ-AID-008 (Sub-Agent Credential Isolation — rewrite):**
- Explore sub-agents: no credential injection at all
- Work sub-agents: receive `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`, `GIT_COMMITTER_NAME`, `GIT_COMMITTER_EMAIL` (same as parent Work conv); do NOT receive `SSH_AUTH_SOCK` or `GIT_SSH_COMMAND`
- Both: informed of their credential capability level via system prompt
- `git commit` works for Work sub-agents; `git push` fails with SSH auth error (expected, documented)

**REQ-AID-009 (Configurable Parameters):** Update cert validity default to 7d (604800s). Add `PHOENIX_CERT_VALIDITY_SECS` description that it represents conversation inactivity timeout used as dead-man's switch TTL.

**REQ-AID-010:** No change (Forgejo setup instructions).

**REQ-AID-011:** No change (graceful degradation).

**REQ-AID-012:** No change (secure storage).

**REQ-AID-013 (Mode Transition Effects — rewrite):**
- Remove dependency on REQ-BED-014–018 (deprecated)
- Remap to REQ-BED-027/028/029:
  - `MintCredential`: triggered by task approval (REQ-BED-028) for Work convs; triggered at Standalone conversation creation for Standalone convs
  - `RevokeCredential`: triggered by Terminal state (REQ-BED-029) for both paths
- Remove `CredentialExpired` event (dropped with REQ-AID-006)

**Add REQ-AID-014: Standalone Conversation Credentials**
- WHEN a Standalone conversation is created, THE SYSTEM SHALL mint a credential scoped to the `conversation_id` (no task ID)
- Principal format: `phoenix-conv-{conversation_id_prefix}`
- Cert TTL = same as Work conversation (configurable, default 7d)
- Audit log records it as a standalone session

**Add REQ-AID-015: Credential Transparency via System Prompt and Synthetic Messages**
- WHEN any conversation starts, THE SYSTEM SHALL include a credential status block in the system prompt describing: current credential level, what operations are available, and any limitations
  - Work conv: "You have SSH credentials for push/commit on this worktree. Sub-agents you spawn in Work mode can commit (using your identity) but cannot push."
  - Explore conv: "You are in Explore mode. No credentials are available. Use `propose_plan` to propose work that requires write access."
  - Work sub-agent: "You are a Work sub-agent. You can commit to the parent's worktree (using the parent's git identity) but cannot push."
  - Explore sub-agent: "You are an Explore sub-agent. No write operations or credentials are available."
  - Standalone: "You have SSH credentials for git operations in this directory."
- WHEN credentials are minted (task approved / Standalone created), THE SYSTEM SHALL inject a synthetic system message confirming credential availability
- WHEN credentials are revoked (Terminal state reached), THE SYSTEM SHALL inject a synthetic system message noting revocation

---

### `design.md`

- Update all mode terminology throughout
- **Certificate TTL section:** Change default from 1800s to 604800s (7d). Remove min/max range. Remove all `CredentialExpired` event handling. Update rationale to state TTL is a dead-man's switch tied to conversation inactivity, not a rotation mechanism.
- **Sub-agent section (REQ-AID-008):** Split credential injection into two tiers: (a) git identity env vars (Work sub-agents get these), (b) SSH auth env vars (parent Work conv only). Remove "System does not provide additional context" — replace with "Sub-agent system prompt explicitly states that push is not available."
- **State machine effects (REQ-AID-013):** Remove `CredentialExpired` from the `Event` enum. Update `MintCredential` trigger to task approval (REQ-BED-028) and Standalone creation. Update `RevokeCredential` trigger to Terminal state (REQ-BED-029). Remove expiry timer logic from the revocation process steps.
- **Standalone section (REQ-AID-014):** Add new section describing Standalone credential minting at conversation creation, principal format, and audit log treatment.
- **Agent transparency section (REQ-AID-015):** Add new section describing system prompt injection and synthetic message events.
- **Dependencies:** Replace "REQ-BED-014 through REQ-BED-018" with "REQ-BED-027, REQ-BED-028, REQ-BED-029". Add REQ-PROJ-008 as a related dependency (partial).
- **SSH socket path:** Update from `/tmp/phoenix-ssh-{id}/agent.sock` to `~/.phoenix-ide/run/phoenix-ssh-{id}/agent.sock` (aligns with the existing `~/.phoenix-ide/` data directory pattern).

---

### `executive.md`

- Update requirements table: renumber (REQ-AID-006 dropped; REQ-AID-013 renumbered; REQ-AID-014, REQ-AID-015 added = 14 total requirements)
- Update Notes column for each requirement to reflect new model references
- Update "Depends on REQ-BED-014 through REQ-BED-018" note to "Depends on REQ-BED-027/028/029"
- Update Technical Summary to reflect new TTL approach, sub-agent identity tier model, Standalone support, agent transparency
- Progress stays 0/14 (nothing implemented)

---

## Acceptance Criteria

- [ ] `requirements.md`: No references to Restricted/Unrestricted mode, `request_mode_upgrade`, or REQ-BED-014/015/016 anywhere
- [ ] `requirements.md`: REQ-AID-006 is dropped; REQ-AID-014 and REQ-AID-015 added
- [ ] `requirements.md`: Sub-agent section (REQ-AID-008) clearly distinguishes git identity injection (Work sub-agents) from SSH push auth (parent Work conv only)
- [ ] `requirements.md`: Cert TTL default is 7 days with rationale as dead-man's switch; no runtime expiry events or re-mint flow
- [ ] `requirements.md`: Standalone mode addressed in REQ-AID-014
- [ ] `design.md`: `CredentialExpired` event removed from state machine design
- [ ] `design.md`: Sub-agent design section has two clearly separated tiers (git identity vs SSH auth)
- [ ] `design.md`: Dependencies reference REQ-BED-027/028/029 (not 014-018)
- [ ] `design.md`: SSH socket path uses `~/.phoenix-ide/run/` not `/tmp/`
- [ ] `executive.md`: Requirements table accurate; Technical Summary updated
- [ ] No new open questions or TODOs left unresolved in any file


## Progress

