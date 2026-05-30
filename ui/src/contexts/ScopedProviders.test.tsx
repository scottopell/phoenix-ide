// Tests for the per-scope reset behavior of the conversation-scoped
// providers used by ConversationPage. See task 02703.
//
// These providers replace what `key={slug}` on KeyedConversationPage used
// to give us “for free” — a clean slate on conversation change. We now
// keep the page mounted and reset state via `scopeKey`, which lets us
// drop the unmount-on-nav flash while keeping cross-conversation
// isolation honest.

import { describe, it, expect } from 'vitest';
import { render, act } from '@testing-library/react';
import { useEffect } from 'react';
import { ReviewNotesProvider, useReviewNotes } from './ReviewNotesContext';

function NotesConsumer({ onCtx }: { onCtx: (ctx: ReturnType<typeof useReviewNotes>) => void }) {
  const ctx = useReviewNotes();
  useEffect(() => { onCtx(ctx); }, [ctx, onCtx]);
  return null;
}

describe('ReviewNotesProvider scopeKey reset (task 02703)', () => {
  it('clears the notes pile when scopeKey changes', () => {
    let latest: ReturnType<typeof useReviewNotes> | null = null;
    const onCtx = (ctx: ReturnType<typeof useReviewNotes>) => { latest = ctx; };

    const { rerender } = render(
      <ReviewNotesProvider scopeKey="conv-A">
        <NotesConsumer onCtx={onCtx} />
      </ReviewNotesProvider>,
    );

    act(() => {
      latest!.addNote(
        { kind: 'file', filePath: '/repo/x.ts', lineNumber: 7 },
        '  const x = 1;',
        'rename to count',
      );
    });
    expect(latest!.notes).toHaveLength(1);

    rerender(
      <ReviewNotesProvider scopeKey="conv-B">
        <NotesConsumer onCtx={onCtx} />
      </ReviewNotesProvider>,
    );
    expect(latest!.notes).toHaveLength(0);
  });

  it('preserves notes on re-render with the same scopeKey', () => {
    let latest: ReturnType<typeof useReviewNotes> | null = null;
    const onCtx = (ctx: ReturnType<typeof useReviewNotes>) => { latest = ctx; };

    const { rerender } = render(
      <ReviewNotesProvider scopeKey="conv-A">
        <NotesConsumer onCtx={onCtx} />
      </ReviewNotesProvider>,
    );
    act(() => {
      latest!.addNote(
        { kind: 'file', filePath: '/repo/x.ts', lineNumber: 1 },
        '',
        'note',
      );
    });
    expect(latest!.notes).toHaveLength(1);

    rerender(
      <ReviewNotesProvider scopeKey="conv-A">
        <NotesConsumer onCtx={onCtx} />
      </ReviewNotesProvider>,
    );
    expect(latest!.notes).toHaveLength(1);
  });
});
