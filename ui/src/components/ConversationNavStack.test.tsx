import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it, vi } from 'vitest';
import { ConversationNavStack } from './ConversationNavStack';
const scrollToUnitIndex = vi.hoisted(() => vi.fn());
vi.mock('./MessageList', async () => {
  const { forwardRef, useImperativeHandle } = await import('react');
  return { MessageList: forwardRef<{ scrollToUnitIndex: typeof scrollToUnitIndex }, { sourceCallTarget: unknown }>(({ sourceCallTarget }, ref) => {
    useImperativeHandle(ref, () => ({ scrollToUnitIndex }));
    return <div data-testid="target">{JSON.stringify(sourceCallTarget)}</div>;
  }) };
});
vi.mock('./ConversationNav', () => ({ ConversationNav: ({ onJump }: { onJump: (index: number) => void }) => <button onClick={() => onJump(0)}>Loaded chapter</button> }));
vi.mock('../hooks/useFloatingNavStack', () => ({ useFloatingNavStack: () => {} }));
describe('source locator decoding', () => {
  it('retires the pending older-page anchor before jumping to a loaded chapter', () => {
    const update = vi.fn();
    render(<MemoryRouter><ConversationNavStack messages={[]} pendingMessages={[]} conversationId="fixture" slug="fixture" streamingRequestId={null} convState={{ type: 'idle' }} onRetry={vi.fn()} onOpenFile={undefined} transcriptPositioning={{ kind: 'idle', view: { conversationId: 'fixture', generation: 1, transcriptGeneration: 1 } }} onUpdateOlderMessagesRestore={update} /></MemoryRouter>);
    fireEvent.click(screen.getByRole('button', { name: 'Loaded chapter' }));
    expect(update).toHaveBeenCalledWith({ kind: 'reader_viewport' });
    expect(scrollToUnitIndex).toHaveBeenCalledWith(0);
    expect(update.mock.invocationCallOrder[0]).toBeLessThan(scrollToUnitIndex.mock.invocationCallOrder[0]!);
  });

  it('rejects malformed external percent escapes without crashing the transcript', () => {
    render(<MemoryRouter initialEntries={['/c/source?source_tool=tool#message-%ZZ']}><ConversationNavStack streamingRequestId={null} transcriptPositioning={{ kind: 'idle', view: { conversationId: 'source', generation: 1, transcriptGeneration: 1 } }} messages={[]} pendingMessages={[]} convState={{ type: 'idle' }} onRetry={vi.fn()} onOpenFile={undefined} /></MemoryRouter>);
    expect(screen.getByTestId('target')).toHaveTextContent('null');
  });
});
