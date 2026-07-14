import type { Message } from '../api';

export type MessageCacheWrite =
  | { kind: 'append'; messages: Message[] }
  | { kind: 'replace'; conversationId: string; messages: Message[] };

export function messageCacheWrite(
  conversationId: string,
  previousMessages: readonly Message[],
  messages: readonly Message[],
  generationChanged: boolean,
): MessageCacheWrite {
  const isPureAppend = !generationChanged
    && previousMessages.length <= messages.length
    && previousMessages.every((message, index) => messages[index] === message);
  return isPureAppend
    ? { kind: 'append', messages: messages.slice(previousMessages.length) }
    : { kind: 'replace', conversationId, messages: [...messages] };
}
