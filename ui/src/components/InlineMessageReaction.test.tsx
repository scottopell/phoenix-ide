import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Message } from '../api';
import { InlineReactionContext, InlineReactionStore, formatInlineReaction } from '../conversation/InlineReactionStore';
import { DraftStore } from '../conversation/DraftStore';
import { FocusScopeProvider } from '../hooks/useFocusScope';
import { InlineMessageReaction } from './InlineMessageReaction';
import { readReactionSelection } from './inlineReactionSelection';
import { MessageContextMenu } from './MessageContextMenu';
import { FilePathContextMenu } from './FilePathContextMenu';

const messages: Message[] = ['old', 'new'].map((id) => ({
  message_id: id, sequence_id: 2, message_type: 'agent',
  conversation_id: `row-${id}`, created_at: '2026-09-19T12:00:00Z',
  content: [{ type: 'text', text: 'Deterministic state patterns' }],
  display_data: { productOccurrenceToken: `row-${id}:${id}` },
}));

function select(start: Node, end = start) {
  const range = document.createRange();
  range.setStart(start, 0);
  range.setEnd(end, end.textContent?.length ?? 0);
  window.getSelection()!.removeAllRanges();
  window.getSelection()!.addRange(range);
  fireEvent(document, new Event('selectionchange'));
}

function Harness({ store, scope = 'conversation-a', append }: { store: InlineReactionStore; scope?: string; append?: ((text: string) => void) | undefined }) {
  return (
    <FocusScopeProvider>
      <InlineReactionContext.Provider value={store}>
        <div id="messages">
          {messages.map((message) => (
            <div key={message.message_id} className="message agent" data-inline-reaction-message={message.message_id} data-message-id={message.message_id} data-sequence-id="2">
              <div className="agent-text-block"><p data-testid={message.message_id}>Deterministic state patterns <code>replay(events)</code></p></div>
            </div>
          ))}
        </div>
        <InlineMessageReaction scopeKey={scope} messages={messages} destination={append ? { append } : undefined} />
        <MessageContextMenu messages={messages} />
        <FilePathContextMenu />
      </InlineReactionContext.Provider>
    </FocusScopeProvider>
  );
}

beforeEach(() => {
  Object.defineProperty(Range.prototype, 'getBoundingClientRect', {
    configurable: true,
    value: () => ({ left: 50, top: 50, bottom: 80, right: 300, width: 250, height: 30 }),
  });
});
afterEach(() => { cleanup(); window.getSelection()?.removeAllRanges(); vi.restoreAllMocks(); });

describe('inline message reactions', () => {
  it('opens without focus, appends to the latest draft exactly once, and retains stable historical identity', async () => {
    const store = new InlineReactionStore();
    const drafts = new DraftStore();
    drafts.dispatch('latest', { type: 'set_draft', text: 'Original draft' });
    const append = vi.fn((text: string) => drafts.dispatch('latest', { type: 'append_draft', text }));
    render(<Harness store={store} append={append} />);
    select(screen.getByTestId('old').firstChild!);
    const input = await screen.findByRole('textbox', { name: 'Your reaction' });
    expect(input).not.toHaveFocus();
    fireEvent.change(input, { target: { value: 'Strong idea\nPlease test cancellation too.' } });
    drafts.dispatch('latest', { type: 'set_draft', text: 'Draft edited while reacting  ' });
    const add = screen.getByRole('button', { name: 'Add to draft' });
    fireEvent.click(add);
    fireEvent.click(add);
    expect(append).toHaveBeenCalledTimes(1);
    expect(drafts.getSnapshot('latest').draft).toContain('Draft edited while reacting  \n\nRegarding message #2 (row-old:old):');
    expect(drafts.getSnapshot('latest').draft).toContain('Deterministic state patterns ');
    expect(drafts.getSnapshot('latest').draft).toContain('Strong idea\nPlease test cancellation too.');
    expect(screen.queryByRole('textbox', { name: 'Your reaction' })).not.toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('Added to draft');
  });

  it('dismisses an empty reaction without reopening it on pointerup', async () => {
    render(<Harness store={new InlineReactionStore()} append={vi.fn()} />);
    select(screen.getByTestId('old').firstChild!);
    await screen.findByRole('textbox', { name: 'Your reaction' });
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss reaction' }));
    fireEvent.pointerUp(document);
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(screen.queryByRole('textbox', { name: 'Your reaction' })).not.toBeInTheDocument();
    expect(window.getSelection()?.toString()).toBe('');
  });

  it('pins typed reactions across selection changes, unavailable destinations, unmounts, and navigation', async () => {
    const store = new InlineReactionStore();
    const append = vi.fn();
    const view = render(<Harness store={store} append={append} />);
    select(screen.getByTestId('old').firstChild!);
    fireEvent.change(await screen.findByRole('textbox', { name: 'Your reaction' }), { target: { value: 'Keep this thought' } });
    select(screen.getByTestId('new').firstChild!);
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(store.getSnapshot('conversation-a')?.source.messageId).toBe('old');
    view.rerender(<Harness store={store} />);
    expect(screen.getByRole('button', { name: 'Add to draft' })).toBeDisabled();
    expect(screen.getByRole('textbox')).toHaveValue('Keep this thought');
    view.rerender(<Harness store={store} scope="conversation-b" append={append} />);
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    view.unmount();
    render(<Harness store={store} append={append} />);
    expect(screen.getByRole('textbox')).toHaveValue('Keep this thought');
    fireEvent.click(screen.getByRole('button', { name: 'Add to draft' }));
    expect(append).toHaveBeenCalledTimes(1);
    expect(append.mock.calls[0]?.[0]).toContain('row-old:old');
  });

  it('does not hijack native context menus or copy keyboard shortcuts while reacting', async () => {
    render(<Harness store={new InlineReactionStore()} append={vi.fn()} />);
    const paragraph = screen.getByTestId('old');
    select(paragraph.firstChild!);
    await screen.findByRole('textbox');
    const contextMenu = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });
    paragraph.dispatchEvent(contextMenu);
    expect(contextMenu.defaultPrevented).toBe(false);
    expect(screen.queryByRole('button', { name: 'Copy as Markdown' })).not.toBeInTheDocument();
    const copy = new KeyboardEvent('keydown', { key: 'c', metaKey: true, bubbles: true, cancelable: true });
    paragraph.dispatchEvent(copy);
    expect(copy.defaultPrevented).toBe(false);
  });

  it('ignores empty, cross-message, and editable selections', () => {
    render(<Harness store={new InlineReactionStore()} append={vi.fn()} />);
    expect(readReactionSelection(window.getSelection(), messages)).toBeNull();
    select(screen.getByTestId('old').firstChild!, screen.getByTestId('new').firstChild!);
    expect(readReactionSelection(window.getSelection(), messages)).toBeNull();
    screen.getByTestId('old').setAttribute('contenteditable', 'true');
    select(screen.getByTestId('old').firstChild!);
    expect(readReactionSelection(window.getSelection(), messages)).toBeNull();
  });

  it('preserves a reaction on append failure and consumes Escape before lower handlers', async () => {
    const append = vi.fn(() => { throw new Error('unavailable'); });
    render(<Harness store={new InlineReactionStore()} append={append} />);
    select(screen.getByTestId('old').firstChild!);
    const input = await screen.findByRole('textbox');
    fireEvent.change(input, { target: { value: 'Keep' } });
    fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true, isComposing: true });
    expect(append).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
    expect(screen.getByRole('textbox')).toHaveValue('Keep');
    expect(screen.getByRole('status')).toHaveTextContent('Could not add');
    const lower = vi.fn();
    document.addEventListener('keydown', lower);
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(lower).not.toHaveBeenCalled();
    expect(screen.getByRole('group', { name: 'Discard this reaction?' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Keep writing' }));
    expect(screen.getByRole('textbox')).toHaveValue('Keep');
    document.removeEventListener('keydown', lower);
  });

  it('quotes complete multiline code without letting embedded fences terminate the quote', () => {
    const quote = '```ts\n' + 'long selected passage '.repeat(30) + '\n```';
    const formatted = formatInlineReaction({ source: { messageId: 'id', sequenceId: 9, quote }, body: 'Keep\nall of it' });
    expect(formatted).toContain('````text\n' + quote + '\n````\n\nKeep\nall of it');
  });

  it('resolves the selected occurrence when a message id appears in multiple segments', () => {
    render(<Harness store={new InlineReactionStore()} append={vi.fn()} />);
    const paragraph = screen.getByTestId('new');
    paragraph.closest('[data-inline-reaction-message]')!.setAttribute('data-message-occurrence', 'row-new:new');
    select(paragraph.firstChild!);
    const repeated = messages.map((message) => ({ ...message, message_id: 'repeated-id' }));
    const selection = readReactionSelection(window.getSelection(), repeated);
    expect(selection?.source.occurrenceToken).toBe('row-new:new');
    expect(selection?.source.messageId).toBe('repeated-id');
  });
});
