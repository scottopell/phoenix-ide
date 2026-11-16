import { describe, expect, it } from 'vitest';
import type { ContentBlock } from '../../api';
import type { RenderUnit } from '../../conversation/renderUnits';
import { buildConversationSearchProjection } from './searchProjections';
import { conversationProjectionSessionMatches } from './projectionSession';

function finalUnit(content: ContentBlock[], key = 'request'): RenderUnit {
  return {
    kind: 'agent_turn', key, isFirstInTurn: true, toolResultsByUseId: new Map(),
    agent: { message_id: key, conversation_id: 'conversation', message_type: 'agent', sequence_id: 1, created_at: '', event_presentation: null, content },
  };
}
function streamed(text: string, query = 'alpha') {
  const units: RenderUnit[] = [{ kind: 'streaming_agent', key: 'request', isFirstInTurn: true }];
  return conversationProjectionSessionMatches(buildConversationSearchProjection(units, query, {
    streamingBuffer: { text, requestId: 'request', lastSequence: 1, startedAt: 0 },
  }), units);
}
function finalized(content: ContentBlock[], query = 'alpha', key = 'request') {
  const units = [finalUnit(content, key)];
  return conversationProjectionSessionMatches(buildConversationSearchProjection(units, query), units);
}

describe('conversation logical occurrence identity', () => {
  it.each(['alpha alpha', 'Intro\n\n## alpha\n\nalpha end', '**alpha**\n\nalpha', 'prefix\n\n> alpha'])('retains bounded prose correspondence for %j while refreshing physical targets', (text) => {
    const before = streamed(text);
    const after = finalized([{ type: 'text', text }]);
    expect(after.map((match) => match.id)).toEqual(before.map((match) => match.id));
    expect(new Set(after.map((match) => match.id)).size).toBe(after.length);
    expect(after[0]!.target).not.toEqual(before[0]!.target);
    expect(after[0]!.target).toMatchObject({ fragmentId: 'agent-text-0', sourceId: 'request:agent-text-0' });
  });

  it('maps the single text block at its actual fragment index, not agent-text-0', () => {
    const after = finalized([{ type: 'tool_use', id: 'thinking', name: 'think', input: { thoughts: 'private' } }, { type: 'text', text: 'alpha' }]);
    expect(after[0]!.id).toBe(streamed('alpha')[0]!.id);
    expect(after[0]!.target).toMatchObject({ fragmentId: 'agent-text-1' });
  });

  it('does not alias multiple text blocks or ambiguous/noncontiguous semantic text', () => {
    const streamingIds = new Set(streamed('alpha alpha').map((match) => match.id));
    const after = finalized([{ type: 'text', text: 'alpha' }, { type: 'text', text: 'alpha' }]);
    expect(after.every((match) => !streamingIds.has(match.id))).toBe(true);
    expect(new Set(after.map((match) => match.id)).size).toBe(2);
    for (const text of ['al**ph**a', '[alpha](alpha)']) {
      expect(finalized([{ type: 'text', text }])[0]!.id).not.toBe(streamed(text)[0]?.id);
    }
  });

  it.each([
    '[al**ph**a](alpha)',
    '[al**ph**a](url "alpha")',
    '![alpha](image.png)al**ph**a',
    '![hidden](image.png "alpha")al**ph**a',
  ])('does not assign visible label provenance to hidden destination/alt/title in %j', (text) => {
    for (const query of ['alpha', 'ph']) {
      const before = new Set(streamed(text, query).map((match) => match.id));
      const after = finalized([{ type: 'text', text }], query);
      expect(before.size).toBeGreaterThan(0);
      expect(after).toHaveLength(1);
      expect(after.every((match) => !before.has(match.id))).toBe(true);
    }
  });

  it('keeps request namespaces distinct and removes vanished occurrences', () => {
    const before = streamed('alpha');
    expect(finalized([{ type: 'text', text: 'alpha' }], 'alpha', 'other')[0]!.id).not.toBe(before[0]!.id);
    expect(finalized([{ type: 'text', text: 'gone' }])).toEqual([]);
    expect(finalized([{ type: 'text', text: 'prefix alpha' }])[0]!.id).not.toBe(before[0]!.id);
  });
});
