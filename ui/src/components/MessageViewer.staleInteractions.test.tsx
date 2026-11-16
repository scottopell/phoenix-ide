import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Message } from '../api';
import { ReviewNotesProvider } from '../contexts/ReviewNotesContext';
import { MessageViewer } from './MessageViewer';
import { CopyButton } from './CopyButton';
import { copyToClipboard } from '../utils/clipboard';

function message(text: string): Message {
  return {
    event_presentation: null,
    message_id: 'agent-1',
    sequence_id: 1,
    conversation_id: 'conv-1',
    message_type: 'agent',
    content: [{ type: 'text', text }],
    display_data: null,
    created_at: '2026-01-01T00:00:00Z',
  };
}

function renderViewer(text: string) {
  return render(
    <ReviewNotesProvider>
      <MessageViewer
        sequenceId={1}
        messages={[message(text)]}
        onClose={vi.fn()}
        onSendNotes={vi.fn()}
        inline
      />
    </ReviewNotesProvider>,
  );
}

function setNavigatorProperty(name: string, value: unknown) {
  const original = Object.getOwnPropertyDescriptor(navigator, name);
  Object.defineProperty(navigator, name, { configurable: true, value });
  return () => {
    if (original) Object.defineProperty(navigator, name, original);
    else Reflect.deleteProperty(navigator, name);
  };
}

function mockExecCommand() {
  const original = Object.getOwnPropertyDescriptor(document, 'execCommand');
  const execCommand = vi.fn().mockReturnValue(true);
  Object.defineProperty(document, 'execCommand', { configurable: true, value: execCommand });
  return {
    execCommand,
    restore: () => {
      if (original) Object.defineProperty(document, 'execCommand', original);
      else Reflect.deleteProperty(document, 'execCommand');
    },
  };
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe('MessageViewer stale interactions', () => {
  it('does not run the legacy copy fallback when clipboard rejection arrives after unmount', async () => {
    let rejectWrite: ((reason: Error) => void) | undefined;
    const writeText = vi.fn(() => new Promise<void>((_resolve, reject) => { rejectWrite = reject; }));
    const restoreClipboard = setNavigatorProperty('clipboard', { writeText });
    const { execCommand, restore: restoreExecCommand } = mockExecCommand();
    const select = vi.spyOn(HTMLTextAreaElement.prototype, 'select');
    const view = renderViewer('Exact markdown to copy');

    try {
      fireEvent.click(screen.getByRole('button', { name: 'Copy message markdown' }));
      expect(writeText).toHaveBeenCalledWith('Exact markdown to copy');
      view.unmount();
      await act(async () => {
        rejectWrite?.(new Error('clipboard denied'));
        await Promise.resolve();
      });

      expect(document.body.querySelector('textarea')).not.toBeInTheDocument();
      expect(select).not.toHaveBeenCalled();
      expect(execCommand).not.toHaveBeenCalled();
    } finally {
      view.unmount();
      restoreClipboard();
      restoreExecCommand();
    }
  });

  it('uses the real legacy copy fallback for a live viewer after clipboard rejection', async () => {
    const writeText = vi.fn().mockRejectedValue(new Error('clipboard denied'));
    const restoreClipboard = setNavigatorProperty('clipboard', { writeText });
    const copiedTextareaValues: string[] = [];
    const select = vi.spyOn(HTMLTextAreaElement.prototype, 'select').mockImplementation(function (this: HTMLTextAreaElement) {
      copiedTextareaValues.push(this.value);
    });
    const { execCommand, restore: restoreExecCommand } = mockExecCommand();
    const view = renderViewer('Fallback markdown');

    try {
      fireEvent.click(screen.getByRole('button', { name: 'Copy message markdown' }));
      await act(async () => { await Promise.resolve(); });

      expect(writeText).toHaveBeenCalledWith('Fallback markdown');
      expect(copiedTextareaValues).toEqual(['Fallback markdown']);
      expect(select).toHaveBeenCalledTimes(1);
      expect(execCommand).toHaveBeenCalledWith('copy');
      expect(document.body.querySelector('textarea')).not.toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'Copied!' })).toBeInTheDocument();
    } finally {
      view.unmount();
      restoreClipboard();
      restoreExecCommand();
    }
  });

  it('copies exact markdown through the native clipboard when it succeeds', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    const restoreClipboard = setNavigatorProperty('clipboard', { writeText });
    const { execCommand, restore: restoreExecCommand } = mockExecCommand();
    const select = vi.spyOn(HTMLTextAreaElement.prototype, 'select');
    const view = renderViewer('# Heading\n\nExact **message** text');

    try {
      fireEvent.click(screen.getByRole('button', { name: 'Copy message markdown' }));
      await act(async () => { await Promise.resolve(); });

      expect(writeText).toHaveBeenCalledTimes(1);
      expect(writeText).toHaveBeenCalledWith('# Heading\n\nExact **message** text');
      expect(execCommand).not.toHaveBeenCalled();
      expect(select).not.toHaveBeenCalled();
      expect(screen.getByRole('button', { name: 'Copied!' })).toBeInTheDocument();
    } finally {
      view.unmount();
      restoreClipboard();
      restoreExecCommand();
    }
  });

  it('preserves the one-argument legacy copy path without a native clipboard', async () => {
    const restoreClipboard = setNavigatorProperty('clipboard', undefined);
    const { execCommand, restore: restoreExecCommand } = mockExecCommand();
    const values: string[] = [];
    vi.spyOn(HTMLTextAreaElement.prototype, 'select').mockImplementation(function (this: HTMLTextAreaElement) {
      values.push(this.value);
    });
    try {
      expect(await copyToClipboard('Legacy exact text')).toBe(true);
      expect(values).toEqual(['Legacy exact text']);
      expect(execCommand).toHaveBeenCalledWith('copy');
      expect(document.body.querySelector('textarea')).not.toBeInTheDocument();
    } finally {
      restoreClipboard();
      restoreExecCommand();
    }
  });

  it.each(['text', 'disabled'] as const)('invalidates pending copy when %s changes', async (change) => {
    let rejectWrite: ((reason: Error) => void) | undefined;
    const writeText = vi.fn(() => new Promise<void>((_resolve, reject) => { rejectWrite = reject; }));
    const restoreClipboard = setNavigatorProperty('clipboard', { writeText });
    const { execCommand, restore: restoreExecCommand } = mockExecCommand();
    const select = vi.spyOn(HTMLTextAreaElement.prototype, 'select');
    const view = render(<CopyButton text="Original" />);
    try {
      fireEvent.click(screen.getByRole('button', { name: 'Copy to clipboard' }));
      expect(writeText).toHaveBeenCalledWith('Original');
      view.rerender(<CopyButton text={change === 'text' ? 'Replacement' : 'Original'} disabled={change === 'disabled'} />);
      await act(async () => {
        rejectWrite?.(new Error('clipboard denied'));
        await Promise.resolve();
      });
      expect(select).not.toHaveBeenCalled();
      expect(execCommand).not.toHaveBeenCalled();
      expect(screen.queryByRole('button', { name: 'Copied!' })).not.toBeInTheDocument();
    } finally {
      view.unmount();
      restoreClipboard();
      restoreExecCommand();
    }
  });

  it('keeps newer copy feedback when a superseded native write later rejects', async () => {
    let rejectFirst: ((reason: Error) => void) | undefined;
    const writeText = vi.fn()
      .mockImplementationOnce(() => new Promise<void>((_resolve, reject) => { rejectFirst = reject; }))
      .mockResolvedValue(undefined);
    const restoreClipboard = setNavigatorProperty('clipboard', { writeText });
    const { execCommand, restore: restoreExecCommand } = mockExecCommand();
    const select = vi.spyOn(HTMLTextAreaElement.prototype, 'select');
    const view = render(<CopyButton text="Exact text" />);
    try {
      const button = screen.getByRole('button', { name: 'Copy to clipboard' });
      fireEvent.click(button);
      fireEvent.click(button);
      await act(async () => { await Promise.resolve(); });
      expect(writeText).toHaveBeenCalledTimes(2);
      expect(screen.getByRole('button', { name: 'Copied!' })).toBeInTheDocument();
      await act(async () => {
        rejectFirst?.(new Error('clipboard denied'));
        await Promise.resolve();
      });
      expect(select).not.toHaveBeenCalled();
      expect(execCommand).not.toHaveBeenCalled();
      expect(screen.getByRole('button', { name: 'Copied!' })).toBeInTheDocument();
    } finally {
      view.unmount();
      restoreClipboard();
      restoreExecCommand();
    }
  });

  it('cancels a touch long-press on touchcancel before its deadline', () => {
    vi.useFakeTimers();
    const vibrate = vi.fn();
    const restoreVibrate = setNavigatorProperty('vibrate', vibrate);
    const view = renderViewer('Touch-cancel target');
    const block = view.container.querySelector('.annotatable') as HTMLElement;

    try {
      fireEvent.touchStart(block, { touches: [{ identifier: 1, target: block, clientX: 10, clientY: 10 }] });
      const focusedBeforeCancel = document.activeElement;
      fireEvent.touchCancel(block, { touches: [], changedTouches: [{ identifier: 1, target: block, clientX: 10, clientY: 10 }] });
      act(() => { vi.advanceTimersByTime(501); });

      expect(vibrate).not.toHaveBeenCalled();
      expect(screen.queryByPlaceholderText('Add your note… (Cmd/Ctrl+Enter to save)')).not.toBeInTheDocument();
      expect(document.activeElement).toBe(focusedBeforeCancel);
    } finally {
      view.unmount();
      restoreVibrate();
    }
  });

  it('opens and focuses the annotation editor after an uninterrupted touch hold', () => {
    vi.useFakeTimers();
    const vibrate = vi.fn();
    const restoreVibrate = setNavigatorProperty('vibrate', vibrate);
    const view = renderViewer('Long-press target');
    const block = view.container.querySelector('.annotatable') as HTMLElement;

    try {
      fireEvent.touchStart(block, { touches: [{ identifier: 1, target: block, clientX: 10, clientY: 10 }] });
      act(() => { vi.advanceTimersByTime(500); });

      const editor = screen.getByPlaceholderText('Add your note… (Cmd/Ctrl+Enter to save)');
      expect(editor).toHaveFocus();
      expect(vibrate).toHaveBeenCalledWith(50);
    } finally {
      view.unmount();
      restoreVibrate();
    }
  });

  it.each(['touchend', 'movement', 'unmount'] as const)(
    'cancels a pending long-press on %s', (cancelBy) => {
      vi.useFakeTimers();
      const vibrate = vi.fn();
      const restoreVibrate = setNavigatorProperty('vibrate', vibrate);
      const view = renderViewer('Cancelled hold target');
      const block = view.container.querySelector('.annotatable') as HTMLElement;

      try {
        fireEvent.touchStart(block, { touches: [{ identifier: 1, target: block, clientX: 10, clientY: 10 }] });
        if (cancelBy === 'touchend') {
          fireEvent.touchEnd(block, { touches: [], changedTouches: [{ identifier: 1, target: block, clientX: 10, clientY: 10 }] });
        } else if (cancelBy === 'movement') {
          fireEvent.touchMove(block, { touches: [{ identifier: 1, target: block, clientX: 30, clientY: 10 }] });
        } else {
          view.unmount();
        }
        act(() => { vi.advanceTimersByTime(501); });

        expect(vibrate).not.toHaveBeenCalled();
        expect(screen.queryByPlaceholderText('Add your note… (Cmd/Ctrl+Enter to save)')).not.toBeInTheDocument();
      } finally {
        view.unmount();
        restoreVibrate();
      }
    },
  );
});
