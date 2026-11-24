import { describe, expect, it } from 'vitest';
import { fromMarkdown } from 'mdast-util-from-markdown';
import { prepareMarkdown, usesGfmSyntax } from './prepareMarkdown';
import { gfmOptions, interpretMarkdown } from './remarkDetails';

describe('prepareMarkdown conditional GFM and disclosure-free fast path', () => {
  it('skips GFM for plain markdown without GFM markers', () => {
    const plain = [
      'Hello world.\n\n**bold** and `inline code`.',
      '# Heading\n\n- item one\n- item two\n\n```ts\nconst x = a | b;\n```',
      'Check ~/dev/phoenix-ide for the file.',
      '[link](https://example.com) is handled by CommonMark — wait, url triggers GFM safely',
    ];
    expect(usesGfmSyntax(plain[0]!)).toBe(false);
    expect(usesGfmSyntax(plain[1]!)).toBe(false);
    expect(usesGfmSyntax(plain[2]!)).toBe(false);
    expect(usesGfmSyntax(plain[3]!)).toBe(true);
  });

  it.each([
    ['outer-pipe table', '| a | b |\n| - | - |\n| 1 | 2 |'],
    ['pipe-less table', 'a | b\n- | -\n1 | 2'],
    ['blockquoted table', '> | a |\n> | - |\n> | 1 |'],
    ['unordered task list', '- [x] done\n- [ ] todo'],
    ['ordered task list', '1. [X] done'],
    ['plus task list', '+ [ ] todo'],
    ['double-tilde strike', '~~removed~~'],
    ['single-tilde strike', '~removed~'],
    ['single-char strike', '~x~'],
    ['https autolink', 'See https://example.com/path'],
    ['www autolink', 'Visit www.example.com today'],
    ['email autolink', 'Mail dev@example.com for details'],
    ['footnote', 'Claim[^1]\n\n[^1]: source'],
  ])('detects %s and preserves full GFM AST parity', (_label, source) => {
    expect(usesGfmSyntax(source)).toBe(true);
    expect(prepareMarkdown(source, 'conversation')).toEqual(fromMarkdown(source, gfmOptions));
  });

  it('skips disclosure recognition sessions when no details/summary tag exists', () => {
    const source = '# Title\n\nPlain paragraph with `code` and [ref][a].\n\n[a]: /target';
    const result = interpretMarkdown(source);
    expect(result.stats.sessions).toBe(0);
    expect(result.stats.documentBytes).toBe(0);
    expect(result.definitions).toHaveLength(1);
    expect(result.tree).toEqual(fromMarkdown(source));
  });
});
