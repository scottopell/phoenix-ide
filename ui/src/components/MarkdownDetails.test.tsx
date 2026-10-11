import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { unified } from 'unified';
import remarkParse from 'remark-parse';
import { MarkdownDetails, MarkdownSummary } from './MarkdownDetails';
import { markdownDetailsRanges, markdownReferenceContext, remarkDetails } from './remarkDetails';
import { AgentMessage } from './MessageComponents';
import { StreamingMessageView } from './StreamingMessage';
import type { Message } from '../api';

const components = { details: MarkdownDetails, summary: MarkdownSummary };
function markdown(source: string) {
  return render(<ReactMarkdown remarkPlugins={[remarkGfm, remarkDetails]} components={components}>{source}</ReactMarkdown>);
}

const basic = '<details>\n<summary>Read **more**</summary>\n\nBody with *emphasis*.\n\n- first\n- second\n\n</details>\n\nAfter disclosure.';

describe('safe basic Markdown disclosures', () => {
  it('renders a collapsed native disclosure with Markdown body and summary', () => {
    const { container } = markdown(basic);
    const details = container.querySelector('details')!;
    expect(details.open).toBe(false);
    expect(details.firstElementChild?.tagName).toBe('SUMMARY');
    expect(details.querySelector('summary strong')).toHaveTextContent('more');
    expect(details.querySelector('em')).toHaveTextContent('emphasis');
    expect(details.querySelectorAll('li')).toHaveLength(2);
    expect(screen.getByText('After disclosure.').closest('details')).toBeNull();
    const summary = details.querySelector('summary')!;
    summary.focus();
    expect(summary).toHaveFocus();
    // Native keyboard/click behavior belongs to the browser, not a custom handler.
    expect(summary).not.toHaveAttribute('role');
    expect(summary).not.toHaveAttribute('tabindex');
  });

  it.each(['open', 'open=""', "OPEN='open'", 'open="false"'])('accepts HTML boolean %s and strips every other attribute', (open) => {
    const { container } = markdown(`<details ${open} id="bad" style="color:red" onclick="alert(1)" ontoggle="alert(2)" name="group"><summary class="bad" onfocus="alert(3)">Title</summary>\nBody\n</details>`);
    const details = container.querySelector('details')!;
    expect(details.open).toBe(true);
    expect(details.getAttributeNames().sort()).toEqual(['class', 'open']);
    expect(details.querySelector('summary')!.getAttributeNames()).toEqual(['class']);
  });

  it('handles multiline summaries, adjacent disclosures, CRLF, and text without blank lines', () => {
    const { container } = markdown('<details>\n<summary>First\nsummary</summary>\n**One**\n</details>\n<details open>\n<summary>Second</summary>\nTwo\n</details>\nTrailing'.replaceAll('\n', '\r\n'));
    expect(container.querySelectorAll('details')).toHaveLength(2);
    expect(container.querySelectorAll('details')[0]).toHaveTextContent('First summary');
    expect(container.querySelector('strong')).toHaveTextContent('One');
    expect(screen.getByText('Trailing').closest('details')).toBeNull();
  });

  it.each([
    '<details>\n<summary>Unfinished</summary>\nBody\n\n# Still visible',
    '<details>\n<summary>Missing end\nBody\n\n# Still visible',
    '<details>\nBody without summary\n</details>\n\n# Still visible',
    '<details open="unfinished>\n<summary>Bad attr</summary>\nBody\n\n# Still visible',
  ])('does not hide subsequent Markdown for malformed input: %s', (source) => {
    const { container } = markdown(source);
    expect(container.querySelector('details')).toBeNull();
    expect(screen.getByRole('heading', { name: 'Still visible' })).toBeInTheDocument();
    expect(container).toHaveTextContent('Body');
  });

  it('ignores disclosure syntax in fenced and inline code', () => {
    const { container } = markdown('```html\n' + basic + '\n```\n\n`<details><summary>literal</summary></details>`');
    expect(container.querySelector('details')).toBeNull();
    expect(container.querySelectorAll('code')).toHaveLength(2);
    expect(markdownDetailsRanges('```html\n' + basic + '\n```')).toEqual([]);
  });

  it('groups only completed disclosure ranges for streaming', () => {
    const source = 'Before\n\n' + basic + '\n\n```mermaid\nStill growing';
    expect(markdownDetailsRanges(source)).toEqual([{ start: source.indexOf('<details>'), end: source.indexOf('</details>') + '</details>'.length }]);
    expect(markdownDetailsRanges('<details><summary>Growing</summary>\n\n```mermaid\nStill growing')).toEqual([]);
  });

  it('keeps fenced closing-tag examples inside the disclosure', () => {
    const { container } = markdown('<details><summary>Code</summary>\n\n```html\n</details>\n<script>alert(1)</script>\n```\n\nAfter code\n</details>\nOutside');
    const details = container.querySelector('details')!;
    expect(details.querySelector('code')).toHaveTextContent('</details>');
    expect(details).toHaveTextContent('After code');
    expect(screen.getByText('Outside').closest('details')).toBeNull();
    expect(container.querySelector('script')).toBeNull();
  });

  it('does not enable arbitrary HTML, event attributes, or unsafe Markdown URLs', () => {
    const { container } = markdown('<details data-value="open" onclick="bad()"><summary><img src=x onerror=bad()>Safe</summary>\n\n<script>bad()</script>\n\n<iframe src="https://evil.example"></iframe>\n\n[unsafe](javascript:alert%281%29) [data](data:text/html,test) [safe](https://example.com)\n\n</details>\n\n<div onclick="bad()">Outside HTML</div>');
    expect(container.querySelector('details')!.open).toBe(false);
    expect(container.querySelector('script, iframe, img, [onclick], [onerror]')).toBeNull();
    expect(screen.getByText('unsafe')).toHaveAttribute('href', '');
    expect(screen.getByText('data')).toHaveAttribute('href', '');
    expect(screen.getByRole('link', { name: 'safe' })).toHaveAttribute('href', 'https://example.com');
    expect(container.querySelector('div[onclick]')).toBeNull();
  });

  it('does not mistake quoted open values for the boolean attribute', () => {
    const { container } = markdown('<details title="open > anything" data-open="yes"><summary title="x > y">Safe</summary>\nBody\n</details>');
    expect(container.querySelector('details')!.open).toBe(false);
  });

  it('falls back visibly for nested disclosures instead of swallowing following prose', () => {
    const { container } = markdown('<details><summary>Outer</summary>\n<details><summary>Inner</summary>\nBody\n</details>\n</details>\n\nAfter nested');
    expect(container.querySelector('details')).toBeNull();
    expect(screen.getByText('After nested')).toBeInTheDocument();
    expect(container).toHaveTextContent('Body');
  });

  it('preserves document-level reference links and Markdown images', () => {
    const { container } = markdown('<details open><summary>References</summary>\n\n[guide][docs] ![safe image](https://example.com/image.png)\n\n</details>\n\n[docs]: https://example.com/docs');
    expect(screen.getByRole('link', { name: 'guide' })).toHaveAttribute('href', 'https://example.com/docs');
    expect(container.querySelector('details img')).toHaveAttribute('src', 'https://example.com/image.png');
  });

  it('supports same-line bodies and gives an empty summary a usable label', () => {
    const { container } = markdown('<details><summary></summary>**body**</details>\n\nAfter');
    expect(container.querySelector('summary')).toHaveTextContent('Details');
    expect(container.querySelector('details strong')).toHaveTextContent('body');
    expect(screen.getByText('After').closest('details')).toBeNull();
  });

  it.each([
    '```html\n<details><summary>literal</summary></details>\n```',
    '~~~~html\n<details><summary>literal</summary></details>\n~~~\n~~~~',
    '`<details><summary>literal</summary></details>`',
    '``literal ` <details><summary>literal</summary></details>``',
    '```literal <details><summary>literal</summary></details>```',
    '`multiline\nliteral <details><summary>literal</summary></details>\ncode`',
    '> ```html\n> <details><summary>literal</summary></details>\n> ```',
    '> ~~~html\n> <details><summary>literal</summary></details>\n> ~~~',
    '- ~~~html\n  <details><summary>literal</summary></details>\n  ~~~',
  ])('does not use code examples as a candidate boundary: %s', (code) => {
    const source = `<details><summary>Examples</summary>\n\n${code}\n\nAfter code\n</details>`;
    const { container } = markdown(source);
    expect(container.querySelectorAll('details')).toHaveLength(1);
    expect(container.querySelector('details')).toHaveTextContent('After code');
    expect(container.querySelector('details code')).toHaveTextContent('<details>');
    expect(markdownDetailsRanges(source)).toEqual([{ start: 0, end: source.length }]);
  });

  it('bounds all reparsing for adjacent disclosures, including swallowed HTML tails', () => {
    const source = Array.from({ length: 80 }, (_, index) =>
      `<details><summary>Item ${index}</summary>\nBody ${index}\n</details>`).join('\n');
    const processor = unified().use(remarkParse).use(remarkDetails);
    const parse = vi.spyOn(processor, 'parse');
    const tree = processor.runSync(processor.parse(source), { value: source });
    expect(tree.children.filter((node) => node.data?.hName === 'details')).toHaveLength(80);
    const parsedCharacters = parse.mock.calls.reduce((total, [input]) => total + String(input).length, 0);
    // Work budget, not wall-clock timing: repeated suffix parses exceed it by orders of magnitude.
    expect(parsedCharacters).toBeLessThan(source.length * 5);
    const final = tree.children.at(-1)!;
    expect(final.position?.end.offset).toBe(source.length);
  });

  it('bounds reparsing for 80 disclosures sharing a document with 80 definitions', () => {
    const disclosures = Array.from({ length: 80 }, (_, index) =>
      `<details><summary>[Item ${index}][docs ${index}]</summary>\n\n[guide][docs ${index}]\n\n</details>`).join('\n');
    const definitions = Array.from({ length: 80 }, (_, index) =>
      `[docs ${index}]: https://example.com/docs/${index}`).join('\n');
    const source = `${disclosures}\n\n${definitions}`;
    const processor = unified().use(remarkParse).use(remarkDetails);
    const parse = vi.spyOn(processor, 'parse');
    const tree = processor.runSync(processor.parse(source), { value: source });
    const details = tree.children.filter((node) => node.data?.hName === 'details');
    expect(details).toHaveLength(80);
    for (const node of details) {
      if (!('children' in node)) throw new Error('Missing disclosure body');
      const paragraph = node.children[1]!;
      if (!('children' in paragraph)) throw new Error('Missing reference paragraph');
      expect(paragraph.children[0]?.type).toBe('linkReference');
      expect(paragraph.position?.start.offset).toBe(source.indexOf('[guide]', node.position!.start.offset));
      expect(node.children.some((child) => child.type === 'definition')).toBe(false);
    }
    const parsedCharacters = parse.mock.calls.reduce((total, [input]) => total + String(input).length, 0);
    expect(parsedCharacters).toBeLessThan(source.length * 6);
  });

  it('supplies only referenced definitions, normalizing full, collapsed, and shortcut labels', () => {
    const definitions = '[Docs Name]: https://example.com/docs\n[other]: https://example.com/other\n[docs name]: https://example.com/ignored';
    const withReferences = markdownReferenceContext(definitions);
    for (const content of ['[guide][DOCS   NAME]', '[docs name][]', '[DoCs\nName]']) {
      expect(withReferences(content)).toBe(`${content}\n\n[Docs Name]: https://example.com/docs`);
    }
    expect(withReferences('No references')).toBe('No references');
    expect(withReferences('[guide][missing]')).toBe('[guide][missing]');
    const { container } = markdown(`<details open><summary>[DOCS NAME][]</summary>\n\n[guide][docs name] [DoCs Name]\n\n</details>\n\n${definitions}`);
    expect(container.querySelectorAll('details a')).toHaveLength(3);
    for (const link of container.querySelectorAll('details a')) {
      expect(link).toHaveAttribute('href', 'https://example.com/docs');
    }
  });

  it('relocates a large body using original line, column, and offset positions', () => {
    const prefix = 'Before\r\n\r\n<details><summary>Positions</summary>\r\n\r\n';
    const body = Array.from({ length: 160 }, (_, index) => `Paragraph **${index}**.`).join('\r\n\r\n');
    const source = `${prefix}${body}\r\n\r\n</details>`;
    const processor = unified().use(remarkParse).use(remarkDetails);
    const tree = processor.runSync(processor.parse(source), { value: source });
    const details = tree.children.find((node) => node.data?.hName === 'details')!;
    expect('children' in details).toBe(true);
    if (!('children' in details)) throw new Error('Missing disclosure body');
    const last = details.children.at(-1)!;
    expect(last.position?.start).toEqual({ offset: source.lastIndexOf('Paragraph'), line: 323, column: 1 });
    expect(last.position?.end.offset).toBe(prefix.length + body.length);
  });

  it('retains original Markdown body offsets for highlights', () => {
    const source = 'Before\n\n' + basic;
    const processor = unified().use(remarkParse).use(remarkDetails);
    const tree = processor.runSync(processor.parse(source), { value: source });
    const details = tree.children.find((node) => node.data?.hName === 'details');
    expect(details && 'children' in details && details.children[1]?.position?.start.offset).toBe(source.indexOf('Body'));
  });
});

const message = (text: string): Message => ({
  message_id: 'details-message', sequence_id: 1, conversation_id: 'details-conversation',
  message_type: 'agent', content: [{ type: 'text', text }], display_data: null,
  created_at: '2026-01-01T00:00:00Z',
});

describe('owning conversation Markdown pipelines', () => {
  it('renders finalized disclosures without GFM and preserves local/browser link actions', () => {
    const onOpenFile = vi.fn();
    const { container } = render(<MemoryRouter><AgentMessage message={message('<details><summary>References</summary>\n\n[local](ui/src/App.tsx) [web](https://example.com)\n\n</details>')} toolResults={new Map()} onOpenFile={onOpenFile} filePathRootDir="/repo" forceExpandedText /></MemoryRouter>);
    expect(container.querySelector('details')).not.toBeNull();
    fireEvent.click(screen.getByText('local'));
    expect(onOpenFile).toHaveBeenCalled();
    expect(screen.getByRole('link', { name: 'web', hidden: true })).toHaveAttribute('rel', 'noopener noreferrer');
  });

  it('preserves the finalized URL and HTML security boundary', () => {
    const { container } = render(<MemoryRouter><AgentMessage message={message('<details open onclick="bad()"><summary onfocus="bad()">Security</summary>\n\n[unsafe](javascript:alert%281%29)\n\n<script>bad()</script>\n\n</details>')} toolResults={new Map()} forceExpandedText /></MemoryRouter>);
    expect(container.querySelector('details')!.open).toBe(true);
    expect(container.querySelector('script, [onclick], [onfocus]')).toBeNull();
    expect(screen.getByText('unsafe')).not.toHaveAttribute('href', expect.stringContaining('javascript:'));
  });

  it('renders GFM in a finalized disclosure', () => {
    const { container } = render(<MemoryRouter><AgentMessage message={message('<details><summary>Table</summary>\n\n| Name | Value |\n| --- | --- |\n| A | B |\n\n</details>')} toolResults={new Map()} forceExpandedText /></MemoryRouter>);
    expect(container.querySelector('details table')).not.toBeNull();
  });

  it('shares streaming reference definitions inside and outside disclosures without changing code chunks', async () => {
    const source = '[before][docs]\n\n<details open><summary>[summary][docs]</summary>\n\n[inside][docs]\n\n</details>\n\n```text\n[guide][docs]\n```\n\n[after][docs]\n\n[docs]: https://example.com/docs';
    const buffer = { text: source, lastSequence: 1, startedAt: Date.now(), requestId: 'shared-definitions' };
    const { container } = render(<MemoryRouter><StreamingMessageView buffer={buffer} /></MemoryRouter>);
    await waitFor(() => expect(screen.getByRole('link', { name: 'inside' })).toHaveAttribute('href', 'https://example.com/docs'));
    for (const name of ['before', 'summary', 'after']) {
      expect(screen.getByRole('link', { name })).toHaveAttribute('href', 'https://example.com/docs');
    }
    expect(screen.getByRole('link', { name: 'inside' }).closest('details')).not.toBeNull();
    expect(screen.getByRole('link', { name: 'after' }).closest('details')).toBeNull();
    expect(container.querySelector('code')).toHaveTextContent('[guide][docs]');
    expect(container.querySelector('code')).not.toHaveTextContent('[docs]:');
  });

  it('updates memoized streaming disclosure and prose chunks when a definition arrives', async () => {
    const source = '<details open><summary>References</summary>\n\n[inside][docs]\n\n</details>\n\n[outside][docs]';
    const buffer = { text: source, lastSequence: 1, startedAt: Date.now(), requestId: 'late-definition' };
    const { container, rerender } = render(<MemoryRouter><StreamingMessageView buffer={buffer} /></MemoryRouter>);
    await waitFor(() => expect(container.querySelector('details')).toHaveTextContent('[inside][docs]'));
    expect(screen.queryByRole('link', { name: 'inside' })).toBeNull();
    rerender(<MemoryRouter><StreamingMessageView buffer={{ ...buffer, text: `${source}\n\n[docs]: https://example.com/docs` }} /></MemoryRouter>);
    await waitFor(() => expect(screen.getByRole('link', { name: 'inside' })).toHaveAttribute('href', 'https://example.com/docs'));
    expect(screen.getByRole('link', { name: 'outside' })).toHaveAttribute('href', 'https://example.com/docs');
  });

  it('keeps streaming fenced code and links inside the disclosure; incomplete input remains visible', async () => {
    const onOpenFile = vi.fn();
    const source = '<details><summary>Stream</summary>\n\n```ts\nconst safe = 1;\n```\n\n[local](ui/src/App.tsx) [web](https://example.com)\n\n</details>\n\nOutside';
    const buffer = { text: source, lastSequence: 1, startedAt: Date.now(), requestId: 'details-request' };
    const { container, rerender } = render(<MemoryRouter><StreamingMessageView buffer={buffer} rootDir="/repo" onOpenFile={onOpenFile} /></MemoryRouter>);
    await waitFor(() => expect(container.querySelector('details')).not.toBeNull());
    expect(container.querySelector('details')).toHaveTextContent('const safe = 1;');
    fireEvent.click(screen.getByText('local'));
    expect(onOpenFile).toHaveBeenCalled();
    expect(screen.getByRole('link', { name: 'web', hidden: true })).toHaveAttribute('rel', 'noopener noreferrer');
    expect(screen.getByText('Outside').closest('details')).toBeNull();
    rerender(<MemoryRouter><StreamingMessageView buffer={{ ...buffer, text: '<details><summary>Stream</summary>\n\nVisible while unclosed' }} /></MemoryRouter>);
    await waitFor(() => expect(container.querySelector('details')).toBeNull());
    expect(screen.getByText('Visible while unclosed')).toBeInTheDocument();
  });
});
