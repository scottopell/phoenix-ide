// Render-isolation tests for the review-notes context (candidate C6, task
// 51001). Adding/removing a note in one scope must not re-render consumers of
// an unrelated scope: a diff-note mutation must leave file-note consumers
// untouched, and a note on path A must leave a path-B consumer untouched.

import { describe, it, expect, vi } from 'vitest';
import { render, act } from '@testing-library/react';
import {
  ReviewNotesProvider,
  useDiffReviewNotesData,
  useFileReviewNotesData,
  useReviewNotesCommands,
} from './ReviewNotesContext';
import type { ReviewNotesCommands } from './ReviewNotesContext';

function FileConsumer({ path, onRender }: { path: string; onRender: () => void }) {
  useFileReviewNotesData(path);
  onRender();
  return null;
}

function DiffConsumer({ onRender }: { onRender: () => void }) {
  useDiffReviewNotesData();
  onRender();
  return null;
}

function CommandsCapture({ onCommands }: { onCommands: (c: ReviewNotesCommands) => void }) {
  onCommands(useReviewNotesCommands());
  return null;
}

describe('ReviewNotesContext render isolation (C6)', () => {
  it('a diff-note mutation does not re-render a file-notes consumer', () => {
    let commands: ReviewNotesCommands | null = null;
    const fileRender = vi.fn();
    const diffRender = vi.fn();

    render(
      <ReviewNotesProvider scopeKey="conv">
        <CommandsCapture onCommands={(c) => { commands = c; }} />
        <FileConsumer path="/repo/a.ts" onRender={fileRender} />
        <DiffConsumer onRender={diffRender} />
      </ReviewNotesProvider>,
    );

    const fileBefore = fileRender.mock.calls.length;
    const diffBefore = diffRender.mock.calls.length;

    act(() => {
      commands!.addNote(
        { kind: 'diff', section: 'uncommitted', filePath: '/repo/a.ts', newLine: 3 },
        'line',
        'diff note',
      );
    });

    // The diff consumer's slice changed → it re-renders. The file consumer's
    // slice is unchanged → it must not.
    expect(diffRender.mock.calls.length).toBeGreaterThan(diffBefore);
    expect(fileRender.mock.calls.length).toBe(fileBefore);
  });

  it('a note on path A does not re-render a consumer of path B', () => {
    let commands: ReviewNotesCommands | null = null;
    const aRender = vi.fn();
    const bRender = vi.fn();

    render(
      <ReviewNotesProvider scopeKey="conv">
        <CommandsCapture onCommands={(c) => { commands = c; }} />
        <FileConsumer path="/repo/a.ts" onRender={aRender} />
        <FileConsumer path="/repo/b.ts" onRender={bRender} />
      </ReviewNotesProvider>,
    );

    const aBefore = aRender.mock.calls.length;
    const bBefore = bRender.mock.calls.length;

    act(() => {
      commands!.addNote(
        { kind: 'file', filePath: '/repo/a.ts', lineNumber: 1 },
        'x',
        'note on A',
      );
    });

    expect(aRender.mock.calls.length).toBeGreaterThan(aBefore);
    expect(bRender.mock.calls.length).toBe(bBefore);
  });

  it('exposes the whole pile through getSnapshot for the send path', () => {
    let commands: ReviewNotesCommands | null = null;
    render(
      <ReviewNotesProvider scopeKey="conv">
        <CommandsCapture onCommands={(c) => { commands = c; }} />
      </ReviewNotesProvider>,
    );

    act(() => {
      commands!.addNote({ kind: 'file', filePath: '/repo/a.ts', lineNumber: 1 }, 'x', 'a');
      commands!.addNote(
        { kind: 'diff', section: 'committed', filePath: '/repo/b.ts', oldLine: 2 },
        'y',
        'b',
      );
    });

    expect(commands!.getSnapshot()).toHaveLength(2);

    act(() => commands!.clear());
    expect(commands!.getSnapshot()).toHaveLength(0);
  });
});
