import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/react';
import {
  parseSearchOutput,
  parseKeywordSearchOutput,
  BrowserConsoleLogsView,
  SearchResultsView,
  KeywordSearchView,
} from './MessageComponents';

describe('parseSearchOutput', () => {
  it('parses path:lineno: content lines', () => {
    const text = [
      'src/foo.rs:12: fn hello() {}',
      'src/foo.rs:34:     println!("hi");',
      'src/bar.rs:5: use foo;',
    ].join('\n');
    const { hits, notes, noMatches } = parseSearchOutput(text);
    expect(noMatches).toBe(false);
    expect(notes).toEqual([]);
    expect(hits).toHaveLength(3);
    expect(hits[0]).toEqual({ path: 'src/foo.rs', lineNumber: 12, content: 'fn hello() {}' });
    expect(hits[1]?.content).toBe('    println!("hi");');
  });

  it('recognizes the no-matches sentinel', () => {
    const { noMatches, hits } = parseSearchOutput('No matches found.');
    expect(noMatches).toBe(true);
    expect(hits).toEqual([]);
  });

  it('extracts bracketed notes (cap / truncation)', () => {
    const text = [
      'a.rs:1: hit',
      '',
      '[Results limited to 50 matches. Use a more specific pattern or path to narrow results.]',
      '[Walk truncated at 100000 entries. Narrow `path` or `include`.]',
    ].join('\n');
    const { hits, notes } = parseSearchOutput(text);
    expect(hits).toHaveLength(1);
    expect(notes).toHaveLength(2);
    expect(notes[0]).toMatch(/Results limited to 50/);
    expect(notes[1]).toMatch(/Walk truncated/);
  });

  it('handles paths with colons via backtracking', () => {
    // Windows-style drive letters or `class::method` style names shouldn't break parsing.
    const { hits } = parseSearchOutput('weird:name.rs:99: body');
    expect(hits).toHaveLength(1);
    expect(hits[0]?.lineNumber).toBe(99);
    expect(hits[0]?.path).toBe('weird:name.rs');
  });

  it('keeps empty match content as empty string', () => {
    const { hits } = parseSearchOutput('a.rs:1: ');
    expect(hits[0]?.content).toBe('');
  });
});

describe('parseKeywordSearchOutput', () => {
  it('parses path: explanation pairs (LLM-filtered shape)', () => {
    const text = [
      '/abs/path/to/foo.rs: implements the foo state machine, primary hit',
      '/abs/path/to/bar.rs: helper utilities referenced from foo',
    ].join('\n');
    const parsed = parseKeywordSearchOutput(text);
    expect(parsed.empty).toBe(false);
    expect(parsed.rawFallback).toBe(false);
    expect(parsed.hits).toHaveLength(2);
    expect(parsed.hits[0]).toEqual({
      path: '/abs/path/to/foo.rs',
      explanation: 'implements the foo state machine, primary hit',
    });
  });

  it('recognizes "No relevant files found" as empty', () => {
    const parsed = parseKeywordSearchOutput('No relevant files found');
    expect(parsed.empty).toBe(true);
    expect(parsed.rawFallback).toBe(false);
  });

  it('recognizes the no-matches-for-terms sentinel as empty', () => {
    const parsed = parseKeywordSearchOutput('No matches found for the given search terms.');
    expect(parsed.empty).toBe(true);
  });

  it('detects raw ripgrep fallback output', () => {
    const text = [
      'src/foo.rs:12:matching line',
      'src/foo.rs-11-context above',
      'src/foo.rs-13-context below',
      '--',
      'src/bar.rs:5:another match',
      'src/bar.rs-4-context',
    ].join('\n');
    const parsed = parseKeywordSearchOutput(text);
    expect(parsed.rawFallback).toBe(true);
    expect(parsed.empty).toBe(false);
    expect(parsed.hits).toEqual([]);
  });

  it('extracts bracketed notes without treating them as hits', () => {
    const text = [
      '/abs/path/to/foo.rs: primary hit',
      '',
      '[note: 2 lower-priority term(s) were dropped to fit the result budget — narrow your terms for full coverage]',
      '[results truncated: search scope is large — use more specific search terms for complete results]',
    ].join('\n');
    const parsed = parseKeywordSearchOutput(text);
    expect(parsed.rawFallback).toBe(false);
    expect(parsed.empty).toBe(false);
    expect(parsed.hits).toHaveLength(1);
    expect(parsed.hits[0]!.path).toBe('/abs/path/to/foo.rs');
    expect(parsed.notes).toHaveLength(2);
    expect(parsed.notes[0]).toMatch(/lower-priority term/);
    expect(parsed.notes[1]).toMatch(/results truncated/);
  });

  it('treats the skipped-broad no-match variant as empty and keeps the note', () => {
    const text = [
      'No matches found for the given search terms.',
      '',
      '[note: 1 search term(s) skipped as too broad — narrow them to include their matches.]',
    ].join('\n');
    const parsed = parseKeywordSearchOutput(text);
    expect(parsed.empty).toBe(true);
    expect(parsed.rawFallback).toBe(false);
    expect(parsed.notes).toHaveLength(1);
    expect(parsed.notes[0]).toMatch(/skipped as too broad/);
  });
});

describe('BrowserConsoleLogsView', () => {
  it('renders per-entry level tags and tallies', () => {
    const json = JSON.stringify([
      { level: 'error', text: 'TypeError: oops' },
      { level: 'warning', text: 'deprecated thing' },
      { level: 'log', text: 'hello' },
      { level: 'log', text: 'world' },
    ]);
    const { container } = render(<BrowserConsoleLogsView rawText={json} />);
    expect(screen.getByText('4 entries')).toBeTruthy();
    expect(screen.getByText('1 error')).toBeTruthy();
    expect(screen.getByText('1 warning')).toBeTruthy();
    expect(screen.getByText('2 log')).toBeTruthy();
    expect(screen.getByText('TypeError: oops')).toBeTruthy();
    // Entry-level styling is applied
    expect(container.querySelector('.console-level-error')).toBeTruthy();
    expect(container.querySelector('.console-level-warning')).toBeTruthy();
  });

  it('renders the file-pointer escape-hatch message verbatim', () => {
    const { getByText } = render(
      <BrowserConsoleLogsView rawText="Logs written to /tmp/phoenix-console-logs-abc.json (use `cat` to view)" />
    );
    expect(getByText(/Logs written to/)).toBeTruthy();
  });

  it('shows empty state for `[]`', () => {
    render(<BrowserConsoleLogsView rawText="[]" />);
    expect(screen.getByText('(no console entries)')).toBeTruthy();
  });

  it('falls back to preformatted text on unparseable JSON', () => {
    const { container } = render(<BrowserConsoleLogsView rawText="not json at all" />);
    const pre = container.querySelector('pre.console-logs-fallback');
    expect(pre?.textContent).toBe('not json at all');
  });
});

describe('SearchResultsView', () => {
  const text = [
    'src/foo.rs:12: fn hello() {}',
    'src/foo.rs:34: println!("hi");',
    'src/bar.rs:5: use foo;',
    '[Results limited to 50 matches.]',
  ].join('\n');

  it('groups hits by file with counts', () => {
    render(<SearchResultsView rawText={text} onOpenFile={undefined} />);
    expect(screen.getByText(/3 matches in 2 files/)).toBeTruthy();
    expect(screen.getByText('2 hits')).toBeTruthy();
    expect(screen.getByText('1 hit')).toBeTruthy();
    expect(screen.getByText('Results limited to 50 matches.')).toBeTruthy();
  });

  it('invokes onOpenFile with the right line on hit click', () => {
    const onOpenFile = vi.fn();
    const { container } = render(<SearchResultsView rawText={text} onOpenFile={onOpenFile} />);
    const lines = container.querySelectorAll('.search-result-line-clickable');
    expect(lines.length).toBe(3);
    fireEvent.click(lines[1]!);
    expect(onOpenFile).toHaveBeenCalledWith('src/foo.rs', new Set([34]), 34);
  });

  it('renders no-matches sentinel as friendly empty state', () => {
    render(<SearchResultsView rawText="No matches found." onOpenFile={undefined} />);
    expect(screen.getByText('No matches found.')).toBeTruthy();
  });

  it('omits the clickable affordance when onOpenFile is undefined', () => {
    const { container } = render(<SearchResultsView rawText={text} onOpenFile={undefined} />);
    expect(container.querySelector('.search-result-line-clickable')).toBeNull();
    expect(container.querySelector('button.search-results-filepath')).toBeNull();
  });
});

describe('KeywordSearchView', () => {
  const llm = [
    '/abs/path/to/foo.rs: implements the foo state machine',
    '/abs/path/to/bar.rs: helper utilities for foo',
  ].join('\n');

  it('renders LLM-filtered hits with explanations', () => {
    const onOpenFile = vi.fn();
    const { container } = render(<KeywordSearchView rawText={llm} onOpenFile={onOpenFile} />);
    expect(screen.getByText('2 relevant files')).toBeTruthy();
    const hits = container.querySelectorAll('.keyword-search-hit');
    expect(hits.length).toBe(2);
    expect(within(hits[0] as HTMLElement).getByText('/abs/path/to/foo.rs')).toBeTruthy();
    expect(
      within(hits[0] as HTMLElement).getByText('implements the foo state machine')
    ).toBeTruthy();
    fireEvent.click(within(hits[1] as HTMLElement).getByRole('button'));
    expect(onOpenFile).toHaveBeenCalledWith('/abs/path/to/bar.rs', new Set(), 0);
  });

  it('falls back to raw text with a notice when LLM output is missing', () => {
    const raw = [
      'src/foo.rs:1:hit one',
      'src/foo.rs-2-ctx',
      '--',
      'src/bar.rs:3:hit two',
      'src/bar.rs-4-ctx',
    ].join('\n');
    const { container } = render(<KeywordSearchView rawText={raw} onOpenFile={undefined} />);
    expect(screen.getByText(/Raw ripgrep results/)).toBeTruthy();
    expect(container.querySelector('pre.keyword-search-raw-text')?.textContent).toBe(raw);
  });

  it('renders the empty/none-found state', () => {
    render(<KeywordSearchView rawText="No relevant files found" onOpenFile={undefined} />);
    expect(screen.getByText('No relevant files found.')).toBeTruthy();
  });
});


describe('search result CSS contracts', () => {
  const css = readFileSync('src/index.css', 'utf8');

  function ruleFor(selector: string): string {
    const escapedSelector = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const match = css.match(new RegExp(`${escapedSelector}\\s*\\{(?<body>[^}]+)\\}`));
    const body = match?.groups?.['body'];
    expect(body, `${selector} rule exists`).toBeTruthy();
    return body!;
  }

  it('keeps scrollable search result children from shrinking vertically', () => {
    expect(ruleFor('.search-results-list')).toMatch(/overflow-y:\s*auto;/);
    expect(ruleFor('.search-results-list')).toMatch(/min-height:\s*0;/);
    expect(ruleFor('.keyword-search-list')).toMatch(/overflow-y:\s*auto;/);
    expect(ruleFor('.keyword-search-list')).toMatch(/min-height:\s*0;/);

    expect(ruleFor('.search-results-file')).toMatch(/flex:\s*0\s+0\s+auto;/);
    expect(ruleFor('.search-result-line')).toMatch(/flex:\s*0\s+0\s+auto;/);
    expect(ruleFor('.keyword-search-hit')).toMatch(/flex:\s*0\s+0\s+auto;/);
  });
});

