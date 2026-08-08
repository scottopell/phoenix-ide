import { api } from '../../../api';
import type { ConversationContentSearchHit } from '../../../api';
import type {
  ConversationContentPaletteItem,
  PaletteItem,
  PaletteSource,
} from '../types';

const SEARCH_LIMIT = 20;

function toConversationContentItem(hit: ConversationContentSearchHit): ConversationContentPaletteItem {
  const item: ConversationContentPaletteItem = {
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
): PaletteSource<'conversation-content', ConversationContentSearchHit> {
  return {
    id: 'conversation-content',
    category: 'Conversation content',

    async search(
      query: string,
      signal?: AbortSignal,
    ): Promise<ConversationContentPaletteItem[]> {
      if (!query.trim()) return [];
      const response = await api.searchConversationContent(query, SEARCH_LIMIT, signal);
      return response.hits.map(toConversationContentItem);
    },

    onSelect(item: PaletteItem<'conversation-content', ConversationContentSearchHit>) {
      if (item.metadata) onNavigate(item.metadata.slug);
    },
  };
}
