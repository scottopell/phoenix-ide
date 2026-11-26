/**
 * Line texts from a bash tool result's `lines` field. Model-facing results
 * list each line as a string (REQ-BASH-004); transcripts persisted before
 * that carry `{ offset, bytes }` objects. Both decode to the same texts.
 */
export function bashResultLineTexts(lines: unknown): string[] {
  if (!Array.isArray(lines)) return [];
  return lines.map((line) => {
    if (typeof line === 'string') return line;
    if (line && typeof line === 'object' && typeof (line as { bytes?: unknown }).bytes === 'string') {
      return (line as { bytes: string }).bytes;
    }
    return '';
  });
}
