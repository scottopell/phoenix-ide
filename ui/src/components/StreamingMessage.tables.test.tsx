import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { act, cleanup, render } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { Message } from '../api';
import { ConversationContext } from '../conversation/ConversationContext';
import { ConversationStore } from '../conversation/ConversationStore';
import aclReply from '../fixtures/assistantFinalization/acl-reply.md?raw';
import { AgentMessage } from './MessageComponents';
import { StreamingMessage } from './StreamingMessage';

beforeEach(() => vi.useFakeTimers());
afterEach(() => { cleanup(); vi.useRealTimers(); });

function finalMessage(text: string): Message {
  return {
    message_id: 'acl-reply', conversation_id: 'acl-conversation', sequence_id: 1,
    message_type: 'agent', content: [{ type: 'text', text }],
    display_data: null, event_presentation: null, created_at: '2026-01-01T00:00:00Z',
  };
}

function tableMarkup(container: HTMLElement) {
  return [...container.querySelectorAll('.markdown-table-scroll')].map(node => node.outerHTML);
}

function assertTableStructure(container: HTMLElement) {
  for (const table of container.querySelectorAll('table')) {
    expect(table.parentElement).toHaveClass('markdown-table-scroll');
    expect([...table.children].map(node => node.tagName)).toEqual(
      table.querySelector('tbody') ? ['THEAD', 'TBODY'] : ['THEAD'],
    );
    for (const row of table.querySelectorAll('tr')) {
      expect(row.children).toHaveLength(3);
      for (const cell of row.children) {
        expect(cell.tagName).toBe(row.parentElement?.tagName === 'THEAD' ? 'TH' : 'TD');
        expect(cell.getAttribute('data-column-kind')).toMatch(/^(label|atomic|compact|prose|numeric)$/);
      }
    }
  }
}

it('preserves the actual ACL reply through cumulative partial table tokens and finalization', () => {
  const store = new ConversationStore();
  store.dispatch('acl-conversation', {
    type: 'sse_state_change', sequenceId: 1,
    phase: { type: 'llm_requesting', attempt: 1 }, stateUpdatedAt: 0,
  });
  const live = render(
    <MemoryRouter><ConversationContext.Provider value={store}>
      <StreamingMessage slug="acl-conversation" requestId="acl-reply" isFirstInTurn />
    </ConversationContext.Provider></MemoryRouter>,
  );
  const markers = [
    '| Path | Required',
    '|---|--',
    '|---|---|---|',
    '| `',
    '| `/Users/scott',
    '| `/Users/scottopell` | Traverse/',
    '| `/Users/scottopell` | Traverse/search | **Blocked** by your home’s `750` permissions |',
    '| `.phoenix-ide` | Traverse/search | Already allowed by `755` |',
    '| `prod.log` | Read | Already allowed by `644` |',
    'So the missing permission is **walking through your home directory**, not reading the log itself.',
  ];
  const ends = markers.map(marker => {
    const start = aclReply.indexOf(marker);
    expect(start, marker).toBeGreaterThanOrEqual(0);
    return start + marker.length;
  });
  ends.push(aclReply.length);
  let previousEnd = 0;
  for (const [index, end] of ends.entries()) {
    expect(end).toBeGreaterThan(previousEnd);
    act(() => store.dispatch('acl-conversation', {
      type: 'sse_token', requestId: 'acl-reply', sequenceId: index + 2,
      delta: aclReply.slice(previousEnd, end),
    }));
    act(() => vi.advanceTimersByTime(32));
    const saved = render(<MemoryRouter><AgentMessage
      message={finalMessage(aclReply.slice(0, end))} toolResults={new Map()}
      forceExpandedText
    /></MemoryRouter>);
    expect(tableMarkup(live.container)).toEqual(tableMarkup(saved.container));
    assertTableStructure(live.container);
    expect(live.container.querySelectorAll('table')).toHaveLength(index < 2 ? 0 : 1);
    saved.unmount();
    previousEnd = end;
  }

  const table = live.container.querySelector('table')!;
  expect([...table.querySelectorAll('th')].map(cell => cell.textContent))
    .toEqual(['Path', 'Required right', 'Current situation']);
  expect([...table.querySelectorAll('tbody tr')].map(row => [...row.children].map(cell => cell.textContent)))
    .toEqual([
      ['/Users/scottopell', 'Traverse/search', 'Blocked by your home’s 750 permissions'],
      ['.phoenix-ide', 'Traverse/search', 'Already allowed by 755'],
      ['prod.log', 'Read', 'Already allowed by 644'],
    ]);
  expect(table.querySelector('strong')).toHaveTextContent('Blocked');
  expect(table.querySelector('code')).toHaveAttribute('data-token-kind', 'breakable');
  expect(table.querySelector('tbody tr:last-child code')).toHaveAttribute('data-token-kind', 'short-atomic');
  expect([...table.querySelectorAll('th')].map(cell => cell.getAttribute('data-column-kind')))
    .toEqual(['atomic', 'atomic', 'label']);

  const saved = render(<MemoryRouter><AgentMessage message={finalMessage(aclReply)} toolResults={new Map()} forceExpandedText /></MemoryRouter>);
  const codeBlocks = (container: HTMLElement) => [...container.querySelectorAll('code.language-text, pre code')].map(node => node.textContent?.trim());
  expect(codeBlocks(live.container)).toEqual(codeBlocks(saved.container));
  expect(codeBlocks(live.container)).toEqual([
    '/Users/scottopell/.phoenix-ide/prod.log',
    'Allow this specific Agent account to search/traverse this directory.',
  ]);
  expect(live.container).toHaveTextContent('No ACL has been changed yet.');
  expect(saved.container.querySelector('.agent-text-fragment')).not.toBeNull();
  expect(tableMarkup(live.container)).toEqual(tableMarkup(saved.container));
});
