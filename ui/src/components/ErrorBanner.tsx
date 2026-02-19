import { AlertCircle, RotateCcw } from 'lucide-react';

interface ErrorBannerProps {
  message: string;
  errorKind?: string;
  onRetry: () => void;
  onDismiss: () => void;
}

/**
 * Parse and humanize an error message from the backend.
 * The backend often returns JSON-formatted error strings.
 */
function humanizeError(message: string): { title: string; details: string | null } {
  // Try to parse as JSON (backend often wraps errors)
  try {
    const parsed = JSON.parse(message);

    // Anthropic-style error
    if (parsed.type === 'error' && parsed.error) {
      const errorType = parsed.error.type || 'unknown_error';
      const errorMsg = parsed.error.message || 'An error occurred';

      const titles: Record<string, string> = {
        api_error: 'API Error',
        rate_limit_error: 'Rate Limited',
        overloaded_error: 'Service Overloaded',
        invalid_request_error: 'Invalid Request',
        authentication_error: 'Authentication Failed',
      };

      return {
        title: titles[errorType] || 'Server Error',
        details: errorMsg,
      };
    }

    if (parsed.message) {
      return { title: 'Error', details: parsed.message };
    }
  } catch {
    // Not JSON, use as-is
  }

  if (message.includes('Internal server error')) {
    return {
      title: 'Server Error',
      details: 'The AI service encountered an internal error. This is usually temporary.',
    };
  }

  if (message.includes('rate limit') || message.includes('Rate limit')) {
    return {
      title: 'Rate Limited',
      details: 'Too many requests. Please wait a moment before retrying.',
    };
  }

  if (message.includes('timeout') || message.includes('Timeout')) {
    return {
      title: 'Request Timeout',
      details: 'The request took too long to complete.',
    };
  }

  return {
    title: 'Error',
    details: message.length > 200 ? message.slice(0, 200) + '…' : message,
  };
}

export function ErrorBanner({ message, errorKind, onRetry, onDismiss }: ErrorBannerProps) {
  const { title, details } = humanizeError(message);

  const isRetryable =
    !errorKind ||
    errorKind === 'unknown' ||
    ['rate_limit', 'overloaded', 'network', 'timeout'].includes(errorKind);

  return (
    <div className="error-input-area">
      {/* Error body — mirrors the conversation area */}
      <div className="error-body">
        <div className="error-body-icon">
          <AlertCircle size={20} />
        </div>
        <div className="error-body-content">
          <div className="error-body-title">{title}</div>
          {details && <div className="error-body-details">{details}</div>}
        </div>
      </div>

      {/* Retry bar — mirrors the input actions bar */}
      <div className="error-action-bar">
        {isRetryable ? (
          <button className="error-retry-btn" onClick={onRetry}>
            <RotateCcw size={14} />
            Retry — sends &ldquo;continue&rdquo;
          </button>
        ) : (
          <span className="error-action-hint">Start a new conversation to continue.</span>
        )}
        <button className="error-dismiss-btn" onClick={onDismiss} title="Dismiss error">
          Dismiss
        </button>
      </div>
    </div>
  );
}
