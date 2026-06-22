import { AlertCircle } from 'lucide-react';
import type { ReactNode } from 'react';
import { useCodexQuota } from '../codexQuota';
import type { ErrorKind } from '../generated/ErrorKind';
import type { ErrorPresentation } from '../errorPresentation';

import type { QuotaDetails } from '../sseSchemas';
import { CodexQuotaBlock } from './CodexQuotaBlock';

interface ErrorBannerProps {
  message: string;
  error?: ErrorPresentation | undefined;
  onRetry?: (() => void) | undefined;
  onDismiss?: (() => void) | undefined;
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
  errorKind?: ErrorKind,
): { title: string; details: string | null } {
  if (errorKind === 'usage_limit_reached') {
    return { title: 'Usage limit reached', details: message };
  }

  if (errorKind === 'invalid_response') {
    return {
      title: 'Malformed response',
      details:
        'The provider returned a response we could not parse. This is usually temporary — retry.',
    };
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

export function ErrorBanner({ message, error, onRetry, onDismiss }: ErrorBannerProps) {
  const codexQuota = useCodexQuota();
  const errorKind = error?.kind;
  const isUsageLimit = errorKind === 'usage_limit_reached';
  const canUserResume = (error?.can_user_resume ?? false) && !!onRetry && !!onDismiss;

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
        {canUserResume ? (
          <>
            <button className="error-retry-btn" onClick={onRetry}>
              ↺ Retry — sends &ldquo;continue&rdquo;
            </button>
            {/* Dismiss returns the conversation to Idle. Offered only for
                resumable errors: a non-resumable error is a dead end (no Idle
                to return to), and the server rejects DismissError for it. */}
            <button className="error-dismiss-btn" onClick={onDismiss} title="Dismiss error">
              Dismiss
            </button>
          </>
        ) : (
          <span className="error-action-hint">
            {errorKind === 'auth'
              ? 'Refresh or fix authentication, then retry to continue this conversation.'
              : 'Start a new conversation to continue.'}
          </span>
        )}
      </div>
    </div>
  );
}
