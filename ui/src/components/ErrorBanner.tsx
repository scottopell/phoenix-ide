import { AlertCircle } from 'lucide-react';
import type { ReactNode } from 'react';
import { useCodexQuota } from '../codexQuota';
import type { QuotaDetails } from '../sseSchemas';
import { CodexQuotaBlock } from './CodexQuotaBlock';

interface ErrorBannerProps {
  message: string;
  errorKind?: string | undefined;
  onRetry: () => void;
  onDismiss: () => void;
}

/**
 * Parse and humanize an error message from the backend.
 * The backend often returns JSON-formatted error strings.
 *
 * For `usage_limit_reached` the backend already supplies a plan-aware
 * string that mirrors codex CLI's `UsageLimitReachedError::fmt`
 * verbatim — including the upgrade / usage-page URLs and the reset
 * time. We pass it through unmodified and let `linkify` turn the URLs
 * into anchors at render time.
 */
function humanizeError(
  message: string,
  errorKind?: string,
): { title: string; details: string | null } {
  if (errorKind === 'usage_limit_reached') {
    return { title: 'Usage limit reached', details: message };
  }

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
    details: message,
  };
}

// Split a string on http/https URLs and return an array of strings and
// anchor elements. Keeps trailing punctuation outside the link — codex
// CLI's strings end sentences with URLs ("…purchase more credits.") and
// a greedy match would swallow the period into the href.
const URL_RE = /(https?:\/\/[^\s)\]<>]+[^\s)\].,;:!?<>])/g;

function linkify(text: string): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  for (const match of text.matchAll(URL_RE)) {
    const start = match.index ?? 0;
    if (start > last) out.push(text.slice(last, start));
    const url = match[0];
    out.push(
      <a key={`${start}-${url}`} href={url} target="_blank" rel="noopener noreferrer">
        {url}
      </a>,
    );
    last = start + url.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

// Suffix the title with the limit's display name when codex reports a
// non-default limit family (e.g. `gpt-5.2-codex-sonic`). The string in
// `message` already mentions this, but a structured title is clearer
// — multi-model plans want to know exactly which limit they hit.
function titleForUsageLimit(quota: QuotaDetails | null): string {
  const name = quota?.limit_name?.trim();
  if (name) return `Usage limit reached — ${name}`;
  return 'Usage limit reached';
}

export function ErrorBanner({ message, errorKind, onRetry, onDismiss }: ErrorBannerProps) {
  const codexQuota = useCodexQuota();

  // `usage_limit_reached` is terminal — retry will hit the same wall.
  // The actionable next step is switching models or waiting until the
  // window resets, not re-sending the same prompt.
  const isUsageLimit = errorKind === 'usage_limit_reached';
  const isRetryable =
    !isUsageLimit &&
    (!errorKind ||
      errorKind === 'unknown' ||
      ['rate_limit', 'overloaded', 'network', 'timeout'].includes(errorKind));

  const { title, details } = humanizeError(message, errorKind);
  // For a usage-limit error, prefer a structured title that includes
  // the active limit's display name when present.
  const displayTitle = isUsageLimit ? titleForUsageLimit(codexQuota) : title;

  return (
    <div className="error-input-area">
      {/* Error body — mirrors the conversation area */}
      <div className="error-body">
        <div className="error-body-icon">
          <AlertCircle size={20} />
        </div>
        <div className="error-body-content">
          <div className="error-body-title">{displayTitle}</div>
          {details && <div className="error-body-details">{linkify(details)}</div>}
          {isUsageLimit && codexQuota && <CodexQuotaBlock quota={codexQuota} />}
        </div>
      </div>

      {/* Retry bar — mirrors the input actions bar */}
      <div className="error-action-bar">
        {isRetryable ? (
          <button className="error-retry-btn" onClick={onRetry}>
            ↺ Retry — sends &ldquo;continue&rdquo;
          </button>
        ) : (
          <span className="error-action-hint">
            {isUsageLimit
              ? 'Switch to a different model in the picker, or wait for the window to reset.'
              : 'Start a new conversation to continue.'}
          </span>
        )}
        <button className="error-dismiss-btn" onClick={onDismiss} title="Dismiss error">
          Dismiss
        </button>
      </div>
    </div>
  );
}
