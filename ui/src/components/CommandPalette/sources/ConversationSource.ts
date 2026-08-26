import { getConvDisplayState } from '../../../api';
import type { Conversation, ProductConversationListRow } from '../../../api';
import type { PaletteSource, PaletteItem } from '../types';
import { fuzzyMatch } from '../fuzzyMatch';

function isConversation(value: unknown): value is Conversation {
  return typeof value === 'object' && value !== null && 'slug' in value && typeof value.slug === 'string';
}

function isProductConversation(value: unknown): value is ProductConversationListRow {
  return typeof value === 'object' && value !== null && 'canonical_route' in value && typeof value.canonical_route === 'string';
}

export function createConversationSource(
  conversations: readonly Conversation[],
  onNavigate: (slug: string) => void,
  productConversations: readonly ProductConversationListRow[] = [],
  onNavigateProduct?: (route: string) => void,
): PaletteSource {
  return {
    id: 'conversations',
    category: 'Conversations',

    search(query: string): Promise<PaletteItem[]> {
      if (productConversations.length > 0) {
        const sorted = productConversations.toSorted((a, b) => b.updated_at.localeCompare(a.updated_at));
        return Promise.resolve((query
          ? fuzzyMatch(sorted, query, row => row.presentation.display_name)
          : sorted).map(toProductItem));
      }
      const sorted = conversations.toSorted(
        (a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime(),
      );
      if (!query) return Promise.resolve(sorted.map(toItem));
      return Promise.resolve(fuzzyMatch(sorted, query, c => c.slug).map(toItem));
    },

    onSelect(item: PaletteItem) {
      if (isProductConversation(item.metadata)) {
        onNavigateProduct?.(item.metadata.canonical_route);
      } else if (isConversation(item.metadata)) {
        onNavigate(item.metadata.slug);
      }
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

function toProductItem(row: ProductConversationListRow): PaletteItem {
  return {
    id: row.product_conversation_id,
    title: row.presentation.display_name,
    ...(row.canonical_root.title ?? row.canonical_root.slug
      ? { subtitle: row.canonical_root.title ?? row.canonical_root.slug ?? '' }
      : {}),
    category: 'Conversations',
    sourceId: 'conversations',
    metadata: row,
  };
}

// Re-export helper for rendering state in the component
export function getConversationState(item: PaletteItem): string {
  if (isProductConversation(item.metadata)) return item.metadata.presentation.kind;
  return isConversation(item.metadata) ? getConvDisplayState(item.metadata) : getConvDisplayState(undefined);
}
