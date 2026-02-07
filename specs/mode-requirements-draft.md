# Conversation Mode Requirements - DRAFT

This document proposes new requirements to be integrated into existing specs.
No new specs - these extend bedrock, bash, and patch.

---

## Bedrock Extensions

### REQ-BED-014: Conversation Mode

WHEN conversation is created
THE SYSTEM SHALL initialize in Restricted mode

WHEN kernel sandbox (Landlock LSM) is unavailable on the host system
THE SYSTEM SHALL operate with only Unrestricted mode available
AND indicate sandbox unavailability to the user

WHEN conversation is in Restricted mode
THE SYSTEM SHALL enforce read-only semantics on all tools
AND execute bash commands within kernel sandbox (when available)
AND disable write-capable tools (patch)

WHEN conversation is in Unrestricted mode
THE SYSTEM SHALL allow full tool capabilities

**Rationale:** Users need safe exploration mode for understanding codebases and triaging issues before committing to changes. Kernel-level enforcement provides defense in depth beyond LLM instruction-following.

---

### REQ-BED-015: Mode Upgrade Request

WHEN LLM needs write capabilities in Restricted mode
THE SYSTEM SHALL provide a `request_mode_upgrade` tool
WHICH accepts a reason string explaining why upgrade is needed

WHEN upgrade is requested
THE SYSTEM SHALL transition to AwaitingModeApproval state
AND notify user of the upgrade request with reason
AND pause agent execution until user responds

WHEN user approves upgrade
THE SYSTEM SHALL transition to Unrestricted mode
AND inject synthetic system message indicating mode change
AND resume agent execution

WHEN user denies upgrade
THE SYSTEM SHALL remain in Restricted mode
AND return denial to agent via tool result
AND resume agent execution

WHEN user does not respond within reasonable time
THE SYSTEM SHALL remain paused (no automatic timeout to unrestricted)

**Rationale:** Agents should be able to request capabilities when needed, with human approval as the gate. This enables "planning mode" → "implementation mode" workflow.

---

### REQ-BED-016: Mode Downgrade

WHEN user requests mode downgrade (Unrestricted → Restricted)
THE SYSTEM SHALL transition immediately to Restricted mode
AND inject synthetic system message indicating mode change
AND NOT require agent approval

WHEN mode changes (either direction)
THE SYSTEM SHALL persist the new mode as part of conversation state

**Rationale:** Users can always tighten permissions. The asymmetry (user approval to upgrade, immediate downgrade) reflects trust model.

---

### REQ-BED-017: Mode Indication to Agent

WHEN mode changes during conversation
THE SYSTEM SHALL inject a synthetic system message visible to the agent
WHICH clearly states the new mode and its implications

WHEN agent is in Restricted mode
THE SYSTEM SHALL NOT modify tool descriptions based on mode
AND rely on synthetic messages and tool error responses to communicate restrictions

WHEN tool is unavailable due to mode restrictions
THE SYSTEM SHALL return clear, actionable error message
WHICH suggests using request_mode_upgrade if write access is needed

**Rationale:** Tool descriptions must remain static throughout conversation (reduces confusion). Mode awareness comes through synthetic messages and clear error responses.

---

### REQ-BED-018: Mode State Machine Integration

WHEN conversation has mode state
THE SYSTEM SHALL include mode in ConversationState
AND support mode transitions as first-class state machine events

WHEN mode transitions occur
THE SYSTEM SHALL emit appropriate effects (persist, notify, inject message)
AND maintain all state machine invariants

WHEN sub-agent is spawned
THE SYSTEM SHALL inherit parent conversation's mode
AND NOT allow sub-agent to request mode upgrade (only parent can)

**Rationale:** Mode is conversation-level state that participates in the state machine. Sub-agents inherit context but cannot escalate privileges.

---

## Bash Tool Extensions

> **Note:** REQ-BASH-007 (Command Safety Checks) is already implemented and documented in specs/bash/.

### REQ-BASH-008: Kernel Sandbox Enforcement

WHEN conversation is in Restricted mode AND Landlock LSM is available
THE SYSTEM SHALL execute bash commands within kernel sandbox providing:
- Read-only filesystem access (writes blocked at kernel level)
- No network access (prevents exfiltration)
- Resource limits (memory, CPU, process count)

WHEN sandbox blocks an operation
THE SYSTEM SHALL return the kernel error (EPERM, etc.)
AND tool description SHALL include clear explanation of sandbox constraints

WHEN bash command fails due to sandbox
THE SYSTEM SHALL NOT retry or attempt to work around restrictions

**Rationale:** Kernel enforcement provides true read-only mode that cannot be bypassed by clever commands. Defense in depth beyond instruction-following.

---

### REQ-BASH-009: Graceful Degradation

WHEN Landlock LSM is unavailable on host system
THE SYSTEM SHALL detect this at startup
AND disable Restricted mode entirely
AND log warning about reduced security posture

WHEN degraded mode is active
THE SYSTEM SHALL still apply command safety checks (REQ-BASH-007)
AND indicate to user that kernel sandbox is unavailable

WHEN user attempts to enable Restricted mode without Landlock
THE SYSTEM SHALL return error explaining kernel requirements

**Rationale:** Not all environments support Landlock (requires Linux 5.13+, ideally 6.12+). System must work gracefully without it, but clearly communicate reduced protection.

---

## Patch Tool Extensions

### REQ-PATCH-009: Mode-Based Availability

WHEN conversation is in Restricted mode
THE SYSTEM SHALL disable the patch tool entirely
AND return error when patch is attempted: "Patch tool is disabled in Restricted mode. Use request_mode_upgrade to request write access."

WHEN conversation is in Unrestricted mode
THE SYSTEM SHALL enable full patch tool functionality

WHEN mode changes from Unrestricted to Restricted
THE SYSTEM SHALL immediately disable patch tool
AND NOT affect any in-flight operations (mode change waits for idle)

**Rationale:** Patch tool writes files - must be disabled in read-only mode. Phoenix-level enforcement is simpler and more informative than kernel errors.

---

## New Tool: request_mode_upgrade

### Tool Schema

```json
{
  "name": "request_mode_upgrade",
  "description": "Request upgrade from Restricted to Unrestricted mode. Only available in Restricted mode. User must approve the request.",
  "input_schema": {
    "type": "object",
    "required": ["reason"],
    "properties": {
      "reason": {
        "type": "string", 
        "description": "Explanation of why write access is needed"
      }
    }
  }
}
```

### Behavior

WHEN called in Unrestricted mode
THE SYSTEM SHALL return error "Already in Unrestricted mode"

WHEN called in Restricted mode
THE SYSTEM SHALL pause execution and await user decision
AND return result only after user responds

---

## State Machine Additions

### New State Component

```rust
enum ConversationMode {
    Restricted,   // Read-only, sandbox enabled (when available)
    Unrestricted, // Full access
}
```

### New Events

```rust
enum Event {
    // ... existing events ...
    
    // Mode events
    ModeUpgradeRequested { reason: String },
    UserApproveUpgrade,
    UserDenyUpgrade,
    UserRequestDowngrade,
}
```

### New State

```rust
enum ConvState {
    // ... existing states ...
    
    /// Waiting for user to approve/deny mode upgrade
    AwaitingModeApproval { 
        reason: String,
        pending_tool_id: String,  // The request_mode_upgrade call
    },
}
```

### Key Transitions

| Current State | Event | Next State | Effects |
|--------------|-------|------------|----------|
| ToolExecuting (request_mode_upgrade) | ModeUpgradeRequested | AwaitingModeApproval | NotifyClient, PersistState |
| AwaitingModeApproval | UserApproveUpgrade | ToolExecuting (continue) | SetMode(Unrestricted), InjectMessage, NotifyClient |
| AwaitingModeApproval | UserDenyUpgrade | ToolExecuting (continue) | ToolResult(denied), NotifyClient |
| Any (Idle preferred) | UserRequestDowngrade | Same | SetMode(Restricted), InjectMessage, NotifyClient |

---

## Property Invariants (for proptesting)

1. **Mode monotonicity during upgrade flow**: Once AwaitingModeApproval, only UserApprove/UserDeny can exit
2. **Sub-agent mode inheritance**: Sub-agent mode always equals parent mode at spawn time
3. **Tool availability consistency**: In Restricted mode, patch tool always returns mode error
4. **Downgrade immediacy**: UserRequestDowngrade from Idle always succeeds immediately
5. **No automatic escalation**: No event sequence from Restricted reaches Unrestricted without UserApproveUpgrade
6. **Sandbox availability determines mode enum**: If !landlock_available, mode is always Unrestricted

---

## Open Questions

1. Should there be a UI indicator showing current mode? (Probably yes - like vim's mode line)
2. Should mode upgrade requests have a timeout? (Currently: no, user must respond)
3. Should we support "temporary upgrade" (auto-downgrade after N tool calls)? (Probably not for v1)
4. What happens if user closes browser while in AwaitingModeApproval? (Persist state, resume on reconnect)
