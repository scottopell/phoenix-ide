import { Mic, MicOff, Loader2 } from 'lucide-react';
import type { VoiceState } from './VoiceRecorder';
import './VoiceInput.css';

interface VoiceButtonProps {
  state: VoiceState;
  onClick: () => void;
  disabled?: boolean | undefined;
}

export function VoiceButton({ state, onClick, disabled }: VoiceButtonProps) {
  const isListening = state === 'listening';
  const isProcessing = state === 'processing' || state === 'requesting';
  const isError = state === 'error';

  const getIcon = () => {
    if (isProcessing) {
      return <Loader2 size={20} className="voice-btn-spinner" />;
    }
    if (isError) {
      return <MicOff size={20} />;
    }
    return <Mic size={20} />;
  };

  const getTitle = () => {
    switch (state) {
      case 'listening':
        return 'Stop recording';
      case 'processing':
        return 'Processing speech...';
      case 'requesting':
        return 'Requesting permission...';
      case 'error':
        return 'Voice input error - click to retry';
      default:
        return 'Start voice input';
    }
  };

  return (
    <button
      type="button"
      className={`voice-btn ${
        isListening ? 'voice-btn--listening' : ''
      } ${
        isError ? 'voice-btn--error' : ''
      } ${
        isProcessing ? 'voice-btn--processing' : ''
      }`}
      onClick={onClick}
      disabled={disabled || isProcessing}
      title={getTitle()}
      aria-label={getTitle()}
      aria-pressed={isListening}
    >
      {getIcon()}
      {isListening && <span className="voice-btn-pulse" />}
    </button>
  );
}
