import { describe, expect, it } from 'vitest';
import { fireEvent, render } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { Message } from '../../api';
import { AgentMessage } from '../MessageComponents';
import { buildAgentTextFragments } from './searchProjections';

function renderMatch(source: string, query: string | null = null) {
  const blocks = [{ type: 'text' as const, text: source }];
  const fragment = buildAgentTextFragments(blocks, 'full', { forSearch: true })[0]!;
  const message: Message = {
    message_id: 'disclosure-find', sequence_id: 1, conversation_id: 'find-conversation',
    message_type: 'agent', content: blocks, display_data: null, created_at: '',
  };
  function view(query: string | null) {
    const start = query === null ? 0 : fragment.semanticText.indexOf(query);
    expect(start).toBeGreaterThanOrEqual(0);
    return <MemoryRouter>
      <AgentMessage
        message={message}
        toolResults={new Map()}
        activeHighlight={query === null ? null : { owner: 'agent-text', fragmentId: fragment.fragmentId, start, end: start + query.length }}
      />
    </MemoryRouter>;
  }
  const result = render(view(query));
  return { ...result, updateMatch: (nextQuery: string | null) => result.rerender(view(nextQuery)) };
}

const disclosure = 'Before\n\n<details open><summary>Summary **needle**</summary>\n\nBody *target*.\n\n</details>\n\nAfter';

describe('transcript disclosure find highlight ownership', () => {
  it('places the summary mark inside a collapsed summary rather than the enclosing details', () => {
    const { container } = renderMatch(disclosure.replace('<details open>', '<details>'), 'needle');
    expect(container.querySelectorAll('mark')).toHaveLength(1);
    expect(container.querySelector('details > summary strong > mark')).toHaveTextContent('needle');
    expect(container.querySelector('details > p mark')).toBeNull();
    expect(container.querySelector('details')).not.toHaveAttribute('open');
  });

  it('places body marks inside their own paragraph after summary text', () => {
    const { container } = renderMatch(disclosure, 'target');
    expect(container.querySelectorAll('mark')).toHaveLength(1);
    expect(container.querySelector('details > p em > mark')).toHaveTextContent('target');
    expect(container.querySelector('summary mark')).toBeNull();
    expect(container.querySelector('details')).toHaveAttribute('open');
  });

  it('opens a collapsed disclosure to reveal an active body match, but not an unrelated disclosure', () => {
    const source = disclosure.replace('<details open>', '<details>')
      + '\n\n<details><summary>Other</summary>\n\nUnrelated body.\n\n</details>';
    const { container, updateMatch } = renderMatch(source);
    const [matched, unrelated] = container.querySelectorAll('details');
    expect(matched).not.toHaveAttribute('open');
    updateMatch('target');
    const mark = matched!.querySelector('p em > mark');
    expect(mark).toHaveTextContent('target');
    expect(mark).toBeVisible();
    expect(matched).toHaveAttribute('open');
    expect(unrelated).not.toHaveAttribute('open');
    updateMatch(null);
    expect(matched).toHaveAttribute('open');
    expect(unrelated).not.toHaveAttribute('open');
  });

  it('leaves unmatched disclosures collapsed when the active match is outside them', () => {
    const { container } = renderMatch(disclosure.replace('<details open>', '<details>'), 'After');
    expect(container.querySelector('details')).not.toHaveAttribute('open');
    expect(container.querySelector('mark')).toHaveTextContent('After');
  });

  it('preserves a user-opened disclosure when a summary search clears and permits closing it', () => {
    const { container, updateMatch } = renderMatch(disclosure.replace('<details open>', '<details>'));
    const details = container.querySelector('details')!;
    fireEvent.click(details.querySelector('summary')!);
    expect(details).toHaveAttribute('open');
    updateMatch('needle');
    expect(details).toHaveAttribute('open');
    updateMatch(null);
    expect(details).toHaveAttribute('open');
    fireEvent.click(details.querySelector('summary')!);
    expect(details).not.toHaveAttribute('open');
  });

  it('allows the user to close a find-opened disclosure and leaves it closed when search clears', () => {
    const { container, updateMatch } = renderMatch(disclosure.replace('<details open>', '<details>'), 'target');
    const details = container.querySelector('details')!;
    expect(details).toHaveAttribute('open');
    fireEvent.click(details.querySelector('summary')!);
    expect(details).not.toHaveAttribute('open');
    updateMatch(null);
    expect(details).not.toHaveAttribute('open');
  });

  it('preserves a user closing a source-open disclosure across search clearing', () => {
    const { container, updateMatch } = renderMatch(disclosure, 'needle');
    const details = container.querySelector('details')!;
    expect(details).toHaveAttribute('open');
    fireEvent.click(details.querySelector('summary')!);
    expect(details).not.toHaveAttribute('open');
    updateMatch(null);
    expect(details).not.toHaveAttribute('open');
  });

  it('highlights a malformed disclosure fallback at rendered-text offsets', () => {
    const { container } = renderMatch('Before\n\n<details><summary>Unfinished</summary>\nBody **target**\n\n# After', 'target');
    expect(container.querySelector('details')).toBeNull();
    expect(container.querySelectorAll('mark')).toHaveLength(1);
    expect(container.querySelector('strong > mark')).toHaveTextContent('target');
    expect(container.querySelector('h1')).toHaveTextContent('After');
  });
});
