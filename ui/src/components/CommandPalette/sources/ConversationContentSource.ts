import { api } from '../../../api';
import type { ConversationContentSearchHit } from '../../../api';
import type { PaletteItem, PaletteSource } from '../types';

const SEARCH_LIMIT = 20;

function isConversationContentItem(item: PaletteItem): item is PaletteItem & { metadata: ConversationContentSearchHit } {
  const metadata = item.metadata;
  return typeof metadata === 'object'
    && metadata !== null
    && 'slug' in metadata
    && typeof metadata.slug === 'string'
    && 'message_id' in metadata
    && typeof metadata.message_id === 'string'
    && 'conversation_id' in metadata
    && typeof metadata.conversation_id === 'string';
}

function toConversationContentItem(hit: ConversationContentSearchHit): PaletteItem {
  const item: PaletteItem = {
    id: `${hit.conversation_id}:${hit.message_id}`,
    title: hit.slug,
    snippet: hit.snippet,
    category: 'Conversation content',
    sourceId: 'conversation-content',
    score: hit.score,
    metadata: hit,
  };
  if (hit.archived) item.badge = 'Archived';
  return item;
}

export function createConversationContentSource(
  onNavigate: (slug: string) => void,
): PaletteSource {
  return {
    id: 'conversation-content',
    category: 'Conversation content',

    async search(query: string, signal?: AbortSignal): Promise<PaletteItem[]> {
      if (!query.trim()) return [];
      const response = await api.searchConversationContent(query, SEARCH_LIMIT, signal);
      return response.hits.map(toConversationContentItem);
    },

    onSelect(item: PaletteItem) {
      if (!isConversationContentItem(item)) return;
      onNavigate(item.metadata.slug);
    },
  };
}
