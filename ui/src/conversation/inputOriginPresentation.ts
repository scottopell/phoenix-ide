import type { InputOrigin } from '../api';

/** API is a channel, not evidence that a human authored the input. */
export function inputOriginPresentation(origin: InputOrigin | undefined): {
  label: string;
  title: string;
  className: 'user' | 'meta';
} {
  if (!origin) return { label: 'Unknown input', title: 'Unknown input', className: 'meta' };
  switch (origin.kind) {
    case 'user_api':
      return { label: 'User · API', title: 'User · API', className: 'user' };
    case 'internal_conversation':
      return {
        label: 'From conversation',
        title: `From conversation ID ${origin.product_conversation_id} · transcript ID ${origin.transcript_id}`,
        className: 'meta',
      };
    case 'subscription_event':
      return { label: 'Conversation event', title: 'Conversation event', className: 'meta' };
    case 'system_generated':
      return { label: 'System input', title: 'System input', className: 'meta' };
    case 'unknown_historical':
      return { label: 'Unknown input', title: 'Unknown input', className: 'meta' };
    default:
      return origin satisfies never;
  }
}
