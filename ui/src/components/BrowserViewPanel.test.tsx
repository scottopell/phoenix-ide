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

function frameMessage(): MessageEvent<ArrayBuffer> {
  return { data: new Uint8Array([0x00, 0, 0, 0, 0]).buffer } as MessageEvent<ArrayBuffer>;
}

const bitmap = {
  width: 1,
  height: 1,
  close: vi.fn(),
} as unknown as ImageBitmap;

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
  const originalCreateImageBitmap = globalThis.createImageBitmap;

  beforeEach(() => {
    stopBrowser.mockReset();
    sockets.length = 0;
    globalThis.WebSocket = MockWebSocket as unknown as typeof WebSocket;
    globalThis.createImageBitmap = vi.fn().mockResolvedValue(bitmap);
  });

  afterEach(() => {
    vi.useRealTimers();
    globalThis.WebSocket = originalWebSocket;
    globalThis.createImageBitmap = originalCreateImageBitmap;
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

  it('does not enter pending stop from an existing error state', async () => {
    stopBrowser.mockResolvedValue({ success: true });
    render(<BrowserViewPanel conversationId="conv-1" />);
    act(() => { sockets[0]!.onmessage?.(statusMessage('error: attach failed')); });

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Stop browser session'));
      await Promise.resolve();
    });

    expect(stopBrowser).toHaveBeenCalledWith('conv-1');
    expect(screen.getByLabelText('Stop browser session')).not.toBeDisabled();
    expect(screen.queryByText('Stopping browser…')).toBeNull();
  });

  it('settles a connecting stop when the socket closes without status', () => {
    stopBrowser.mockReturnValue(new Promise(() => {}));
    render(<BrowserViewPanel conversationId="conv-1" />);
    fireEvent.click(screen.getByLabelText('Stop browser session'));
    expect(screen.getByLabelText('Stop browser session')).toBeDisabled();

    act(() => { sockets[0]!.onclose?.(); });

    expect(screen.getByLabelText('Stop browser session')).not.toBeDisabled();
    expect(screen.queryByText('Stopping browser…')).toBeNull();
  });

  it('settles pending stop when websocket reports an error status', async () => {
    stopBrowser.mockResolvedValue({ success: true });
    render(<BrowserViewPanel conversationId="conv-1" />);
    fireEvent.click(screen.getByLabelText('Stop browser session'));

    act(() => { sockets[0]!.onmessage?.(statusMessage('error: attach failed')); });

    expect(screen.getByLabelText('Stop browser session')).not.toBeDisabled();
    expect(screen.getByLabelText('Browser view status: error')).toBeTruthy();
    expect(screen.queryByText('Stopping browser…')).toBeNull();
  });

  it('settles pending stop when the websocket errors', async () => {
    stopBrowser.mockResolvedValue({ success: true });
    render(<BrowserViewPanel conversationId="conv-1" />);
    fireEvent.click(screen.getByLabelText('Stop browser session'));

    act(() => { sockets[0]!.onerror?.(); });

    expect(screen.getByLabelText('Stop browser session')).not.toBeDisabled();
    expect(screen.getByLabelText('Browser view status: error')).toBeTruthy();
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

  it('restores live status when a frame was suppressed before stop failed', async () => {
    let rejectStop: (error: Error) => void = () => {};
    stopBrowser.mockReturnValue(new Promise((_, reject) => { rejectStop = reject; }));
    render(<BrowserViewPanel conversationId="conv-1" />);

    fireEvent.click(screen.getByLabelText('Stop browser session'));
    act(() => { sockets[0]!.onmessage?.(frameMessage()); });
    await act(async () => { rejectStop(new Error('stop failed')); await Promise.resolve(); });

    expect(screen.getByLabelText('Browser view status: live')).toBeTruthy();
    expect(screen.getByRole('alert').textContent).toContain('stop failed');
  });

  it('reconnects when socket closure was suppressed before stop failed', async () => {
    let rejectStop: (error: Error) => void = () => {};
    stopBrowser.mockReturnValue(new Promise((_, reject) => { rejectStop = reject; }));
    render(<BrowserViewPanel conversationId="conv-1" />);
    act(() => { sockets[0]!.onmessage?.(statusMessage('started')); });

    fireEvent.click(screen.getByLabelText('Stop browser session'));
    act(() => { sockets[0]!.onclose?.(); });
    await act(async () => { rejectStop(new Error('stop failed')); await Promise.resolve(); });

    expect(sockets).toHaveLength(2);
    expect(screen.getByRole('alert').textContent).toContain('stop failed');
  });

  it('treats ended-with-reconnect as pending and restores reconnect when stop fails', async () => {
    vi.useFakeTimers();
    stopBrowser.mockRejectedValue(new Error('stop failed'));
    render(<BrowserViewPanel conversationId="conv-1" />);
    act(() => { sockets[0]!.onmessage?.(statusMessage('started')); });
    act(() => { sockets[0]!.onclose?.(); });
    expect(screen.getByLabelText('Browser view status: ended')).toBeTruthy();

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Stop browser session'));
      await Promise.resolve();
    });
    expect(screen.getByRole('alert').textContent).toContain('stop failed');
    expect(sockets).toHaveLength(2);
    vi.useRealTimers();
  });

  it('suppresses reconnect while stop from ended-with-reconnect is pending', () => {
    vi.useFakeTimers();
    stopBrowser.mockReturnValue(new Promise(() => {}));
    render(<BrowserViewPanel conversationId="conv-1" />);
    act(() => { sockets[0]!.onmessage?.(statusMessage('started')); });
    act(() => { sockets[0]!.onclose?.(); });

    fireEvent.click(screen.getByLabelText('Stop browser session'));
    expect(screen.getByLabelText('Stop browser session')).toBeDisabled();
    act(() => { vi.advanceTimersByTime(1500); });

    expect(sockets).toHaveLength(1);
    vi.useRealTimers();
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
