import { describe, expect, it } from 'vitest';
import type { Message } from '../api';
import { deriveToolStripItems } from './agentTurnToolStrip';
import { isSignificantText, SIGNIFICANCE_THRESHOLD } from '../hooks/useDensity';

function agentMessage(content: unknown): Message {
  return {
    message_id: 'm1',
    message_type: 'agent',
    content,
  } as unknown as Message;
}

function toolResult(toolUseId: string, opts: { isError?: boolean } = {}): Message {
  return {
    message_id: `r-${toolUseId}`,
    message_type: 'tool',
    content: {
      tool_use_id: toolUseId,
      content: 'ok',
      is_error: opts.isError ?? false,
    },
  } as unknown as Message;
}

describe('deriveToolStripItems', () => {
  it('emits one item per non-think tool_use block, in document order', () => {
    const msg = agentMessage([
      { type: 'text', text: 'hello' },
      { type: 'tool_use', id: 't1', name: 'bash' },
      { type: 'tool_use', id: 't2', name: 'patch' },
      { type: 'tool_use', id: 't3', name: 'bash' },
    ]);
    const items = deriveToolStripItems(msg, new Map());
    expect(items.map((i) => i.name)).toEqual(['bash', 'patch', 'bash']);
    expect(items.map((i) => i.toolId)).toEqual(['t1', 't2', 't3']);
  });

  it('excludes think blocks', () => {
    const msg = agentMessage([
      { type: 'tool_use', id: 'k1', name: 'think' },
      { type: 'tool_use', id: 't1', name: 'bash' },
    ]);
    const items = deriveToolStripItems(msg, new Map());
    expect(items).toHaveLength(1);
    expect(items[0]?.name).toBe('bash');
  });

  it('flags sub-agent launches distinctly', () => {
    const msg = agentMessage([{ type: 'tool_use', id: 's1', name: 'spawn_agents' }]);
    const items = deriveToolStripItems(msg, new Map());
    expect(items[0]?.isSubAgent).toBe(true);
  });

  it('reports result presence and error state from the paired result map', () => {
    const msg = agentMessage([
      { type: 'tool_use', id: 't1', name: 'bash' },
      { type: 'tool_use', id: 't2', name: 'bash' },
      { type: 'tool_use', id: 't3', name: 'bash' },
    ]);
    const results = new Map<string, Message>([
      ['t1', toolResult('t1')],
      ['t2', toolResult('t2', { isError: true })],
    ]);
    const items = deriveToolStripItems(msg, results);
    expect(items[0]).toMatchObject({ hasResult: true, isError: false });
    expect(items[1]).toMatchObject({ hasResult: true, isError: true });
    expect(items[2]).toMatchObject({ hasResult: false, isError: false });
  });

  it('returns empty for a turn with no tool blocks', () => {
    const msg = agentMessage([{ type: 'text', text: 'just prose' }]);
    expect(deriveToolStripItems(msg, new Map())).toEqual([]);
  });
});

describe('isSignificantText', () => {
  it('treats text at or above the threshold as significant', () => {
    expect(isSignificantText('a'.repeat(SIGNIFICANCE_THRESHOLD))).toBe(true);
    expect(isSignificantText('a'.repeat(SIGNIFICANCE_THRESHOLD + 50))).toBe(true);
  });

  it('treats text below the threshold as insignificant', () => {
    expect(isSignificantText('a'.repeat(SIGNIFICANCE_THRESHOLD - 1))).toBe(false);
    expect(isSignificantText('short reply')).toBe(false);
  });

  it('pins the default threshold at 280 characters', () => {
    expect(SIGNIFICANCE_THRESHOLD).toBe(280);
  });
});
