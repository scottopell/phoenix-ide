/**
 * Pure mapping layer between Phoenix file review-note anchors and Pierre's
 * `@pierre/diffs` *file* model (the non-diff `CodeViewFileItem`). Mirror of
 * `pierreDiffMapping` for the single-file viewer: nothing here touches the DOM,
 * React, or Pierre's rendered output — it only converts a file's raw text and
 * Phoenix `kind: 'file'` anchors into a Pierre `CodeViewFileItem` /
 * `LineAnnotation`s and back. Unit-tested in `pierreFileMapping.test.ts`.
 *
 * A file viewer shows exactly one file, so identity is trivial: a single item
 * id derived from the absolute path. Annotations carry no side (unlike diffs):
 * a file note anchors on a bare 1-based line number, and the full Phoenix
 * anchor rides in the annotation metadata so jump/indicator logic reads typed
 * data rather than reconstructing it.
 */

import type { CodeViewFileItem, LineAnnotation } from '@pierre/diffs';
import type { NoteAnchor, ReviewNote } from '../../contexts/ReviewNotesContext';

/**
 * Metadata stored on every Pierre file annotation. The full Phoenix anchor is
 * kept verbatim so jump/indicator logic reads typed data rather than
 * re-deriving it from the line number.
 */
export interface PhoenixFileAnnotationMeta {
  noteId: string;
  anchor: NoteAnchor;
}

export type PhoenixFileItem = CodeViewFileItem<PhoenixFileAnnotationMeta>;
export type PhoenixFileAnnotation = LineAnnotation<PhoenixFileAnnotationMeta>;

/** Stable item id for a file path. The viewer holds one file, so a fixed
 *  prefix plus the path is unique; the path is read back from `item.file.name`,
 *  never parsed out of the id. */
export function fileItemId(filePath: string): string {
  return `file:${filePath}`;
}

/**
 * Build the single Pierre file item for a viewed file. Pierre infers the
 * highlighting language from `name`, so the caller passes the path verbatim and
 * the raw contents; no language override is set.
 */
export function buildFileItem(filePath: string, content: string): PhoenixFileItem {
  return {
    id: fileItemId(filePath),
    type: 'file',
    file: { name: filePath, contents: content },
  };
}

/** The bare source line at a 1-based line number, or '' past the end. Used to
 *  quote the line a note is anchored to (Pierre's file onLineClick reports only
 *  the line number). */
export function lineTextAt(content: string, lineNumber: number): string {
  if (lineNumber < 1) return '';
  const lines = content.split('\n');
  return lines[lineNumber - 1] ?? '';
}

/** Convert a single Phoenix review note into a Pierre file line annotation.
 *  Returns null for anything that isn't a file-anchored note for this path. */
export function noteToAnnotation(note: ReviewNote, filePath: string): PhoenixFileAnnotation | null {
  const a = note.anchor;
  if (a.kind !== 'file' || a.filePath !== filePath) return null;
  return { lineNumber: a.lineNumber, metadata: { noteId: note.id, anchor: a } };
}

/** All line annotations for a file, in note order. */
export function annotationsForFile(
  notes: readonly ReviewNote[],
  filePath: string,
): PhoenixFileAnnotation[] {
  const out: PhoenixFileAnnotation[] = [];
  for (const note of notes) {
    const ann = noteToAnnotation(note, filePath);
    if (ann) out.push(ann);
  }
  return out;
}

/** Stable 32-bit FNV-1a hash of a string, base-36 encoded — folds file text
 *  into the render signature without carrying the whole string for equality. */
function hashContent(s: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(36);
}

/**
 * A signature of everything PhoenixFileCodeView draws for the file item: the
 * content fingerprint, the line notes (and which is flashed), the modified-line
 * set, and the flashed line. A change here bumps the controlled
 * `CodeViewItem.version` so Pierre's reconciler re-renders instead of keeping a
 * stale annotation/decoration record.
 */
export function fileItemRenderSignature(
  filePath: string,
  content: string,
  notes: readonly ReviewNote[],
  modifiedLines: ReadonlySet<number>,
  highlightedNoteId: string | null,
  highlightedLine: number | null,
): string {
  const fp = `${filePath}|${content.length}|${hashContent(content)}`;
  const lineNotes: string[] = [];
  for (const n of notes) {
    const a = n.anchor;
    if (a.kind !== 'file' || a.filePath !== filePath) continue;
    const flash = n.id === highlightedNoteId ? '*' : '';
    lineNotes.push(`${n.id}:${a.lineNumber}:${n.body}:${flash}`);
  }
  const mods = [...modifiedLines].sort((x, y) => x - y).join(',');
  return `${fp}#L[${lineNotes.join(',')}]#M[${mods}]#H[${highlightedLine ?? ''}]`;
}

/** The Pierre scroll target for a file note's anchored line, or null when the
 *  note isn't a file note for this path. */
export interface FileNoteScrollTarget {
  id: string;
  lineNumber: number;
}

export function scrollTargetForNote(
  note: ReviewNote,
  filePath: string,
): FileNoteScrollTarget | null {
  const a = note.anchor;
  if (a.kind !== 'file' || a.filePath !== filePath) return null;
  return { id: fileItemId(filePath), lineNumber: a.lineNumber };
}

/**
 * CSS injected via Pierre's `unsafeCSS` option to shade patch-modified lines
 * and flash the jump-target line. Pierre file rows carry `data-line="N"`; there
 * is no native per-line decoration primitive, so declarative CSS keyed to that
 * attribute is the typed-boundary way to highlight specific lines without
 * scraping Pierre's rendered DOM. Returns '' when there is nothing to shade.
 */
export function lineDecorationCSS(
  modifiedLines: ReadonlySet<number>,
  highlightedLine: number | null,
): string {
  const rules: string[] = [];
  const mods = [...modifiedLines].filter((n) => n !== highlightedLine);
  if (mods.length > 0) {
    const sel = mods.map((n) => `[data-line="${n}"]`).join(',');
    rules.push(`${sel}{background:var(--viewer-modified-line-bg);}`);
  }
  if (highlightedLine !== null) {
    rules.push(`[data-line="${highlightedLine}"]{background:var(--viewer-highlight-line-bg);}`);
  }
  return rules.join('\n');
}

/**
 * Resolve the 1-based line number under a touch point from a pointer event's
 * composed path. Pierre drives `onLineEnter` off mouse pointer-moves only, so a
 * stationary touch never records a hovered line and the long-press handler must
 * find the line itself. A file viewer holds a single item, so only the line
 * number is needed — no item/side disambiguation.
 *
 * This mirrors the single sanctioned DOM-attribute read in the diff wrapper:
 * there is no typed Pierre callback for a touch press target, so the `data-line`
 * attribute Pierre itself emits on each rendered line is read here. Returns
 * undefined when the path does not land on a resolvable line.
 */
export function resolveTouchedLineNumber(path: readonly EventTarget[]): number | undefined {
  for (const node of path) {
    if (!(node instanceof HTMLElement)) continue;
    const raw = node.getAttribute('data-line') ?? node.getAttribute('data-column-number');
    if (raw != null) {
      const n = Number.parseInt(raw, 10);
      if (!Number.isNaN(n)) return n;
    }
  }
  return undefined;
}
