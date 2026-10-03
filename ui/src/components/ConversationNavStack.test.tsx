import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it, vi } from 'vitest';
import { ConversationNavStack } from './ConversationNavStack';
vi.mock('./MessageList', () => ({ MessageList: ({ sourceCallTarget }: { sourceCallTarget: unknown }) => <div data-testid="target">{JSON.stringify(sourceCallTarget)}</div> }));
vi.mock('./ConversationNav', () => ({ ConversationNav: () => null }));
vi.mock('../hooks/useFloatingNavStack', () => ({ useFloatingNavStack: () => {} }));
describe('source locator decoding', () => {
  it('rejects malformed external percent escapes without crashing the transcript', () => {
    render(<MemoryRouter initialEntries={['/c/source?source_tool=tool#message-%ZZ']}><ConversationNavStack transcriptPositioning={{ kind: 'idle', view: { conversationId: 'source', generation: 1, transcriptGeneration: 1 } }} messages={[]} pendingMessages={[]} convState={{ type: 'idle' }} onRetry={vi.fn()} onOpenFile={undefined} /></MemoryRouter>);
    expect(screen.getByTestId('target')).toHaveTextContent('null');
  });
});
