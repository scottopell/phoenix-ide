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

  // Group by display section. File anchors group per file path. Diff
  // anchors group per (filePath, section) so the recipient can tell a
  // committed-section note from an uncommitted-section note on the
  // same file at the same line number — the two share a `diffPos`
  // namespace per section and a single label like "New line 1" would
  // otherwise be ambiguous.
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
      const sectionLabel = n.anchor.section === 'committed' ? 'committed' : 'uncommitted';
      const s = sectionFor(
        `diff:${sectionLabel}:${n.anchor.filePath}`,
        `Diff (${sectionLabel}): \`${n.anchor.filePath}\``,
      );
      const label =
        n.anchor.newLine !== undefined
          ? `New line ${n.anchor.newLine}`
          : n.anchor.oldLine !== undefined
            ? `Removed line ${n.anchor.oldLine}`
            : `Diff position ${n.anchor.diffPos}`;
      s.entries.push(formatLineEntry(label, n.lineContent, n.body));
    } else {
      // diff-file (file-level diff note)
      const sectionLabel = n.anchor.section === 'committed' ? 'committed' : 'uncommitted';
      const s = sectionFor(
        `diff:${sectionLabel}:${n.anchor.filePath}`,
        `Diff (${sectionLabel}): \`${n.anchor.filePath}\``,
      );
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
  return `- **${label}**: ${wrapInCodeSpan(quoted)}\n  ${body.replace(/\n/g, '\n  ')}`;
}

/**
 * Wrap a string of source code in a markdown inline code span, picking
 * a backtick delimiter longer than any backtick run in the content so
 * the result is always valid CommonMark even if the line itself
 * contains backticks (e.g. `` `array.length` ``). Per the CommonMark
 * spec, the content must also be padded with a space when it begins or
 * ends with a backtick or whitespace.
 */
function wrapInCodeSpan(content: string): string {
  // Find the longest run of consecutive backticks in `content`; the
  // delimiter must be longer.
  let longest = 0;
  let cur = 0;
  for (const ch of content) {
    if (ch === '`') {
      cur += 1;
      if (cur > longest) longest = cur;
    } else {
      cur = 0;
    }
  }
  const fence = '`'.repeat(longest + 1);
  // CommonMark: if the content begins or ends with a backtick, the
  // delimiter would visually merge with the content. Padding with a
  // space prevents that — the renderer strips a single leading/
  // trailing space inside a code span. Whitespace-only or
  // whitespace-bracketed content doesn't need this special case (the
  // renderer's whitespace-collapsing rules already handle it).
  const needsPad = content.startsWith('`') || content.endsWith('`');
  const padded = needsPad ? ` ${content} ` : content;
  return `${fence}${padded}${fence}`;
}

function formatFileEntry(body: string): string {
  return `- **File-level**:\n  ${body.replace(/\n/g, '\n  ')}`;
}
