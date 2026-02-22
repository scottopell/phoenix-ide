import type { Conversation } from '../../../api';
import type { PaletteSource, PaletteItem } from '../types';
import { fuzzyMatch } from '../fuzzyMatch';

export function createConversationSource(
  conversations: Conversation[],
  onNavigate: (slug: string) => void,
): PaletteSource {
  return {
    id: 'conversations',
    category: 'Conversations',

    search(query: string): PaletteItem[] {
      // Sort by updated_at descending (most recent first)
      const sorted = [...conversations].sort(
        (a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime(),
      );

      if (!query) {
        return sorted.slice(0, 10).map(toItem);
      }

      return fuzzyMatch(sorted, query, c => c.slug).slice(0, 10).map(toItem);
    },

    onSelect(item: PaletteItem) {
      const conv = item.metadata as Conversation;
      onNavigate(conv.slug);
    },
  };
}

function toItem(conv: Conversation): PaletteItem {
  return {
    id: conv.id,
    title: conv.slug,
    subtitle: conv.cwd,
    category: 'Conversations',
    metadata: conv,
  };
}

// Re-export helper for rendering state in the component
export function getConversationState(item: PaletteItem): string {
  const conv = item.metadata as Conversation | undefined;
  return conv?.display_state || 'idle';
}
