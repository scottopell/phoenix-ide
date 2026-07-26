import { describe, expect, it } from 'vitest';
import { getMobileConversationListFixtureData, mobileConversationListScenarios } from './scenarios';

describe('mobile conversation list fixture scenarios', () => {
  it('covers overview, chain, naming, long-list, and archived scenarios', () => {
    expect(mobileConversationListScenarios.map((scenario) => scenario.id)).toEqual([
      'active-overview-dark',
      'active-overview-light',
      'chains-dark',
      'naming-context-dark',
      'long-list-dark',
      'archived-dark',
    ]);
  });

  it('keeps every scenario activeSlug backed by fixture data', () => {
    for (const scenario of mobileConversationListScenarios) {
      if (!scenario.activeSlug) continue;
      const fixtureData = getMobileConversationListFixtureData(scenario);
      const slugs = new Set([
        ...fixtureData.conversations.map((conv) => conv.slug),
        ...fixtureData.archivedConversations.map((conv) => conv.slug),
      ]);
      expect(slugs.has(scenario.activeSlug)).toBe(true);
    }
  });

  it('builds a deterministic long mobile dataset with a long active continuation chain', () => {
    const scenario = mobileConversationListScenarios.find((item) => item.id === 'long-list-dark');
    expect(scenario).toBeDefined();

    const fixtureData = getMobileConversationListFixtureData(scenario!);
    expect(fixtureData.conversations.length).toBeGreaterThanOrEqual(30);

    const standaloneRows = fixtureData.conversations.filter((conv) => conv.id.startsWith('long-standalone-'));
    expect(standaloneRows).toHaveLength(18);

    const chainRows = fixtureData.conversations.filter((conv) => conv.id.startsWith('long-chain-'));
    expect(chainRows).toHaveLength(12);

    const active = fixtureData.conversations.find((conv) => conv.slug === scenario!.activeSlug);
    expect(active?.id).toBe('long-chain-current-12');
    expect(active?.continued_in_conv_id).toBeNull();
    expect(active?.presentation_mode).toBe('working');

    const root = fixtureData.conversations.find((conv) => conv.id === 'long-chain-root-01');
    expect(root?.continued_in_conv_id).toBe('long-chain-link-02');
    expect(root?.chain_name).toBe('mobile long continuation chain');

    const chainStates = new Set(chainRows.map((conv) => conv.state.type));
    expect(chainStates).toEqual(new Set([
      'awaiting_llm',
      'awaiting_task_approval',
      'awaiting_user_response',
      'terminal',
      'error',
      'context_exhausted',
    ]));
  });
});
