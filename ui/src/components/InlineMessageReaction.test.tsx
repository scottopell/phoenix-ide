import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Message } from '../api';
import { InlineReactionContext, InlineReactionStore, formatInlineReaction } from '../conversation/InlineReactionStore';
import { DraftStore } from '../conversation/DraftStore';
import { FocusScopeProvider, useRegisterFocusScope } from '../hooks/useFocusScope';
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
const initialVisualViewport = window.visualViewport;

function select(start: Node, end = start) {
  const range = document.createRange();
  range.setStart(start, 0);
  range.setEnd(end, end.textContent?.length ?? 0);
  window.getSelection()!.removeAllRanges();
  window.getSelection()!.addRange(range);
  fireEvent(document, new Event('selectionchange'));
}

function setCoarsePointer(matches: boolean) {
  vi.spyOn(window, 'matchMedia').mockImplementation((query) => ({
    matches: matches && query === '(any-pointer: coarse)',
    media: query,
    onchange: null,
    addListener: vi.fn(), removeListener: vi.fn(), addEventListener: vi.fn(), removeEventListener: vi.fn(), dispatchEvent: vi.fn(),
  }));
}

function OtherScope() {
  useRegisterFocusScope('other-scope');
  return <button type="button">Other scope control</button>;
}

function Harness({ store, scope = 'conversation-a', append, sourceMounted = true, otherScope = false, returnToSource, tokenless = false }: { store: InlineReactionStore; scope?: string; append?: ((text: string) => void) | undefined; sourceMounted?: boolean; otherScope?: boolean; returnToSource?: () => boolean | Promise<boolean>; tokenless?: boolean }) {
  const renderedMessages = tokenless ? messages.map((message) => ({ ...message, display_data: {} })) : messages;
  return (
    <FocusScopeProvider>
      <InlineReactionContext.Provider value={store}>
        <div id="messages">
          {renderedMessages.filter((message) => sourceMounted || message.message_id !== 'old').map((message) => (
            <div key={message.message_id} className="message agent" data-inline-reaction-message={message.message_id} data-message-occurrence={tokenless ? undefined : `${message.conversation_id}:${message.message_id}`} data-message-id={message.message_id} data-sequence-id="2">
              <div className="agent-text-block" data-fragment-id="text-0"><p data-testid={message.message_id}>Deterministic state patterns <code>replay(events)</code></p></div>
            </div>
          ))}
        </div>
        <button type="button" data-testid="unrelated">Unrelated surface</button>
        <InlineMessageReaction scopeKey={scope} messages={renderedMessages} destination={append ? { append } : undefined} returnToSource={returnToSource} />
        {otherScope && <OtherScope />}
        <MessageContextMenu messages={messages} />
        <FilePathContextMenu />
      </InlineReactionContext.Provider>
    </FocusScopeProvider>
  );
}

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    return this.classList.contains('reaction-pill')
      ? new DOMRect(0, 0, 420, 54)
      : new DOMRect(0, 0, 600, 500);
  });
  Object.defineProperty(Range.prototype, 'getBoundingClientRect', {
    configurable: true,
    value: () => ({ left: 50, top: 50, bottom: 80, right: 300, width: 250, height: 30 }),
  });
  Object.defineProperty(Range.prototype, 'getClientRects', {
    configurable: true,
    value: () => [new DOMRect(50, 50, 250, 30)],
  });
});
afterEach(() => {
  cleanup();
  window.getSelection()?.removeAllRanges();
  Object.defineProperty(window, 'visualViewport', { configurable: true, value: initialVisualViewport });
  vi.restoreAllMocks();
});

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
    fireEvent.change(input, { target: { value: 'Strong idea. Please test cancellation too.' } });
    drafts.dispatch('latest', { type: 'set_draft', text: 'Draft edited while reacting  ' });
    const add = screen.getByRole('button', { name: 'Add to draft' });
    fireEvent.click(add);
    fireEvent.click(add);
    expect(append).toHaveBeenCalledTimes(1);
    expect(drafts.getSnapshot('latest').draft).toContain('Draft edited while reacting  \n\nRegarding message #2 (row-old:old):');
    expect(drafts.getSnapshot('latest').draft).toContain('Deterministic state patterns ');
    expect(drafts.getSnapshot('latest').draft).toContain('Strong idea. Please test cancellation too.');
    expect(screen.queryByRole('textbox', { name: 'Your reaction' })).not.toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('Added to draft');
  });

  it('does not replace an unchanged source when tapping or focusing the reaction editor', async () => {
    setCoarsePointer(true);
    const store = new InlineReactionStore();
    render(<Harness store={store} append={vi.fn()} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'touch' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    const input = await screen.findByRole('textbox', { name: 'Your reaction' });
    const source = store.getSnapshot('conversation-a')?.source;
    expect(source).toBeDefined();
    const dispatch = vi.spyOn(store, 'dispatch');
    fireEvent.pointerDown(input, { pointerType: 'touch' });
    input.focus();
    expect(store.getSnapshot('conversation-a')?.source).toBe(source);
    expect(dispatch).not.toHaveBeenCalled();
  });

  it('uses the mobile dock for coarse-pointer selection without autofocus and captures before focus clears selection', async () => {
    setCoarsePointer(true);
    const store = new InlineReactionStore();
    const drafts = new DraftStore();
    drafts.dispatch('latest', { type: 'set_draft', text: 'Existing mobile draft' });
    const append = vi.fn((text: string) => drafts.dispatch('latest', { type: 'append_draft', text }));
    render(<Harness store={store} append={append} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'touch' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    const dock = await screen.findByRole('region', { name: 'Docked reaction' });
    const input = screen.getByRole('textbox', { name: 'Your reaction' });
    expect(dock).toHaveTextContent('Deterministic state patterns');
    expect(input).not.toHaveFocus();
    const adjusted = document.createRange();
    adjusted.setStart(text, 0);
    adjusted.setEnd(text, 13);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(adjusted);
    const original = store.getSnapshot('conversation-a')?.source;
    const dispatch = vi.spyOn(store, 'dispatch');
    fireEvent.pointerDown(input, { pointerType: 'touch' });
    const captured = store.getSnapshot('conversation-a')?.source;
    expect(captured?.quote).toBe('Deterministic');
    expect(captured).not.toBe(original);
    expect(dispatch).toHaveBeenCalledWith('conversation-a', expect.objectContaining({ type: 'select', source: captured }));
    window.getSelection()?.removeAllRanges();
    fireEvent(document, new Event('selectionchange'));
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(store.getSnapshot('conversation-a')?.source).toEqual(captured);
    input.focus();
    expect(store.getSnapshot('conversation-a')?.source).toEqual(captured);
    fireEvent.change(input, { target: { value: 'Keep this mobile observation' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add to draft' }));
    expect(append).toHaveBeenCalledOnce();
    expect(drafts.getSnapshot('latest').draft).toBe(`Existing mobile draft\n\n${formatInlineReaction({ source: captured!, body: 'Keep this mobile observation' })}`);
  });

  it.each([false, true])('keeps mouse selection in the floating pill when coarse-pointer capability is %s', async (coarse) => {
    setCoarsePointer(coarse);
    render(<Harness store={new InlineReactionStore()} append={vi.fn()} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'mouse' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'mouse' });
    expect(await screen.findByRole('region', { name: 'React to selected text' })).toBeInTheDocument();
    expect(screen.queryByText(/“Deterministic state patterns/)).not.toBeInTheDocument();
  });

  it('preserves mouse modality when messages rerender before pointerup on a hybrid device', async () => {
    setCoarsePointer(true);
    const store = new InlineReactionStore();
    const view = render(<Harness store={store} append={vi.fn()} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'mouse' });
    select(text);
    view.rerender(<Harness store={store} append={vi.fn()} />);
    fireEvent.pointerUp(screen.getByTestId('old'), { pointerType: 'mouse' });
    expect(await screen.findByRole('region', { name: 'React to selected text' })).toBeInTheDocument();
    expect(store.getSnapshot('conversation-a')?.presentation).toBe('floating');
  });

  it('does not let touch scrolling in a wrapped-selection gap retarget an unchanged mouse selection', async () => {
    setCoarsePointer(true);
    vi.spyOn(Range.prototype, 'getBoundingClientRect').mockReturnValue(new DOMRect(50, 50, 250, 100));
    vi.spyOn(Range.prototype, 'getClientRects').mockReturnValue([
      new DOMRect(50, 50, 250, 20),
      new DOMRect(50, 130, 250, 20),
    ] as unknown as DOMRectList);
    const store = new InlineReactionStore();
    render(<Harness store={store} append={vi.fn()} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'mouse' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'mouse' });
    expect(await screen.findByRole('region', { name: 'React to selected text' })).toBeInTheDocument();
    const source = store.getSnapshot('conversation-a')?.source;
    const dispatch = vi.spyOn(store, 'dispatch');
    const scrollTarget = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(scrollTarget, { pointerType: 'touch', clientX: 150, clientY: 100 });
    fireEvent(document, new Event('selectionchange'));
    await act(async () => { await new Promise(requestAnimationFrame); });
    fireEvent.pointerUp(scrollTarget, { pointerType: 'touch' });
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(dispatch).not.toHaveBeenCalled();
    expect(store.getSnapshot('conversation-a')?.source).toBe(source);
    expect(store.getSnapshot('conversation-a')?.presentation).toBe('floating');
    expect(screen.getByRole('region', { name: 'React to selected text' })).toBeInTheDocument();
  });

  it('uses the current gesture when the same range is deliberately reselected', async () => {
    setCoarsePointer(true);
    const store = new InlineReactionStore();
    render(<Harness store={store} append={vi.fn()} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'mouse' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'mouse' });
    expect(await screen.findByRole('region', { name: 'React to selected text' })).toBeInTheDocument();
    fireEvent.pointerDown(text, { pointerType: 'touch', clientX: 100, clientY: 60 });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    expect(await screen.findByRole('region', { name: 'Docked reaction' })).toBeInTheDocument();
    expect(store.getSnapshot('conversation-a')?.presentation).toBe('touch-docked');
  });

  it('retains gesture modality through a provisional native collapse', async () => {
    setCoarsePointer(true);
    const store = new InlineReactionStore();
    render(<Harness store={store} append={vi.fn()} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'mouse' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'mouse' });
    expect(await screen.findByRole('region', { name: 'React to selected text' })).toBeInTheDocument();

    fireEvent.pointerDown(text, { pointerType: 'touch', clientX: 100, clientY: 60 });
    select(text);
    await act(async () => { await new Promise(requestAnimationFrame); });
    window.getSelection()?.removeAllRanges();
    fireEvent(document, new Event('selectionchange'));
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(store.getSnapshot('conversation-a')?.presentation).toBe('touch-docked');
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    expect(await screen.findByRole('region', { name: 'Docked reaction' })).toBeInTheDocument();
    expect(store.getSnapshot('conversation-a')?.presentation).toBe('touch-docked');
  });

  it('uses touch presentation when reselecting a tokenless source', async () => {
    setCoarsePointer(true);
    const store = new InlineReactionStore();
    render(<Harness store={store} append={vi.fn()} tokenless />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'mouse' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'mouse' });
    expect(await screen.findByRole('region', { name: 'React to selected text' })).toBeInTheDocument();
    expect(store.getSnapshot('conversation-a')?.source.occurrenceToken).toBeUndefined();
    fireEvent.pointerDown(text, { pointerType: 'touch', clientX: 100, clientY: 60 });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    expect(await screen.findByRole('region', { name: 'Docked reaction' })).toBeInTheDocument();
    expect(store.getSnapshot('conversation-a')?.presentation).toBe('touch-docked');
  });

  it('keeps keyboard selection in the floating pill on a hybrid device', async () => {
    setCoarsePointer(true);
    render(<Harness store={new InlineReactionStore()} append={vi.fn()} />);
    fireEvent.keyDown(document, { key: 'ArrowRight', shiftKey: true });
    select(screen.getByTestId('old').firstChild!);
    expect(await screen.findByRole('region', { name: 'React to selected text' })).toBeInTheDocument();
  });
  it('preserves touch docking when native selection handles adjust the range without pointer events', async () => {
    setCoarsePointer(true);
    const store = new InlineReactionStore();
    render(<Harness store={store} append={vi.fn()} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'touch' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    expect(await screen.findByRole('region', { name: 'Docked reaction' })).toBeInTheDocument();
    select(screen.getByTestId('new').firstChild!);
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(store.getSnapshot('conversation-a')?.source.messageId).toBe('new');
    expect(store.getSnapshot('conversation-a')?.presentation).toBe('touch-docked');
  });

  it('clears ignored pointer modality after a typed reaction gesture', async () => {
    setCoarsePointer(true);
    const store = new InlineReactionStore();
    render(<Harness store={store} append={vi.fn()} />);
    const passageText = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(passageText, { pointerType: 'touch' });
    select(passageText);
    fireEvent.pointerUp(passageText, { pointerType: 'touch' });
    const input = await screen.findByRole('textbox');
    fireEvent.change(input, { target: { value: 'Pinned body' } });
    const retainedSource = structuredClone(store.getSnapshot('conversation-a')!.source);
    const other = screen.getByTestId('new').firstChild!;
    fireEvent.pointerDown(other, { pointerType: 'mouse' });
    select(other);
    fireEvent.pointerUp(other, { pointerType: 'mouse' });
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(store.getSnapshot('conversation-a')?.source).toEqual(retainedSource);
    const restored = document.createRange();
    restored.setStart(passageText, retainedSource.textAnchor!.start.offset);
    restored.setEnd(passageText, retainedSource.textAnchor!.end.offset);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(restored);
    fireEvent(document, new Event('selectionchange'));
    fireEvent.change(input, { target: { value: '' } });
    fireEvent(document, new Event('selectionchange'));
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(store.getSnapshot('conversation-a')?.source).toEqual(retainedSource);
    expect(store.getSnapshot('conversation-a')?.presentation).toBe('touch-docked');
  });

  it('keeps or discards the exact touch-docked reaction in the existing store', async () => {
    setCoarsePointer(true);
    const store = new InlineReactionStore();
    render(<Harness store={store} append={vi.fn()} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'touch' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    fireEvent.change(await screen.findByRole('textbox'), { target: { value: 'Retain this' } });
    const owned = store.getSnapshot('conversation-a');
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss reaction' }));
    fireEvent.click(screen.getByRole('button', { name: 'Keep' }));
    expect(store.getSnapshot('conversation-a')).toEqual(owned);
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss reaction' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
    expect(store.getSnapshot('conversation-a')).toBeNull();
  });

  it('restores a retained touch reaction as a dock after conversation navigation', async () => {
    setCoarsePointer(true);
    const store = new InlineReactionStore();
    const append = vi.fn();
    const view = render(<Harness store={store} append={append} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'touch' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    fireEvent.change(await screen.findByRole('textbox'), { target: { value: 'Retain presentation' } });
    view.rerender(<Harness store={store} scope="conversation-b" append={append} />);
    view.rerender(<Harness store={store} append={append} />);
    expect(screen.getByRole('region', { name: 'Docked reaction' })).toBeInTheDocument();
    expect(screen.getByRole('textbox')).toHaveValue('Retain presentation');
  });

  it('retains an empty touch reaction when scrolling virtualizes its selected source', async () => {
    setCoarsePointer(true);
    const store = new InlineReactionStore();
    const append = vi.fn();
    const view = render(<Harness store={store} append={append} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'touch' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    expect(await screen.findByRole('region', { name: 'Docked reaction' })).toBeInTheDocument();
    const owned = store.getSnapshot('conversation-a');
    expect(owned?.body).toBe('');
    view.rerender(<Harness store={store} append={append} sourceMounted={false} />);
    window.getSelection()?.removeAllRanges();
    fireEvent(document, new Event('selectionchange'));
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(store.getSnapshot('conversation-a')).toEqual(owned);
    expect(screen.getByRole('region', { name: 'Docked reaction' })).toBeInTheDocument();

    fireEvent.pointerDown(screen.getByTestId('unrelated'), { pointerType: 'mouse' });
    fireEvent.pointerUp(screen.getByTestId('unrelated'), { pointerType: 'mouse' });
    await act(async () => { await new Promise(requestAnimationFrame); });
    view.rerender(<Harness store={store} append={append} />);
    const restoredText = screen.getByTestId('old').firstChild!;
    const restored = document.createRange();
    restored.setStart(restoredText, owned!.source.textAnchor!.start.offset);
    restored.setEnd(restoredText, owned!.source.textAnchor!.end.offset);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(restored);
    fireEvent(document, new Event('selectionchange'));
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(store.getSnapshot('conversation-a')?.presentation).toBe('touch-docked');
  });

  it('preserves touch presentation when source restoration emits selectionchange', async () => {
    setCoarsePointer(false);
    const store = new InlineReactionStore();
    store.dispatch('conversation-a', {
      type: 'select', presentation: 'touch-docked',
      source: { messageId: 'old', sequenceId: 2, occurrenceToken: 'row-old:old', quote: 'Deterministic state patterns', textAnchor: { start: { fragmentId: 'text-0', offset: 0 }, end: { fragmentId: 'text-0', offset: 28 } } },
    });
    render(<Harness store={store} append={vi.fn()} />);
    const text = screen.getByTestId('old').firstChild!;
    const restored = document.createRange();
    restored.setStart(text, 0);
    restored.setEnd(text, 28);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(restored);
    fireEvent(document, new Event('selectionchange'));
    expect(await screen.findByRole('region', { name: 'Docked reaction' })).toBeInTheDocument();
    expect(store.getSnapshot('conversation-a')?.presentation).toBe('touch-docked');
  });

  it('ignores a hidden focus scope gesture when returning to a retained touch passage', async () => {
    setCoarsePointer(false);
    const store = new InlineReactionStore();
    store.dispatch('conversation-a', {
      type: 'select', presentation: 'touch-docked',
      source: { messageId: 'old', sequenceId: 2, occurrenceToken: 'row-old:old', quote: 'Deterministic state patterns', textAnchor: { start: { fragmentId: 'text-0', offset: 0 }, end: { fragmentId: 'text-0', offset: 28 } } },
    });
    let passageVisible = false;
    vi.spyOn(Range.prototype, 'getBoundingClientRect').mockImplementation(() =>
      passageVisible
        ? { left: 50, top: 50, bottom: 80, right: 300, width: 250, height: 30 } as DOMRect
        : { left: 50, top: 800, bottom: 830, right: 300, width: 250, height: 30 } as DOMRect);
    const append = vi.fn();
    const returnToSource = vi.fn(() => {
      passageVisible = true;
      window.dispatchEvent(new Event('scroll'));
      return true;
    });
    const view = render(<Harness store={store} append={append} returnToSource={returnToSource} />);
    expect(await screen.findByRole('button', { name: /Return to passage/ })).toBeInTheDocument();

    view.rerender(<Harness store={store} append={append} returnToSource={returnToSource} otherScope />);
    expect(document.querySelector('[role="region"][aria-label="Docked reaction"]')).toHaveAttribute('hidden');
    fireEvent.pointerDown(screen.getByRole('button', { name: 'Other scope control' }), { pointerType: 'mouse' });
    select(screen.getByTestId('old').firstChild!);
    fireEvent.pointerUp(screen.getByRole('button', { name: 'Other scope control' }), { pointerType: 'mouse' });
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(store.getSnapshot('conversation-a')?.presentation).toBe('touch-docked');

    view.rerender(<Harness store={store} append={append} returnToSource={returnToSource} />);
    fireEvent.keyDown(document, { key: 'Shift', shiftKey: true });
    const returnButton = screen.getByRole('button', { name: /Return to passage/ });
    fireEvent.pointerDown(returnButton, { pointerType: 'mouse' });
    fireEvent.click(returnButton);
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(store.getSnapshot('conversation-a')?.presentation).toBe('touch-docked');
    fireEvent(document, new Event('selectionchange'));
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(returnToSource).toHaveBeenCalledOnce();
    expect(store.getSnapshot('conversation-a')?.presentation).toBe('touch-docked');
    expect(screen.getByRole('region', { name: 'Docked reaction' })).toBeInTheDocument();
  });

  it('retains an empty touch reaction when its mounted source is clipped by the transcript', async () => {
    setCoarsePointer(true);
    vi.spyOn(Range.prototype, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 100, 100, 20));
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      if (this.id === 'messages') return new DOMRect(0, 200, 600, 500);
      return this.classList.contains('reaction-pill') ? new DOMRect(0, 0, 420, 54) : new DOMRect(0, 0, 600, 500);
    });
    const store = new InlineReactionStore();
    render(<Harness store={store} append={vi.fn()} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'touch' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    expect(await screen.findByRole('button', { name: /Return to passage/ })).toBeInTheDocument();
    const retained = store.getSnapshot('conversation-a');
    window.getSelection()?.removeAllRanges();
    act(() => { fireEvent(document, new Event('selectionchange')); });
    expect(store.getSnapshot('conversation-a')).toEqual(retained);
    expect(screen.getByRole('button', { name: /Return to passage/ })).toBeInTheDocument();
  });

  it('retains an empty source inside the reserved dock band of a clipped visual viewport', async () => {
    setCoarsePointer(true);
    Object.defineProperty(window, 'visualViewport', { configurable: true, value: {
      offsetLeft: 0, offsetTop: 0, width: 600, height: 400,
      addEventListener: vi.fn(), removeEventListener: vi.fn(),
    } });
    vi.spyOn(Range.prototype, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 350, 100, 20));
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      return this.classList.contains('reaction-pill') ? new DOMRect(0, 0, 420, 54) : new DOMRect(0, 0, 600, 500);
    });
    const store = new InlineReactionStore();
    render(<Harness store={store} append={vi.fn()} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'touch' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    expect(await screen.findByRole('region', { name: 'Docked reaction' })).toBeInTheDocument();
    const transcript = document.getElementById('messages')!;
    transcript.classList.add('reaction-dock-reserved');
    transcript.style.setProperty('--reaction-dock-height', '70px');
    const retained = store.getSnapshot('conversation-a');
    expect(retained).not.toBeNull();
    window.getSelection()?.removeAllRanges();
    act(() => { fireEvent(document, new Event('selectionchange')); });
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(store.getSnapshot('conversation-a')).toEqual(retained);
  });

  it('protects a reaction until delayed source return completes', async () => {
    setCoarsePointer(true);
    vi.spyOn(Range.prototype, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, -120, 100, 20));
    let finish!: (found: boolean) => void;
    const returnToSource = vi.fn(() => new Promise<boolean>((resolve) => { finish = resolve; }));
    const store = new InlineReactionStore();
    render(<Harness store={store} append={vi.fn()} returnToSource={returnToSource} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'touch' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    const returnButton = await screen.findByRole('button', { name: /Return to passage/ });
    fireEvent.pointerDown(returnButton, { pointerType: 'touch' });
    fireEvent.click(returnButton);
    window.getSelection()?.removeAllRanges();
    fireEvent(document, new Event('selectionchange'));
    await act(async () => {
      await new Promise(requestAnimationFrame);
      await new Promise(requestAnimationFrame);
      await new Promise(requestAnimationFrame);
    });
    expect(store.getSnapshot('conversation-a')).not.toBeNull();
    expect(screen.getByRole('button', { name: /Return to passage/ })).toBeDisabled();
    await act(async () => { finish(false); });
    expect(store.getSnapshot('conversation-a')).not.toBeNull();
  });

  it('clears an empty touch reaction when its mounted selection collapses', async () => {
    setCoarsePointer(true);
    vi.spyOn(Range.prototype, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 100, 100, 20));
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      return this.classList.contains('reaction-pill') ? new DOMRect(0, 0, 420, 54) : new DOMRect(0, 0, 600, 500);
    });
    const store = new InlineReactionStore();
    render(<Harness store={store} append={vi.fn()} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'touch' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    expect(await screen.findByRole('region', { name: 'Docked reaction' })).toBeInTheDocument();
    window.getSelection()?.removeAllRanges();
    act(() => { fireEvent(document, new Event('selectionchange')); });
    expect(store.getSnapshot('conversation-a')).toBeNull();
  });

  it('clears an empty touch reaction when a live excluded selection replaces it', async () => {
    setCoarsePointer(true);
    const store = new InlineReactionStore();
    render(<Harness store={store} append={vi.fn()} />);
    const text = screen.getByTestId('old').firstChild!;
    fireEvent.pointerDown(text, { pointerType: 'touch' });
    select(text);
    fireEvent.pointerUp(text, { pointerType: 'touch' });
    expect(await screen.findByRole('region', { name: 'Docked reaction' })).toBeInTheDocument();
    const unrelatedText = screen.getByTestId('unrelated').firstChild!;
    const range = document.createRange();
    range.selectNodeContents(unrelatedText);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);
    fireEvent(document, new Event('selectionchange'));
    await act(async () => { await new Promise(requestAnimationFrame); });
    expect(store.getSnapshot('conversation-a')).toBeNull();
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
    expect(screen.getByText('Discard reaction?')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Keep' }));
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
