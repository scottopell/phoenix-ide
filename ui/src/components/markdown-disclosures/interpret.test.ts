import { describe, expect, it } from 'vitest';
import { fromMarkdown } from 'mdast-util-from-markdown';
import { unified } from 'unified';
import remarkParse from 'remark-parse';
import remarkGfm from 'remark-gfm';
import type { Nodes, Root } from 'mdast';
import {
  basicOptions, gfmOptions, interpretMarkdown, markdownDetailsRanges,
  markdownReferenceContext, remarkDetails,
} from '../remarkDetails';

const envelope = (body: string, summary = 'T') => `<details><summary>${summary}</summary>${body}</details>`;
function nodes<T extends Nodes['type']>(tree: Nodes, type: T): Extract<Nodes, { type: T }>[] {
  const result: Extract<Nodes, { type: T }>[] = [];
  const stack = [tree];
  while (stack.length) {
    const node = stack.pop()!;
    if (node.type === type) result.push(node as Extract<Nodes, { type: T }>);
    if ('children' in node) stack.push(...[...node.children].reverse());
  }
  return result;
}
const withoutPositions = (value: unknown): unknown => JSON.parse(JSON.stringify(value, (key, item: unknown) => key === 'position' ? undefined : item));
const ordinary = [
  '', 'para\n---', '# Heading\n\n## H\n\n- first\n  continuation\n\n  second',
  '> - first\n>   second', '- [x] done\n- [ ] todo',
  '| a | b |\n| - | - |\n| **yes** | ~~no~~ |',
  'https://example.com and a@b.com ~~old~~', '[x][r] ![alt][r]\n\n[r]: /u "t"',
  'line  \nbreak\n\n```js\nx\n```', '普通 😺\r\n\r\nparagraph',
  'foot[^a]\n\n[^a]: hi', '---\n\n# Heading\n\n![a](u) [b](v)',
  '    code\n\n> quote\n\n[x][no]', 'a\rb\r\rc',
];

for (const [name, options] of [['basic', basicOptions], ['GFM', gfmOptions]] as const) {
  describe(`${name} ordinary native AST parity`, () => {
    it.each(ordinary)('preserves exact nodes and positions: %j', source => {
      expect(interpretMarkdown(source, options).tree).toEqual(fromMarkdown(source, options));
      const processor = unified().use(remarkParse);
      if (name === 'GFM') processor.use(remarkGfm);
      processor.use(remarkDetails);
      expect(processor.parse(source)).toEqual(fromMarkdown(source, options));
    });
  });
}

describe('disclosure syntax and native contexts', () => {
  const bodies = [
    '**body**', '[literal](<details>)', '![literal](</details>)',
    '[literal](https://example.com "<details></details></summary>")',
    "[literal](https://example.com '<details></summary>')",
    '[literal](https://example.com (<details></summary>))',
    '[literal](https://example.com "first\nnext <details></summary>")',
    '`</details>`', '\n\n```html\n</details>\n```\n\nAfter\n',
    '> quote </details>\n\nAfter\n', '- item </details>\n\nAfter\n',
    '2. item </details>\n\nAfter\n', '> - ```\n>   </details>\n>   ```\n\nAfter\n',
    '\n\n    </details>\n\nAfter\n', '\n\n> ```html\n> </details>\n\nAfter\n',
    '\n\n- ```html\n  </details>\n\nAfter\n', '\n\n> quote\n>\n>     </details>\n\nAfter\n',
    '| head |\n| - |\n| </details> |\n\nAfter\n', '[^x]: </details>\n\nAfter\n',
    '> | a |\n> | - |\n> | </details> |\n>\n> tail\n\nAfter\n',
    'head\n---\n\n# Heading\n\n- [x] task\n\n~~strike~~ https://example.com\n',
    '`unmatched\n\n<script>\n`\n</details>\n</script>\n\nBody\n',
  ];
  it.each(bodies)('retains native same-line body structure: %j', body => {
    const source = envelope(body);
    const result = interpretMarkdown(source, gfmOptions);
    expect(result.ranges).toEqual([{ start: 0, end: source.length }]);
    const details = nodes(result.tree, 'disclosureDetails');
    expect(details).toHaveLength(1);
    expect(withoutPositions(details[0]!.children.slice(1))).toEqual(withoutPositions(fromMarkdown(body, gfmOptions).children));
  });

  for (const image of ['', '!']) for (const label of ['</details>', 'escaped\\]</details>', 'escaped\\[</details>']) {
    for (const defined of [false, true]) it(`syntax-first label ${image}${label}, defined=${defined}`, () => {
      const frame = envelope(`${image}[text][${label}] remaining`);
      const source = frame + (defined ? `\n\n[${label}]: /url` : '');
      const result = interpretMarkdown(source, gfmOptions);
      expect(result.ranges).toEqual([{ start: 0, end: frame.length }]);
      expect(nodes(result.tree, image ? 'image' : 'link')).toHaveLength(defined ? 1 : 0);
      if (!defined) {
        const body = `${image}[text][${label}] remaining`;
        expect(withoutPositions(nodes(result.tree, 'disclosureDetails')[0]!.children.slice(1))).toEqual(withoutPositions(fromMarkdown(body, gfmOptions).children));
      }
    });
  }
  for (const image of ['', '!']) it.each(['unterminated </details>', 'unescaped[</details>]'])('malformed label does not hide a close: %s', label => {
    const source = envelope(`${image}[text][${label} remaining`);
    expect(interpretMarkdown(source).ranges).toEqual([{ start: 0, end: source.indexOf('</details>') + 10 }]);
  });

  it('resolves the cyclic summary reference only after native definitions are collected', () => {
    const source = '<details><summary>[x][</summary><script>]</summary>\n\n[</summary><script>]: /url\n\n</script>\n\nBody\n</details>';
    const result = interpretMarkdown(source, gfmOptions);
    expect(result.ranges).toEqual([{ start: 0, end: source.length }]);
    expect(nodes(result.tree, 'link').map(n => n.url)).toEqual(['/url']);
  });
  it('does not turn an opaque cyclic definition into a link', () => {
    const source = '<details><summary>[x][</summary>] remaining</summary><script>\n[</summary>]: /url\n</script>\n\nBody\n</details>';
    const result = interpretMarkdown(source);
    expect(result.ranges).toEqual([{ start: 0, end: source.length }]);
    expect(result.definitions).toEqual([]);
    expect(nodes(result.tree, 'link')).toEqual([]);
    const native = fromMarkdown('[x][</summary>] remaining').children[0]!;
    expect(native.type).toBe('paragraph');
    if (native.type !== 'paragraph') throw new Error('Native summary oracle is not phrasing');
    expect(withoutPositions(nodes(result.tree, 'disclosureSummary')[0]!.children)).toEqual(withoutPositions(native.children));
  });

  for (const tag of ['script', 'style', 'pre', 'textarea']) for (const lead of ['', '\n\n']) {
    it(`preserves opaque ${tag} with lead ${JSON.stringify(lead)}`, () => {
      const raw = `<${tag}>\n</summary>\n</details>\n[docs]: /fake\n\n<details><summary>Fake</summary>Fake</details>\n</${tag}>`;
      const result = interpretMarkdown(envelope(`${lead}${raw}\n\n[body][docs]\n\n`, '[Title][docs]') + '\n[docs]: /real', gfmOptions);
      expect(nodes(result.tree, 'disclosureDetails')).toHaveLength(1);
      expect(nodes(result.tree, 'html')[0]!.value).toBe(raw);
      expect(nodes(result.tree, 'link').map(n => n.url)).toEqual(['/real', '/real']);
    });
  }

  it.each(['<details><summary>T</summary>unfinished', '<details>missing summary</details>', envelope(envelope('nested')), '> ' + envelope('inside'), '1. ' + envelope('inside')])('rejects invalid or non-root frames: %j', source => {
    const result = interpretMarkdown(source);
    expect(result.ranges).toEqual([]);
    expect(nodes(result.tree, 'disclosureDetails')).toEqual([]);
    if (source.startsWith('> ') || source.startsWith('1. ')) {
      expect(result.tree).toEqual(fromMarkdown(source));
    } else {
      expect(nodes(result.tree, 'text').map(n => n.value).join('')).toContain('<details>');
    }
  });
  for (const prefix of ['<details>bad', '<details><summary>unfinished']) for (const suffix of ['', '</details>']) {
    it(`keeps a complete child of malformed parent visible without promotion ${prefix} ${suffix}`, () => {
      const child = envelope('**child**', '[title][d]');
      const source = prefix + child + suffix + '\n\n[d]: /late "native"';
      const result = interpretMarkdown(source);
      expect(result.ranges).toEqual([]);
      expect(nodes(result.tree, 'disclosureDetails')).toHaveLength(0);
      expect(result.tree.children[0]!.type).toBe('paragraph');
      expect(nodes(result.tree, 'link')).toMatchObject([{ url: '/late', title: 'native' }]);
      expect(nodes(result.tree, 'strong')).toHaveLength(1);
      expect(nodes(result.tree, 'text').map(n => n.value).join('')).toContain('<details>');
    });
  }
});

describe('global references and summary phrasing', () => {
  for (const definition of ['[docs]: <details>', '[docs]: https://example.com "<details></details></summary>"', '[docs]: https://example.com\n  "first\n<details>\n</summary>\nlast"']) {
    it.each(['before', 'body', 'after'])(`uses native decoded definition ${definition} in %s`, where => {
      const source = (where === 'before' ? definition + '\n\n' : '') + envelope('[body][docs]\n\n' + (where === 'body' ? definition + '\n\n' : ''), '[summary][docs]') + (where === 'after' ? '\n' + definition : '');
      const result = interpretMarkdown(source);
      const expected = nodes(fromMarkdown(definition), 'definition')[0]!;
      expect(nodes(result.tree, 'link').map(({ url, title }) => ({ url, title }))).toEqual(Array(2).fill({ url: expected.url, title: expected.title }));
    });
  }
  it('uses source-order first wins and keeps definition node identity', () => {
    const result = interpretMarkdown('[d]: /first "one"\n\n' + envelope('[b][d]\n\n[d]: /second\n', '[s][d]') + '\n[d]: /third');
    expect(nodes(result.tree, 'link')).toMatchObject([{ url: '/first', title: 'one' }, { url: '/first', title: 'one' }]);
    expect(nodes(result.tree, 'definition')).toHaveLength(3);
    expect(result.definitions).toHaveLength(1);
    expect(result.definitions[0]).toBe(nodes(result.tree, 'definition')[0]);
    expect(markdownReferenceContext(result)).toBe(result);
    expect(markdownDetailsRanges(result)).toBe(result.ranges);
  });
  it('parses summaries as phrasing, never definitions or block structure', () => {
    const source = envelope('[outside][fake]\n\n[real]: /real\n\n', '# heading\n\n[fake]: /fake\n\n**bold** [link][real]');
    const result = interpretMarkdown(source);
    const summary = nodes(result.tree, 'disclosureSummary')[0]!;
    expect(summary).toBeDefined();
    expect(nodes(summary, 'definition')).toEqual([]);
    expect(nodes(summary, 'heading')).toEqual([]);
    expect(nodes(summary, 'paragraph')).toEqual([]);
    expect(nodes(summary, 'strong')).toHaveLength(1);
    expect(nodes(summary, 'link')).toMatchObject([{ url: '/real' }]);
    expect(result.definitions.map(n => n.identifier)).toEqual(['real']);
    expect(nodes(result.tree, 'text').map(n => n.value).join('')).toContain('[outside][fake]');
  });
  it('resolves late GFM footnotes across the disclosure boundary', () => {
    const result = interpretMarkdown(envelope('Note[^x]') + '\n\n[^x]: foot', gfmOptions);
    expect(nodes(result.tree, 'footnoteReference')).toMatchObject([{ identifier: 'x' }]);
    expect(nodes(result.tree, 'footnoteDefinition')).toMatchObject([{ identifier: 'x' }]);
  });
});

describe('positions, source partitions and bounded work counters', () => {
  it.each(['\n', '\r', '\r\n'])('uses original UTF-16 offsets and line coordinates with BOM and %j', newline => {
    const source = `\ufeff<details class="first${newline}> second" onclick="x"><summary title='x${newline}>y'>T 😺</summary>**B**</details>`;
    const result = interpretMarkdown(source);
    expect(result.ranges).toEqual([{ start: 1, end: source.length }]);
    const strong = nodes(result.tree, 'strong')[0]!;
    expect(source.slice(strong.position!.start.offset, strong.position!.end.offset)).toBe('**B**');
    expect(strong.position!.start.line).toBe(3);
    expect(strong.position!.start.column).toBe(source.slice(source.lastIndexOf(newline) + newline.length).indexOf('**B**') + 1);
    expect(result.tree.position!.end.offset).toBe(source.length);
    expect(nodes(result.tree, 'disclosureDetails')[0]!.data.hProperties).toEqual({});
  });
  it('keeps the ordinary BOM outside child offsets', () => {
    const source = '\ufeff# BOM\n\ntext';
    const result = interpretMarkdown(source);
    expect(withoutPositions(result.tree)).toEqual(withoutPositions(fromMarkdown(source)));
    expect(result.tree.children[0]!.position!.start.offset).toBe(1);
    expect(result.tree.position!.end.offset).toBe(source.length);
  });
  for (const count of [20, 40, 80, 160]) it(`partitions ${count} adjacent envelopes without resubmitting source`, () => {
    const unit = 'x\n' + envelope('<!--x-->\nx ') + '\n';
    const source = unit.repeat(count);
    const result = interpretMarkdown(source);
    expect(result.ranges).toEqual(Array.from({ length: count }, (_, i) => ({ start: i * unit.length + 2, end: (i + 1) * unit.length - 1 })));
    const boundaries = [0, ...result.ranges.flatMap(range => [range.start, range.end]), source.length];
    expect(boundaries.slice(1).map((end, i) => source.slice(boundaries[i], end)).join('')).toBe(source);
    expect(nodes(result.tree, 'disclosureDetails')).toHaveLength(count);
    expect(result.stats.indexed).toBe(source.length);
    expect(result.stats.submittedUnits).toBeLessThanOrEqual(source.length);
    expect(result.stats.submissions).toBeLessThanOrEqual(3 * count + 1);
    expect(result.stats.horizonBytes + result.stats.documentBytes).toBeLessThan(4 * source.length);
    const stack: Nodes[] = [result.tree];
    while (stack.length) {
      const node = stack.pop()!;
      if (node.position) {
        expect(node.position.start.offset).toBeGreaterThanOrEqual(0);
        expect(node.position.end.offset).toBeLessThanOrEqual(source.length);
        expect(node.position.end.offset).toBeGreaterThanOrEqual(node.position.start.offset!);
      }
      if ('children' in node) stack.push(...node.children);
    }
  });
  it.each([20, 40, 80])('bounds unmatched backtick work at %i runs', count => {
    const body = Array.from({ length: count }, (_, i) => 'x ' + '`'.repeat(i + 1) + ' tail\n').join('') + '<' + 'a'.repeat(10000) + '\n';
    const source = envelope(body);
    const { stats } = interpretMarkdown(source);
    expect(stats.horizonBytes + stats.documentBytes + stats.dispatch).toBeLessThan(6 * source.length);
  });
  it.each([20, 80, 160])('keeps malformed depth %i visible without losing late references', count => {
    const source = '<details>bad'.repeat(count) + envelope('[x][d]') + '\n\n[d]: /last';
    const result = interpretMarkdown(source);
    expect(nodes(result.tree, 'disclosureDetails')).toHaveLength(0);
    expect(nodes(result.tree, 'link')).toMatchObject([{ url: '/last' }]);
    expect(result.stats.submittedUnits).toBeLessThanOrEqual(source.length);
  });
  it.each([20, 40, 80, 160])('bounds GFM table recognition at %i rows', count => {
    const source = envelope('| h |\n| - |\n' + '| </details> |\n'.repeat(count) + '\nAfter\n');
    const result = interpretMarkdown(source, gfmOptions);
    expect(nodes(result.tree, 'table')[0]!.children).toHaveLength(count + 1);
    expect(result.stats.documentBytes + result.stats.horizonBytes + result.stats.dispatch).toBeLessThan(8 * source.length);
  });
  it('accepts a 150k source without a semantic size cap', () => {
    const source = envelope('x'.repeat(150000 - envelope('').length));
    expect(interpretMarkdown(source).ranges).toEqual([{ start: 0, end: 150000 }]);
  });
});

describe('remark parser integration', () => {
  it('supports an already selected canonical tree without reparsing child text', () => {
    const tree: Root = interpretMarkdown(envelope('**B**')).tree;
    expect(unified().use(remarkParse).use(remarkDetails, { tree }).parse('not the source')).toBe(tree);
  });
  it.each(['', ' open', ' OPEN="false"', " open='0'", ' open=no'])('keeps only boolean open for attributes %j', attribute => {
    const source = `<details${attribute} id="bad" style="color:red" onclick="evil()"><summary class="bad">T</summary>B</details>`;
    const tree = interpretMarkdown(source).tree;
    expect(nodes(tree, 'disclosureDetails')[0]!.data).toEqual({ hName: 'details', hProperties: attribute ? { open: true } : {} });
    expect(nodes(tree, 'disclosureSummary')[0]!.data).toEqual({ hName: 'summary', hProperties: {} });
  });
});

describe('candidate admission and restrictive recovery regressions', () => {
  it.each([
    'prefix ' + envelope('B') + ' suffix',
    '*prefix ' + envelope('**B**') + ' suffix*',
    '[prefix ' + envelope('**B**') + ' suffix](/target)',
    '<details><summary>Unclosed</summary>prefix ' + envelope('**B**'),
  ])('never promotes an inline opening to root authority: %j', source => {
    const result = interpretMarkdown(source, gfmOptions);
    expect(result.ranges).toEqual([]);
    expect(result.tree.children.map(n => n.type)).toEqual(['paragraph']);
    expect(nodes(result.tree, 'text').map(n => n.value).join('')).toContain('<details><summary>T</summary>');
    if (source.includes('**B**')) expect(nodes(result.tree, 'strong')).toHaveLength(1);
    if (source.startsWith('*')) expect(nodes(result.tree, 'emphasis')).toHaveLength(1);
    if (source.startsWith('[')) expect(nodes(result.tree, 'link')).toHaveLength(1);
  });
  it.each([20, 40, 80, 160])('keeps %i prose-inline envelopes literal with bounded work', count => {
    const source = ('x ' + envelope('<!--x-->\nx ') + '\n').repeat(count);
    const result = interpretMarkdown(source);
    expect(result.ranges).toEqual([]);
    expect(nodes(result.tree, 'disclosureDetails')).toHaveLength(0);
    expect(result.stats.submittedUnits).toBe(source.length);
    expect(result.stats.indexed + result.stats.validatedUnits + result.stats.frameSteps + result.stats.dispatch + result.stats.horizonBytes + result.stats.documentBytes + result.stats.submittedUnits).toBeLessThan(12 * source.length);
  });
  it.each(['prefix\n', 'prefix\n\n', 'prefix\r\n', 'prefix\r'])('admits root flow after %j', prefix => {
    const source = prefix + envelope('**B**');
    expect(interpretMarkdown(source).ranges).toEqual([{ start: prefix.length, end: source.length }]);
  });
  it('keeps indented code outside root admission', () => {
    const result = interpretMarkdown('prefix\n\n    ' + envelope('B'));
    expect(result.ranges).toEqual([]);
    expect(nodes(result.tree, 'code')).toHaveLength(1);
  });
  it('admits same-line adjacent completed root envelopes', () => {
    const first = envelope('**A**');
    const second = envelope('*B*');
    expect(interpretMarkdown(first + second).ranges).toEqual([
      { start: 0, end: first.length }, { start: first.length, end: first.length + second.length },
    ]);
  });
  it('still invalidates an outer frame for inline nested tags', () => {
    expect(interpretMarkdown(envelope('prefix ' + envelope('B') + ' suffix')).ranges).toEqual([]);
  });

  it.each([
    '<details><summary>T</summary>`<details broken` **B**</details>',
    '`<details broken`\n\n<details><summary>T</summary>B</details>',
    '<details broken\n\n<details><summary>T</summary>B</details>',
  ])('does not let a candidate hide later native-admitted tags: %j', source => {
    const start = source.lastIndexOf('<details><summary>');
    const result = interpretMarkdown(source);
    expect(result.ranges).toEqual([{ start, end: source.length }]);
    if (source.includes('**B**')) expect(nodes(result.tree, 'strong')).toHaveLength(1);
  });
  it('does not promote from provisional summary phrasing', () => {
    const source = '<details><summary>unfinished\n\n> <details><summary>T</summary>B</details>';
    const result = interpretMarkdown(source);
    expect(result.ranges).toEqual([]);
    expect(nodes(result.tree, 'disclosureDetails')).toHaveLength(0);
    expect(nodes(result.tree, 'text').map(n => n.value).join('')).toContain('B</details>');
  });
  it('promotes a native-root child only from an unclosed body', () => {
    const prefix = '<details><summary>Unclosed</summary>\n**Visible prefix**\n';
    const source = prefix + envelope('**child**');
    const result = interpretMarkdown(source);
    expect(result.ranges).toEqual([{ start: prefix.length, end: source.length }]);
    expect(nodes(result.tree, 'strong')).toHaveLength(2);
  });
  it('suppresses a completed invalid ancestor but accepts a following independent wrapper', () => {
    const prefix = '<details>bad' + envelope('**nested**') + '</details>\n\n';
    const result = interpretMarkdown(prefix + envelope('**independent**'));
    expect(result.ranges).toEqual([{ start: prefix.length, end: prefix.length + envelope('**independent**').length }]);
    expect(nodes(result.tree, 'strong')).toHaveLength(2);
    expect(result.tree.children[0]!.type).toBe('paragraph');
  });
  it.each([100, 1000, 4000])('bounds lazy validation with %i overlapping long candidates', count => {
    const source = '`' + '<details broken '.repeat(count) + '`\n\n' + envelope('B');
    const result = interpretMarkdown(source);
    expect(result.ranges).toHaveLength(1);
    expect(result.stats.validatedUnits).toBeLessThanOrEqual(source.length);
    expect(result.stats.validatedUnits).toBe(envelope('').length - 1);
  });
});

describe('disjoint validation and recovery provenance', () => {
  it.each([100, 1000, 4000])('bounds admitted candidate validation at %i starts', count => {
    const prefix = '<details broken '.repeat(count) + '\n\n';
    const source = prefix + envelope('**B**');
    const result = interpretMarkdown(source);
    expect(result.ranges).toEqual([{ start: prefix.length, end: source.length }]);
    expect(nodes(result.tree, 'strong')).toHaveLength(1);
    expect(result.stats.validatedUnits).toBeLessThanOrEqual(source.length);
    expect(result.stats.indexed + result.stats.validatedUnits + result.stats.frameSteps + result.stats.dispatch + result.stats.horizonBytes + result.stats.documentBytes + result.stats.submittedUnits).toBeLessThan(12 * source.length);
  });
  it.each([
    '<details><summary>unfinished' + envelope('**child**') + '</summary>body',
    '<details>missing' + envelope('**child**') + '<summary>late</summary>body',
  ])('does not retroactively promote a child after reaching body phase: %j', source => {
    const result = interpretMarkdown(source);
    expect(result.ranges).toEqual([]);
    expect(nodes(result.tree, 'strong')).toHaveLength(1);
    expect(result.tree.children.map(n => n.type)).toEqual(['paragraph']);
  });
  it.each(['> ', '- ', '    '])('keeps native container ownership under unclosed body %j', container => {
    const source = '<details><summary>Unclosed</summary>\n\n' + container + envelope('**child**');
    const result = interpretMarkdown(source);
    expect(result.ranges).toEqual([]);
    expect(nodes(result.tree, 'disclosureDetails')).toHaveLength(0);
  });
  it('keeps rejected syntax and Markdown visible in a native paragraph', () => {
    const source = 'before <details broken! **visible** after';
    const result = interpretMarkdown(source);
    expect(result.tree.children.map(n => n.type)).toEqual(['paragraph']);
    expect(nodes(result.tree, 'strong')).toHaveLength(1);
    expect(nodes(result.tree, 'text').map(n => n.value).join('')).toBe('before <details broken! visible after');
  });
  it('keeps quoted attribute ownership despite indexed tag-shaped starts', () => {
    const source = '<details title="<details><summary>attribute</summary></details>"><summary>T</summary>**B**</details>';
    const result = interpretMarkdown(source);
    expect(result.ranges).toEqual([{ start: 0, end: source.length }]);
    expect(nodes(result.tree, 'disclosureDetails')).toHaveLength(1);
    expect(result.stats.validatedUnits).toBe(source.length - 6);
  });
});

describe('candidate grammar stays within native ownership', () => {
  function rendererTree(source: string): Root {
    return unified().use(remarkParse).use(remarkGfm).use(remarkDetails).parse(source);
  }

  for (const newline of ['\n', '\r\n', '\r']) for (const blank of ['', ' \t ']) {
    it(`does not borrow a quote across a native blank line ${JSON.stringify([newline, blank])}`, () => {
      const prefix = `<details title="unterminated${newline}${blank}${newline}`;
      const child = envelope('B');
      const source = `${prefix}${child}${newline}${newline}" >`;
      const result = interpretMarkdown(source, gfmOptions);
      expect(result.ranges).toEqual([{ start: prefix.length, end: prefix.length + child.length }]);
      expect(nodes(result.tree, 'text').map(n => n.value).join('')).toContain('<details title="unterminated');
      expect(rendererTree(source)).toEqual(result.tree);
      expect(nodes(rendererTree(source), 'disclosureDetails')).toHaveLength(1);
    });
  }

  it.each(['<details broken!', '<summary broken!', '</details broken!', '</summary broken!'])('keeps invalid body candidates literal: %s', body => {
    const source = envelope(body + ' **visible**');
    const result = interpretMarkdown(source, gfmOptions);
    expect(result.ranges).toEqual([{ start: 0, end: source.length }]);
    expect(nodes(result.tree, 'text').map(n => n.value).join('')).toContain(body);
    expect(nodes(result.tree, 'strong')).toHaveLength(1);
    expect(rendererTree(source)).toEqual(result.tree);
  });

  it.each(['\n', '\r\n', '\r'])('retains valid multiline attributes: %j', newline => {
    const source = `<details title="first${newline}<details>quoted</details>"${newline}open><summary title='first${newline}last'>T</summary>B</details>`;
    const result = interpretMarkdown(source, gfmOptions);
    expect(result.ranges).toEqual([{ start: 0, end: source.length }]);
    expect(nodes(result.tree, 'disclosureDetails')[0]!.data!.hProperties).toEqual({ open: true });
    expect(rendererTree(source)).toEqual(result.tree);
  });

  it('bounds malformed quotes in summary phrasing as well as root flow', () => {
    const source = envelope('B', '<details title="unterminated\n\nT') + '\n\n" >';
    const end = source.indexOf('</details>') + '</details>'.length;
    const result = interpretMarkdown(source, gfmOptions);
    expect(result.ranges).toEqual([{ start: 0, end }]);
    expect(nodes(result.tree, 'text').map(n => n.value).join('')).toContain('<details title="unterminated');
    expect(rendererTree(source)).toEqual(result.tree);
  });

  it.each(['<script>\n', '```html\n', '    '])('does not claim native-owned candidates after a broken quote: %s', lead => {
    const close = lead.startsWith('<') ? '\n</script>' : lead.startsWith('`') ? '\n```' : '';
    const prefix = '<details title="unterminated\n\n';
    const source = prefix + lead + envelope('hidden') + close + '\n\n" >';
    const result = interpretMarkdown(source, gfmOptions);
    expect(result.ranges).toEqual([]);
    expect(rendererTree(source)).toEqual(result.tree);
    expect(nodes(result.tree, lead.startsWith('<') ? 'html' : 'code')).toHaveLength(1);
  });

  it.each(['<details open\n\n>', '<details title="ok"\n\n>', '<details title=\n\nvalue>', '</details\n\n>'])('does not complete tag grammar across a blank line: %j', prefix => {
    const source = prefix + '\n\n' + envelope('B');
    const result = interpretMarkdown(source, gfmOptions);
    expect(result.ranges).toEqual([{ start: prefix.length + 2, end: source.length }]);
    expect(rendererTree(source)).toEqual(result.tree);
    expect(nodes(result.tree, 'text').map(n => n.value).join('')).toContain(prefix.slice(0, prefix.indexOf('\n')));
  });

  it('leaves an inline horizon boundary candidate for native root admission', () => {
    const prefix = 'paragraph\n';
    const source = prefix + '<details title="first\nlast"><summary>T</summary>B</details>';
    const result = interpretMarkdown(source, gfmOptions);
    expect(result.ranges).toEqual([{ start: prefix.length, end: source.length }]);
    expect(rendererTree(source)).toEqual(result.tree);
  });

  it('does not let inline attribute grammar jump a native fenced block', () => {
    const source = 'paragraph <details title="unfinished\n```html\n' + envelope('hidden') + '\n```\n" >';
    const result = interpretMarkdown(source, gfmOptions);
    expect(result.ranges).toEqual([]);
    expect(nodes(result.tree, 'code')).toHaveLength(1);
    expect(rendererTree(source)).toEqual(result.tree);
  });

  it('still suppresses valid nested structural tags in the renderer AST', () => {
    const source = envelope(envelope('nested'));
    const result = interpretMarkdown(source, gfmOptions);
    expect(result.ranges).toEqual([]);
    expect(nodes(rendererTree(source), 'disclosureDetails')).toHaveLength(0);
    expect(rendererTree(source)).toEqual(result.tree);
    expect(nodes(result.tree, 'text').map(n => n.value).join('')).toBe(source);
  });

  it.each([100, 400])('bounds repeated broken quote validation at %i candidates', count => {
    const source = ('<details title="unterminated\n\n' + envelope('B') + '\n\n" >\n\n').repeat(count);
    const result = interpretMarkdown(source, gfmOptions);
    expect(result.ranges).toHaveLength(count);
    expect(result.stats.validatedUnits).toBeLessThan(source.length);
    expect(result.stats.submittedUnits).toBeLessThanOrEqual(source.length);
  });
});
