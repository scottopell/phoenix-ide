import type { ContentBlock, Message } from '../api';

export function getMessageMarkdown(message: Message): string {
  const type = message.message_type || (message as unknown as Record<string, unknown>)['type'];

  if (type === 'user') {
    const content = message.content as { text?: string };
    return content.text || (typeof message.content === 'string' ? message.content : '');
  }

  if (type === 'agent') {
    const blocks = Array.isArray(message.content) ? (message.content as ContentBlock[]) : [];
    return blocks
      .filter((block) => block.type === 'text' && block.text)
      .map((block) => block.text!)
      .join('\n\n');
  }

  return '';
}
