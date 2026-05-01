/**
 * Unified-diff parser for the diff viewer.
 *
 * Splits the raw `git diff` text into per-file segments, each with a
 * sequence of hunks containing typed lines. The parser is permissive —
 * unknown header lines are silently kept as raw context, binary file
 * markers are folded into the segment, and truncation mid-hunk is
 * handled by best-effort stopping.
 *
 * Pure function. Unit-tested in `diffParse.test.ts`.
 */

export type DiffLineKind =
  | 'add'           // `+...` (new content)
  | 'del'           // `-...` (removed content)
  | 'context'       // ` ...` (unchanged, shown for context)
  | 'hunk-header'   // `@@ -X,Y +A,B @@`
  | 'file-header'   // `diff --git`, `index`, `---`, `+++`, `rename ...`, `Binary files ...`
  | 'no-newline';   // `\ No newline at end of file`

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
  /** Line number in the OLD file (for `del`, `context`). Undefined for
   *  add lines and headers. */
  oldLine?: number;
  /** Line number in the NEW file (for `add`, `context`). Undefined for
   *  del lines and headers. */
  newLine?: number;
  /** 0-indexed position within the unified-diff text. Stable across
   *  re-renders and used as the primary identity key when anchoring a
   *  review note. */
  diffPos: number;
}

export interface DiffSegment {
  /** New-file path (or old-file path if the file was deleted). */
  filePath: string;
  /** Old-file path when different from `filePath` (renames). Undefined
   *  for adds, modifies, and deletes. */
  oldPath?: string;
  /** True for `Binary files differ` segments. The lines array will be
   *  empty in that case but the file-header lines are still attached
   *  via the parent diff text. */
  binary: boolean;
  /** All lines belonging to this file segment (file-header,
   *  hunk-header, add, del, context, no-newline) in source order. */
  lines: DiffLine[];
}

/** Parse a `git diff` output into per-file segments. */
export function parseUnifiedDiff(diff: string): DiffSegment[] {
  if (!diff) return [];
  const rawLines = diff.split('\n');
  // The trailing element is empty when `diff` ends with \n; drop it.
  if (rawLines.length > 0 && rawLines[rawLines.length - 1] === '') {
    rawLines.pop();
  }

  const segments: DiffSegment[] = [];
  let current: DiffSegment | null = null;
  let oldLine = 0;
  let newLine = 0;

  const startSegment = (filePath: string, oldPath?: string): DiffSegment => {
    const seg: DiffSegment = oldPath !== undefined
      ? { filePath, oldPath, binary: false, lines: [] }
      : { filePath, binary: false, lines: [] };
    segments.push(seg);
    oldLine = 0;
    newLine = 0;
    return seg;
  };

  for (let i = 0; i < rawLines.length; i++) {
    const text = rawLines[i] ?? '';

    // File-segment boundary: `diff --git a/<old> b/<new>`.
    const gitHeader = /^diff --git a\/(.+?) b\/(.+)$/.exec(text);
    if (gitHeader) {
      const [, a, b] = gitHeader;
      // For rename/copy detection we'll wait for `rename to`/`copy to`
      // headers to overwrite filePath; default to the b-side.
      current = startSegment(b ?? a ?? '', a !== b ? a : undefined);
      current.lines.push({ kind: 'file-header', text, diffPos: i });
      continue;
    }

    if (!current) {
      // Garbage before the first `diff --git` — skip.
      continue;
    }

    // File-level header lines that don't start a new segment.
    if (
      text.startsWith('index ') ||
      text.startsWith('--- ') ||
      text.startsWith('+++ ') ||
      text.startsWith('old mode ') ||
      text.startsWith('new mode ') ||
      text.startsWith('deleted file ') ||
      text.startsWith('new file ') ||
      text.startsWith('similarity index ') ||
      text.startsWith('rename from ') ||
      text.startsWith('rename to ') ||
      text.startsWith('copy from ') ||
      text.startsWith('copy to ')
    ) {
      // Update filePath/oldPath when rename/copy headers arrive.
      const renameTo = /^(?:rename|copy) to (.+)$/.exec(text);
      if (renameTo) current.filePath = renameTo[1] ?? current.filePath;
      const renameFrom = /^(?:rename|copy) from (.+)$/.exec(text);
      if (renameFrom && renameFrom[1]) current.oldPath = renameFrom[1];
      current.lines.push({ kind: 'file-header', text, diffPos: i });
      continue;
    }

    if (text.startsWith('Binary files ')) {
      current.binary = true;
      current.lines.push({ kind: 'file-header', text, diffPos: i });
      continue;
    }

    // Hunk header: `@@ -oldStart,oldCount +newStart,newCount @@`
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(text);
    if (hunk) {
      oldLine = parseInt(hunk[1] ?? '1', 10);
      newLine = parseInt(hunk[2] ?? '1', 10);
      current.lines.push({ kind: 'hunk-header', text, diffPos: i });
      continue;
    }

    if (text.startsWith('\\ No newline')) {
      current.lines.push({ kind: 'no-newline', text, diffPos: i });
      continue;
    }

    if (text.startsWith('+')) {
      current.lines.push({ kind: 'add', text, newLine, diffPos: i });
      newLine += 1;
      continue;
    }
    if (text.startsWith('-')) {
      current.lines.push({ kind: 'del', text, oldLine, diffPos: i });
      oldLine += 1;
      continue;
    }
    if (text.startsWith(' ') || text === '') {
      current.lines.push({
        kind: 'context',
        text,
        oldLine,
        newLine,
        diffPos: i,
      });
      oldLine += 1;
      newLine += 1;
      continue;
    }

    // Unknown line — keep as a file-header so it renders neutrally.
    current.lines.push({ kind: 'file-header', text, diffPos: i });
  }

  return segments;
}
