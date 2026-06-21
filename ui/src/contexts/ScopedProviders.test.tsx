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
import {
  ReviewNotesProvider,
  useFileReviewNotesData,
  useReviewNotesCommands,
} from './ReviewNotesContext';
import type { ReviewNote, ReviewNotesCommands } from './ReviewNotesContext';

const PATH = '/repo/x.ts';

function NotesConsumer({
  onCommands,
  onNotes,
}: {
  onCommands: (commands: ReviewNotesCommands) => void;
  onNotes: (notes: ReviewNote[]) => void;
}) {
  const commands = useReviewNotesCommands();
  const fileNotes = useFileReviewNotesData(PATH);
  useEffect(() => { onCommands(commands); }, [commands, onCommands]);
  useEffect(() => { onNotes(fileNotes); }, [fileNotes, onNotes]);
  return null;
}

describe('ReviewNotesProvider scopeKey reset (task 02703)', () => {
  it('clears the notes pile when scopeKey changes', () => {
    let commands: ReviewNotesCommands | null = null;
    let notes: ReviewNote[] = [];
    const onCommands = (c: ReviewNotesCommands) => { commands = c; };
    const onNotes = (n: ReviewNote[]) => { notes = n; };

    const { rerender } = render(
      <ReviewNotesProvider scopeKey="conv-A">
        <NotesConsumer onCommands={onCommands} onNotes={onNotes} />
      </ReviewNotesProvider>,
    );

    act(() => {
      commands!.addNote(
        { kind: 'file', filePath: PATH, lineNumber: 7 },
        '  const x = 1;',
        'rename to count',
      );
    });
    expect(notes).toHaveLength(1);

    rerender(
      <ReviewNotesProvider scopeKey="conv-B">
        <NotesConsumer onCommands={onCommands} onNotes={onNotes} />
      </ReviewNotesProvider>,
    );
    expect(notes).toHaveLength(0);
  });

  it('preserves notes on re-render with the same scopeKey', () => {
    let commands: ReviewNotesCommands | null = null;
    let notes: ReviewNote[] = [];
    const onCommands = (c: ReviewNotesCommands) => { commands = c; };
    const onNotes = (n: ReviewNote[]) => { notes = n; };

    const { rerender } = render(
      <ReviewNotesProvider scopeKey="conv-A">
        <NotesConsumer onCommands={onCommands} onNotes={onNotes} />
      </ReviewNotesProvider>,
    );
    act(() => {
      commands!.addNote(
        { kind: 'file', filePath: PATH, lineNumber: 1 },
        '',
        'note',
      );
    });
    expect(notes).toHaveLength(1);

    rerender(
      <ReviewNotesProvider scopeKey="conv-A">
        <NotesConsumer onCommands={onCommands} onNotes={onNotes} />
      </ReviewNotesProvider>,
    );
    expect(notes).toHaveLength(1);
  });
});
