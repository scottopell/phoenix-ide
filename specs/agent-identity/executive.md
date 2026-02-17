# Agent Identity - Executive Summary

## Requirements Summary

The agent identity system enables Phoenix IDE to provide cryptographically verifiable commit attribution for agent tasks. When a user approves a mode upgrade from Restricted to Unrestricted, the system generates task-scoped credentials that automatically authenticate git operations. Credentials are short-lived (30-minute default) and encode the task identifier for traceability. On mode downgrade or credential expiry, credentials are immediately revoked. Sub-agents never receive credentials regardless of parent mode. All credential operations are recorded in an audit log for security review. The system supports graceful degradation - if signing credentials are unavailable, write mode still works but without cryptographic commit attribution.

## Technical Summary

Credential management uses SSH certificates signed by a locally-held Certificate Authority key. The CA key is auto-generated on first run or loaded from a configured path. On mode upgrade approval, the system generates an ephemeral Ed25519 keypair, creates an SSH user certificate with the CA signature, and loads it into a per-conversation ssh-agent process. Git environment variables are injected into bash tool execution to provide identity and authentication. Certificate principal encodes task identity for Forgejo audit logs. Credential revocation kills the ssh-agent and records the event. The audit log is stored in SQLite alongside conversation data. State machine integration adds `MintCredential` and `RevokeCredential` effects triggered by mode transitions.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-AID-001:** Automatic Signing Key Setup | ❌ Not Started | - |
| **REQ-AID-002:** Task-Scoped Credentials | ❌ Not Started | - |
| **REQ-AID-003:** Transparent Git Authentication | ❌ Not Started | - |
| **REQ-AID-004:** Proper Commit Attribution | ❌ Not Started | - |
| **REQ-AID-005:** Credential Revocation on Downgrade | ❌ Not Started | - |
| **REQ-AID-006:** Credential Expiry Notification | ❌ Not Started | - |
| **REQ-AID-007:** Credential Audit Trail | ❌ Not Started | - |
| **REQ-AID-008:** Sub-Agent Credential Isolation | ❌ Not Started | - |
| **REQ-AID-009:** Configurable Credential Parameters | ❌ Not Started | - |
| **REQ-AID-010:** Git Server Trust Setup | ❌ Not Started | - |
| **REQ-AID-011:** Graceful Degradation | ❌ Not Started | - |
| **REQ-AID-012:** Secure Credential Storage | ❌ Not Started | - |
| **REQ-AID-013:** Mode Transition Credential Effects | ❌ Not Started | Depends on REQ-BED-014 through REQ-BED-018 |

**Progress:** 0 of 13 complete
