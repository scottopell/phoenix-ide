# Voice Input - Executive Summary

## Requirements Summary

Voice input enables users to dictate messages instead of typing, particularly valuable on mobile devices. The system provides a microphone button that activates speech recognition with clear visual feedback for recording and processing states. Transcribed text appears in the input field for review and editing before sending. The feature handles permissions and browser limitations gracefully. On mobile, it attempts to prevent keyboard conflicts and handles interruptions. The implementation seamlessly integrates with existing message composition, preserving drafts and image attachments.

## Technical Summary

Implementation uses the Web Speech API where available, with automatic fallback to native keyboard voice input. A progressive enhancement approach shows the microphone button only on supported browsers. The architecture introduces a VoiceRecorder component that manages speech recognition lifecycle and integrates with InputArea via callbacks. Visual states (idle, listening, processing, error) provide clear feedback. Mobile optimizations include keyboard suppression during recording and touch-friendly controls. No backend changes needed - voice produces text that follows existing message API flow. Browser compatibility varies, with Chrome offering full support and Firefox users relying on native keyboard features.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-VOICE-001:** Voice Dictation Activation | ✅ Complete | `VoiceButton.tsx`; `isWebSpeechSupported()` hides button on unsupported browsers |
| **REQ-VOICE-002:** Recording State Feedback | ✅ Complete | `VoiceState` union, pulse animation CSS, processing spinner in `VoiceRecorder.tsx` |
| **REQ-VOICE-003:** Transcription Review | ✅ Complete | `voiceBase`/`voiceInterim` state in `InputArea.tsx` renders transcription as editable draft |
| **REQ-VOICE-004:** Stop Recording Control | ✅ Complete | `recognition.abort()` on button re-tap; Escape key handler |
| **REQ-VOICE-005:** Error Handling | ✅ Complete | `VoicePermission.tsx` with `permission`/`not-supported`/`unknown` error types and retry |
| **REQ-VOICE-006:** Integration with Message Composition | ✅ Complete | `voiceBase` appended to existing draft on stop; cleared on send; image attachments preserved |
| **REQ-VOICE-007:** Mobile Optimization | ⚠️ Manual verification only | CSS has `@media (max-width: 480px)` and 44×44px touch target; keyboard suppression and orientation/interruption handling not found in code |
| **REQ-VOICE-008:** Voice Ownership During Late Expansion Recovery | Candidate — hosted verification pending | `InputArea` captures final + interim text and retires the submitted recorder; `VoiceRecorder` guards retired callbacks. Recovery produces an ordinary editable draft under REQ-IR-009. |

**Progress:** 6 of 8 complete (1 partial, 1 candidate)

## Candidate Verification and Scope

The implementation owner reports 83 focused tests plus a draft-lifetime regression passing. Hosted verification is pending; this is not a shipped or production-acceptance claim. Candidate coverage includes late expansion rejection, newer drafts and attachments, independent recording, retired callbacks, and scope/lifetime/revision guards. Transport retry queues and Close behavior are unchanged.

## Adjacent TODOs — Outside This Candidate

- External draft insertion during an active recording can be overwritten by the existing voice-end composition path. Investigate separately; this candidate only fences submitted/retired recordings and late expansion recovery.
- The existing asynchronous `addImages` path uses a captured attachment list and can lose a concurrent attachment update. Investigate separately; no attachment-upload redesign is included.
