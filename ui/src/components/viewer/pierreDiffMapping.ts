/**
 * Pure mapping layer between Phoenix review-note anchors and Pierre's
 * `@pierre/diffs` model. This is the typed boundary: nothing here touches the
 * DOM, React, or Pierre's rendered output — it only converts raw diff text and
 * Phoenix anchors into Pierre `CodeViewDiffItem`s / `DiffLineAnnotation`s and
 * back. Unit-tested in `pierreDiffMapping.test.ts`.
 *
 * Identity is structural: an item id is `${section}:${filePath}`, so the same
 * file appearing in both the committed and uncommitted sections occupies two
 * distinct items that can never collide. A line note's identity is
 * (section, filePath, side, lineNumber); the full Phoenix anchor is carried in
 * the annotation metadata so the renderer never has to reconstruct it.
 */

import { parsePatchFiles } from '@pierre/diffs';
import type {
  AnnotationSide,
  CodeViewDiffItem,
  DiffLineAnnotation,
  FileDiffMetadata,
} from '@pierre/diffs';
import type { DiffSection, NoteAnchor, ReviewNote } from '../../contexts/ReviewNotesContext';

/**
 * Metadata stored on every Pierre annotation. The full Phoenix anchor is kept
 * verbatim so jump/indicator logic reads typed data rather than re-deriving the
 * anchor from line numbers (which would collide across sections).
 */
export interface PhoenixDiffAnnotationMeta {
  noteId: string;
  anchor: NoteAnchor;
}

export type PhoenixDiffItem = CodeViewDiffItem<PhoenixDiffAnnotationMeta>;
export type PhoenixDiffAnnotation = DiffLineAnnotation<PhoenixDiffAnnotationMeta>;

/** Stable item id. `filePath` may legally contain `:`; recovery never splits on
 *  it (see {@link sectionFromItemId} — section is a fixed prefix, file path is
 *  read from `fileDiff.name`). */
export function itemId(section: DiffSection, filePath: string): string {
  return `${section}:${filePath}`;
}

/** Recover the section from an item id by its fixed prefix. Returns null for an
 *  id that isn't one of ours. The file path is intentionally NOT parsed out of
 *  the id — callers read it from `item.fileDiff.name`, which survives the
 *  collision-suffix scheme in {@link buildSectionItems}. */
export function sectionFromItemId(id: string): DiffSection | null {
  if (id.startsWith('committed:')) return 'committed';
  if (id.startsWith('uncommitted:')) return 'uncommitted';
  return null;
}

export interface BuiltSection {
  /** Parsed file items for this section, with collision-free ids. */
  items: PhoenixDiffItem[];
  /** Non-null when the raw diff was non-empty but could not be parsed; the
   *  viewer shows a section-scoped fallback rather than crashing. */
  error: string | null;
}

/**
 * Parse one section's raw unified diff into Pierre diff items. Permissive: a
 * parse failure is captured as `error` (not thrown) so a malformed section
 * degrades to a fallback without taking down the conversation page. Item ids
 * are de-duplicated with a `#n` suffix should the same path appear twice within
 * a section (pathological, but keeps CodeView's unique-id invariant intact).
 */
export function buildSectionItems(section: DiffSection, rawDiff: string): BuiltSection {
  if (!rawDiff.trim()) return { items: [], error: null };

  let parsed;
  try {
    // cacheKeyPrefix keyed by section so Pierre's parse cache can't alias the
    // same path across committed/uncommitted. throwOnError=false: we surface
    // partial results and report below rather than crash.
    parsed = parsePatchFiles(rawDiff, section, false);
  } catch (err) {
    return { items: [], error: err instanceof Error ? err.message : 'Failed to parse diff' };
  }

  const items: PhoenixDiffItem[] = [];
  const seen = new Set<string>();
  for (const patch of parsed) {
    for (const fileDiff of patch.files) {
      const base = itemId(section, fileDiff.name);
      let id = base;
      let n = 1;
      while (seen.has(id)) id = `${base}#${n++}`;
      seen.add(id);
      items.push({ id, type: 'diff', fileDiff });
    }
  }

  // Non-empty input that yielded no files is itself a malformed/unsupported
  // diff — report it so the section shows a fallback instead of silent blank.
  if (items.length === 0) return { items, error: 'Could not parse any files from this diff.' };
  return { items, error: null };
}

/**
 * Convert a single Phoenix review note into a Pierre line annotation. Returns
 * null for notes that are not line-anchored diff notes (file-level diff notes
 * are rendered through the header, not as line annotations; file-viewer notes
 * belong to a different surface entirely).
 */
export function noteToAnnotation(note: ReviewNote): PhoenixDiffAnnotation | null {
  const a = note.anchor;
  if (a.kind !== 'diff') return null;
  const metadata: PhoenixDiffAnnotationMeta = { noteId: note.id, anchor: a };
  if (a.newLine !== undefined) return { side: 'additions', lineNumber: a.newLine, metadata };
  if (a.oldLine !== undefined) return { side: 'deletions', lineNumber: a.oldLine, metadata };
  return null;
}

/**
 * A signature of everything PhoenixDiffCodeView draws for one diff item: the
 * parsed file fingerprint plus the line/file notes (and which one is flashed).
 * The wrapper turns a change in this string into a bumped `CodeViewItem.version`
 * so Pierre's controlled reconciler re-renders the item — without a version
 * bump it keeps the prior record and the inline annotation / flash / file-note
 * count would go stale even though the items array is new.
 */
export function itemRenderSignature(
  fileDiff: FileDiffMetadata,
  notes: readonly ReviewNote[],
  section: DiffSection,
  highlightedNoteId: string | null,
): string {
  const filePath = fileDiff.name;
  // Cheap content fingerprint: changes when a refetch reparses this path.
  const fp = [
    fileDiff.name,
    fileDiff.prevName ?? '',
    fileDiff.type,
    fileDiff.unifiedLineCount,
    fileDiff.hunks.length,
    fileDiff.additionLines.length,
    fileDiff.deletionLines.length,
  ].join('|');
  const lineNotes: string[] = [];
  const fileNotes: string[] = [];
  for (const n of notes) {
    const a = n.anchor;
    const flash = n.id === highlightedNoteId ? '*' : '';
    if (a.kind === 'diff' && a.section === section && a.filePath === filePath) {
      lineNotes.push(`${n.id}:${a.newLine ?? ''}:${a.oldLine ?? ''}:${n.body}:${flash}`);
    } else if (a.kind === 'diff-file' && a.section === section && a.filePath === filePath) {
      fileNotes.push(`${n.id}:${n.body}:${flash}`);
    }
  }
  return `${fp}#L[${lineNotes.join(',')}]#F[${fileNotes.join(',')}]`;
}

/** All line annotations for a given (section, filePath), in note order. */
export function annotationsForFile(
  notes: readonly ReviewNote[],
  section: DiffSection,
  filePath: string,
): PhoenixDiffAnnotation[] {
  const out: PhoenixDiffAnnotation[] = [];
  for (const note of notes) {
    if (note.anchor.kind !== 'diff') continue;
    if (note.anchor.section !== section || note.anchor.filePath !== filePath) continue;
    const ann = noteToAnnotation(note);
    if (ann) out.push(ann);
  }
  return out;
}

/** File-level (header) diff notes for a given (section, filePath). */
export function fileNotesFor(
  notes: readonly ReviewNote[],
  section: DiffSection,
  filePath: string,
): ReviewNote[] {
  return notes.filter(
    (n) =>
      n.anchor.kind === 'diff-file' &&
      n.anchor.section === section &&
      n.anchor.filePath === filePath,
  );
}

/**
 * Recover the raw text of a diff line from typed Pierre hunk data — no DOM
 * scraping. `additionLines`/`deletionLines` hold the per-side content in source
 * order; each hunk records where its slice starts (`additionLineIndex`) and the
 * file line number of that slice's first row (`additionStart`), so a side+line
 * number maps to an array index by simple offset.
 */
export function lineTextAt(
  fileDiff: FileDiffMetadata,
  side: AnnotationSide,
  lineNumber: number,
): string | undefined {
  for (const h of fileDiff.hunks) {
    if (side === 'additions') {
      if (lineNumber >= h.additionStart && lineNumber < h.additionStart + h.additionCount) {
        return stripEol(fileDiff.additionLines[h.additionLineIndex + (lineNumber - h.additionStart)]);
      }
    } else if (lineNumber >= h.deletionStart && lineNumber < h.deletionStart + h.deletionCount) {
      return stripEol(fileDiff.deletionLines[h.deletionLineIndex + (lineNumber - h.deletionStart)]);
    }
  }
  return undefined;
}

/** Pierre's per-side line arrays retain the source line ending; strip a single
 *  trailing newline so the quoted note line is the bare source line. */
function stripEol(line: string | undefined): string | undefined {
  return line === undefined ? undefined : line.replace(/\r?\n$/, '');
}

/** The Pierre scroll target for a note's anchor, or null when the note isn't a
 *  diff note. File-level notes target the item; line notes target the line. */
export interface NoteScrollTarget {
  id: string;
  /** Present for line notes; absent for file-level (item-level) targets. */
  line?: { lineNumber: number; side: AnnotationSide };
}

export function scrollTargetForNote(note: ReviewNote): NoteScrollTarget | null {
  const a = note.anchor;
  if (a.kind === 'diff-file') return { id: itemId(a.section, a.filePath) };
  if (a.kind !== 'diff') return null;
  const id = itemId(a.section, a.filePath);
  if (a.newLine !== undefined) return { id, line: { lineNumber: a.newLine, side: 'additions' } };
  if (a.oldLine !== undefined) return { id, line: { lineNumber: a.oldLine, side: 'deletions' } };
  return { id };
}
