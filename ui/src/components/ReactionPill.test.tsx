import { createRef } from 'react';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ReactionPill } from './ReactionPill';
import { restoreReactionRange } from './reactionRange';
import { FocusScopeProvider } from '../hooks/useFocusScope';
import type { ReactionSource } from '../conversation/InlineReactionStore';

const source: ReactionSource = { messageId: 'answer', sequenceId: 2, occurrenceToken: 'earlier:answer', quote: 'second', textOffsets: { start: 6, end: 12 } };
const add = vi.fn();
const close = vi.fn();
const navigate = vi.fn(() => true);
let offscreen = false;
function Fixture({ mounted = true, body = 'Keep this guarantee' }: { mounted?: boolean; body?: string }) {
  return <FocusScopeProvider>
    <div id="messages">
      {mounted && <div data-inline-reaction-message="answer" data-message-occurrence="earlier:answer">first <strong>second</strong> third</div>}
    </div>
    <ReactionPill source={source} bubbleRef={createRef<HTMLDivElement>()} scopeId="test" body={body} available onChange={() => {}} onAdd={add} onClose={close} returnToSource={navigate} />
  </FocusScopeProvider>;
}

beforeEach(() => {
  offscreen = false;
  vi.clearAllMocks();
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 600, 500));
  Object.defineProperty(Range.prototype, 'getBoundingClientRect', { configurable: true, value: () => new DOMRect(20, offscreen ? -100 : 100, 150, 20) });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe('reaction pill', () => {
  it('focuses on Enter without appending, but leaves controls and composition alone', () => {
    render(<><Fixture /><textarea aria-label="Composer" /><button>Other action</button></>);
    const input = screen.getByRole('textbox', { name: 'Your reaction' });
    expect(input).not.toHaveFocus();
    fireEvent.keyDown(document, { key: 'Enter', isComposing: true });
    expect(input).not.toHaveFocus();
    fireEvent.keyDown(document, { key: 'Enter', shiftKey: true });
    expect(input).not.toHaveFocus();
    const composer = screen.getByRole('textbox', { name: 'Composer' });
    composer.focus();
    fireEvent.keyDown(composer, { key: 'Enter' });
    expect(composer).toHaveFocus();
    const button = screen.getByRole('button', { name: 'Other action' });
    button.focus();
    fireEvent.keyDown(button, { key: 'Enter' });
    expect(button).toHaveFocus();
    button.blur();
    fireEvent.keyDown(document, { key: 'Enter' });
    expect(input).toHaveFocus();
    expect(add).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: 'Enter', metaKey: true });
    expect(add).toHaveBeenCalledOnce();
  });

  it('restores the exact passage through DOM remount and returns without focusing the input', async () => {
    const view = render(<Fixture />);
    expect(restoreReactionRange(source)?.toString()).toBe('second');
    expect(screen.getByRole('textbox')).toHaveAttribute('type', 'text');
    expect(screen.getByRole('textbox')).not.toHaveFocus();
    view.rerender(<Fixture mounted={false} />);
    const dock = await screen.findByRole('button', { name: /Return to passage/ });
    expect(restoreReactionRange(source)).toBeNull();
    expect(screen.queryByRole('textbox')).toBeNull();
    fireEvent.click(dock);
    expect(navigate).toHaveBeenCalledWith(source);
    view.rerender(<Fixture />);
    await waitFor(() => expect(screen.getByRole('textbox')).toHaveValue('Keep this guarantee'));
    expect(window.getSelection()?.toString()).toBe('second');
    expect(screen.getByRole('textbox')).not.toHaveFocus();
  });

  it('automatically undocks on manual return and preserves the same one-line editor for long text', async () => {
    const body = 'A detailed reaction '.repeat(40);
    render(<Fixture body={body} />);
    offscreen = true;
    fireEvent.scroll(window);
    await screen.findByRole('region', { name: 'Docked reaction' });
    offscreen = false;
    fireEvent.scroll(window);
    const input = await screen.findByRole('textbox');
    expect(input).toHaveValue(body);
    expect(input.tagName).toBe('INPUT');
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(add).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: 'Enter', metaKey: true, isComposing: true });
    expect(add).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: 'Enter', metaKey: true });
    expect(add).toHaveBeenCalledOnce();
  });

  it.each([true, false])('offers explicit keep/discard when the source is mounted=%s', (mounted) => {
    render(<Fixture mounted={mounted} />);
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss reaction' }));
    expect(close).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Keep' }));
    expect(close).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss reaction' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
    expect(close).toHaveBeenCalledOnce();
  });
});
