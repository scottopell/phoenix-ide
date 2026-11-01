import type { InputOrigin } from '../api';

function remoteConversationTitle(
  label: string,
  origin: Extract<InputOrigin, { kind: 'remote_conversation' | 'remote_coordinator' }>,
): string {
  const sourceCall = origin.source_call
    ? ` · source message ${origin.source_call.message_id} · tool call ${origin.source_call.tool_use_id}`
    : ' · source call unavailable';
  return `${label} · instance ID ${origin.instance_id} · conversation ID ${origin.product_conversation_id} · transcript ID ${origin.transcript_id}${sourceCall}`;
}

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
    case 'remote_api':
      return {
        label: 'Remote API',
        title: `Remote API · instance ID ${origin.instance_id}`,
        className: 'meta',
      };
    case 'remote_conversation':
      return {
        label: 'From remote conversation',
        title: remoteConversationTitle('From remote conversation', origin),
        className: 'meta',
      };
    case 'remote_coordinator':
      return {
        label: 'From remote Coordinator',
        title: remoteConversationTitle('From remote Coordinator', origin),
        className: 'meta',
      };
    case 'slack_gateway':
      return { label: 'Slack request', title: `Slack request ${origin.request_id}`, className: 'user' };
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
