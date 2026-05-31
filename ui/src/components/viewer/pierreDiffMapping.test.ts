import { describe, it, expect } from 'vitest';
import {
  annotationsForFile,
  buildSectionItems,
  fileNotesFor,
  itemId,
  lineTextAt,
  noteToAnnotation,
  scrollTargetForNote,
  sectionFromItemId,
} from './pierreDiffMapping';
import type { ReviewNote } from '../../contexts/ReviewNotesContext';

const ADD_FILE = [
  'diff --git a/src/foo.ts b/src/foo.ts',
  'index 0000000..1111111 100644',
  '--- a/src/foo.ts',
  '+++ b/src/foo.ts',
  '@@ -1,3 +1,4 @@',
  ' const a = 1;',
  '-const b = 2;',
  '+const b = 3;',
  '+const c = 4;',
  ' const d = 5;',
].join('\n');

const SECOND_FILE = [
  'diff --git a/src/bar.ts b/src/bar.ts',
  '--- a/src/bar.ts',
  '+++ b/src/bar.ts',
  '@@ -1,1 +1,1 @@',
  '-old line',
  '+new line',
].join('\n');

function note(anchor: ReviewNote['anchor'], extra?: Partial<ReviewNote>): ReviewNote {
  return { id: 'n1', anchor, lineContent: '', body: 'b', createdAt: 0, ...extra };
}

describe('itemId / sectionFromItemId', () => {
  it('round-trips section as a fixed prefix', () => {
    expect(itemId('committed', 'src/foo.ts')).toBe('committed:src/foo.ts');
    expect(itemId('uncommitted', 'src/foo.ts')).toBe('uncommitted:src/foo.ts');
    expect(sectionFromItemId('committed:src/foo.ts')).toBe('committed');
    expect(sectionFromItemId('uncommitted:a/b:c.ts')).toBe('uncommitted');
  });

  it('returns null for foreign ids', () => {
    expect(sectionFromItemId('weird:thing')).toBeNull();
    expect(sectionFromItemId('nocolon')).toBeNull();
  });
});

describe('buildSectionItems', () => {
  it('returns empty (no error) for blank diff', () => {
    expect(buildSectionItems('committed', '')).toEqual({ items: [], error: null });
    expect(buildSectionItems('committed', '   \n')).toEqual({ items: [], error: null });
  });

  it('parses multiple files into section-prefixed diff items', () => {
    const { items, error } = buildSectionItems('committed', `${ADD_FILE}\n${SECOND_FILE}`);
    expect(error).toBeNull();
    expect(items.map((i) => i.id)).toEqual(['committed:src/foo.ts', 'committed:src/bar.ts']);
    expect(items.every((i) => i.type === 'diff')).toBe(true);
    expect(items[0]!.fileDiff.name).toBe('src/foo.ts');
  });

  it('namespaces the same path across sections so they never collide', () => {
    const c = buildSectionItems('committed', ADD_FILE);
    const u = buildSectionItems('uncommitted', ADD_FILE);
    expect(c.items[0]!.id).toBe('committed:src/foo.ts');
    expect(u.items[0]!.id).toBe('uncommitted:src/foo.ts');
    expect(c.items[0]!.id).not.toBe(u.items[0]!.id);
  });

  it('de-duplicates a repeated path within one section with a #n suffix', () => {
    const { items } = buildSectionItems('committed', `${ADD_FILE}\n${ADD_FILE}`);
    expect(items.map((i) => i.id)).toEqual(['committed:src/foo.ts', 'committed:src/foo.ts#1']);
    // File path identity is preserved on both (read from fileDiff.name, not id).
    expect(items[1]!.fileDiff.name).toBe('src/foo.ts');
  });

  it('reports an error for non-empty unparseable input rather than throwing', () => {
    const { items, error } = buildSectionItems('committed', 'this is not a diff at all');
    expect(items).toEqual([]);
    expect(error).toBeTruthy();
  });
});

describe('noteToAnnotation', () => {
  it('maps newLine to the additions side', () => {
    const ann = noteToAnnotation(
      note({ kind: 'diff', section: 'committed', filePath: 'src/foo.ts', newLine: 3 }),
    );
    expect(ann).toMatchObject({ side: 'additions', lineNumber: 3 });
    expect(ann!.metadata!.noteId).toBe('n1');
  });

  it('maps oldLine to the deletions side', () => {
    const ann = noteToAnnotation(
      note({ kind: 'diff', section: 'committed', filePath: 'src/foo.ts', oldLine: 2 }),
    );
    expect(ann).toMatchObject({ side: 'deletions', lineNumber: 2 });
  });

  it('returns null for non-line notes (file-level, file-viewer)', () => {
    expect(noteToAnnotation(note({ kind: 'diff-file', section: 'committed', filePath: 'x' }))).toBeNull();
    expect(noteToAnnotation(note({ kind: 'file', filePath: '/x', lineNumber: 1 }))).toBeNull();
  });
});

describe('annotationsForFile / fileNotesFor', () => {
  const notes: ReviewNote[] = [
    note({ kind: 'diff', section: 'committed', filePath: 'src/foo.ts', newLine: 3 }, { id: 'c1' }),
    note({ kind: 'diff', section: 'uncommitted', filePath: 'src/foo.ts', newLine: 3 }, { id: 'u1' }),
    note({ kind: 'diff-file', section: 'committed', filePath: 'src/foo.ts' }, { id: 'f1' }),
  ];

  it('scopes line annotations by section + file', () => {
    const c = annotationsForFile(notes, 'committed', 'src/foo.ts');
    expect(c.map((a) => a.metadata!.noteId)).toEqual(['c1']);
    const u = annotationsForFile(notes, 'uncommitted', 'src/foo.ts');
    expect(u.map((a) => a.metadata!.noteId)).toEqual(['u1']);
  });

  it('scopes file-level notes by section + file', () => {
    expect(fileNotesFor(notes, 'committed', 'src/foo.ts').map((n) => n.id)).toEqual(['f1']);
    expect(fileNotesFor(notes, 'uncommitted', 'src/foo.ts')).toEqual([]);
  });
});

describe('lineTextAt', () => {
  const fileDiff = buildSectionItems('committed', ADD_FILE).items[0]!.fileDiff;

  it('recovers addition-side line text from typed hunk data (no DOM)', () => {
    // New file: 1 const a, 2 const b = 3, 3 const c, 4 const d
    expect(lineTextAt(fileDiff, 'additions', 2)).toBe('const b = 3;');
    expect(lineTextAt(fileDiff, 'additions', 3)).toBe('const c = 4;');
  });

  it('recovers deletion-side line text', () => {
    // Old file: 1 const a, 2 const b = 2, 3 const d
    expect(lineTextAt(fileDiff, 'deletions', 2)).toBe('const b = 2;');
  });

  it('returns undefined for an out-of-range line', () => {
    expect(lineTextAt(fileDiff, 'additions', 999)).toBeUndefined();
  });
});

describe('scrollTargetForNote', () => {
  it('targets a line for a diff note', () => {
    expect(
      scrollTargetForNote(note({ kind: 'diff', section: 'committed', filePath: 'src/foo.ts', newLine: 3 })),
    ).toEqual({ id: 'committed:src/foo.ts', line: { lineNumber: 3, side: 'additions' } });
  });

  it('targets the item for a file-level note', () => {
    expect(
      scrollTargetForNote(note({ kind: 'diff-file', section: 'uncommitted', filePath: 'src/foo.ts' })),
    ).toEqual({ id: 'uncommitted:src/foo.ts' });
  });

  it('returns null for a file-viewer note', () => {
    expect(scrollTargetForNote(note({ kind: 'file', filePath: '/x', lineNumber: 1 }))).toBeNull();
  });
});
