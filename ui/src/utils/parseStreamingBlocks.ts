/**
 * Block-aware streaming markdown parser.
 *
 * Parses an accumulated streaming buffer into a sequence of typed blocks that
 * can be rendered progressively without layout jumps. The key insight: open code
 * fences are rendered as plain monospace `<pre>` containers with matching
 * dimensions, so when the closing fence arrives and syntax highlighting activates,
 * the only visual change is colors appearing — no reflow.
 *
 * This is a pure function; call it on every render with the full buffer.
 */

export type StreamingBlock =
  | { type: 'markdown'; content: string }
  | { type: 'code'; lang: string; content: string; complete: boolean };

/**
 * Match an opening fence line. Returns { char, length, lang } or null.
 *
 * A fence opener is 3 or more backticks (`) or tildes (~) at the start of a
 * line, optionally followed by an info string (language hint).
 */
function matchOpenFence(line: string): { char: string; length: number; lang: string } | null {
  const m = /^(`{3,}|~{3,})(.*)$/.exec(line);
  if (!m) return null;
  const fence = m[1]!;
  const char = fence[0]!;
  const length = fence.length;
  const lang = m[2]!.trim().split(/\s+/)[0] ?? '';
  return { char, length, lang };
}

/**
 * Match a closing fence line given the open fence character and minimum length.
 *
 * CommonMark: closing fence must use the same character and be at least as long
 * as the opening fence, with no trailing non-whitespace characters.
 */
function matchCloseFence(line: string, char: string, minLength: number): boolean {
  const escaped = char === '`' ? '`' : '~';
  const re = new RegExp(`^${escaped}{${minLength},}\\s*$`);
  return re.test(line);
}

/**
 * Parse a streaming buffer into typed blocks.
 *
 * Rules:
 * - Text outside a fenced code block → `markdown` block (rendered via ReactMarkdown)
 * - Fenced code block that has a closing fence → `code` block with complete=true
 * - Fenced code block without a closing fence → `code` block with complete=false
 *
 * The fence lines themselves (opening and closing) are NOT included in block
 * content — only the body lines are.
 *
 * Block content always ends with '\n' unless it is the last block AND the buffer
 * itself does not end with a newline (property P9).
 */
export function parseStreamingBlocks(buffer: string): StreamingBlock[] {
  if (buffer === '') return [];

  const blocks: StreamingBlock[] = [];

  // Split the buffer into logical lines.
  // Strategy: split on '\n' to get segments, then reconstruct lines with their
  // newline terminators. The last segment is either:
  //   - '' if the buffer ends with '\n' (all preceding segments are complete lines)
  //   - a non-empty fragment if the buffer does NOT end with '\n'
  const parts = buffer.split('\n');
  const endsWithNewline = buffer.endsWith('\n');

  // Build the lines array. Each entry is a complete line (with '\n') except
  // possibly the very last one if the buffer doesn't end with '\n'.
  const lines: string[] = [];
  for (let i = 0; i < parts.length; i++) {
    const part = parts[i]!;
    const isLastPart = i === parts.length - 1;

    if (isLastPart && endsWithNewline) {
      // The last part after splitting a newline-terminated buffer is always ''.
      // It doesn't represent a new line — the previous '\n' was the terminator
      // of the line before it. Skip it.
      break;
    }

    if (isLastPart && !endsWithNewline) {
      // Unterminated trailing fragment. Only include it if non-empty.
      if (part !== '') {
        lines.push(part);
      }
      break;
    }

    // Normal middle segment: add back the '\n' that split() removed.
    lines.push(part + '\n');
  }

  // Fence state
  let insideFence = false;
  let fenceChar = '';
  let fenceLength = 0;
  let fenceLang = '';

  // Accumulators
  let mdAccum = '';
  let codeAccum = '';

  function flushMarkdown() {
    if (mdAccum !== '') {
      blocks.push({ type: 'markdown', content: mdAccum });
      mdAccum = '';
    }
  }

  function flushCode(complete: boolean) {
    blocks.push({ type: 'code', lang: fenceLang, content: codeAccum, complete });
    codeAccum = '';
    insideFence = false;
    fenceChar = '';
    fenceLength = 0;
    fenceLang = '';
  }

  for (const line of lines) {
    // For fence matching, use the line without its trailing newline.
    const bare = line.endsWith('\n') ? line.slice(0, -1) : line;

    if (insideFence) {
      if (matchCloseFence(bare, fenceChar, fenceLength)) {
        flushCode(true);
      } else {
        codeAccum += line;
      }
    } else {
      const opener = matchOpenFence(bare);
      if (opener) {
        flushMarkdown();
        insideFence = true;
        fenceChar = opener.char;
        fenceLength = opener.length;
        fenceLang = opener.lang;
        // Opening fence line is consumed; don't accumulate it.
      } else {
        mdAccum += line;
      }
    }
  }

  // Emit any open code fence as incomplete
  if (insideFence) {
    flushCode(false);
  }

  // Emit any trailing markdown
  flushMarkdown();

  return blocks;
}
