import { describe, expect, it } from 'vitest';
import type { Message } from '../api';
import { messageCacheWrite } from './messageCachePersistence';

function message(sequenceId: number): Message {
  return {
    message_id: `m-${sequenceId}`,
    conversation_id: 'conv-1',
    sequence_id: sequenceId,
    message_type: 'user',
    content: [{ type: 'text', text: String(sequenceId) }],
    created_at: '2026-01-01T00:00:00Z',
  };
}

describe('messageCacheWrite', () => {
  it('writes only appended rows when the existing prefix is unchanged', () => {
    const first = message(1);
    const second = message(2);
    const appended = message(3);

    expect(messageCacheWrite('conv-1', [first, second], [first, second, appended], false)).toEqual({
      kind: 'append',
      messages: [appended],
    });
  });

  it('writes the full transcript when a snapshot prepends rows', () => {
    const tail = message(3);
    const full = [message(1), message(2), tail];

    expect(messageCacheWrite('conv-1', [tail], full, false)).toEqual({
      kind: 'replace',
      conversationId: 'conv-1',
      messages: full,
    });
  });

  it('writes the full transcript when an existing row is replaced', () => {
    const first = message(1);
    const replacement = { ...first, content: [{ type: 'text' as const, text: 'updated' }] };

    expect(messageCacheWrite('conv-1', [first], [replacement], false)).toEqual({
      kind: 'replace',
      conversationId: 'conv-1',
      messages: [replacement],
    });
  });

  it('writes all rows after a transcript generation change', () => {
    const first = message(1);

    expect(messageCacheWrite('conv-1', [first], [first], true)).toEqual({
      kind: 'replace',
      conversationId: 'conv-1',
      messages: [first],
    });
  });
});
