import { describe, expect, it } from 'vitest';
import {
  getMessageListScenario,
  messageListFixtureData,
  prefixContinuityEarlierMessages,
} from './scenarios';

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
