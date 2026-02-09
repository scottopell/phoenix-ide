---
created: 2026-02-06
priority: p1
status: done
---

# Create SPEARS Spec for Voice-to-Text Integration

## Summary

Write a complete SPEARS specification (requirements.md, design.md, executive.md) for voice dictation functionality in the Phoenix UI. This is a **spec-only task** - no implementation code will be written.

## Context

Voice-to-text / voice dictation is a high-value UX feature, especially on mobile. Modern browsers provide the Web Speech API, and iOS/Safari has excellent native dictation support. However, the UX around voice input has subtle pitfalls:

1. **Input clearing behavior**: Some apps fail to clear the input field after sending a voice-dictated message, leading to accidental re-sends or confusion
2. **Visual feedback**: Users need clear indication when recording is active vs processing vs ready
3. **Error states**: Microphone permission denied, speech recognition failures, network issues for cloud-based recognition
4. **Integration with existing input flow**: How voice input interacts with draft persistence, image attachments, and the existing send/cancel states

The goal is to define these behaviors precisely BEFORE implementation, avoiding the common UX bugs seen in other apps.

## Relevant Files to Review

### SPEARS Methodology
- `/home/exedev/phoenix-ide/SPEARS.md` - Full methodology documentation
- `/home/exedev/phoenix-ide/SPEARS_AGENT.md` - Agent workflow rules and checklists

### Existing UI Spec (for consistency)
- `/home/exedev/phoenix-ide/specs/ui/requirements.md` - Current UI requirements (REQ-UI-001 through REQ-UI-011)
- `/home/exedev/phoenix-ide/specs/ui/design.md` - Current UI design decisions

### Current Input Implementation (for understanding context)
- `/home/exedev/phoenix-ide/ui/src/components/InputArea.tsx` - Current input component
- `/home/exedev/phoenix-ide/ui/src/pages/ConversationPage.tsx` - How InputArea is used
- `/home/exedev/phoenix-ide/ui/src/hooks/index.ts` - Draft persistence hook (useDraft)

## Acceptance Criteria

- [ ] Create `specs/voice-input/requirements.md` with EARS-formatted requirements
  - Focus on user needs and outcomes:
    - Users can dictate messages instead of typing
    - Users understand when the system is listening
    - Users can review and edit dictated text before sending
    - Users can cancel dictation at any time
    - System handles errors gracefully (permissions, network issues, etc.)
    - Voice input integrates smoothly with existing message composition features

- [ ] Create `specs/voice-input/design.md` with technical approach
  - Explore available browser APIs and their capabilities
  - Define fallback strategies for unsupported environments
  - Design state management for voice input flow
  - Determine integration approach with existing components
  - All design decisions must trace to specific requirements

- [ ] Create `specs/voice-input/executive.md` with status tracking
  - Requirements summary (250 words max)
  - Technical summary (250 words max)
  - Status table (all ❌ Not Started initially)
  - NO code blocks

- [ ] Requirements pass SPEARS quality checklist:
  - [ ] User-benefit titles (not implementation-focused)
  - [ ] No implementation details in requirements (no "Web Speech API", "SpeechRecognition")
  - [ ] Testable criteria
  - [ ] Self-contained (no "as before" or "maintain existing behavior")

## Key UX Considerations

The spec should explore these user experience aspects:

1. **Message flow**: How does voice input fit into the overall message composition and sending flow?
2. **Feedback mechanisms**: How do users know the system is listening, processing, or has completed transcription?
3. **Error recovery**: What happens when things go wrong, and how can users recover?
4. **User control**: What level of control do users have over the dictation process?
5. **Platform differences**: How might the experience vary across different devices and browsers?

## Notes

- This is a META task: the deliverable is documentation, not code
- Follow SPEARS_AGENT.md workflow strictly
- Run the quality checklist before marking complete
- Consider REQ-UI-003 (Message Composition) and REQ-UI-004 (Message Delivery States) for consistency
- Learn from common voice input UX issues in other applications, but let requirements emerge from user needs

## Research References

- Research voice input patterns in messaging applications
- Investigate browser capabilities for speech recognition
- Study accessibility best practices for voice interfaces
- Consider both dedicated voice buttons and native keyboard integration options
