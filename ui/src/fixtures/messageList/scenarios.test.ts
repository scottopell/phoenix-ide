import { describe, expect, it } from 'vitest';
import {
  getMessageListScenario,
  messageListFixtureData,
  mobileTablePreviewCases,
  prefixContinuityEarlierMessages,
} from './scenarios';

describe('mobile table preview cases', () => {
  it('preserves the original fixture plus eight historical excerpts and provenance', () => {
    expect(mobileTablePreviewCases).toHaveLength(9);
    expect(mobileTablePreviewCases[0]?.id).toBe('global-status');
    expect(mobileTablePreviewCases.slice(1).every((previewCase) => (
      previewCase.caption.startsWith('Historical conversation excerpt ·')
      && previewCase.caption.includes('conversation ')
      && previewCase.caption.includes('message ')
    ))).toBe(true);
    expect(mobileTablePreviewCases.find((previewCase) => previewCase.id === 'latency-measurements')?.markdown)
      .toContain('|---|---:|---:|');
    expect(mobileTablePreviewCases.find((previewCase) => previewCase.id === 'model-performance')?.markdown)
      .toContain('| Model | Completed | Failed | p50 total | p95 total | Max |');
  });
});

describe('message-list continuity fixture', () => {
  it('provides a deterministic tall anchor and an earlier prefix', () => {
    const scenario = getMessageListScenario('prefix-continuity-offset-bug');
    const data = messageListFixtureData(scenario);
    const anchor = data.messages.find((message) => message.message_id === 'continuity-agent-anchor');

    expect(anchor).toBeDefined();
    expect(JSON.stringify(anchor?.content)).toContain('Continuity marker 28');
    expect(prefixContinuityEarlierMessages).toHaveLength(18);
    expect(prefixContinuityEarlierMessages.at(-1)?.sequence_id).toBeLessThan(
      data.messages[0]!.sequence_id,
    );
  });
});
