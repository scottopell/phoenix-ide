# Agent Identity and Credentials

## User Story

As a developer using Phoenix IDE to make changes to a codebase, I need commits made by the agent to be properly attributed and cryptographically signed so that I can track which changes came from which agent task and maintain audit trails in version control.

## Context

This system integrates with the Conversation Mode system (REQ-BED-014 through REQ-BED-018). When a user approves a mode upgrade from Restricted to Unrestricted, the agent gains write access to files. This is also the moment when the agent should receive credentials for git operations:

- **Restricted Mode**: Read-only filesystem, no git write operations, no credentials
- **Unrestricted Mode**: Full filesystem access, can commit and push, receives ephemeral SSH certificate

The IDE itself acts as the credential broker. It holds an SSH Certificate Authority (CA) private key and mints short-lived certificates on demand when mode upgrades are approved.

---

## Requirements

### REQ-AID-001: Automatic Signing Key Setup

WHEN Phoenix IDE server starts for the first time
THE SYSTEM SHALL automatically generate signing credentials
AND store them securely for future use
AND display setup instructions for the git server

WHEN Phoenix IDE server starts with existing signing credentials
THE SYSTEM SHALL load them for use

WHEN signing credentials cannot be loaded or generated
THE SYSTEM SHALL log a warning
AND operate without commit signing capability

**Rationale:** Users should not need manual cryptographic setup to start using Phoenix IDE. The system should work out of the box while guiding users through git server configuration.

---

### REQ-AID-002: Task-Scoped Credentials

WHEN user approves a mode upgrade to Unrestricted
THE SYSTEM SHALL generate fresh credentials for that specific task
AND limit credential validity to 30 minutes by default
AND encode the task identifier in the credential

WHEN credentials are generated
THE SYSTEM SHALL record the issuance in the audit log

**Rationale:** Users need confidence that agent credentials are short-lived and task-specific. If credentials leak, the blast radius is limited to a single task and time window.

---

### REQ-AID-003: Transparent Git Authentication

WHEN conversation is in Unrestricted mode with active credentials
THE SYSTEM SHALL automatically authenticate git operations
WITHOUT requiring agent awareness of authentication details

WHEN conversation is in Restricted mode
THE SYSTEM SHALL NOT provide any authentication credentials

**Rationale:** Users want git push/pull to "just work" when the agent has write access, without the agent needing to handle SSH keys or tokens explicitly.

---

### REQ-AID-004: Proper Commit Attribution

WHEN agent makes a git commit in Unrestricted mode
THE SYSTEM SHALL attribute the commit to a configured identity
AND NOT modify any persistent git configuration files

WHEN commit attribution is configured
THE SYSTEM SHALL use the configured name and email

WHEN commit attribution is not configured
THE SYSTEM SHALL use sensible defaults that identify the commit as agent-generated

**Rationale:** Users need commits to be properly attributed for code review and audit purposes. The attribution should be transient (not persisted in config files) to avoid polluting the user's environment.

---

### REQ-AID-005: Credential Revocation on Downgrade

WHEN user downgrades from Unrestricted to Restricted mode
THE SYSTEM SHALL immediately revoke all active credentials for that conversation
AND record the revocation in the audit log

WHEN credentials are revoked
THE SYSTEM SHALL clean up any associated processes or resources

**Rationale:** Users need confidence that downgrading mode immediately removes write capabilities. There should be no window where old credentials could be used.

---

### REQ-AID-006: Credential Expiry Notification

WHEN credentials expire during an active conversation
THE SYSTEM SHALL notify the agent that credentials have expired
AND suggest requesting a mode upgrade if write access is still needed

WHEN credentials expire
THE SYSTEM SHALL revoke them and record the expiry in the audit log

**Rationale:** Users and agents need clear feedback when credentials expire. The agent should not silently fail git operations due to expired credentials.

---

### REQ-AID-007: Credential Audit Trail

WHEN any credential is issued, revoked, or expires
THE SYSTEM SHALL record the event with:
  - Task identifier
  - Credential fingerprint
  - Timestamp
  - Reason (issuance, downgrade, expiry, cleanup)
  - Approving user (for issuance)

WHEN user queries the audit log
THE SYSTEM SHALL support queries by task identifier and time range

**Rationale:** Users need a complete audit trail to trace which credentials were active when, and to investigate any security concerns about agent-made commits.

---

### REQ-AID-008: Sub-Agent Credential Isolation

WHEN sub-agent is spawned
THE SYSTEM SHALL NOT provide any credentials to the sub-agent
REGARDLESS of the parent conversation's mode

WHEN sub-agent attempts authenticated git operations
THE SYSTEM SHALL fail with a clear error message explaining the restriction

**Rationale:** Users need assurance that less-supervised sub-agents cannot perform authenticated writes. Only the parent conversation with direct human oversight should have write credentials.

---

### REQ-AID-009: Configurable Credential Parameters

WHEN administrator configures Phoenix IDE
THE SYSTEM SHALL allow configuration of:
  - Signing key location
  - Credential validity duration
  - Git identity (name and email)
  - Delegating user identity for audit

WHEN configuration is not provided
THE SYSTEM SHALL use sensible defaults

**Rationale:** Users need flexibility to match their organization's security requirements (shorter credential lifetimes for high-security environments) while having zero-config defaults for personal use.

---

### REQ-AID-010: Git Server Trust Setup

WHEN Phoenix IDE generates signing credentials
THE SYSTEM SHALL provide clear instructions for configuring the git server to trust those credentials

WHEN using Forgejo as the git server
THE SYSTEM SHALL document the specific configuration needed

**Rationale:** Users need actionable guidance to complete the setup. The system should bridge the gap between credential generation and git server trust.

---

### REQ-AID-011: Graceful Degradation

WHEN signing credentials are unavailable
THE SYSTEM SHALL still allow mode upgrades
AND allow git operations that don't require authentication
AND clearly indicate the degraded state to the user

WHEN git push fails due to missing credentials
THE SYSTEM SHALL provide an actionable error message

**Rationale:** Users should not be blocked from using write mode just because signing isn't set up. The system should work at reduced capability rather than failing entirely.

---

### REQ-AID-012: Secure Credential Storage

WHEN storing the signing key
THE SYSTEM SHALL use restricted file permissions
AND store in a configurable location

WHEN handling ephemeral credentials
THE SYSTEM SHALL NOT write private key material to disk
AND clear credentials from memory when no longer needed

**Rationale:** Users need confidence that credentials are handled securely. The signing CA key is long-lived and needs disk protection; ephemeral keys should never touch disk.

---

### REQ-AID-013: Mode Transition Credential Effects

WHEN user approves mode upgrade (REQ-BED-015)
THE SYSTEM SHALL emit credential minting effects as part of the transition

WHEN user requests mode downgrade (REQ-BED-016)
THE SYSTEM SHALL emit credential revocation effects as part of the transition

WHEN conversation reaches terminal state
THE SYSTEM SHALL clean up any active credentials

**Rationale:** Users expect credential lifecycle to be tightly coupled with mode transitions. Credentials should appear exactly when write access is granted and disappear exactly when it's revoked.
