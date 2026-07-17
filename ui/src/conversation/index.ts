export { ConversationProvider } from './ConversationProvider';
export { ConversationStore } from './ConversationStore';
export { useConversationsRefresh } from './useConversationsRefresh';
export { useCreateConversationWithStore } from './useCreateConversationWithStore';
export {
  useConversationAtom,
  useConversationView,
  useConversationEventCursorRef,
  useLastSseEventAt,
  useLastSseEventAtRef,
  useTranscriptGeneration,
  useConversationSnapshot,
  useConversationsList,
  useConversationSelectors,
  useWorkScope,
} from './useConversationAtom';
export type { ConversationPageView } from './useConversationAtom';
export { conversationReducer, createInitialAtom } from './atom';
export type {
  ConversationAtom,
  SSEAction,
  InitPayload,
  StreamingBuffer,
  UIError,
} from './atom';

const createIntentByConversationId = new Map<string, { prompt: string | null }>();

export function rememberCreateIntent(conversationId: string, prompt: string | null): void {
  createIntentByConversationId.set(conversationId, { prompt });
}

export function readCreateIntent(conversationId: string | null | undefined): { prompt: string | null } | null {
  if (!conversationId) return null;
  return createIntentByConversationId.get(conversationId) ?? null;
}

export function clearCreateIntent(conversationId: string | null | undefined): void {
  if (!conversationId) return;
  createIntentByConversationId.delete(conversationId);
}
