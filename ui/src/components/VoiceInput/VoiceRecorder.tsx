import { useState, useEffect, useCallback, useRef } from 'react';
import { VoiceButton } from './VoiceButton';
import { VoicePermission } from './VoicePermission';

export type VoiceState = 'idle' | 'requesting' | 'listening' | 'processing' | 'error';

export interface VoiceError {
  type: 'permission' | 'not-supported' | 'unknown';
  message: string;
  recoverable: boolean;
}

// Web Speech API type definitions
interface SpeechRecognitionResult {
  readonly isFinal: boolean;
  readonly length: number;
  item(index: number): SpeechRecognitionAlternative;
  [index: number]: SpeechRecognitionAlternative;
}

interface SpeechRecognitionAlternative {
  readonly transcript: string;
  readonly confidence: number;
}

interface SpeechRecognitionResultList {
  readonly length: number;
  item(index: number): SpeechRecognitionResult;
  [index: number]: SpeechRecognitionResult;
}

interface SpeechRecognitionEvent extends Event {
  readonly resultIndex: number;
  readonly results: SpeechRecognitionResultList;
}

interface SpeechRecognitionErrorEvent extends Event {
  readonly error: string;
  readonly message: string;
}

interface SpeechRecognitionInstance extends EventTarget {
  continuous: boolean;
  interimResults: boolean;
  maxAlternatives: number;
  lang: string;
  onstart: ((this: SpeechRecognitionInstance, ev: Event) => void) | null;
  onend: ((this: SpeechRecognitionInstance, ev: Event) => void) | null;
  onerror: ((this: SpeechRecognitionInstance, ev: SpeechRecognitionErrorEvent) => void) | null;
  onresult: ((this: SpeechRecognitionInstance, ev: SpeechRecognitionEvent) => void) | null;
  start(): void;
  stop(): void;
  abort(): void;
}

interface SpeechRecognitionConstructor {
  new (): SpeechRecognitionInstance;
}

// Declare types for Web Speech API on window
declare global {
  interface Window {
    SpeechRecognition?: SpeechRecognitionConstructor;
    webkitSpeechRecognition?: SpeechRecognitionConstructor;
  }
}

// Check if Web Speech API is available
export function isWebSpeechSupported(): boolean {
  return !!(
    typeof window !== 'undefined' &&
    (window.SpeechRecognition || window.webkitSpeechRecognition)
  );
}

interface VoiceRecorderProps {
  onTranscript: (text: string) => void;
  disabled?: boolean;
}

export function VoiceRecorder({ onTranscript, disabled }: VoiceRecorderProps) {
  const [state, setState] = useState<VoiceState>('idle');
  const [error, setError] = useState<VoiceError | null>(null);
  const [interimText, setInterimText] = useState('');
  
  const recognitionRef = useRef<SpeechRecognitionInstance | null>(null);
  const finalTranscriptRef = useRef('');
  const containerRef = useRef<HTMLDivElement>(null);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (recognitionRef.current) {
        recognitionRef.current.abort();
        recognitionRef.current = null;
      }
    };
  }, []);

  // Handle escape key and outside click
  useEffect(() => {
    if (state !== 'listening') return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        stopRecording();
      }
    };

    const handleClickOutside = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        stopRecording();
      }
    };

    document.addEventListener('keydown', handleKeyDown);
    document.addEventListener('mousedown', handleClickOutside);

    return () => {
      document.removeEventListener('keydown', handleKeyDown);
      document.removeEventListener('mousedown', handleClickOutside);
    };
  }, [state]);

  const createRecognition = useCallback((): SpeechRecognitionInstance => {
    const SpeechRecognitionClass = window.SpeechRecognition || window.webkitSpeechRecognition;
    if (!SpeechRecognitionClass) {
      throw new Error('Speech recognition not supported');
    }
    const recognition = new SpeechRecognitionClass();

    recognition.continuous = true;
    recognition.interimResults = true;
    recognition.maxAlternatives = 1;
    recognition.lang = navigator.language || 'en-US';

    recognition.onstart = () => {
      setState('listening');
      setError(null);
      finalTranscriptRef.current = '';
      setInterimText('');
    };

    recognition.onresult = (event: SpeechRecognitionEvent) => {
      let interim = '';
      let final = '';

      for (let i = event.resultIndex; i < event.results.length; i++) {
        const result = event.results[i];
        if (result.isFinal) {
          final += result[0].transcript;
        } else {
          interim += result[0].transcript;
        }
      }

      if (final) {
        finalTranscriptRef.current += final;
      }
      setInterimText(interim);
    };

    recognition.onerror = (event: SpeechRecognitionErrorEvent) => {
      console.error('Speech recognition error:', event.error);

      let voiceError: VoiceError;
      switch (event.error) {
        case 'not-allowed':
        case 'permission-denied':
          voiceError = {
            type: 'permission',
            message: 'Microphone access was denied. Please allow microphone access in your browser settings.',
            recoverable: true,
          };
          break;
        case 'not-supported':
          voiceError = {
            type: 'not-supported',
            message: 'Voice input is not supported in this browser.',
            recoverable: false,
          };
          break;
        case 'aborted':
          // User stopped - not an error
          return;
        case 'no-speech':
          // No speech detected - just stop gracefully
          setState('idle');
          return;
        default:
          voiceError = {
            type: 'unknown',
            message: `Speech recognition error: ${event.error}`,
            recoverable: true,
          };
      }

      setError(voiceError);
      setState('error');
    };

    recognition.onend = () => {
      // Send final transcript when recording ends
      const transcript = finalTranscriptRef.current.trim();
      if (transcript) {
        onTranscript(transcript);
      }
      
      // Only reset to idle if we're not in error state
      setState(prev => prev === 'error' ? 'error' : 'idle');
      setInterimText('');
      recognitionRef.current = null;
    };

    return recognition;
  }, [onTranscript]);

  const startRecording = useCallback(async () => {
    if (!isWebSpeechSupported()) {
      setError({
        type: 'not-supported',
        message: 'Voice input is not supported in this browser.',
        recoverable: false,
      });
      setState('error');
      return;
    }

    setState('requesting');
    setError(null);

    try {
      // Request microphone permission first
      await navigator.mediaDevices.getUserMedia({ audio: true });
      
      const recognition = createRecognition();
      recognitionRef.current = recognition;
      recognition.start();
    } catch (err) {
      console.error('Failed to start speech recognition:', err);
      setError({
        type: 'permission',
        message: 'Could not access microphone. Please check your browser permissions.',
        recoverable: true,
      });
      setState('error');
    }
  }, [createRecognition]);

  const stopRecording = useCallback(() => {
    if (recognitionRef.current) {
      setState('processing');
      recognitionRef.current.stop();
    }
  }, []);

  const handleButtonClick = useCallback(() => {
    if (state === 'listening') {
      stopRecording();
    } else if (state === 'idle' || state === 'error') {
      startRecording();
    }
  }, [state, startRecording, stopRecording]);

  const handleRetry = useCallback(() => {
    setError(null);
    setState('idle');
    startRecording();
  }, [startRecording]);

  const handleDismissError = useCallback(() => {
    setError(null);
    setState('idle');
  }, []);

  // Don't render if not supported
  if (!isWebSpeechSupported()) {
    return null;
  }

  return (
    <div ref={containerRef} className="voice-recorder">
      <VoiceButton
        state={state}
        onClick={handleButtonClick}
        disabled={disabled}
      />
      
      {interimText && state === 'listening' && (
        <div className="voice-interim">
          <span className="voice-interim-text">{interimText}</span>
        </div>
      )}

      {error && (
        <VoicePermission
          error={error}
          onRetry={handleRetry}
          onDismiss={handleDismissError}
        />
      )}
    </div>
  );
}
