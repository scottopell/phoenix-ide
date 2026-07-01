import type { ErrorKind } from './generated/ErrorKind';
import type { ErrorPresentation } from './generated/ErrorPresentation';
export type { ErrorPresentation } from './generated/ErrorPresentation';

export function getErrorPresentation(errorKind?: ErrorKind): ErrorPresentation | undefined {
  if (!errorKind) return undefined;
  switch (errorKind) {
    case 'auth':
      return { kind: errorKind, can_auto_retry: false, can_user_resume: true };
    // invalid_response: the provider returned bytes we couldn't parse — a
    // server/transport fault, retryable and resumable like server_error.
    case 'rate_limit':
    case 'network':
    case 'server_error':
    case 'invalid_response':
    case 'timed_out':
      return { kind: errorKind, can_auto_retry: true, can_user_resume: true };
    // usage_limit_reached: a quota window resets on a clock boundary, so the
    // user can resume once it clears — user-resumable, never auto-retried.
    case 'server_overloaded':
    case 'usage_limit_reached':
    case 'output_limit_exceeded':
      return { kind: errorKind, can_auto_retry: false, can_user_resume: true };
    case 'invalid_request':
    case 'cancelled':
    case 'sub_agent_error':
    case 'context_exhausted':
    case 'content_filter':
      return { kind: errorKind, can_auto_retry: false, can_user_resume: false };
    default:
      errorKind satisfies never;
      return undefined;
  }
}
