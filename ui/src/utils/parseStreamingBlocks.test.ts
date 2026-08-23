import { describe, it, expect } from 'vitest';
import * as fc from 'fast-check';
import { parseStreamingBlocks, type StreamingBlock } from './parseStreamingBlocks';

const NESTED_MARKDOWN_STRESS = `\`\`\`markdown
You are starting fresh in the \`datadog-agent\` repository on a new branch.

## Context

Recommended destination:

\`\`\`text
tools/check-skip-observability-demo/index.html
\`\`\`

## Current Agent behavior to understand

### Scheduler

\`\`\`text
pkg/collector/scheduler/job.go
\`\`\`

The worker logs something similar to:

\`\`\`text
Check is already running, skipping execution...
\`\`\`

## Desired design direction

\`\`\`text
time →
scheduled ticks:        |   |   |   |   |
actual run spans:       [=======]   [=======]
skipped attempts:           x           x
current gauge samples:          ●           ●
\`\`\`

## Deliverable

Create or replace:

\`\`\`text
tools/check-skip-observability-demo/index.html
\`\`\`
\`\`\`
`;


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

  it('keeps a tilde fence open when later backtick fences cannot close it', () => {
    const buf = '~~~\n```js\n\n```\n';

    expect(parseStreamingBlocks(buf)).toEqual([
      { type: 'code', lang: '', content: '```js\n\n```\n', complete: false },
    ]);
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

  it('keeps fenced markdown documents with nested fences in one code block', () => {
    const result = parseStreamingBlocks(NESTED_MARKDOWN_STRESS);

    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({ type: 'code', lang: 'markdown', complete: true });
    expect(result[0]!.content).toContain('```text\ntools/check-skip-observability-demo/index.html\n```');
    expect(result[0]!.content).toContain('```text\ntime →');
    expect(result[0]!.content).toContain('## Deliverable\n');
  });

  it('does not churn completed block boundaries while streaming nested markdown fences', () => {
    let maxBlocks = 0;
    let previousStablePrefix = '';

    for (let i = 1; i <= NESTED_MARKDOWN_STRESS.length; i++) {
      const blocks = parseStreamingBlocks(NESTED_MARKDOWN_STRESS.slice(0, i));
      maxBlocks = Math.max(maxBlocks, blocks.length);

      const stablePrefix = blocks
        .slice(0, -1)
        .map((block) =>
          block.type === 'code'
            ? `${block.type}:${block.lang}:${block.complete}:${block.content}`
            : `${block.type}:::${block.content}`
        )
        .join('\n---block---\n');
      expect(stablePrefix.startsWith(previousStablePrefix)).toBe(true);
      previousStablePrefix = stablePrefix;
    }

    expect(maxBlocks).toBe(1);
  });

  it('closes same-length unlabeled fences inside markdown documents', () => {
    const result = parseStreamingBlocks('```markdown\nHere:\n```\nfoo\n```\nEnd\n```\n');

    expect(result).toEqual([
      { type: 'code', lang: 'markdown', content: 'Here:\n', complete: true },
      { type: 'markdown', content: 'foo\n' },
      { type: 'code', lang: '', content: 'End\n', complete: true },
    ]);
  });

  it('preserves the outer close after an opener-looking literal line', () => {
    const result = parseStreamingBlocks('```markdown\n```text\n```\n');

    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({
      type: 'code',
      lang: 'markdown',
      content: '```text',
      complete: true,
    });
  });
  it('keeps shorter unlabeled nested fences inside longer markdown wrappers', () => {
    const result = parseStreamingBlocks('````markdown\nHere:\n```\nfoo\n```\nEnd\n````\n');

    expect(result).toEqual([
      { type: 'code', lang: 'markdown', content: 'Here:\n```\nfoo\n```\nEnd\n', complete: true },
    ]);
  });

  it('prefers real outer closes after colon-ended markdown before later fences', () => {
    const result = parseStreamingBlocks('```markdown\ndoc:\n```\ntext\n```\ncode\n```\n');

    expect(result).toEqual([
      { type: 'code', lang: 'markdown', content: 'doc:\n', complete: true },
      { type: 'markdown', content: 'text\n' },
      { type: 'code', lang: '', content: 'code\n', complete: true },
    ]);
  });

  it('keeps longer markdown wrappers incomplete after empty inner closes', () => {
    const result = parseStreamingBlocks('````markdown\n```text\n```\n');

    expect(result).toEqual([
      { type: 'code', lang: 'markdown', content: '```text\n```\n', complete: false },
    ]);
  });
  it('prefers real outer closes before later independent fences', () => {
    const result = parseStreamingBlocks('```markdown\ndoc\n```\ntext\n```\ncode\n```\n');

    expect(result).toEqual([
      { type: 'code', lang: 'markdown', content: 'doc\n', complete: true },
      { type: 'markdown', content: 'text\n' },
      { type: 'code', lang: '', content: 'code\n', complete: true },
    ]);
  });

  it('keeps empty nested prefixes incomplete when more content follows', () => {
    const result = parseStreamingBlocks('```markdown\n```text\n```\nmore');

    expect(result).toEqual([
      { type: 'code', lang: 'markdown', content: '```text\n```\nmore', complete: false },
    ]);
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
  let fenceLang = '';
  let nestedFenceChar = '';
  let nestedFenceLength = 0;

  const closeRe = (char: string, length: number) => new RegExp(`^(${char === '`' ? '`' : '~'}{${length},})\\s*$`);
  const isMarkdownLang = (lang: string) => ['markdown', 'md', 'mdx'].includes(lang.toLowerCase());

  for (let i = 0; i < lines.length; i++) {
    const part = lines[i]!;
    const isLast = i === lines.length - 1;
    const line = isLast && !endsWithNewline ? part : part + '\n';
    const bare = line.endsWith('\n') ? line.slice(0, -1) : line;

    if (insideFence) {
      const nestedOpener = isMarkdownLang(fenceLang) ? /^(`{3,}|~{3,})(.*)$/.exec(bare) : null;
      if (nestedFenceChar !== '') {
        result.push(line);
        if (closeRe(nestedFenceChar, nestedFenceLength).test(bare)) {
          nestedFenceChar = '';
          nestedFenceLength = 0;
        }
      } else if (nestedOpener && !closeRe(fenceChar, fenceLength).test(bare)) {
        nestedFenceChar = nestedOpener[1]![0]!;
        nestedFenceLength = nestedOpener[1]!.length;
        result.push(line);
      } else if (closeRe(fenceChar, fenceLength).test(bare)) {
        insideFence = false;
        fenceChar = '';
        fenceLength = 0;
        fenceLang = '';
      } else {
        result.push(line);
      }
    } else {
      const m = /^(`{3,}|~{3,})(.*)$/.exec(bare);
      if (m) {
        insideFence = true;
        fenceChar = m[1]![0]!;
        fenceLength = m[1]!.length;
        fenceLang = m[2]!.trim().split(/\s+/)[0] ?? '';
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

  it('parses generated single-fence documents exactly', () => {
    const plainLineArb = fc
      .array(fc.constantFrom('a', 'b', 'c', ' ', '0', '1'), { minLength: 0, maxLength: 20 })
      .map((chars) => chars.join(''));
    const fencedDocumentArb = fc.record({
      before: fc.array(plainLineArb, { maxLength: 3 }),
      fenceChar: fc.constantFrom('`', '~'),
      openerLength: fc.integer({ min: 3, max: 6 }),
      closerExtraLength: fc.integer({ min: 0, max: 3 }),
      lang: fc.constantFrom('', 'rust', 'js', 'python'),
      body: fc.array(plainLineArb, { maxLength: 4 }),
      closed: fc.boolean(),
      after: fc.array(plainLineArb, { maxLength: 3 }),
    });

    fc.assert(
      fc.property(fencedDocumentArb, (document) => {
        const before = document.before.map((line) => `${line}\n`).join('');
        const opener = document.fenceChar.repeat(document.openerLength);
        const body = document.body.map((line) => `${line}\n`).join('');
        const closer = document.fenceChar.repeat(
          document.openerLength + document.closerExtraLength
        );
        const after = document.after.map((line) => `${line}\n`).join('');
        const buffer =
          before +
          `${opener}${document.lang}\n` +
          body +
          (document.closed ? `${closer}\n${after}` : after);

        const expected: StreamingBlock[] = [];
        if (before !== '') expected.push({ type: 'markdown', content: before });
        expected.push({
          type: 'code',
          lang: document.lang,
          content: body + (document.closed ? '' : after),
          complete: document.closed,
        });
        if (document.closed && after !== '') {
          expected.push({ type: 'markdown', content: after });
        }

        expect(parseStreamingBlocks(buffer)).toEqual(expected);
      }),
      { numRuns: 500 }
    );
  });
});
