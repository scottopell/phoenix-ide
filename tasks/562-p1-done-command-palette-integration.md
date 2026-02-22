---
id: 562
priority: p1
status: done
title: Command Palette Integration
created: 2025-02-22
requirements:
  - REQ-CP-001
  - REQ-CP-002
  - REQ-CP-003
  - REQ-CP-004
  - REQ-CP-005
  - REQ-CP-006
  - REQ-CP-007
  - REQ-CP-008
spec: specs/command-palette/
---

# Command Palette Integration

Integration task for Command Palette after core implementation is complete (task 564).

This task wires the Command Palette into the existing app:
- Connect `ConversationSource` to app's conversation list state
- Wire action handlers to existing functionality
- Ensure state indicators reuse REQ-UI-012 styling

## Prerequisites

- Task 564 (Command Palette implementation) must be complete

## Scope

### Wire Up Data Sources
1. `ConversationSource` reads from `appMachine` context or API
2. Actions trigger existing handlers (navigate, archive, new conversation, etc.)

### Integration Points
- Conversation list data (existing API/state)
- Navigation (react-router `useNavigate`)
- State indicators (reuse `.state-dot` CSS from task 561)
- Toast notifications (existing `useToast`)

## Acceptance Criteria

- [x] Palette shows real conversation data from app state
- [x] Selecting conversation navigates correctly
- [x] Actions trigger correct app behaviors
- [x] State indicators match conversation list styling (reuses .conv-state-dot)
- [x] No regressions to existing functionality
