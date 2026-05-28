import type { ErrorKind } from './generated/ErrorKind';
import type { ErrorPresentation } from './generated/ErrorPresentation';
export type { ErrorPresentation } from './generated/ErrorPresentation';

export function getErrorPresentation(errorKind?: ErrorKind): ErrorPresentation | undefined {
  if (!errorKind) return undefined;
  switch (errorKind) {
    case 'auth':
      return { kind: errorKind, can_auto_retry: false, can_user_resume: true };
    case 'rate_limit':
    case 'network':
    case 'server_error':
    case 'timed_out':
      return { kind: errorKind, can_auto_retry: true, can_user_resume: true };
    case 'server_overloaded':
      return { kind: errorKind, can_auto_retry: false, can_user_resume: true };
    case 'usage_limit_reached':
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
