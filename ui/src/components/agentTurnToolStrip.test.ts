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

function toolResult(toolUseId: string, opts: { isError?: boolean; content?: string } = {}): Message {
  return {
    message_id: `r-${toolUseId}`,
    message_type: 'tool',
    content: {
      tool_use_id: toolUseId,
      content: opts.content ?? 'ok',
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

  it('adds input summaries that distinguish repeated search and read_file tools', () => {
    const msg = agentMessage([
      { type: 'tool_use', id: 's1', name: 'search', input: { pattern: 'compact|Tool|tool', path: 'ui/src', include: '*.tsx' } },
      { type: 'tool_use', id: 'r1', name: 'read_file', input: { path: 'ui/src/components/MessageComponents.tsx', offset: 711, limit: 40 } },
      { type: 'tool_use', id: 'r2', name: 'read_file', input: { path: 'ui/src/components/agentTurnToolStrip.ts' } },
    ]);

    const items = deriveToolStripItems(msg, new Map());

    expect(items.map((item) => item.inputSummary)).toEqual([
      'compact|Tool|tool in ui/src (*.tsx)',
      'ui/src/components/MessageComponents.tsx:711-750',
      'ui/src/components/agentTurnToolStrip.ts',
    ]);
  });

  it('adds cheap result summaries for search, read_file, and bash', () => {
    const msg = agentMessage([
      { type: 'tool_use', id: 's1', name: 'search', input: { pattern: 'x' } },
      { type: 'tool_use', id: 'r1', name: 'read_file', input: { path: 'README.md' } },
      { type: 'tool_use', id: 'b1', name: 'bash', input: { op: 'run', cmd: './dev.py check' } },
    ]);
    const results = new Map<string, Message>([
      ['s1', toolResult('s1', { content: 'a.ts:1: one\na.ts:2: two\nb.ts:3: three' })],
      ['r1', toolResult('r1', { content: 'line one\nline two' })],
      ['b1', toolResult('b1', { content: JSON.stringify({ status: 'exited', exit_code: 0, lines: [] }) })],
    ]);

    const items = deriveToolStripItems(msg, results);

    expect(items.map((item) => item.resultSummary)).toEqual([
      '3 matches in 2 files',
      '2 lines',
      'exited 0',
    ]);
  });

  it('uses a scalar fallback for unknown tools instead of name-only rendering', () => {
    const msg = agentMessage([
      { type: 'tool_use', id: 'u1', name: 'future_tool', input: { target: 'alpha', nested: { ignored: true } } },
    ]);

    const items = deriveToolStripItems(msg, new Map());

    expect(items[0]).toMatchObject({ name: 'future_tool', inputSummary: 'target: alpha' });
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
