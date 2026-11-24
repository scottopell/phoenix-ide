import { describe, expect, it } from 'vitest';
import type { ProductConversationListRow } from '../../../api';
import { createConversationSource } from './ConversationSource';

const row = (id: string, updatedAt: string, monitored: boolean, mode = 'working'): ProductConversationListRow => ({
  product_conversation_id: id,
  canonical_route: `/c/${id}`,
  canonical_root: { transcript_row_id: `root-${id}`, slug: id, title: id },
  lifecycle: { state: 'open', close_action: { availability: 'available' } },
  latest_transcript_row_id: `latest-${id}`,
  updated_at: updatedAt,
  presentation: { kind: 'state', display_name: id, presentation_mode: mode },
  read_state: { state: 'read' },
  cached_pr: null,
  monitored,
}) as ProductConversationListRow;

describe('conversation palette source', () => {
  const rows = [
    row('mine-old', '2026-11-01T00:00:00Z', false),
    row('ember-new', '2026-11-05T00:00:00Z', true),
    row('ember-asking', '2026-11-02T00:00:00Z', true, 'needs_action'),
    row('mine-new', '2026-11-04T00:00:00Z', false),
  ];
  const source = createConversationSource([], () => {}, rows, () => {});

  it('lists the user\'s own conversations before the ones Ember is handling when browsing', async () => {
    const items = await source.search('');
    expect(items.map((item) => item.id)).toEqual(['mine-new', 'ember-asking', 'mine-old', 'ember-new']);
  });

  it('does not demote a monitored conversation the user searched for', async () => {
    const items = await source.search('ember-new');
    expect(items[0]?.id).toBe('ember-new');
  });
});
