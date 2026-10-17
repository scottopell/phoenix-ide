import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, render } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { MarkdownRenderer } from './MarkdownRenderer';
import { prepareMarkdown } from './prepareMarkdown';
import { StreamingBlocks } from './StreamingMessage';
import { prepareStreamingBlocks } from './prepareStreamingBlocks';

describe('canonical streaming chunk selection', () => {
  it('keeps complete root containers and disclosures together', () => {
    const source = '<details><summary>Title</summary>\n\n```js\n1\n```\n\nBody\n</details>\n\n- list\n\n  ```js\n  2\n  ```';
    const blocks = prepareStreamingBlocks(source);
    expect(blocks).toHaveLength(1);
    expect(blocks[0]?.tree?.children.map(node => node.type)).toEqual(['disclosureDetails', 'list']);
  });

  it('keeps completed chunk revisions stable when only the tail grows', () => {
    const source = 'First paragraph\n\n```js\n1\n```\n\nTail';
    const before = prepareStreamingBlocks(source);
    const after = prepareStreamingBlocks(source + ' grows');
    expect(before).toHaveLength(3);
    expect(after[0]?.revision).toBe(before[0]?.revision);
    expect(after[1]).toEqual(before[1]);
    expect(after[2]?.revision).not.toBe(before[2]?.revision);
  });

  it('invalidates an earlier reference chunk when its global destination changes', () => {
    const source = '[link][ref]\n\n```js\n1\n```\n\n[ref]: /a';
    const before = prepareStreamingBlocks(source);
    const after = prepareStreamingBlocks(source + 'b');
    expect(after[0]?.revision).not.toBe(before[0]?.revision);
    expect(after[0]?.revision).toContain('/ab');
  });

  it('preserves open code-fence rendering', () => {
    const blocks = prepareStreamingBlocks('Prose\n\n```js\nlet x =');
    expect(blocks[1]).toEqual({ type: 'code', lang: 'js', content: 'let x =', meta: null, complete: false });
  });
});

const renders = vi.hoisted(() => ({ code: vi.fn(), mermaid: vi.fn() }));
vi.mock('../utils/syntaxHighlighter', () => ({
  oneDark: {}, oneLight: {},
  SyntaxHighlighter: ({ children }: { children: string }) => {
    renders.code(children);
    return <code data-highlighted>{children}</code>;
  },
}));
vi.mock('./MermaidDiagram', () => ({
  MermaidDiagram: ({ code }: { code: string }) => {
    renders.mermaid(code);
    return <code data-mermaid>{code}</code>;
  },
}));

beforeEach(() => { vi.useFakeTimers(); renders.code.mockClear(); renders.mermaid.mockClear(); });
afterEach(() => { cleanup(); vi.useRealTimers(); });
const frame = () => act(() => { vi.advanceTimersByTime(32); });
const stream = (text: string) => <MemoryRouter><StreamingBlocks text={text} /></MemoryRouter>;

it.each([
  ['```js extra\nx\n```', true],
  ['````js extra\nx\n```', false],
  ['````js extra\nx\n`````  \t', true],
  ['~~~js extra\nx\n~~~~', true],
  ['~~~js extra\nx\n```', false],
  ['```js extra\nx\n``` info', false],
  ['```js extra\nx\n    ```', false],
  ['  ~~~js extra\n  x\n   ~~~', true],
  ['```js extra', false],
  ['    indented\n    code', true],
  ['    ```\n    x', true],
  ['~~~js extra\r\nx\r\n~~~', true],
])('uses canonical values and narrow fence closure for %j', (source, complete) => {
  const node = prepareMarkdown(source, 'conversation').children[0];
  expect(node?.type).toBe('code');
  if (node?.type !== 'code') throw new Error('expected native code');
  expect(prepareStreamingBlocks(source)[0]).toEqual({
    type: 'code', content: node.value, lang: node.lang, meta: node.meta, complete,
  });
});

it('removes the legacy same-delimiter markdown nesting heuristic only from streaming rendering', () => {
  const source = '```markdown\nouter\n```js\ninner\n```\nafter\n```';
  const nodes = prepareMarkdown(source, 'conversation').children;
  const blocks = prepareStreamingBlocks(source);
  expect(nodes.map(node => node.type)).toEqual(['code', 'paragraph', 'code']);
  expect(blocks.filter(block => block.type === 'code').map(block => block.content))
    .toEqual(nodes.filter(node => node.type === 'code').map(node => node.value));
  const live = render(stream(source));
  frame();
  const final = render(<MarkdownRenderer mode="conversation">{source}</MarkdownRenderer>);
  expect([...live.container.querySelectorAll('code')].map(node => node.textContent?.replace(/\n$/, '')))
    .toEqual([...final.container.querySelectorAll('code')].map(node => node.textContent?.replace(/\n$/, '')));
  expect(live.container.querySelector('p')?.textContent).toBe(final.container.querySelector('p')?.textContent);
});

function footnotes(container: HTMLElement) {
  return {
    refs: [...container.querySelectorAll('[data-footnote-ref]')].map(node => [node.id, node.getAttribute('href'), node.textContent]),
    backs: [...container.querySelectorAll('[data-footnote-backref]')].map(node => [node.getAttribute('href'), node.getAttribute('aria-label')]),
    footer: container.querySelector('[data-footnotes]')?.textContent,
  };
}

it('couples cross-code late footnotes with global order, duplicate refs, first definition wins and one footer', () => {
  const prefix = 'Independent\n\n```js\nprior\n```\n\nFirst[^b]\n\n```js\nstable\n```\n\nThen[^a] again[^b]\n\n';
  const view = render(stream(prefix));
  frame();
  expect(view.container.querySelector('[data-footnotes]')).toBeNull();
  for (const definitions of [
    '[^a]: Alpha\n\n[^b]: Beta',
    '[^a]: Alpha\n\n[^b]: Beta updated\n\n[^b]: duplicate ignored',
  ]) {
    const source = prefix + definitions;
    view.rerender(stream(source)); frame();
    const final = render(<MarkdownRenderer mode="conversation">{source}</MarkdownRenderer>);
    expect(footnotes(view.container)).toEqual(footnotes(final.container));
    expect(view.container.querySelectorAll('[data-footnotes]')).toHaveLength(1);
    expect(footnotes(view.container).refs.map(ref => ref[2])).toEqual(['1', '2', '1']);
    const ids = [...view.container.querySelectorAll('[id]')].map(node => node.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(view.container.querySelector('[data-footnotes]')?.textContent).not.toContain('duplicate ignored');
    final.unmount();
  }
});

it('keeps code and Mermaid memoized within the coupled suffix while footnote prose updates', () => {
  const prefix = 'First[^n]\n\n```js\nstable\n```\n\n```mermaid\ngraph TD; A-->B\n```\n\n[^n]: ';
  const view = render(stream(prefix + 'note'));
  frame();
  expect(renders.code).toHaveBeenCalledTimes(1);
  expect(renders.mermaid).toHaveBeenCalledTimes(1);
  view.rerender(stream(prefix + 'note grows')); frame();
  expect(view.container.querySelectorAll('pre')).toHaveLength(2);
  expect(view.container.querySelector('pre pre')).toBeNull();
  expect(view.container.querySelector('[data-highlighted]')?.parentElement?.tagName).toBe('PRE');
  expect(view.container.querySelector('[data-mermaid]')?.parentElement?.tagName).toBe('PRE');
  expect(renders.code).toHaveBeenCalledTimes(1);
  expect(renders.mermaid).toHaveBeenCalledTimes(1);
});

it('keeps a native open fence live inside the coupled suffix then highlights once closed', () => {
  const prefix = 'First[^n]\n\n[^n]: Note\n\n```js\n';
  const view = render(stream(prefix + 'let x ='));
  frame();
  expect(view.container.querySelector('.streaming-code')?.textContent).toBe('let x =');
  expect(view.container.querySelector('pre pre')).toBeNull();
  expect(renders.code).not.toHaveBeenCalled();
  view.rerender(stream(prefix + 'let x = 1\n```')); frame();
  expect(view.container.querySelector('.streaming-code')).toBeNull();
  expect(renders.code).toHaveBeenCalledExactlyOnceWith('let x = 1');
});

it('starts coupling at a definition before references nested in a disclosure', () => {
  const source = 'Independent\n\n[^n]: Note\n\n```js\nx\n```\n\n<details><summary>Title[^n]</summary>\n\nBody[^n]\n</details>';
  const blocks = prepareStreamingBlocks(source);
  expect(blocks).toHaveLength(2);
  expect(blocks[0]?.tree?.children.map(node => node.type)).toEqual(['paragraph']);
  expect(blocks[1]?.tree?.children.map(node => node.type)).toEqual(['footnoteDefinition', 'code', 'disclosureDetails']);
  const live = render(stream(source)); frame();
  const final = render(<MarkdownRenderer mode="conversation">{source}</MarkdownRenderer>);
  expect(footnotes(live.container)).toEqual(footnotes(final.container));
  expect(live.container.querySelectorAll('[data-footnotes]')).toHaveLength(1);
});

it('does not rehighlight completed standalone code while the prose tail grows', () => {
  const prefix = '```js\nstable\n```\n\n';
  const view = render(stream(prefix + 'Tail')); frame();
  view.rerender(stream(prefix + 'Tail grows')); frame();
  expect(renders.code).toHaveBeenCalledExactlyOnceWith('stable');
});

it.each(['text', 'mermaid', ''])('owns one shell for standalone and coupled completed %j fences', (lang) => {
  const value = lang === 'mermaid' ? 'graph TD; A-->B' : 'plain text';
  for (const prefix of ['', 'Ref[^n]\n\n[^n]: Note\n\n']) {
    const open = `${prefix}\`\`\`${lang}\n${value}`;
    const view = render(stream(open)); frame();
    expect(view.container.querySelectorAll('pre')).toHaveLength(1);
    expect(view.container.querySelector('pre > code.streaming-code')?.textContent).toBe(value);
    view.rerender(stream(`${open}\n\`\`\``)); frame();
    expect(view.container.querySelectorAll('pre')).toHaveLength(1);
    expect(view.container.querySelector('pre pre')).toBeNull();
    expect(view.container.querySelector('.streaming-code')).toBeNull();
    expect(view.container.querySelector('pre > code')?.textContent).toBe(value + (lang ? '' : '\n'));
    view.unmount();
  }
});

const nestedFences = [
  { name: 'blockquote', prefix: '> ', before: '', after: '' },
  { name: 'list', prefix: '  ', before: '- item\n\n', after: '' },
  { name: 'footnote', prefix: '    ', before: 'Ref[^n]\n\n[^n]: Note\n\n', after: '' },
  { name: 'disclosure quote', prefix: '> ', before: '<details><summary>Title</summary>\n\n', after: '\n\n</details>' },
  { name: 'disclosure list', prefix: '  ', before: '<details><summary>Title</summary>\n\n- item\n\n', after: '\n\n</details>' },
];

for (const { name, prefix, before, after } of nestedFences) {
  it.each(['js', 'mermaid', ''])(`defers unfinished %s in ${name}, closes natively and memoizes the result`, (lang) => {
    const value = lang === 'mermaid' ? 'graph TD; A-->B' : 'let x = 1';
    const code = `${before}${prefix}\`\`\`${lang}\n${prefix}${value}`;
    const view = render(stream(code + after)); frame();
    if (after) expect(view.container.querySelector('details')).not.toBeNull();
    expect(view.container.querySelector('.streaming-code')?.textContent).toBe(value + (name === 'disclosure list' ? '\n' : ''));
    expect(renders.code).not.toHaveBeenCalled();
    expect(renders.mermaid).not.toHaveBeenCalled();
    view.rerender(stream(code + ' ' + after)); frame();
    expect(renders.code).not.toHaveBeenCalled();
    expect(renders.mermaid).not.toHaveBeenCalled();
    const closed = `${code}\n${prefix}\`\`\`${after}`;
    view.rerender(stream(closed)); frame();
    expect(view.container.querySelector('.streaming-code')).toBeNull();
    expect(view.container.querySelectorAll('pre')).toHaveLength(1);
    expect(view.container.querySelector('pre pre')).toBeNull();
    const expensive = lang === 'mermaid' ? renders.mermaid : renders.code;
    if (lang) {
      expect(expensive).toHaveBeenCalledExactlyOnceWith(value);
      expect(view.container.querySelector('pre > code')).not.toBeNull();
    } else {
      expect(expensive).not.toHaveBeenCalled();
      expect(view.container.querySelector('pre > code')?.textContent).toBe(value + '\n');
      expect(view.container.querySelector('[data-highlighted]')).toBeNull();
    }
    view.rerender(stream(closed + '\n\nTrailing prose')); frame();
    view.rerender(stream(closed + '\n\nTrailing prose grows')); frame();
    expect(expensive).toHaveBeenCalledTimes(lang ? 1 : 0);
    expect(view.container.querySelector('pre pre')).toBeNull();
  });
}

it('distinguishes closed footnote code from a different genuinely open fence', () => {
  const source = 'Ref[^n]\n\n[^n]: Note\n\n    ```js\n    closed\n    ```\n\n> ```mermaid\n> graph TD; A-->B';
  const view = render(stream(source)); frame();
  expect(renders.code).toHaveBeenCalledExactlyOnceWith('closed');
  expect(renders.mermaid).not.toHaveBeenCalled();
  expect(view.container.querySelector('.streaming-code')?.textContent).toBe('graph TD; A-->B');
});

it('does not promote an incomplete disclosure containing real unfinished quote code', () => {
  const source = '<details><summary>Title</summary>\n\n> ```js\n> open';
  const view = render(stream(source)); frame();
  expect(view.container.querySelector('details')).toBeNull();
  expect(view.container.querySelector('.streaming-code')?.textContent).toBe('open');
  expect(renders.code).not.toHaveBeenCalled();
});

it('keeps streaming annotations out of canonical and standard trees', () => {
  const source = '> ```js\n> x\n> ```';
  const canonical = prepareMarkdown(source, 'conversation');
  const standard = prepareMarkdown(source, 'standard');
  expect(canonical).toEqual(standard);
  const snapshot = JSON.stringify(canonical);
  expect(prepareStreamingBlocks(source)[0]?.revision).toContain('data-streaming-complete');
  expect(JSON.stringify(canonical)).toBe(snapshot);
  expect(prepareMarkdown(source, 'conversation')).toEqual(standard);
});
