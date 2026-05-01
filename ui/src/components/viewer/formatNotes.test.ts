import { describe, it, expect } from 'vitest';
import { formatNotesForSend } from './formatNotes';
import type { ReviewNote } from '../../contexts/ReviewNotesContext';

const note = (overrides: Partial<ReviewNote> & { anchor: ReviewNote['anchor'] }): ReviewNote => ({
  id: 'n-' + Math.random().toString(36).slice(2),
  lineContent: '',
  body: '',
  createdAt: 0,
  ...overrides,
});

describe('formatNotesForSend', () => {
  it('returns null for an empty pile', () => {
    expect(formatNotesForSend([])).toBeNull();
  });

  it('formats a single file note with line content quoted', () => {
    const out = formatNotesForSend([
      note({
        anchor: { kind: 'file', filePath: 'src/foo.rs', lineNumber: 42 },
        lineContent: 'fn bar() {}',
        body: 'why empty?',
      }),
    ]);
    expect(out).toContain('## Review notes');
    expect(out).toContain('### `src/foo.rs`');
    expect(out).toContain('**Line 42**');
    expect(out).toContain('`fn bar() {}`');
    expect(out).toContain('why empty?');
  });

  it('groups file notes by path and diff notes by (filePath, section)', () => {
    const out = formatNotesForSend([
      note({
        anchor: { kind: 'file', filePath: 'src/a.rs', lineNumber: 1 },
        body: 'file note',
      }),
      note({
        anchor: { kind: 'diff', section: 'committed', filePath: 'src/a.rs', newLine: 5, diffPos: 2 },
        body: 'committed diff note',
      }),
      note({
        anchor: { kind: 'diff', section: 'uncommitted', filePath: 'src/a.rs', newLine: 5, diffPos: 2 },
        body: 'uncommitted diff note',
      }),
    ]);
    // Three distinct sections in the output.
    expect(out).toContain('### `src/a.rs`');
    expect(out).toContain('### Diff (committed): `src/a.rs`');
    expect(out).toContain('### Diff (uncommitted): `src/a.rs`');
    // Each note's body lands under its own heading (committed and
    // uncommitted notes at the same diffPos must NOT collide).
    expect(out).toContain('committed diff note');
    expect(out).toContain('uncommitted diff note');
  });

  it('keeps committed and uncommitted entries distinct even when the labels would otherwise be identical', () => {
    // Both notes share filePath, kind, newLine, and diffPos — only
    // `section` differs. Pre-fix, this produced two indistinguishable
    // "New line 1" entries under the same Diff group; now they go to
    // separate group headings.
    const out = formatNotesForSend([
      note({
        anchor: { kind: 'diff', section: 'committed', filePath: 'x.txt', newLine: 1, diffPos: 0 },
        body: 'note-c',
      }),
      note({
        anchor: { kind: 'diff', section: 'uncommitted', filePath: 'x.txt', newLine: 1, diffPos: 0 },
        body: 'note-u',
      }),
    ]);
    const committedIdx = out!.indexOf('### Diff (committed): `x.txt`');
    const uncommittedIdx = out!.indexOf('### Diff (uncommitted): `x.txt`');
    expect(committedIdx).toBeGreaterThanOrEqual(0);
    expect(uncommittedIdx).toBeGreaterThanOrEqual(0);
    expect(committedIdx).not.toBe(uncommittedIdx);
    // Body landed under the right heading.
    const committedBlock = out!.slice(committedIdx, uncommittedIdx);
    const uncommittedBlock = out!.slice(uncommittedIdx);
    expect(committedBlock).toContain('note-c');
    expect(committedBlock).not.toContain('note-u');
    expect(uncommittedBlock).toContain('note-u');
    expect(uncommittedBlock).not.toContain('note-c');
  });

  it('renders file-level diff notes using the same per-section grouping', () => {
    const out = formatNotesForSend([
      note({
        anchor: { kind: 'diff-file', section: 'committed', filePath: 'src/a.rs', diffPos: 0 },
        body: 'whole-file critique',
      }),
    ]);
    expect(out).toContain('### Diff (committed): `src/a.rs`');
    expect(out).toContain('**File-level**');
    expect(out).toContain('whole-file critique');
  });

  it('truncates very long line content with an ellipsis in the quoted span', () => {
    const longLine = 'x'.repeat(500);
    const out = formatNotesForSend([
      note({
        anchor: { kind: 'file', filePath: 'a.txt', lineNumber: 1 },
        lineContent: longLine,
        body: 'note',
      }),
    ]);
    // The truncation marker is the unicode ellipsis character.
    expect(out).toMatch(/`x{200}…`/);
  });

  it('uses a longer backtick delimiter when source line contains backticks', () => {
    // `array.length` and `` `keyword` `` are common in code reviews;
    // the wrapper must pick a fence longer than any inner backtick
    // run so the resulting markdown is unambiguous.
    const out = formatNotesForSend([
      note({
        anchor: { kind: 'file', filePath: 'a.ts', lineNumber: 5 },
        lineContent: 'const x = `template ${y}`;',
        body: 'note',
      }),
    ]);
    // Inner content has runs of 1 backtick at most; fence should be 2.
    expect(out).toContain('``const x = `template ${y}`;``');
  });

  it('pads with a space when source line starts or ends with a backtick', () => {
    const out = formatNotesForSend([
      note({
        anchor: { kind: 'file', filePath: 'a.ts', lineNumber: 1 },
        lineContent: '`leading',
        body: 'note',
      }),
    ]);
    // CommonMark: opening fence must be followed by a space when the
    // content starts with a backtick (otherwise the delimiter merges
    // with the content).
    expect(out).toContain('`` `leading ``');
  });

  it('handles very long backtick runs by escalating the fence length', () => {
    const longRun = '```'; // run of 3 backticks inside the line
    const out = formatNotesForSend([
      note({
        anchor: { kind: 'file', filePath: 'a.ts', lineNumber: 1 },
        lineContent: `before ${longRun} after`,
        body: 'note',
      }),
    ]);
    // Fence must be at least 4 backticks long (one more than the
    // longest inner run).
    expect(out).toContain('````before ``` after````');
  });

  it('escapes embedded newlines in the note body via indentation', () => {
    const out = formatNotesForSend([
      note({
        anchor: { kind: 'file', filePath: 'a.txt', lineNumber: 1 },
        body: 'line one\nline two',
      }),
    ]);
    // Continuation lines are indented to nest inside the bullet point.
    expect(out).toContain('line one\n  line two');
  });
});
