import { createRef } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ReactionPill } from './ReactionPill';
import { restoreReactionRange } from './reactionRange';
import { FocusScopeProvider } from '../hooks/useFocusScope';
import type { ReactionSource } from '../conversation/InlineReactionStore';

const source: ReactionSource = { messageId: 'answer', sequenceId: 2, occurrenceToken: 'earlier:answer', quote: 'second', textAnchor: { start: { fragmentId: 'text-0', offset: 6 }, end: { fragmentId: 'text-0', offset: 12 } } };
const add = vi.fn();
const close = vi.fn();
const navigate = vi.fn<(source: ReactionSource, signal: AbortSignal) => boolean | Promise<boolean>>(() => true);
let offscreen = false;
function Fixture({ mounted = true, body = 'Keep this guarantee', touchDocked = false, captureSource }: { mounted?: boolean; body?: string; touchDocked?: boolean; captureSource?: () => void }) {
  return <FocusScopeProvider>
    <div id="messages">
      {mounted && <div data-inline-reaction-message="answer" data-message-occurrence="earlier:answer"><div className="agent-text-block" data-fragment-id="text-0">first <strong>second</strong> third</div></div>}
    </div>
    <footer id="input-area" />
    <ReactionPill source={source} touchDocked={touchDocked} {...(captureSource ? { captureSource } : {})} bubbleRef={createRef<HTMLDivElement>()} scopeId="test" body={body} available onChange={() => {}} onAdd={add} onClose={close} returnToSource={navigate} />
  </FocusScopeProvider>;
}

beforeEach(() => {
  offscreen = false;
  vi.clearAllMocks();
  navigate.mockImplementation(() => true);
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

  it('leaves Enter to native disclosure controls and custom focus stops', () => {
    render(<><Fixture /><details><summary>Details</summary>Content</details><div tabIndex={0}>Custom control</div></>);
    for (const control of [screen.getByText('Details'), screen.getByText('Custom control')]) {
      control.focus();
      expect(fireEvent.keyDown(control, { key: 'Enter' })).toBe(true);
      expect(control).toHaveFocus();
    }
  });

  it('restores prose independently of preceding tool/header text and fails closed on changed prose', () => {
    const view = render(<Fixture />);
    const owner = view.container.querySelector('[data-inline-reaction-message]')!;
    const tool = document.createElement('div');
    tool.textContent = 'Expanded thinking and changing tool status';
    owner.prepend(tool);
    expect(restoreReactionRange(source)?.toString()).toBe('second');
    tool.remove();
    expect(restoreReactionRange(source)?.toString()).toBe('second');
    owner.querySelector('strong')!.textContent = 'WRONG!';
    expect(restoreReactionRange(source)).toBeNull();
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
    expect(navigate).toHaveBeenCalledWith(source, expect.any(AbortSignal));
    view.rerender(<Fixture />);
    await waitFor(() => expect(screen.getByRole('textbox')).toHaveValue('Keep this guarantee'));
    expect(window.getSelection()?.toString()).toBe('second');
    expect(screen.getByRole('textbox')).not.toHaveFocus();
  });

  it('keeps the dock usable after an asynchronous load failure and aborts on unmount', async () => {
    let finish!: (found: boolean) => void;
    navigate.mockImplementation(() => new Promise<boolean>((resolve) => { finish = resolve; }));
    const view = render(<Fixture mounted={false} />);
    fireEvent.click(screen.getByRole('button', { name: /Return to passage/ }));
    expect(screen.getByRole('button', { name: /Returning to passage/ })).toBeDisabled();
    await act(async () => { finish(false); });
    const retry = screen.getByRole('button', { name: /Passage unavailable/ });
    expect(retry).toBeEnabled();
    fireEvent.click(retry);
    const signal = navigate.mock.calls[1]![1];
    view.unmount();
    expect(signal.aborted).toBe(true);
    await act(async () => { finish(false); });
  });

  it('shows touch users a retryable source-return failure', async () => {
    navigate.mockResolvedValue(false);
    render(<Fixture mounted={false} touchDocked />);
    fireEvent.click(screen.getByRole('button', { name: /Return to passage/ }));
    expect(await screen.findByRole('button', { name: /Return to passage/ })).toHaveTextContent('Passage unavailable. Your reaction is saved here.');
  });

  it('clears a touch return error when the source mounts through another retry path', async () => {
    navigate.mockResolvedValue(false);
    const view = render(<Fixture mounted={false} touchDocked />);
    fireEvent.click(screen.getByRole('button', { name: /Return to passage/ }));
    expect(await screen.findByRole('button', { name: /Return to passage/ })).toHaveTextContent('Passage unavailable. Your reaction is saved here.');
    view.rerender(<Fixture touchDocked />);
    await waitFor(() => expect(screen.queryByRole('button', { name: /Return to passage/ })).not.toBeInTheDocument());
    view.rerender(<Fixture mounted={false} touchDocked />);
    expect(await screen.findByRole('button', { name: /Return to passage/ })).toHaveTextContent('“second”');
  });

  it('reserves transcript space equal to the touch dock height and releases it on unmount', () => {
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      if (this.classList.contains('reaction-pill')) return new DOMRect(0, 0, 366, 54);
      return new DOMRect(0, 0, 390, 700);
    });
    const view = render(<Fixture touchDocked />);
    const scroller = document.getElementById('messages')!;
    expect(scroller).toHaveClass('reaction-dock-reserved');
    expect(scroller.style.getPropertyValue('--reaction-dock-height')).toBe('66px');
    view.unmount();
    expect(scroller).not.toHaveClass('reaction-dock-reserved');
    expect(scroller.style.getPropertyValue('--reaction-dock-height')).toBe('');
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

  it('uses the unfocused editor dock on touch and captures the source before focus', () => {
    const captureSource = vi.fn();
    render(<Fixture touchDocked captureSource={captureSource} body="" />);
    const dock = screen.getByRole('region', { name: 'Docked reaction' });
    const input = screen.getByRole('textbox', { name: 'Your reaction' });
    expect(dock).toHaveTextContent('“second”');
    expect(input).not.toHaveFocus();
    expect(screen.queryByRole('button', { name: /Return to passage/ })).not.toBeInTheDocument();
    fireEvent.pointerDown(input, { pointerType: 'touch' });
    expect(captureSource).toHaveBeenCalledOnce();
    input.focus();
    expect(input).toHaveFocus();
  });

  it('keeps the touch dock above the composer as the visual viewport resizes for the keyboard', async () => {
    const listeners = new Map<string, EventListener>();
    const viewport = {
      offsetLeft: 0,
      offsetTop: 0,
      width: 390,
      height: 700,
      addEventListener: vi.fn((type: string, listener: EventListener) => listeners.set(type, listener)),
      removeEventListener: vi.fn(),
    };
    Object.defineProperty(window, 'visualViewport', { configurable: true, value: viewport });
    let composerTop = 620;
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      if (this.id === 'input-area') return new DOMRect(0, composerTop, 390, 80);
      if (this.classList.contains('reaction-pill')) return new DOMRect(0, 0, 366, 54);
      return new DOMRect(0, 0, 390, 700);
    });
    render(<Fixture touchDocked body="" />);
    const dock = screen.getByRole('region', { name: 'Docked reaction' });
    expect(dock).toHaveStyle({ top: '554px' });
    screen.getByRole('textbox').focus();
    viewport.height = 420;
    composerTop = 340;
    act(() => listeners.get('resize')?.(new Event('resize')));
    await waitFor(() => expect(dock).toHaveStyle({ top: '274px' }));
    expect(screen.getByRole('textbox')).toHaveFocus();
  });

  it('clamps the touch dock inside visual-viewport safe-area insets', () => {
    Object.defineProperty(window, 'visualViewport', { configurable: true, value: {
      offsetLeft: 0, offsetTop: 0, width: 390, height: 700,
      addEventListener: vi.fn(), removeEventListener: vi.fn(),
    } });
    document.documentElement.style.setProperty('--safe-area-top', '20px');
    document.documentElement.style.setProperty('--safe-area-right', '18px');
    document.documentElement.style.setProperty('--safe-area-bottom', '24px');
    document.documentElement.style.setProperty('--safe-area-left', '16px');
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      if (this.id === 'input-area') return new DOMRect(0, 640, 390, 60);
      if (this.classList.contains('reaction-pill')) return new DOMRect(0, 0, 332, 54);
      return new DOMRect(0, 0, 390, 700);
    });
    render(<Fixture touchDocked body="" />);
    expect(screen.getByRole('region', { name: 'Docked reaction' })).toHaveStyle({ left: '28px', top: '574px', width: '332px' });
    document.documentElement.style.removeProperty('--safe-area-top');
    document.documentElement.style.removeProperty('--safe-area-right');
    document.documentElement.style.removeProperty('--safe-area-bottom');
    document.documentElement.style.removeProperty('--safe-area-left');
  });

  it('keeps or discards a typed touch-dock reaction without changing its source', () => {
    render(<Fixture touchDocked />);
    expect(screen.getByRole('region', { name: 'Docked reaction' })).toHaveTextContent('“second”');
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss reaction' }));
    fireEvent.click(screen.getByRole('button', { name: 'Keep' }));
    expect(screen.getByRole('textbox')).toHaveValue('Keep this guarantee');
    expect(screen.getByText('“second”')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss reaction' }));
    fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
    expect(close).toHaveBeenCalledOnce();
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
