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
    case 'remote_api':
      return {
        label: `API · instance ${origin.instance_id}`,
        title: `API-channel input from authenticated instance ${origin.instance_id}`,
        className: 'meta',
      };
    case 'remote_conversation':
    case 'remote_coordinator': {
      const sender = origin.kind === 'remote_coordinator' ? 'remote Coordinator' : 'remote conversation';
      const label = `From ${sender} ID ${origin.product_conversation_id} · instance ${origin.instance_id} · transcript ID ${origin.transcript_id}`;
      return {
        label,
        title: `${label} · ${origin.source_call ? `source message ${origin.source_call.message_id} · tool ${origin.source_call.tool_use_id}` : 'source call unavailable (not recorded)'}`,
        className: 'meta',
      };
    }
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
