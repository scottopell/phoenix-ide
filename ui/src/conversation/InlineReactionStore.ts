import { createContext } from 'react';
import { RoutedStore } from './RoutedStore';

export interface ReactionSource {
  messageId: string;
  sequenceId: number;
  occurrenceToken?: string | undefined;
  quote: string;
  textOffsets?: { start: number; end: number };
}

export interface InlineReaction {
  source: ReactionSource;
  body: string;
}

type Action =
  | { type: 'select'; source: ReactionSource }
  | { type: 'edit'; body: string }
  | { type: 'clear' };

export class InlineReactionStore extends RoutedStore<string, InlineReaction | null, Action> {
  constructor() {
    super(() => null, (current, action) => {
      switch (action.type) {
        case 'select':
          return current?.body ? current : { source: action.source, body: '' };
        case 'edit':
          return current ? { ...current, body: action.body } : null;
        case 'clear':
          return null;
      }
    });
  }
}

export const InlineReactionContext = createContext<InlineReactionStore | null>(null);

export function formatInlineReaction({ source, body }: InlineReaction): string {
  const identity = source.occurrenceToken ?? source.messageId;
  const longest = Array.from(source.quote.matchAll(/`+/g)).reduce((length, match) => Math.max(length, match[0].length), 2);
  const fence = '`'.repeat(longest + 1);
  return `Regarding message #${source.sequenceId} (${identity}):\n\n${fence}text\n${source.quote}\n${fence}\n\n${body}`;
}
