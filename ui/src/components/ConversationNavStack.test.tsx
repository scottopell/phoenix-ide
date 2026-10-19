import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { Chapter } from '../conversation/conversationChapters';
import { describe, expect, it, vi } from 'vitest';
import { ConversationNavStack } from './ConversationNavStack';
import { coordinatorConversationLinkState } from './conversationReturnOrigin';
const scrollToUnitIndex = vi.hoisted(() => vi.fn());
vi.mock('./MessageList', async () => {
  const { forwardRef, useImperativeHandle } = await import('react');
  return { MessageList: forwardRef<{ scrollToUnitIndex: typeof scrollToUnitIndex }, { sourceCallTarget: unknown }>(({ sourceCallTarget }, ref) => {
    useImperativeHandle(ref, () => ({ scrollToUnitIndex }));
    return <div data-testid="target">{JSON.stringify(sourceCallTarget)}</div>;
  }) };
});
vi.mock('./ConversationNav', () => ({ ConversationNav: ({ onJump, onJumpBoundary }: { onJump: (chapter: Chapter) => void; onJumpBoundary: (messageId: string) => void }) => {
  const location = useLocation();
  return <>
    <button onClick={() => onJump({ kind: 'prompt', unitIndex: 0, label: 'Loaded', sequenceId: 1, target: { kind: 'persisted', messageId: 'loaded-message', transcriptId: 'fixture' } })}>Loaded chapter</button>
    <button onClick={() => onJump({ kind: 'prompt', unitIndex: 1, label: 'Pending', sequenceId: undefined, target: { kind: 'pending' } })}>Pending chapter</button>
    <button onClick={() => onJump({ kind: 'prompt', unitIndex: 2, label: 'Other member', sequenceId: 2, target: { kind: 'persisted', messageId: 'row-2-message', transcriptId: 'row-2' } })}>Other member chapter</button>
    <button onClick={() => onJumpBoundary('row-2-boundary')}>Unloaded member boundary</button>
    <div data-testid="nav-location">{location.pathname}{location.search}{location.hash}</div>
    <div data-testid="nav-state">{JSON.stringify(location.state)}</div>
  </>;
} }));
vi.mock('../hooks/useFloatingNavStack', () => ({ useFloatingNavStack: () => {} }));
describe('source locator decoding', () => {
  it.each(['Loaded chapter', 'Pending chapter', 'Unloaded member boundary'])('preserves Coordinator return origin and unrelated state through %s, then clears pending-only state', button => {
    const origin = coordinatorConversationLinkState('/global/coordinator?viewer=inspect#message-origin', '/c/fixture')!;
    render(<MemoryRouter initialEntries={[{ pathname: '/c/fixture', state: { ...origin, unrelated: 'keep', chapterSelection: 'pending' } }]}><ConversationNavStack
      messages={[]} pendingMessages={[]} historyBoundaries={[{ message_id: 'row-2-boundary', predecessor_transcript_row_id: 'row-2', successor_transcript_row_id: 'row-3', represented_successor_opening_message_id: null, segment_ordinal: 1 }]}
      streamingRequestId={null} convState={{ type: 'idle' }} onRetry={vi.fn()} onOpenFile={undefined}
      transcriptPositioning={{ kind: 'idle', view: { conversationId: 'fixture', generation: 1, transcriptGeneration: 1 } }}
    /></MemoryRouter>);
    fireEvent.click(screen.getByRole('button', { name: button }));
    expect(JSON.parse(screen.getByTestId('nav-state').textContent!)).toEqual({ ...origin, unrelated: 'keep', ...(button === 'Pending chapter' ? { chapterSelection: 'pending' } : {}) });
    fireEvent.click(screen.getByRole('button', { name: 'Pending chapter' }));
    fireEvent.click(screen.getByRole('button', { name: 'Loaded chapter' }));
    expect(JSON.parse(screen.getByTestId('nav-state').textContent!)).toEqual({ ...origin, unrelated: 'keep' });
  });

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

  it.each([
    ['Other member chapter', 'row-2-message'],
    ['Unloaded member boundary', 'row-2-boundary'],
  ])('selects the owning member from a hashless Global route: %s', (button, messageId) => {
    render(<MemoryRouter initialEntries={['/global/row-1?filter=keep']}><ConversationNavStack
      messages={[]} pendingMessages={[]} conversationId="product-aggregate" slug="row-1"
      streamingRequestId={null} convState={{ type: 'idle' }} onRetry={vi.fn()} onOpenFile={undefined}
      transcriptPositioning={{ kind: 'idle', view: { conversationId: 'row-1', generation: 1, transcriptGeneration: 1 } }}
      historyBoundaries={[{ message_id: 'row-2-boundary', predecessor_transcript_row_id: 'row-2', successor_transcript_row_id: 'row-3', represented_successor_opening_message_id: null, segment_ordinal: 1 }]}
    /></MemoryRouter>);
    fireEvent.click(screen.getByRole('button', { name: button }));
    expect(screen.getByTestId('nav-location')).toHaveTextContent(`/global/row-1?filter=keep&source_transcript=row-2#message-${messageId}`);
  });

  it('keeps unpinned same-row fallback chapter navigation hash-only', () => {
    render(<MemoryRouter initialEntries={['/c/fixture?filter=keep']}><ConversationNavStack
      messages={[]} pendingMessages={[]} streamingRequestId={null} convState={{ type: 'idle' }}
      onRetry={vi.fn()} onOpenFile={undefined} transcriptPositioning={{ kind: 'idle', view: { conversationId: 'fixture', generation: 1, transcriptGeneration: 1 } }}
    /></MemoryRouter>);
    fireEvent.click(screen.getByRole('button', { name: 'Loaded chapter' }));
    expect(screen.getByTestId('nav-location')).toHaveTextContent('/c/fixture?filter=keep#message-loaded-message');
  });

  it('rejects malformed external percent escapes without crashing the transcript', () => {
    render(<MemoryRouter initialEntries={['/c/source?source_tool=tool#message-%ZZ']}><ConversationNavStack streamingRequestId={null} transcriptPositioning={{ kind: 'idle', view: { conversationId: 'source', generation: 1, transcriptGeneration: 1 } }} messages={[]} pendingMessages={[]} convState={{ type: 'idle' }} onRetry={vi.fn()} onOpenFile={undefined} /></MemoryRouter>);
    expect(screen.getByTestId('target')).toHaveTextContent('null');
  });
});
