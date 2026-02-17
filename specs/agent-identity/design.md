# Agent Identity - Design Document

## Overview

This document describes the technical design for implementing REQ-AID-001 through REQ-AID-013. The agent identity system provides cryptographically verifiable commit attribution by minting ephemeral SSH certificates when users approve mode upgrades.

## Architecture

The credential system consists of three main components:

1. **Credential Manager** - Holds the SSH CA key, manages active credentials per conversation
2. **SSH Agent Lifecycle** - Starts/stops per-conversation ssh-agent processes
3. **Audit Log** - Records all credential operations in SQLite

Credentials flow through the system as follows:
- Mode upgrade approval triggers `MintCredential` effect (REQ-AID-013)
- Credential Manager generates ephemeral keypair and certificate (REQ-AID-002)
- ssh-agent is started and key is loaded (REQ-AID-003)
- Environment variables injected into bash tool (REQ-AID-003, REQ-AID-004)
- Mode downgrade triggers `RevokeCredential` effect (REQ-AID-005, REQ-AID-013)
- Expiry timer triggers `RevokeCredential` effect (REQ-AID-006)

---

## REQ-AID-001: Automatic Signing Key Setup

### CA Key Generation

On first startup, generate an Ed25519 SSH CA keypair:
- Algorithm: Ed25519 (modern, fast, secure)
- Storage: User-configurable path, default `~/.phoenix-ide/ca_key`
- Permissions: 0600 for key file, 0700 for containing directory

On subsequent startups, load the existing key from the configured path.

### Setup Instructions Output

When CA key is generated, log the public key with instructions:
```
Generated SSH CA key. To enable signed commits, add this public key to your git server.

For Forgejo, add to app.ini:
  [server]
  SSH_TRUSTED_USER_CA_KEYS = /path/to/ca_key.pub

Public key:
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... Phoenix IDE CA
```

### Failure Handling

If CA key cannot be loaded (permissions, corruption, missing when auto-generate disabled):
- Log warning with specific error
- Set `ca_available = false`
- Continue startup (graceful degradation per REQ-AID-011)

---

## REQ-AID-002: Task-Scoped Credentials

### Certificate Contents

Each certificate includes:
- **Key type**: Ed25519 (ephemeral, generated per credential)
- **Principal**: `phoenix-task-{conversation_id_prefix}` (8 chars of UUID)
- **Valid-after**: Current timestamp
- **Valid-before**: Current timestamp + validity duration (default 1800 seconds)
- **Key ID**: Same as principal (for logging)
- **Extensions**: `permit-agent-forwarding` (allows ssh-agent use)

### Validity Duration

Configurable via `PHOENIX_CERT_VALIDITY_SECS` environment variable:
- Default: 1800 (30 minutes)
- Minimum: 60 (1 minute, for testing)
- Maximum: 86400 (24 hours)

### Task Identifier Encoding

The certificate principal encodes task identity:
- Format: `phoenix-task-{id}` where `{id}` is first 8 characters of conversation UUID
- This appears in Forgejo SSH auth logs
- Combined with audit log, enables full traceability

---

## REQ-AID-003: Transparent Git Authentication

### Per-Conversation ssh-agent

Each conversation with active credentials gets a dedicated ssh-agent:
- Socket path: `/tmp/phoenix-ssh-{conversation_id_prefix}/agent.sock`
- Contains only the ephemeral key for that conversation
- Isolated from other conversations and system ssh-agent

### Environment Injection

When credentials are active, bash tool receives these environment variables:
- `SSH_AUTH_SOCK`: Path to conversation's ssh-agent socket
- `GIT_SSH_COMMAND`: `ssh -o IdentitiesOnly=yes -o IdentityAgent=$SSH_AUTH_SOCK`

The `IdentitiesOnly=yes` prevents ssh from trying other keys.
The `IdentityAgent` ensures only the conversation's agent is used.

### No Credentials Case

When `ToolContext.credentials` is `None`:
- No SSH environment variables injected
- Git operations use whatever authentication the user has configured
- Push to authenticated remotes will fail (expected in Restricted mode)

---

## REQ-AID-004: Proper Commit Attribution

### Git Identity Environment

When credentials are active, bash tool also receives:
- `GIT_AUTHOR_NAME`: Configured or default `"Phoenix Agent"`
- `GIT_AUTHOR_EMAIL`: Configured or default `"phoenix@localhost"`
- `GIT_COMMITTER_NAME`: Same as author name
- `GIT_COMMITTER_EMAIL`: Same as author email

### No Config File Modification

Identity is set ONLY via environment variables:
- Never writes to `~/.gitconfig`
- Never writes to `.git/config`
- No `git config` commands executed
- Identity is transient - disappears when process exits

### Configuration

Identity configurable via:
- `PHOENIX_GIT_NAME`: Author/committer name
- `PHOENIX_GIT_EMAIL`: Author/committer email

---

## REQ-AID-005: Credential Revocation on Downgrade

### Revocation Process

1. Remove credential from active credentials map
2. Cancel expiry timer (if running)
3. Kill ssh-agent process (SIGTERM)
4. Remove socket directory
5. Record revocation in audit log

### Immediate Effect

Revocation is synchronous with mode transition:
- No delay between downgrade approval and credential removal
- Any in-flight git operation using the credential may fail (acceptable)
- Subsequent git operations will not have credentials

---

## REQ-AID-006: Credential Expiry Notification

### Expiry Timer

When credential is minted:
- Start tokio timer for validity duration
- On expiry, send `CredentialExpired` event to conversation runtime

### Expiry Handling

When `CredentialExpired` event received:
- Emit `RevokeCredential` effect with reason `Expired`
- Inject synthetic system message: "Credentials expired. Request mode upgrade again if write access is still needed."
- Conversation remains in Unrestricted mode (can still use patch tool)
- Only signing/push capability is lost

### Timer Cancellation

Expiry timer is cancelled when:
- Mode is downgraded (revoked for different reason)
- Conversation reaches terminal state

---

## REQ-AID-007: Credential Audit Trail

### Database Schema

New table `credential_audit`:
- `id`: Auto-increment primary key
- `conversation_id`: Foreign key to conversations
- `fingerprint`: SHA256 fingerprint of certificate
- `principal`: Certificate principal (task identifier)
- `issued_at`: ISO8601 timestamp
- `expires_at`: ISO8601 timestamp
- `approved_by`: User who approved mode upgrade
- `revoked_at`: ISO8601 timestamp (null if not revoked)
- `revocation_reason`: Enum (downgrade, expired, cleanup, error)

### Query Support

- By conversation ID: All credentials for a task
- By time range: Credentials active during a period
- By fingerprint: Specific credential lookup

---

## REQ-AID-008: Sub-Agent Credential Isolation

### Enforcement Points

1. **Runtime spawn**: When creating sub-agent `ToolContext`, set `credentials = None`
2. **Tool registry**: Sub-agents get `ToolRegistry::new_for_subagent()` which already restricts tools
3. **Bash tool**: Credential injection checks `ctx.credentials`, which is None for sub-agents

### Error Message

When sub-agent attempts authenticated git operation and fails:
- SSH will fail with auth error (no credentials provided)
- Agent sees: "Permission denied (publickey)"
- System does not provide additional context (sub-agent doesn't know about credential system)

---

## REQ-AID-009: Configurable Credential Parameters

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PHOENIX_SSH_CA_KEY` | `~/.phoenix-ide/ca_key` | CA private key path |
| `PHOENIX_SSH_CA_AUTO_GENERATE` | `true` | Generate CA if missing |
| `PHOENIX_CERT_VALIDITY_SECS` | `1800` | Certificate lifetime |
| `PHOENIX_GIT_NAME` | `Phoenix Agent` | Git author name |
| `PHOENIX_GIT_EMAIL` | `phoenix@localhost` | Git author email |
| `PHOENIX_DELEGATING_USER` | `$USER` | Audit log approver |

### Configuration Loading

All configuration loaded at startup via `AgentIdentityConfig::from_env()`.
Configuration is immutable after startup.

---

## REQ-AID-010: Git Server Trust Setup

### Forgejo Configuration

Documented setup process:

1. Get CA public key from Phoenix startup logs
2. Save to file accessible by Forgejo (e.g., `/etc/forgejo/phoenix_ca.pub`)
3. Add to `app.ini`:
   ```ini
   [server]
   SSH_TRUSTED_USER_CA_KEYS = /etc/forgejo/phoenix_ca.pub
   ```
4. Create bot account (e.g., `phoenix-agent`)
5. Certificates with any principal from the CA will authenticate as the bot account
6. Grant bot account repository permissions

### What Forgejo Sees

- SSH auth log: Connection from certificate with principal `phoenix-task-{id}`
- Commit: Author/committer from git environment
- No native delegation tracking (Forgejo doesn't parse trailers)

---

## REQ-AID-011: Graceful Degradation

### Degraded States

| Component | Degraded Behavior |
|-----------|------------------|
| CA key unavailable | Mode upgrades work, no credentials minted |
| ssh-agent fails to start | Warning logged, no credentials for this conversation |
| Certificate generation fails | Warning logged, no credentials for this conversation |

### User Feedback

When operating in degraded mode, inject synthetic message:
"Warning: Could not generate signing credentials. Git push may require manual authentication."

### Git Operation Failures

When git push fails due to missing credentials:
- SSH error: "Permission denied (publickey)"
- Agent can request mode upgrade again or inform user
- No special handling - standard tool error flow

---

## REQ-AID-012: Secure Credential Storage

### CA Key Protection

- File permissions: 0600 (owner read/write only)
- Directory permissions: 0700 (owner only)
- Never logged (only public key logged)

### Ephemeral Key Handling

- Generated in memory
- Written to temp file only for `ssh-add` command
- Temp file deleted immediately after `ssh-add`
- Private key material not retained after ssh-agent has it

### Audit Log Protection

- Stored in same database as conversations
- Contains fingerprints, NOT private keys or full certificates
- Protected by database file permissions

---

## REQ-AID-013: Mode Transition Credential Effects

### New State Machine Effects

Added to `Effect` enum:
- `MintCredential { conversation_id: String }`
- `RevokeCredential { conversation_id: String, reason: RevocationReason }`

### New State Machine Events

Added to `Event` enum:
- `CredentialExpired { conversation_id: String }`

### Transition Integration

| Mode Transition | Credential Effect |
|----------------|------------------|
| `AwaitingModeApproval` + `UserApproveUpgrade` | `MintCredential` |
| Any + `UserRequestDowngrade` (when Unrestricted) | `RevokeCredential(Downgrade)` |
| Any + `CredentialExpired` | `RevokeCredential(Expired)` |
| Terminal state reached | `RevokeCredential(Cleanup)` |

### Extended ToolContext

`ToolContext` gains `credentials: Option<CredentialInfo>` field:
- Set by executor when creating tool context
- Populated from `CredentialManager.get_credential(conversation_id)`
- `None` for Restricted mode, sub-agents, or degraded operation

---

## Dependencies

### Rust Crates

- `ssh-key`: SSH key and certificate operations (Ed25519, OpenSSH format)
- `tempfile`: Temporary files for ssh-add (likely already present)

### System Requirements

- `ssh-agent` binary available in PATH
- `ssh-add` binary available in PATH
- Unix-like OS for ssh-agent socket support

### Feature Dependencies

- REQ-BED-014 through REQ-BED-018 (Conversation Mode) must be implemented first
- Credential effects are emitted by mode transitions
