import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { BrowserViewPanel } from './BrowserViewPanel';
import { api } from '../api';

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    api: { ...actual.api, stopConversationBrowserSession: vi.fn() },
  };
});

const stopBrowser = vi.mocked(api.stopConversationBrowserSession);

const sockets: MockWebSocket[] = [];

function statusMessage(status: string): MessageEvent<ArrayBuffer> {
  const text = new TextEncoder().encode(status);
  const payload = new Uint8Array(text.length + 1);
  payload[0] = 0x02;
  payload.set(text, 1);
  return { data: payload.buffer } as MessageEvent<ArrayBuffer>;
}

class MockWebSocket {
  binaryType = 'arraybuffer';
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: MessageEvent<ArrayBuffer>) => void) | null = null;

  constructor(public readonly url: string) {
    sockets.push(this);
  }

  close() {
    this.onclose?.();
  }
}

describe('BrowserViewPanel stop-session control', () => {
  const originalWebSocket = globalThis.WebSocket;

  beforeEach(() => {
    stopBrowser.mockReset();
    sockets.length = 0;
    globalThis.WebSocket = MockWebSocket as unknown as typeof WebSocket;
  });

  afterEach(() => {
    globalThis.WebSocket = originalWebSocket;
  });

  it('renders mobile back-to-conversation and stop-session as separate controls', () => {
    render(<BrowserViewPanel conversationId="conv-1" onClose={() => {}} />);

    expect(screen.getByLabelText('Back to conversation')).toBeTruthy();
    expect(screen.queryByLabelText('Close browser view')).toBeNull();
    expect(screen.getByLabelText('Stop browser session')).toBeTruthy();
    expect(screen.getByText('Stop browser')).toBeTruthy();
  });

  it('keeps the compact close control for inline panels', () => {
    render(<BrowserViewPanel conversationId="conv-1" onClose={() => {}} inline />);

    expect(screen.getByLabelText('Close browser view')).toBeTruthy();
    expect(screen.queryByLabelText('Back to conversation')).toBeNull();
  });

  it('uses the large back control for an inline narrow-layout takeover', () => {
    render(<BrowserViewPanel conversationId="conv-1" onClose={() => {}} inline takeover />);

    expect(screen.getByLabelText('Back to conversation')).toBeTruthy();
    expect(screen.queryByLabelText('Close browser view')).toBeNull();
  });

  it('clicking stop calls the conversation browser-session endpoint', async () => {
    stopBrowser.mockResolvedValue({ success: true });
    render(<BrowserViewPanel conversationId="conv-1" />);

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Stop browser session'));
      await Promise.resolve();
    });

    expect(stopBrowser).toHaveBeenCalledWith('conv-1');
    expect(screen.getByLabelText('Stop browser session')).toBeDisabled();
    expect(screen.getByLabelText('Browser view status: stopping')).toBeTruthy();
    expect(screen.getByRole('status').textContent).toBe('Stopping browser…');
  });

  it.each(['no-session', 'ended'])('settles pending stop when websocket reports %s', async (terminalStatus) => {
    stopBrowser.mockResolvedValue({ success: true });
    render(<BrowserViewPanel conversationId="conv-1" />);

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Stop browser session'));
      await Promise.resolve();
    });
    expect(screen.getByLabelText('Stop browser session')).toBeDisabled();

    act(() => { sockets[0]!.onmessage?.(statusMessage(terminalStatus)); });

    expect(screen.getByLabelText('Stop browser session')).not.toBeDisabled();
    expect(screen.queryByText('Stopping browser…')).toBeNull();
  });

  it.each(['no-session', 'ended'])('does not enter pending stop from an already-%s state', async (terminalStatus) => {
    stopBrowser.mockResolvedValue({ success: true });
    render(<BrowserViewPanel conversationId="conv-1" />);
    act(() => { sockets[0]!.onmessage?.(statusMessage(terminalStatus)); });

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Stop browser session'));
      await Promise.resolve();
    });

    expect(stopBrowser).toHaveBeenCalledWith('conv-1');
    expect(screen.getByLabelText('Stop browser session')).not.toBeDisabled();
    expect(screen.queryByText('Stopping browser…')).toBeNull();
  });

  it('back to conversation remains available while teardown is pending', async () => {
    const onClose = vi.fn();
    stopBrowser.mockResolvedValue({ success: true });
    render(<BrowserViewPanel conversationId="conv-1" onClose={onClose} />);

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Stop browser session'));
      await Promise.resolve();
    });
    fireEvent.click(screen.getByLabelText('Back to conversation'));

    expect(onClose).toHaveBeenCalledOnce();
  });

  it('stop failure is rendered visibly and allows retry', async () => {
    stopBrowser.mockRejectedValue(new Error('stop failed'));
    render(<BrowserViewPanel conversationId="conv-1" />);

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Stop browser session'));
      await Promise.resolve();
    });

    expect(screen.getByRole('alert').textContent).toContain('stop failed');
    expect(screen.getByLabelText('Stop browser session')).not.toBeDisabled();
  });
});
