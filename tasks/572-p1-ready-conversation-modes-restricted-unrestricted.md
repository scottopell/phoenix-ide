---
created: 2026-02-26
priority: p1
status: ready
---

# Implement Conversation Modes (Restricted / Unrestricted)

## Summary

Add a mode system to conversations: **Restricted** (read-only, no network) and **Unrestricted** (full access). Agents start in Restricted mode for safe exploration, then request upgrade when they need to make changes. Users approve or deny.

This is a cross-cutting feature touching the state machine, tool execution, UI, and (optionally) kernel-level sandboxing.

## spEARS References

All requirements and design details live in existing specs:

| Spec | Requirements | What it covers |
|------|-------------|----------------|
| `specs/bedrock/requirements.md` | REQ-BED-014 through REQ-BED-018 | Mode lifecycle: init, upgrade request, downgrade, communication, sub-agent enforcement |
| `specs/bedrock/design.md` | (mode sections) | State transitions, AwaitingModeApproval state, synthetic messages |
| `specs/bash/requirements.md` | REQ-BASH-008, REQ-BASH-009 | Landlock enforcement, graceful degradation without Landlock |
| `specs/bash/design.md` | (safety checks) | Command safety checks (already implemented), Landlock integration point |
| `specs/patch/requirements.md` | REQ-PATCH-009 | Patch tool disabled in Restricted mode |
| `specs/agent-identity/requirements.md` | REQ-AID-001 through REQ-AID-013 | Credential lifecycle tied to mode (future phase) |

## Implementation Phases

### Phase 1: Mode-aware state machine (platform-independent)

- Add `ConversationMode` enum (`Restricted`, `Unrestricted`) to `ConvContext` or `ConvState`
- Add `AwaitingModeApproval` state to state machine
- Add `request_mode_upgrade` tool
- Mode-aware tool filtering (block patch in Restricted, clear error messages)
- Synthetic system messages on mode transitions (REQ-BED-017)
- Sub-agent mode enforcement (REQ-BED-018)
- Persist mode in conversation state / DB

### Phase 2: UI for mode approval

- Display mode upgrade request with reason to user
- Approve/deny buttons
- Mode indicator in conversation header
- User-initiated downgrade

### Phase 3: Kernel-level enforcement (platform-specific)

- **Linux:** Landlock integration for bash tool (REQ-BASH-008)
- **macOS:** Research in progress -- `sandbox-exec` (deprecated), no modern equivalent for runtime child process sandboxing. Tool-level gating is the baseline.
- Detection at startup, capability advertisement

## Open Questions

- Should Restricted mode on macOS (no Landlock) still be offered with tool-level-only enforcement, or disabled entirely? Current spec (REQ-BASH-009) says disable. Recommendation: offer it as degraded-but-functional.
- Does disabling bash entirely (big hammer) warrant a `ReadFile` + `ListDirectory` tool to compensate? Likely not worth the capability loss.
- Credential system (agent-identity spec) -- defer to a later task or bundle?

## Key Files (Expected Changes)

- `src/state_machine/state.rs` -- `ConversationMode` enum, mode in `ConvContext`
- `src/state_machine/transition.rs` -- `AwaitingModeApproval` transitions
- `src/state_machine/event.rs` -- mode-related events
- `src/tools.rs` -- mode-aware tool registry filtering
- `src/tools/bash.rs` -- Landlock wrapper (Phase 3)
- `src/tools/patch.rs` -- mode check before execution
- `src/api/` -- mode approval endpoint, mode in SSE state
- `ui/src/` -- approval UI, mode indicator
