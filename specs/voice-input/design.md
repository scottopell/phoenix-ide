# Voice Input Design

## Technology Analysis

### Browser APIs

Two primary approaches for voice input:

1. **Web Speech API** (SpeechRecognition)
   - Direct access to speech recognition
   - Real-time transcription events
   - Language and grammar configuration
   - Browser support: Chrome, Edge, Safari (limited)
   - Requires HTTPS in production

2. **Native Keyboard Voice Input**
   - Built into mobile keyboards (iOS/Android)
   - No API access or configuration needed
   - Appears as regular text input
   - Universal support on mobile
   - No permission dialogs

### Implementation Strategy

**Progressive Enhancement Approach:**
- Detect Web Speech API availability
- Show microphone button only when API is available
- Always support native keyboard voice input
- Graceful fallback for unsupported browsers

## Architecture

### Component Structure

```
components/
  VoiceInput/
    VoiceButton.tsx        # Microphone button with state indicator
    VoiceRecorder.tsx      # Recording logic and state management
    VoicePermission.tsx    # Permission request and error display
    index.ts              # Public exports
  InputArea.tsx           # Modified to integrate VoiceRecorder
```

### State Management

**Voice Input States:**

```typescript
type VoiceState = 
  | 'idle'          // Ready to record
  | 'requesting'    // Requesting permission
  | 'listening'     // Actively recording
  | 'processing'    // Converting speech to text
  | 'error'         // Error occurred

interface VoiceError {
  type: 'permission' | 'not-supported' | 'unknown';
  message: string;
  recoverable: boolean;
}
```

### Integration Points

**REQ-VOICE-006**: Integration with Message Composition

1. **VoiceRecorder** component manages:
   - Speech recognition lifecycle
   - Transcription events
   - Error handling

2. **InputArea** component integrates by:
   - Rendering VoiceButton when supported
   - Receiving transcribed text via callback
   - Appending to existing draft
   - Managing focus and cursor position

3. **State coordination**:
   - Voice recording disables send button
   - Draft text persists across recording sessions
   - Images remain attached during voice input

## Technical Decisions

### Speech Recognition Configuration

**REQ-VOICE-002**: Recording State Feedback

```typescript
interface RecognitionConfig {
  continuous: true,         // Keep listening for multiple sentences
  interimResults: true,     // Show text as speaking
  maxAlternatives: 1,       // Use best match only
  lang: navigator.language  // Use browser language
}
```

### Visual Feedback Design

**REQ-VOICE-002**: Recording State Feedback

1. **Idle State**: Standard microphone icon
2. **Listening State**: 
   - Microphone icon with pulsing animation
   - Subtle background color change
   - Audio level indicator (if available)
3. **Processing State**: 
   - Loading spinner replacing microphone
   - "Processing..." text
4. **Error State**:
   - Red microphone with exclamation
   - Error message below button

### Error Recovery Strategy

**REQ-VOICE-005**: Error Handling

```typescript
interface ErrorRecovery {
  'permission': {
    action: 'Show permission guide',
    retriable: true,
    fallback: 'Use keyboard voice input'
  },
  'not-supported': {
    action: 'Hide voice button',
    retriable: false,
    fallback: 'Use native keyboard'
  }
}
```

### Mobile Optimizations

**REQ-VOICE-007**: Mobile Optimization

1. **Keyboard Management**:
   - Blur input field when recording starts
   - Restore focus after transcription
   - Prevent keyboard popup during recording

2. **Touch Target**:
   - Minimum 44x44px button size
   - Positioned for thumb reach
   - Clear tap feedback

3. **Interruption Handling**:
   - Save partial transcription on pause
   - Auto-resume when possible
   - Clear state indication

### Privacy Implementation

**REQ-VOICE-008**: Privacy and Clarity

1. **Disclosure**:
   - First-use tooltip explaining voice processing
   - Link to privacy information
   - Clear permission dialog text

2. **Data Handling**:
   - No audio storage
   - Transcription only

3. **Visual Design**:
   - Standard microphone icon (🎙️)
   - ARIA labels for accessibility
   - High contrast states

## API Integration

No backend changes required - voice input produces text that follows the existing message flow:

1. Voice → Text (client-side)
2. Text → API (existing POST /messages)
3. Response handling (unchanged)

## Browser Compatibility Matrix

| Browser | Web Speech API | Native Voice | Fallback Strategy |
|---------|---------------|--------------|------------------|
| Chrome Desktop | ✅ Full | ❌ | Show voice button |
| Chrome Mobile | ✅ Full | ✅ | Both options |
| Safari Desktop | ⚠️ Limited | ❌ | Feature detection |
| Safari Mobile | ⚠️ Limited | ✅ | Prefer native |
| Firefox | ❌ | ⚡ OS-dependent | Hide button |
| Edge | ✅ Full | ⚡ Windows | Show voice button |

## Performance Considerations

1. **Lazy Loading**: Load speech recognition only when requested
2. **Continuous Recording**: No artificial time limits - user controls when to stop
3. **Memory Cleanup**: Properly dispose of recognition instances
4. **Network Efficiency**: Batch interim results updates

## Accessibility

**REQ-VOICE-008**: Privacy and Clarity

1. **Keyboard Navigation**:
   - Tab-accessible microphone button
   - Escape key cancels recording
   - Enter confirms transcription

2. **Screen Reader Support**:
   - Announce state changes
   - Read transcribed text
   - Clear button labels

3. **Visual Indicators**:
   - High contrast active states
   - Motion-reduced alternatives
   - Clear error messages
