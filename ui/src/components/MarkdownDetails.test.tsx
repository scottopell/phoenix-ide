import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { toString } from 'mdast-util-to-string';
import { gfmOptions, interpretMarkdown, markdownDetailsRanges, remarkDetails } from './remarkDetails';

afterEach(cleanup);

function markdown(source: string) {
  return render(<ReactMarkdown remarkPlugins={[remarkGfm, remarkDetails]}>{source}</ReactMarkdown>);
}

describe('ReactMarkdown with the actual disclosure parser', () => {
  it.each([
    ['prefix <details><summary>T</summary>B</details> suffix', 'prefix <details><summary>T</summary>B</details> suffix'],
    ['*prefix <details><summary>T</summary>**B**</details> suffix*', 'prefix <details><summary>T</summary>B</details> suffix'],
    ['[prefix <details><summary>T</summary>**B**</details> suffix](/target)', 'prefix <details><summary>T</summary>B</details> suffix'],
    ['<details><summary>Unclosed</summary>prefix <details><summary>T</summary>**B**</details>', '<details><summary>Unclosed</summary>prefix <details><summary>T</summary>B</details>'],
  ])('keeps inline wrappers literal in the same paragraph: %s', (source, text) => {
    const { container } = markdown(source);
    expect(container.querySelector('details')).toBeNull();
    expect(container.querySelectorAll('p')).toHaveLength(1);
    expect(container.querySelector('p')?.textContent).toBe(text);
    if (source.includes('**B**')) expect(container.querySelector('strong')?.textContent).toBe('B');
    if (source.startsWith('*')) expect(container.querySelector('em')?.textContent).toBe(text);
    if (source.startsWith('[')) expect(container.querySelector('a')).toHaveAttribute('href', '/target');
  });
  it.each(['prefix\n', 'prefix\n\n', ''])('renders admitted root and adjacent envelopes after %j', prefix => {
    const { container } = markdown(prefix + '<details><summary>T</summary>**A**</details><details><summary>U</summary>*B*</details>');
    expect(container.querySelectorAll('details')).toHaveLength(2);
    expect(container.querySelector('details strong')?.textContent).toBe('A');
    expect(container.querySelector('details em')?.textContent).toBe('B');
  });
  it('does not admit an indented code envelope', () => {
    const { container } = markdown('prefix\n\n    <details><summary>T</summary>B</details>');
    expect(container.querySelector('details')).toBeNull();
    expect(container.querySelector('pre code')?.textContent).toContain('<details>');
  });
  it('rejects inline nesting in a completed root envelope', () => {
    const source = '<details><summary>Outer</summary>prefix <details><summary>T</summary>B</details> suffix</details>';
    const { container } = markdown(source);
    expect(container.querySelector('details')).toBeNull();
    expect(container.querySelector('p')?.textContent).toBe(source);
  });

  it('renders native details/summary through hName with Markdown and find-ready text', () => {
    const source = 'Before\n\n<details><summary>**Find title** [docs][d]</summary>\n\n## Find body\n\n- [x] done\n\n~~old~~\n\n</details>\n\n[d]: https://example.com/docs\n\nAfter';
    const interpretation = interpretMarkdown(source, gfmOptions);
    const { container } = render(<ReactMarkdown remarkPlugins={[remarkGfm, remarkDetails]}>{source}</ReactMarkdown>);
    const details = container.querySelector('details');
    expect(details).not.toBeNull();
    expect(details!.hasAttribute('open')).toBe(false);
    expect(details!.firstElementChild?.tagName).toBe('SUMMARY');
    expect(details!.querySelector('summary strong')?.textContent).toBe('Find title');
    expect(details!.querySelector('h2')?.textContent).toBe('Find body');
    expect(details!.querySelector('del')?.textContent).toBe('old');
    expect(details!.querySelector('input')).toHaveProperty('checked', true);
    expect(screen.getByRole('link', { name: 'docs' })).toHaveAttribute('href', 'https://example.com/docs');
    const searchable = toString(interpretation.tree);
    for (const text of ['Before', 'Find title', 'Find body', 'done', 'old', 'After']) {
      expect(searchable).toContain(text);
      expect(container.textContent).toContain(text);
    }
    expect(searchable).not.toContain('<details>');
    expect(searchable).not.toContain('[d]:');
  });

  it.each(['open', 'OPEN="false"', "open='0'", 'open=no'])('preserves boolean presence %s and drops other source properties', attribute => {
    const source = `<details ${attribute} id="untrusted" style="color:red" onclick="evil()" data-extra="bad"><summary id="summary-id" onmouseover="evil()">Safe</summary>Body</details>`;
    const { container } = render(<ReactMarkdown remarkPlugins={[remarkDetails]}>{source}</ReactMarkdown>);
    const details = container.querySelector('details')!;
    expect(details.open).toBe(true);
    expect(details.getAttributeNames()).toEqual(['open']);
    expect(details.querySelector('summary')!.getAttributeNames()).toEqual([]);
  });

  it('retains default URL sanitation for links, images and global references', () => {
    const source = '<details><summary>[bad](javascript:alert%281%29) [ref][unsafe]</summary>![bad image](javascript:alert%281%29) [safe](https://example.com)</details>\n\n[unsafe]: javascript:alert%281%29';
    const { container } = render(<ReactMarkdown remarkPlugins={[remarkDetails]}>{source}</ReactMarkdown>);
    const links = [...container.querySelectorAll('a')];
    expect(links.map(link => link.getAttribute('href'))).toEqual(['', '', 'https://example.com']);
    expect(container.querySelector('img')?.getAttribute('src')).toBe('');
    expect(container.innerHTML).not.toContain('javascript:');
  });

  it('does not enable raw HTML parsing inside or outside an envelope', () => {
    const source = '<details><summary>T <img src=x onerror=evil()></summary>\n\n<script>\nwindow.evil()\n</script>\n\nBody\n</details>\n\n<iframe src="https://example.com"></iframe>';
    const { container } = render(<ReactMarkdown remarkPlugins={[remarkDetails]}>{source}</ReactMarkdown>);
    expect(container.querySelectorAll('details')).toHaveLength(1);
    expect(container.querySelector('script, iframe, img')).toBeNull();
    expect(container.textContent).toContain('<script>');
    expect(container.textContent).toContain('<img src=x onerror=evil()>');
    expect(container.textContent).toContain('Body');
  });

  it.each([
    ['link destination', '[literal](<details>)', 'a', 'href', 'details'],
    ['image destination', '![literal](<details>)', 'img', 'src', 'details'],
    ['closing-details destination', '[literal](</details>)', 'a', 'href', '/details'],
    ['closing-summary destination', '[literal](</summary>)', 'a', 'href', '/summary'],
    ['summary image destination', '![literal](<summary>)', 'img', 'src', 'summary'],
    ['angle destination with spaces', '[literal](<details open>)', 'a', 'href', 'details%20open'],
    ['link title', '[literal](https://example.com "<details></details></summary>")', 'a', 'title', '<details></details></summary>'],
    ['image title', '![literal](https://example.com "<details></details></summary>")', 'img', 'title', '<details></details></summary>'],
    ['single-quoted title', "[literal](https://example.com '<details></summary>')", 'a', 'title', '<details></summary>'],
    ['parenthesized title', '[literal](https://example.com (<details></summary>))', 'a', 'title', '<details></summary>'],
    ['multiline title', '[literal](https://example.com "first\nnext <details></summary>")', 'a', 'title', 'first\nnext <details></summary>'],
    ['escaped destination', String.raw`[literal](https://example.com/\<details\>)`, 'a', 'href', 'https://example.com/%3Cdetails%3E'],
  ])('preserves %s tokens in the summary and body', (_name, literal, selector, attribute, value) => {
    const source = `<details><summary>Title ${literal} end</summary>\n\n${literal}\n\nAfter literal\n</details>`;
    const { container } = markdown(source);
    expect(container.querySelectorAll('details')).toHaveLength(1);
    expect(container.querySelector('summary')).toHaveTextContent('end');
    expect(container.querySelector('details')).toHaveTextContent('After literal');
    expect(container.querySelectorAll(`details ${selector}`)).toHaveLength(2);
    for (const node of container.querySelectorAll(`details ${selector}`)) expect(node).toHaveAttribute(attribute, value);
    expect(markdownDetailsRanges(source)).toEqual([{ start: 0, end: source.length }]);
  });

  it('renders a selected canonical subtree with references resolved outside it', () => {
    const interpretation = interpretMarkdown('<details><summary>[title][d]</summary>[body][d]</details>\n\n[d]: /target');
    const selected = { ...interpretation.tree, children: interpretation.tree.children.slice(0, 1) };
    const { container } = render(<ReactMarkdown remarkPlugins={[[remarkDetails, { tree: selected }]]}>{'not parsed'}</ReactMarkdown>);
    expect(container.querySelectorAll('details')).toHaveLength(1);
    expect([...container.querySelectorAll('a')].map(link => link.getAttribute('href'))).toEqual(['/target', '/target']);
    expect(container.textContent).not.toContain('not parsed');
  });
});
