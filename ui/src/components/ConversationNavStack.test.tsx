import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { Chapter } from '../conversation/conversationChapters';
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
vi.mock('./ConversationNav', () => ({ ConversationNav: ({ onJump }: { onJump: (chapter: Chapter) => void }) => {
  const location = useLocation();
  return <>
    <button onClick={() => onJump({ kind: 'prompt', unitIndex: 0, label: 'Loaded', sequenceId: 1, target: { kind: 'persisted', messageId: 'loaded-message', transcriptId: 'fixture' } })}>Loaded chapter</button>
    <button onClick={() => onJump({ kind: 'prompt', unitIndex: 1, label: 'Pending', sequenceId: undefined, target: { kind: 'pending' } })}>Pending chapter</button>
    <div data-testid="nav-location">{location.pathname}{location.search}{location.hash}</div>
  </>;
} }));
vi.mock('../hooks/useFloatingNavStack', () => ({ useFloatingNavStack: () => {} }));
describe('source locator decoding', () => {
  it.each([false, true])('uses one persisted chapter owner and local pending navigation (aggregate=%s)', aggregate => {
    const update = vi.fn();
    scrollToUnitIndex.mockClear();
    render(<MemoryRouter initialEntries={['/global/fixture?source_transcript=older&source_tool=old-tool&filter=keep#message-unloaded-boundary']}><ConversationNavStack messages={[]} pendingMessages={[]} {...(aggregate ? { historyBoundaries: [] } : {})} conversationId="product-aggregate" slug="fixture" streamingRequestId={null} convState={{ type: 'idle' }} onRetry={vi.fn()} onOpenFile={undefined} transcriptPositioning={{ kind: 'idle', view: { conversationId: 'fixture', generation: 1, transcriptGeneration: 1 } }} onUpdateOlderMessagesRestore={update} /></MemoryRouter>);
    fireEvent.click(screen.getByRole('button', { name: 'Loaded chapter' }));
    expect(update).toHaveBeenCalledWith({ kind: 'reader_viewport' });
    expect(scrollToUnitIndex).not.toHaveBeenCalled();
    expect(screen.getByTestId('nav-location')).toHaveTextContent('/global/fixture?source_transcript=fixture&filter=keep#message-loaded-message');
    expect(screen.getByTestId('target')).toHaveTextContent('null');
    fireEvent.click(screen.getByRole('button', { name: 'Pending chapter' }));
    expect(scrollToUnitIndex).toHaveBeenCalledWith(1);
    expect(screen.getByTestId('nav-location')).toHaveTextContent('/global/fixture?filter=keep');
    expect(screen.getByTestId('nav-location')).not.toHaveTextContent('source_transcript');
    expect(screen.getByTestId('nav-location')).not.toHaveTextContent('#');
    expect(update.mock.invocationCallOrder[0]).toBeLessThan(scrollToUnitIndex.mock.invocationCallOrder[0]!);
  });

  it('rejects malformed external percent escapes without crashing the transcript', () => {
    render(<MemoryRouter initialEntries={['/c/source?source_tool=tool#message-%ZZ']}><ConversationNavStack streamingRequestId={null} transcriptPositioning={{ kind: 'idle', view: { conversationId: 'source', generation: 1, transcriptGeneration: 1 } }} messages={[]} pendingMessages={[]} convState={{ type: 'idle' }} onRetry={vi.fn()} onOpenFile={undefined} /></MemoryRouter>);
    expect(screen.getByTestId('target')).toHaveTextContent('null');
  });
});
