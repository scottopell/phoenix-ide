import type { ReviewNote } from '../../contexts/ReviewNotesContext';

/**
 * Format a review-notes pile into a single markdown blob suitable for
 * dropping into the chat input as a self-code-review message.
 *
 * Notes are grouped by file (or by `diff` for diff anchors), then
 * rendered as a per-line list with the original source line quoted in a
 * code span and the user's body underneath.
 *
 * Returns `null` when the pile is empty so callers can short-circuit
 * before invoking the send path.
 */
export function formatNotesForSend(notes: ReviewNote[]): string | null {
  if (notes.length === 0) return null;

  // Group by display section. File anchors group by filePath; diff
  // anchors collapse into a single "Diff" section regardless of the
  // file they touch — they're all part of the same diff view.
  type Section = { title: string; entries: string[] };
  const sections: Section[] = [];
  const byKey = new Map<string, Section>();

  const sectionFor = (key: string, title: string): Section => {
    let s = byKey.get(key);
    if (!s) {
      s = { title, entries: [] };
      byKey.set(key, s);
      sections.push(s);
    }
    return s;
  };

  for (const n of notes) {
    if (n.anchor.kind === 'file') {
      const s = sectionFor(`file:${n.anchor.filePath}`, `\`${n.anchor.filePath}\``);
      s.entries.push(formatLineEntry(`Line ${n.anchor.lineNumber}`, n.lineContent, n.body));
    } else if (n.anchor.kind === 'diff') {
      const s = sectionFor(`diff:${n.anchor.filePath}`, `Diff: \`${n.anchor.filePath}\``);
      const label =
        n.anchor.newLine !== undefined
          ? `New line ${n.anchor.newLine}`
          : n.anchor.oldLine !== undefined
            ? `Removed line ${n.anchor.oldLine}`
            : `Diff position ${n.anchor.diffPos}`;
      s.entries.push(formatLineEntry(label, n.lineContent, n.body));
    } else {
      // diff-file (file-level diff note)
      const s = sectionFor(`diff:${n.anchor.filePath}`, `Diff: \`${n.anchor.filePath}\``);
      s.entries.push(formatFileEntry(n.body));
    }
  }

  const header = '## Review notes';
  const body = sections
    .map((s) => `### ${s.title}\n\n${s.entries.join('\n\n')}`)
    .join('\n\n');
  return `${header}\n\n${body}`;
}

function formatLineEntry(label: string, lineContent: string, body: string): string {
  const quoted = lineContent.length > 200 ? `${lineContent.slice(0, 200)}…` : lineContent;
  return `- **${label}**: \`${quoted}\`\n  ${body.replace(/\n/g, '\n  ')}`;
}

function formatFileEntry(body: string): string {
  return `- **File-level**:\n  ${body.replace(/\n/g, '\n  ')}`;
}
