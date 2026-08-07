import { getConvDisplayState } from '../../../api';
import type { Conversation } from '../../../api';
import type { PaletteSource, PaletteItem } from '../types';
import { fuzzyMatch } from '../fuzzyMatch';

function isConversation(value: unknown): value is Conversation {
  return typeof value === 'object' && value !== null && 'slug' in value && typeof value.slug === 'string';
}

export function createConversationSource(
  conversations: readonly Conversation[],
  onNavigate: (slug: string) => void,
): PaletteSource {
  return {
    id: 'conversations',
    category: 'Conversations',

    search(query: string): Promise<PaletteItem[]> {
      const sorted = conversations.toSorted(
        (a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime(),
      );
      if (!query) {
        return Promise.resolve(sorted.map(toItem));
      }
      return Promise.resolve(fuzzyMatch(sorted, query, c => c.slug).map(toItem));
    },

    onSelect(item: PaletteItem) {
      if (!isConversation(item.metadata)) return;
      onNavigate(item.metadata.slug);
    },
  };
}

function toItem(conv: Conversation): PaletteItem {
  return {
    id: conv.id,
    title: conv.slug,
    subtitle: conv.cwd,
    category: 'Conversations',
    sourceId: 'conversations',
    metadata: conv,
  };
}

// Re-export helper for rendering state in the component
export function getConversationState(item: PaletteItem): string {
  return isConversation(item.metadata) ? getConvDisplayState(item.metadata) : getConvDisplayState(undefined);
}
