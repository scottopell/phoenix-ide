import { describe, expect, it } from 'vitest';
import type { ProductConversationListRow } from '../api';
import {
  productConversationDisplayTitle,
  productConversationOwnsRoute,
  renamedProductConversationLocation,
} from './ConversationList.presentation';

function row(overrides: Partial<ProductConversationListRow> = {}): ProductConversationListRow {
  return {
    product_conversation_id: 'product-uuid',
    canonical_route: '/c/new-name-2',
    canonical_root: {
      title: 'Old title',
      slug: 'old-name',
      transcript_row_id: 'transcript-uuid',
    } as ProductConversationListRow['canonical_root'],
    lifecycle: {} as ProductConversationListRow['lifecycle'],
    latest_transcript_row_id: 'latest-transcript-uuid',
    updated_at: '2026-10-18T00:00:00Z',
    presentation: { kind: 'state', display_name: 'Current name', presentation_mode: 'idle' },
    ...overrides,
  };
}

describe('product conversation identity presentation', () => {
  it('uses the presentation name instead of stale title or slug values', () => {
    expect(productConversationDisplayTitle(row())).toBe('Current name');
  });

  it('recognizes UUID, current slug, and transcript routes for rename refresh', () => {
    const previous = row();
    expect(productConversationOwnsRoute('/c/old-name', previous)).toBe(true);
    expect(productConversationOwnsRoute('/c/product-uuid', previous)).toBe(true);
    expect(productConversationOwnsRoute('/c/transcript-uuid', previous)).toBe(true);
    expect(productConversationOwnsRoute('/c/some-other-conversation', previous)).toBe(false);
  });

  it.each(['c', 'product-conversations'])('refreshes terminal-slash /%s routes while rejecting nested routes', (prefix) => {
    const previous = row();
    const renamed = row({ canonical_route: '/c/nouveau' });
    expect(productConversationOwnsRoute(`/${prefix}/old-name/`, previous)).toBe(true);
    expect(renamedProductConversationLocation(
      `/${prefix}/latest-transcript-uuid/`, '?keep=yes', '#message-1', previous, renamed,
    )).toEqual({ pathname: '/c/nouveau', search: '?keep=yes&source_transcript=latest-transcript-uuid', hash: '#message-1' });
    for (const path of [`/${prefix}/old-name/child`, `/${prefix}/old-name//`]) {
      expect(productConversationOwnsRoute(path, previous)).toBe(false);
      expect(renamedProductConversationLocation(path, '', '', previous, renamed)).toBeNull();
    }
  });

  it('pins the exact matched root or latest transcript when moving an ID route to a slug', () => {
    const previous = row();
    const renamed = row({ canonical_route: '/c/nouveau-nom' });
    expect(renamedProductConversationLocation('/c/transcript-uuid', '?filter=open', '#m-1', previous, renamed)).toEqual({
      pathname: '/c/nouveau-nom',
      search: '?filter=open&source_transcript=transcript-uuid',
      hash: '#m-1',
    });
    expect(renamedProductConversationLocation('/c/latest-transcript-uuid', '', '', previous, renamed)).toEqual({
      pathname: '/c/nouveau-nom',
      search: '?source_transcript=latest-transcript-uuid',
      hash: '',
    });
    expect(renamedProductConversationLocation(
      '/c/latest-transcript-uuid', '?source_transcript=transcript-uuid&keep=yes', '', previous, renamed,
    )?.search).toBe('?source_transcript=transcript-uuid&keep=yes');
  });

  it('decodes unicode route aliases and rejects malformed percent escapes', () => {
    const previous = row({ canonical_root: {
      title: 'Titre', slug: 'café-日本', transcript_row_id: 'transcript-uuid',
    } as ProductConversationListRow['canonical_root'] });
    expect(productConversationOwnsRoute('/c/caf%C3%A9-%E6%97%A5%E6%9C%AC', previous)).toBe(true);
    expect(renamedProductConversationLocation(
      '/product-conversations/caf%C3%A9-%E6%97%A5%E6%9C%AC', '', '', previous,
      row({ canonical_route: '/c/nouveau' }),
    )).toEqual({ pathname: '/c/nouveau', search: '', hash: '' });
    expect(productConversationOwnsRoute('/c/%E0%A4%A', previous)).toBe(false);
    expect(renamedProductConversationLocation('/c/%E0%A4%A', '', '', previous, row())).toBeNull();
  });

  it('refreshes only the owned route and preserves transcript selection, search, hash', () => {
    const previous = row();
    const renamed = row({ canonical_route: '/c/new-name-2' });
    expect(renamedProductConversationLocation(
      '/c/old-name', '?source_transcript=transcript-uuid&filter=open', '#message-42', previous, renamed,
    )).toEqual({
      pathname: '/c/new-name-2',
      search: '?source_transcript=transcript-uuid&filter=open',
      hash: '#message-42',
    });
    expect(renamedProductConversationLocation(
      '/c/another-conversation', '', '', previous, renamed,
    )).toBeNull();
  });
});
