import { AlertCircle, X } from 'lucide-react';
import type { VoiceError } from './VoiceRecorder';

interface VoicePermissionProps {
  error: VoiceError;
  onRetry: () => void;
  onDismiss: () => void;
}

export function VoicePermission({ error, onRetry, onDismiss }: VoicePermissionProps) {
  // Don't show error UI for non-supported browsers - just hide the button
  if (error.type === 'not-supported') {
    return null;
  }

  return (
    <div className="voice-permission-overlay">
      <div className="voice-permission-dialog">
        <button
          className="voice-permission-close"
          onClick={onDismiss}
          aria-label="Dismiss error"
        >
          <X size={18} />
        </button>
        
        <div className="voice-permission-icon">
          <AlertCircle size={32} />
        </div>
        
        <h3 className="voice-permission-title">
          {error.type === 'permission' ? 'Microphone Access Needed' : 'Voice Input Error'}
        </h3>
        
        <p className="voice-permission-message">
          {error.message}
        </p>
        
        {error.type === 'permission' && (
          <div className="voice-permission-instructions">
            <p>To enable voice input:</p>
            <ol>
              <li>Click the lock/info icon in your browser's address bar</li>
              <li>Find "Microphone" in the permissions list</li>
              <li>Change it to "Allow"</li>
              <li>Click "Retry" below</li>
            </ol>
          </div>
        )}
        
        <div className="voice-permission-actions">
          <button
            className="voice-permission-btn voice-permission-btn--secondary"
            onClick={onDismiss}
          >
            Cancel
          </button>
          {error.recoverable && (
            <button
              className="voice-permission-btn voice-permission-btn--primary"
              onClick={onRetry}
            >
              Retry
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
