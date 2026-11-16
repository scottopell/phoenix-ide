import { describe, expect, it } from 'vitest';
import { api } from '../../api';
import { installSidebarFixtureApi } from './mockApi';
import { sidebarFixtureData, sidebarScenarios } from './scenarios';

describe('sidebar fixture scenarios', () => {
  it('covers expanded lifecycle/project states and collapsed overflow', () => {
    expect(sidebarScenarios.map((scenario) => scenario.id)).toEqual([
      'expanded-all-active',
      'expanded-project-archived',
      'expanded-empty-project',
      'product-actions-continued',
      'sidebar-visual-normal',
      'sidebar-visual-narrow',
      'collapsed-overflow',
    ]);

    expect(sidebarFixtureData.projects.map((project) => project.id)).toEqual(['phoenix', 'agents', 'docs']);
    expect(sidebarFixtureData.conversations.filter((conv) => conv.project_id === 'docs')).toHaveLength(0);
    expect(sidebarFixtureData.archivedConversations.filter((conv) => conv.project_id === 'phoenix')).toHaveLength(2);
    expect(sidebarFixtureData.conversations.length).toBeGreaterThan(9);
  });

  it('scopes product conversation rows to the continued-actions scenario', () => {
    expect(sidebarFixtureData.productConversations?.['product-actions-continued']).toHaveLength(1);
    for (const scenario of sidebarScenarios.filter((item) => item.id !== 'product-actions-continued' && !item.id.startsWith('sidebar-visual-'))) {
      expect(sidebarFixtureData.productConversations?.[scenario.id] ?? []).toHaveLength(0);
    }
  });

  it('serves a typed product snapshot for the continued-actions scenario', async () => {
    const restore = installSidebarFixtureApi(sidebarFixtureData, 'product-actions-continued');
    try {
      const snapshot = await api.getProductConversationSnapshot('pc-continued-fixture', { message_limit: 1 });
      expect(snapshot.product_conversation_id).toBe('pc-continued-fixture');
      expect(snapshot.latest_transcript_row_id).toBe('pc-latest-continuation');
      expect(snapshot.segments).toEqual([{
        segment_ordinal: 0,
        transcript_row_id: 'pc-latest-continuation',
        slug: 'fixture-product-root',
        title: 'Fixture Product Root',
        messages: [],
        handoff: null,
      }]);
      await expect(api.getProductConversationSnapshot('missing-fixture')).rejects.toThrow('Unknown product conversation fixture');
    } finally {
      restore();
    }
  });

  it('mocks settings and version endpoints and restores their original API methods', async () => {
    const originals = {
      getNotificationSettings: api.getNotificationSettings,
      getLlmLanguageSetting: api.getLlmLanguageSetting,
      getVersion: api.getVersion,
    };
    const restore = installSidebarFixtureApi(sidebarFixtureData, 'product-actions-continued');
    try {
      expect(await api.getNotificationSettings()).toEqual({
        enabled: true,
        notify_task_approval: true,
        notify_question: true,
        notify_error: true,
        notify_idle: true,
        notify_reminder: true,
      });
      expect(await api.getLlmLanguageSetting()).toEqual({
        language: 'en',
        available: ['en'],
        languages: [],
      });
      expect(await api.getVersion()).toEqual({
        version: 'fixture',
        git_sha: 'fixture000000000000000000000000000000000000',
        socket_activated: false,
      });
    } finally {
      restore();
    }

    expect(api.getNotificationSettings).toBe(originals.getNotificationSettings);
    expect(api.getLlmLanguageSetting).toBe(originals.getLlmLanguageSetting);
    expect(api.getVersion).toBe(originals.getVersion);
  });

  it('covers minimum/default widths, every PR state and a long badge-bearing title', () => {
    const visualScenarios = sidebarScenarios.filter(scenario => scenario.id.startsWith('sidebar-visual-'));
    expect(visualScenarios.map(scenario => scenario.sidebarWidth)).toEqual([280, 160]);
    const rows = sidebarFixtureData.productConversations?.['sidebar-visual-normal'] ?? [];
    expect(rows.map(row => row.cached_pr?.display_state)).toEqual(['open', 'draft', 'merged', 'closed', undefined]);
    expect(rows[1]?.canonical_root.title?.length).toBeGreaterThan(60);
    expect(rows.filter(row => row.read_state.state === 'unread')).toHaveLength(2);
    expect(rows[0]?.presentation.presentation_mode).toBe('working');
    expect(rows[3]?.presentation.presentation_mode).toBe('error');
  });

  it('keeps every active scenario slug backed by fixture data', () => {
    const slugs = new Set([
      ...sidebarFixtureData.conversations.map((conv) => conv.slug),
      ...sidebarFixtureData.archivedConversations.map((conv) => conv.slug),
      ...Object.values(sidebarFixtureData.productConversations ?? {}).flatMap((rows) => (rows ?? []).flatMap((row) => [
        row.product_conversation_id,
        row.canonical_root.slug,
        row.latest_transcript_row_id,
      ].filter((identity): identity is string => Boolean(identity)))),
    ]);

    for (const scenario of sidebarScenarios) {
      if (scenario.activeSlug) expect(slugs.has(scenario.activeSlug)).toBe(true);
    }
  });
});
