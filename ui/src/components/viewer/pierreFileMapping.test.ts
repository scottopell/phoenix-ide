import { describe, it, expect } from 'vitest';
import type { ReviewNote } from '../../contexts/ReviewNotesContext';
import {
  annotationsForFile,
  buildFileItem,
  fileItemId,
  fileItemRenderSignature,
  lineDecorationCSS,
  lineTextAt,
  noteToAnnotation,
  resolveTouchedLineNumber,
  scrollTargetForNote,
} from './pierreFileMapping';

const PATH = '/tmp/project/src/main.rs';

function fileNote(id: string, lineNumber: number, body = 'note'): ReviewNote {
  return { id, anchor: { kind: 'file', filePath: PATH, lineNumber }, body, lineContent: '', createdAt: 0 };
}

describe('pierreFileMapping', () => {
  it('builds a single sideless file item from path + content', () => {
    const item = buildFileItem(PATH, 'fn main() {}\n');
    expect(item).toEqual({
      id: fileItemId(PATH),
      type: 'file',
      file: { name: PATH, contents: 'fn main() {}\n' },
    });
  });

  it('reads the bare source line at a 1-based number', () => {
    const content = 'a\nb\nc';
    expect(lineTextAt(content, 1)).toBe('a');
    expect(lineTextAt(content, 3)).toBe('c');
    expect(lineTextAt(content, 99)).toBe('');
    expect(lineTextAt(content, 0)).toBe('');
  });

  it('maps a file note for this path to a sideless line annotation, and rejects others', () => {
    expect(noteToAnnotation(fileNote('n1', 5), PATH)).toEqual({
      lineNumber: 5,
      metadata: { noteId: 'n1', anchor: { kind: 'file', filePath: PATH, lineNumber: 5 } },
    });
    // Different path.
    expect(noteToAnnotation(fileNote('n2', 5), '/other')).toBeNull();
    // A diff note is not a file annotation.
    const diffNote: ReviewNote = {
      id: 'd1',
      anchor: { kind: 'diff', section: 'committed', filePath: PATH, newLine: 5 },
      body: 'x',
      lineContent: '',
      createdAt: 0,
    };
    expect(noteToAnnotation(diffNote, PATH)).toBeNull();
  });

  it('collects only this file path annotations in note order', () => {
    const notes = [fileNote('a', 3), fileNote('b', 1), { ...fileNote('c', 9), anchor: { kind: 'file' as const, filePath: '/other', lineNumber: 9 } }];
    const anns = annotationsForFile(notes, PATH);
    expect(anns.map((a) => a.lineNumber)).toEqual([3, 1]);
  });

  it('changes the render signature when content, notes, modified lines, or flash change', () => {
    const base = fileItemRenderSignature(PATH, 'a\nb', [], new Set(), null, null);
    expect(fileItemRenderSignature(PATH, 'a\nc', [], new Set(), null, null)).not.toBe(base);
    expect(fileItemRenderSignature(PATH, 'a\nb', [fileNote('n', 1)], new Set(), null, null)).not.toBe(base);
    expect(fileItemRenderSignature(PATH, 'a\nb', [], new Set([2]), null, null)).not.toBe(base);
    expect(fileItemRenderSignature(PATH, 'a\nb', [], new Set(), null, 2)).not.toBe(base);
    // Stable for identical inputs.
    expect(fileItemRenderSignature(PATH, 'a\nb', [], new Set(), null, null)).toBe(base);
  });

  it('targets a file note line for jump, and rejects non-file notes', () => {
    expect(scrollTargetForNote(fileNote('n', 7), PATH)).toEqual({ id: fileItemId(PATH), lineNumber: 7 });
    expect(scrollTargetForNote(fileNote('n', 7), '/other')).toBeNull();
  });

  it('emits per-line shading CSS, with the flashed line taking precedence over modified shading', () => {
    expect(lineDecorationCSS(new Set(), null)).toBe('');
    const css = lineDecorationCSS(new Set([2, 5]), null);
    expect(css).toContain('[data-line="2"]');
    expect(css).toContain('[data-line="5"]');
    expect(css).toContain('--viewer-modified-line-bg');

    // A line that is both modified and highlighted is shaded only as highlighted.
    const both = lineDecorationCSS(new Set([3]), 3);
    expect(both).toContain('--viewer-highlight-line-bg');
    expect(both).not.toContain('--viewer-modified-line-bg');
  });

  it('resolves a touched line number from a composed path via data-line', () => {
    const lineEl = document.createElement('span');
    lineEl.setAttribute('data-line', '42');
    const outer = document.createElement('div');
    expect(resolveTouchedLineNumber([lineEl, outer])).toBe(42);
    expect(resolveTouchedLineNumber([outer])).toBeUndefined();
  });
});
