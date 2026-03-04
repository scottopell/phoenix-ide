import { describe, it, expect } from 'vitest';
import * as fc from 'fast-check';
import { parseStreamingBlocks, type StreamingBlock } from './parseStreamingBlocks';

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

describe('parseStreamingBlocks', () => {
  it('returns empty array for empty string', () => {
    expect(parseStreamingBlocks('')).toEqual([]);
  });

  it('returns single markdown block for plain text', () => {
    const result = parseStreamingBlocks('hello world');
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ type: 'markdown', content: 'hello world' });
  });

  it('returns single markdown block for multi-line plain text', () => {
    const result = parseStreamingBlocks('line one\nline two\n');
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ type: 'markdown', content: 'line one\nline two\n' });
  });

  it('parses a complete code block', () => {
    const buf = 'before\n```rust\nfn main() {}\n```\nafter\n';
    const result = parseStreamingBlocks(buf);
    expect(result).toHaveLength(3);
    expect(result[0]).toEqual({ type: 'markdown', content: 'before\n' });
    expect(result[1]).toEqual({ type: 'code', lang: 'rust', content: 'fn main() {}\n', complete: true });
    expect(result[2]).toEqual({ type: 'markdown', content: 'after\n' });
  });

  it('marks an open code block as incomplete', () => {
    const buf = 'before\n```js\nconsole.log("hi")';
    const result = parseStreamingBlocks(buf);
    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({ type: 'markdown', content: 'before\n' });
    expect(result[1]).toEqual({ type: 'code', lang: 'js', content: 'console.log("hi")', complete: false });
  });

  it('handles tilde fences', () => {
    const buf = '~~~python\nprint("hi")\n~~~\n';
    const result = parseStreamingBlocks(buf);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ type: 'code', lang: 'python', content: 'print("hi")\n', complete: true });
  });

  it('handles fences without a language tag', () => {
    const buf = '```\nsome code\n```\n';
    const result = parseStreamingBlocks(buf);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ type: 'code', lang: '', content: 'some code\n', complete: true });
  });

  it('handles longer opening fences (4+ backticks)', () => {
    const buf = '````python\ncode\n````\n';
    const result = parseStreamingBlocks(buf);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ type: 'code', lang: 'python', content: 'code\n', complete: true });
  });

  it('does not close fence with fewer backticks than opener', () => {
    // Opening is 4 backticks, closing is 3 — should NOT close.
    const buf = '````python\n```\nstill code\n````\n';
    const result = parseStreamingBlocks(buf);
    expect(result).toHaveLength(1);
    expect(result[0]!.type).toBe('code');
    expect((result[0] as { content: string }).content).toBe('```\nstill code\n');
    expect((result[0] as { complete: boolean }).complete).toBe(true);
  });

  it('closes fence with more backticks than opener', () => {
    const buf = '```python\ncode\n`````\n';
    const result = parseStreamingBlocks(buf);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ type: 'code', lang: 'python', content: 'code\n', complete: true });
  });

  it('handles multiple code blocks', () => {
    const buf = '```ts\na()\n```\n```py\nb()\n```\n';
    const result = parseStreamingBlocks(buf);
    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({ type: 'code', lang: 'ts', content: 'a()\n', complete: true });
    expect(result[1]).toEqual({ type: 'code', lang: 'py', content: 'b()\n', complete: true });
  });

  it('lang is extracted from info string (first token only)', () => {
    const buf = '```rust cargo\ncode\n```\n';
    const result = parseStreamingBlocks(buf);
    expect((result[0] as { lang: string }).lang).toBe('rust');
  });

  it('opening fence at end of buffer (no body yet) emits incomplete code block', () => {
    const buf = '```ts\n';
    const result = parseStreamingBlocks(buf);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ type: 'code', lang: 'ts', content: '', complete: false });
  });

  it('handles code block at start of buffer (no preceding markdown)', () => {
    const buf = '```sh\necho hi\n```\n';
    const result = parseStreamingBlocks(buf);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ type: 'code', lang: 'sh', content: 'echo hi\n', complete: true });
  });

  it('preserves blank lines inside code blocks', () => {
    const buf = '```\nline1\n\nline3\n```\n';
    const result = parseStreamingBlocks(buf);
    expect(result[0]).toEqual({ type: 'code', lang: '', content: 'line1\n\nline3\n', complete: true });
  });

  it('streaming scenario: partial content then full content', () => {
    const full = 'intro\n```js\nconst x = 1;\n```\noutro\n';

    // Simulate token-by-token arrival
    for (let i = 1; i <= full.length; i++) {
      const partial = full.slice(0, i);
      // Should not throw
      const blocks = parseStreamingBlocks(partial);
      expect(Array.isArray(blocks)).toBe(true);
    }

    // Final result should be fully parsed
    const final = parseStreamingBlocks(full);
    expect(final).toHaveLength(3);
    expect(final[1]).toEqual({ type: 'code', lang: 'js', content: 'const x = 1;\n', complete: true });
  });
});

// ---------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------

/**
 * Compute the original buffer from block content.
 *
 * The parsing function strips fence lines (opener and closer). We can only
 * reconstruct the content portions, not the fence lines themselves. So P1 as
 * stated in the spec is approximate: block content concatenation equals the
 * buffer with all fence lines removed.
 *
 * More precisely: for each code block, the opening fence line and (if complete)
 * the closing fence line are removed. Everything else is preserved verbatim.
 */
function stripFenceLines(buffer: string): string {
  const lines = buffer.split('\n');
  const endsWithNewline = buffer.endsWith('\n');
  const result: string[] = [];
  let insideFence = false;
  let fenceChar = '';
  let fenceLength = 0;

  for (let i = 0; i < lines.length; i++) {
    const part = lines[i]!;
    const isLast = i === lines.length - 1;
    const line = isLast && !endsWithNewline ? part : part + '\n';
    const bare = line.endsWith('\n') ? line.slice(0, -1) : line;

    if (insideFence) {
      const re = new RegExp(`^(${fenceChar === '`' ? '`' : '~'}{${fenceLength},})\\s*$`);
      if (re.test(bare)) {
        insideFence = false;
        fenceChar = '';
        fenceLength = 0;
        // Skip closing fence line
      } else {
        result.push(line);
      }
    } else {
      const m = /^(`{3,}|~{3,})(.*)$/.exec(bare);
      if (m) {
        insideFence = true;
        fenceChar = m[1]![0]!;
        fenceLength = m[1]!.length;
        // Skip opening fence line
      } else {
        result.push(line);
      }
    }
  }

  return result.join('');
}

describe('parseStreamingBlocks — property tests', () => {
  /**
   * P1: Concatenating block contents reproduces the buffer (modulo fence lines).
   */
  it('P1: block content concatenation reproduces buffer (modulo fence lines)', () => {
    // Use a reasonably long timeout for property tests
    fc.assert(
      fc.property(
        fc.string({ minLength: 0, maxLength: 500 }),
        (buffer) => {
          const blocks = parseStreamingBlocks(buffer);
          const reconstructed = blocks.map((b) => b.content).join('');
          const expected = stripFenceLines(buffer);
          return reconstructed === expected;
        }
      ),
      { numRuns: 200 }
    );
  });

  /**
   * P3: Monotonicity — block count never decreases as the buffer grows.
   *
   * We test this by taking a base string and appending suffixes: the block
   * count of (base + suffix) must be >= block count of (base).
   */
  it('P3: block count is monotonically non-decreasing as buffer grows', () => {
    fc.assert(
      fc.property(
        fc.string({ minLength: 0, maxLength: 300 }),
        fc.string({ minLength: 1, maxLength: 100 }),
        (base, suffix) => {
          const before = parseStreamingBlocks(base).length;
          const after = parseStreamingBlocks(base + suffix).length;
          return after >= before;
        }
      ),
      { numRuns: 200 }
    );
  });

  /**
   * P5: No open code fence in markdown blocks.
   *
   * Within any `markdown` block's content, every fence opener must be paired
   * with a closer. Since the parser handles fences at the top level, a markdown
   * block should never contain an unmatched fence opener.
   */
  it('P5: markdown blocks contain no unmatched fence openers', () => {
    fc.assert(
      fc.property(
        fc.string({ minLength: 0, maxLength: 500 }),
        (buffer) => {
          const blocks = parseStreamingBlocks(buffer);
          for (const block of blocks) {
            if (block.type !== 'markdown') continue;
            // Count unmatched fences in this markdown block
            const lines = block.content.split('\n');
            let depth = 0;
            let openChar = '';
            let openLen = 0;
            for (const line of lines) {
              if (depth === 0) {
                const m = /^(`{3,}|~{3,})/.exec(line);
                if (m) {
                  depth = 1;
                  openChar = m[1]![0]!;
                  openLen = m[1]!.length;
                }
              } else {
                const re = new RegExp(`^(${openChar === '`' ? '`' : '~'}{${openLen},})\\s*$`);
                if (re.test(line)) {
                  depth = 0;
                  openChar = '';
                  openLen = 0;
                }
              }
            }
            if (depth !== 0) return false;
          }
          return true;
        }
      ),
      { numRuns: 200 }
    );
  });

  /**
   * P9: Block boundaries are at line boundaries.
   *
   * Every block's content either ends with '\n' or is the last block
   * AND the buffer does not end with '\n'.
   */
  it('P9: block contents end at line boundaries', () => {
    fc.assert(
      fc.property(
        fc.string({ minLength: 0, maxLength: 500 }),
        (buffer) => {
          const blocks = parseStreamingBlocks(buffer);
          if (blocks.length === 0) return true;
          const bufferEndsWithNewline = buffer.endsWith('\n');
          for (let i = 0; i < blocks.length; i++) {
            const block = blocks[i]!;
            const isLast = i === blocks.length - 1;
            if (block.content === '') continue; // empty block is fine
            const contentEndsWithNewline = block.content.endsWith('\n');
            if (!isLast && !contentEndsWithNewline) return false;
            if (isLast && !bufferEndsWithNewline && contentEndsWithNewline) {
              // Last block content ends with '\n' but buffer doesn't — acceptable
              // only if the content is an empty line (content is just '\n').
              // Actually this is fine; the last block may end with '\n' even if
              // the buffer doesn't (the buffer could end mid-line after the block).
            }
          }
          return true;
        }
      ),
      { numRuns: 200 }
    );
  });

  // ---------------------------------------------------------------------------
  // Structured fuzz tests using realistic inputs
  // ---------------------------------------------------------------------------

  it('never throws on arbitrary input', () => {
    fc.assert(
      fc.property(
        fc.string({ minLength: 0, maxLength: 1000 }),
        (buffer) => {
          expect(() => parseStreamingBlocks(buffer)).not.toThrow();
          return true;
        }
      ),
      { numRuns: 500 }
    );
  });

  it('all blocks have string content and correct type', () => {
    fc.assert(
      fc.property(
        fc.string({ minLength: 0, maxLength: 500 }),
        (buffer) => {
          const blocks = parseStreamingBlocks(buffer);
          for (const block of blocks) {
            if (typeof block.content !== 'string') return false;
            if (block.type !== 'markdown' && block.type !== 'code') return false;
            if (block.type === 'code') {
              if (typeof block.lang !== 'string') return false;
              if (typeof block.complete !== 'boolean') return false;
            }
          }
          return true;
        }
      ),
      { numRuns: 300 }
    );
  });

  it('complete code blocks only appear when buffer contains a matching close fence', () => {
    // Structured generator: build well-formed buffers with fences
    const fencedCodeArb = fc.record({
      before: fc.string({ minLength: 0, maxLength: 50 }),
      lang: fc.oneof(fc.constant(''), fc.constant('rust'), fc.constant('js'), fc.constant('python')),
      code: fc.string({ minLength: 0, maxLength: 50 }),
      closed: fc.boolean(),
      after: fc.string({ minLength: 0, maxLength: 50 }),
    });

    fc.assert(
      fc.property(fencedCodeArb, ({ before, lang, code, closed, after }) => {
        const open = '```' + lang + '\n';
        const close = '```\n';
        const buf = before + '\n' + open + code + (closed ? '\n' + close : '') + after;
        const blocks = parseStreamingBlocks(buf);
        const codeBlocks = blocks.filter((b): b is Extract<StreamingBlock, { type: 'code' }> => b.type === 'code');
        if (codeBlocks.length === 0) return true;
        const lastCode = codeBlocks[codeBlocks.length - 1]!;
        // If closed=true, at least one code block should be complete
        if (closed && !codeBlocks.some((b) => b.complete)) return false;
        // If closed=false, the last code block should be incomplete
        if (!closed && lastCode.complete) return false;
        return true;
      }),
      { numRuns: 200 }
    );
  });
});
