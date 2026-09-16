import { describe, expect, it } from 'vitest';
import { sidebarFixtureData, sidebarScenarios } from './scenarios';

describe('sidebar fixture scenarios', () => {
  it('covers expanded lifecycle/project states and collapsed overflow', () => {
    expect(sidebarScenarios.map((scenario) => scenario.id)).toEqual([
      'expanded-all-active',
      'expanded-project-archived',
      'expanded-empty-project',
      'product-actions-continued',
      'collapsed-overflow',
    ]);

    expect(sidebarFixtureData.projects.map((project) => project.id)).toEqual(['phoenix', 'agents', 'docs']);
    expect(sidebarFixtureData.conversations.filter((conv) => conv.project_id === 'docs')).toHaveLength(0);
    expect(sidebarFixtureData.archivedConversations.filter((conv) => conv.project_id === 'phoenix')).toHaveLength(2);
    expect(sidebarFixtureData.conversations.length).toBeGreaterThan(9);
  });

  it('scopes product conversation rows to the continued-actions scenario', () => {
    expect(sidebarFixtureData.productConversations?.['product-actions-continued']).toHaveLength(1);
    for (const scenario of sidebarScenarios.filter((item) => item.id !== 'product-actions-continued')) {
      expect(sidebarFixtureData.productConversations?.[scenario.id] ?? []).toHaveLength(0);
    }
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
