import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { BrowserViewPanel } from './BrowserViewPanel';
import { api } from '../api';

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    api: { ...actual.api, stopWorkScopeBrowserSession: vi.fn() },
  };
});

const stopBrowser = vi.mocked(api.stopWorkScopeBrowserSession);

class MockWebSocket {
  binaryType = 'arraybuffer';
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: MessageEvent<ArrayBuffer>) => void) | null = null;

  constructor(public readonly url: string) {}

  close() {
    this.onclose?.();
  }
}

describe('BrowserViewPanel stop-session control', () => {
  const originalWebSocket = globalThis.WebSocket;

  beforeEach(() => {
    stopBrowser.mockReset();
    globalThis.WebSocket = MockWebSocket as unknown as typeof WebSocket;
  });

  afterEach(() => {
    globalThis.WebSocket = originalWebSocket;
  });

  it('renders close-view and stop-session as separate controls', () => {
    render(<BrowserViewPanel conversationId="conv-1" scopeKey="ws-1" onClose={() => {}} />);

    expect(screen.getByLabelText('Close browser view')).toBeTruthy();
    expect(screen.getByLabelText('Stop browser session')).toBeTruthy();
    expect(screen.getByText('Stop browser')).toBeTruthy();
  });

  it('clicking stop calls the work-scope browser-session endpoint', async () => {
    stopBrowser.mockResolvedValue({ success: true });
    render(<BrowserViewPanel conversationId="conv-1" scopeKey="worktree:%2Ftmp%2Fproj" />);

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Stop browser session'));
      await Promise.resolve();
    });

    expect(stopBrowser).toHaveBeenCalledWith('worktree:%2Ftmp%2Fproj');
  });

  it('stop failure is rendered visibly', async () => {
    stopBrowser.mockRejectedValue(new Error('stop failed'));
    render(<BrowserViewPanel conversationId="conv-1" scopeKey="ws-1" />);

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Stop browser session'));
      await Promise.resolve();
    });

    expect(screen.getByRole('alert').textContent).toContain('stop failed');
  });
});
