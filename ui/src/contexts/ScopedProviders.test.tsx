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
import { DiffViewerStateProvider, useDiffViewerState } from './ViewerStateContext';
import type { DiffViewerPayload } from './ViewerStateContext';

function NotesConsumer({ onCtx }: { onCtx: (ctx: ReturnType<typeof useReviewNotes>) => void }) {
  const ctx = useReviewNotes();
  useEffect(() => { onCtx(ctx); }, [ctx, onCtx]);
  return null;
}

function DiffConsumer({ onCtx }: { onCtx: (ctx: ReturnType<typeof useDiffViewerState>) => void }) {
  const ctx = useDiffViewerState();
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

describe('DiffViewerStateProvider scopeKey reset (task 02703)', () => {
  const samplePayload: DiffViewerPayload = {
    comparator: 'HEAD',
    commit_log: '',
    committed_diff: 'diff --git a/x b/x',
    uncommitted_diff: '',
  };

  it('drops the diff payload when scopeKey changes', () => {
    let latest: ReturnType<typeof useDiffViewerState> | null = null;
    const onCtx = (ctx: ReturnType<typeof useDiffViewerState>) => { latest = ctx; };

    const { rerender } = render(
      <DiffViewerStateProvider scopeKey="conv-A">
        <DiffConsumer onCtx={onCtx} />
      </DiffViewerStateProvider>,
    );

    act(() => { latest!.open(samplePayload); });
    expect(latest!.payload).not.toBeNull();

    rerender(
      <DiffViewerStateProvider scopeKey="conv-B">
        <DiffConsumer onCtx={onCtx} />
      </DiffViewerStateProvider>,
    );
    expect(latest!.payload).toBeNull();
  });
});
