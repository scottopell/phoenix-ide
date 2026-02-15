import { AlertCircle } from 'lucide-react';

interface ErrorBannerProps {
  message: string;
  errorKind?: string;
  onRetry: () => void;
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
      
      // Map error types to user-friendly titles
      const titles: Record<string, string> = {
        'api_error': 'API Error',
        'rate_limit_error': 'Rate Limited',
        'overloaded_error': 'Service Overloaded',
        'invalid_request_error': 'Invalid Request',
        'authentication_error': 'Authentication Failed',
      };
      
      return {
        title: titles[errorType] || 'Server Error',
        details: errorMsg,
      };
    }
    
    // Generic parsed error
    if (parsed.message) {
      return { title: 'Error', details: parsed.message };
    }
  } catch {
    // Not JSON, use as-is
  }
  
  // Check for common error patterns
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
  
  // Fallback
  return {
    title: 'Error',
    details: message.length > 200 ? message.slice(0, 200) + '...' : message,
  };
}

export function ErrorBanner({ message, errorKind, onRetry }: ErrorBannerProps) {
  const { title, details } = humanizeError(message);
  
  // Determine if this is likely retryable
  const isRetryable = !errorKind || errorKind === 'unknown' || 
    ['rate_limit', 'overloaded', 'network', 'timeout'].includes(errorKind);

  return (
    <div className="error-banner">
      <div className="error-banner-icon">
        <AlertCircle size={24} />
      </div>
      <div className="error-banner-content">
        <div className="error-banner-title">{title}</div>
        {details && <div className="error-banner-details">{details}</div>}
        <div className="error-banner-hint">
          The agent stopped due to this error. Send a message to continue the conversation.
        </div>
      </div>
      {isRetryable && (
        <button className="error-banner-retry" onClick={onRetry}>
          Retry
        </button>
      )}
    </div>
  );
}
