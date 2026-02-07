# Voice Input Implementation - Full Autonomous Completion

## DIRECTIVE: Complete 100% Implementation Without Pausing

You are tasked with implementing the **entire** voice input feature as specified in `/home/exedev/phoenix-ide/specs/voice-input/`. This is a single-shot, complete implementation task. Do NOT pause for feedback, do NOT implement partially, do NOT ask for clarification. The specs are complete and final.

## Implementation Checklist

You must implement ALL of these items in a single session:

### 1. Voice Components (REQ-VOICE-001, REQ-VOICE-002, REQ-VOICE-004)
- [ ] Create `ui/src/components/VoiceInput/VoiceButton.tsx`
  - Microphone icon button with all visual states (idle, listening, processing)
  - Pulsing animation during recording
  - Click to start/stop recording
- [ ] Create `ui/src/components/VoiceInput/VoiceRecorder.tsx`
  - Web Speech API integration with feature detection
  - State management (idle → listening → processing → idle)
  - Continuous recording with no time limits
  - Handle stop via button click, escape key, or outside click
- [ ] Create `ui/src/components/VoiceInput/index.ts` with exports

### 2. InputArea Integration (REQ-VOICE-003, REQ-VOICE-006)
- [ ] Modify `ui/src/components/InputArea.tsx`
  - Import and render VoiceButton when Web Speech API is supported
  - Add callback to receive transcribed text
  - Append transcribed text to END of existing draft
  - Position cursor after new text
  - Preserve images during voice input

### 3. Error Handling (REQ-VOICE-005)
- [ ] Create `ui/src/components/VoiceInput/VoicePermission.tsx`
  - Permission denied error with retry button
  - Browser not supported (hide button)
  - Generic speech recognition errors
  - NO network error handling (that's for message sending)

### 4. Mobile Optimizations (REQ-VOICE-007)
- [ ] Keyboard suppression: blur input field during recording
- [ ] Touch-friendly button (minimum 44x44px)
- [ ] Handle device rotation without interrupting recording
- [ ] Handle interruptions gracefully

### 5. Styling
- [ ] Add CSS for all voice components
- [ ] Pulsing animation for recording state
- [ ] Smooth transitions between states
- [ ] High contrast for accessibility
- [ ] Mobile-responsive sizing

### 6. Testing & Verification
- [ ] Use browser tools to test on desktop (Chrome/Edge will have Web Speech API)
- [ ] Test all state transitions
- [ ] Test error cases (deny permission)
- [ ] Test integration with existing draft text
- [ ] Test that images are preserved
- [ ] Take screenshots of all states

## Technical Requirements

```typescript
// Core types you must implement
type VoiceState = 'idle' | 'requesting' | 'listening' | 'processing' | 'error';

interface VoiceError {
  type: 'permission' | 'not-supported' | 'unknown';
  message: string;
  recoverable: boolean;
}

// Speech Recognition config
const recognition = new (window.SpeechRecognition || window.webkitSpeechRecognition)();
recognition.continuous = true;
recognition.interimResults = true;
recognition.maxAlternatives = 1;
recognition.lang = navigator.language;
```

## Development Process

1. **Start the dev server**: Use `python3 ui/dev.py` to run the development server
2. **Use the browser**: Navigate to `https://meteor-rain.exe.xyz:8000` to test
3. **Take screenshots**: Document each visual state with browser screenshots
4. **Test thoroughly**: Use browser DevTools to verify all functionality

## DO NOT

- Do NOT implement partially and ask "should I continue?"
- Do NOT skip error handling or mobile optimizations
- Do NOT add features not in the spec (no audio visualization, no language selection)
- Do NOT create a PR or ask for review - implement everything
- Do NOT implement network error handling for voice (that's message sending's job)

## EXPECTED OUTCOME

After your implementation:
1. Users can click a microphone button to start voice recording
2. Visual feedback shows recording is active
3. Speech is transcribed and appended to the message input
4. Users can stop recording multiple ways
5. All errors are handled gracefully
6. Mobile experience is optimized
7. The feature is 100% complete and tested

## Final Commit

Create a single commit with message:
```
Implement voice input feature (REQ-VOICE-001 through REQ-VOICE-007)

- Add VoiceButton, VoiceRecorder, and VoicePermission components
- Integrate with InputArea for seamless message composition
- Handle all error states gracefully
- Optimize for mobile with keyboard suppression
- Support unlimited recording duration
- Test coverage for all user flows
```

Now proceed with COMPLETE implementation. Show me the working voice input feature with screenshots of all states.