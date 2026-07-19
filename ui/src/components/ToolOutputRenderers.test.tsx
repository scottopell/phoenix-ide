import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/react';
import {
  parseSearchOutput,
  parseKeywordSearchOutput,
  BrowserConsoleLogsView,
  SearchResultsView,
  KeywordSearchView,
  __readFileResultTestables,
  ReadFileResultView,
  PatchResultView,
} from './MessageComponents';
import { buildKeywordSearchOutputProjection, buildPatchOutputProjection, buildReadFileOutputProjection } from './viewer-find';
import { buildSearchOutputProjection } from './viewer-find/searchProjections';

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
    expect(hits[0]).toMatchObject({ path: 'src/foo.rs', lineNumber: 12, content: 'fn hello() {}' });
    expect(hits[1]?.content).toBe('    println!("hi");');
  });

  it('shares builder parity with the typed search projection', () => {
    const text = [
      'src/foo.rs:12: fn hello() {}',
      'src/foo.rs:34:     println!("hi");',
      '[Results limited to 50 matches.]',
    ].join('\n');
    expect(parseSearchOutput(text)).toEqual(buildSearchOutputProjection(text));
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
    expect(notes[0]?.text).toMatch(/Results limited to 50/);
    expect(notes[1]?.text).toMatch(/Walk truncated/);
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

describe('buildKeywordSearchOutputProjection parity', () => {
  it('matches parseKeywordSearchOutput for structured hits and stable reveal metadata', () => {
    const text = [
      '/abs/path/to/foo.rs: implements the foo state machine, primary hit',
      '/abs/path/to/bar.rs: helper utilities referenced from foo',
    ].join('\n');
    const parsed = parseKeywordSearchOutput(text);
    const built = buildKeywordSearchOutputProjection(text, { toolUseId: 'tool-1' });
    expect(built.empty).toBe(parsed.empty);
    expect(built.rawFallback).toBe(parsed.rawFallback);
    expect(built.hits.map((hit) => ({ path: hit.path, explanation: hit.explanation }))).toEqual(
      parsed.hits.map((hit) => ({ path: hit.path, explanation: hit.explanation }))
    );
    expect(built.fragments.map((fragment) => fragment.fragmentId)).toEqual([
      expect.stringMatching(/^keyword-search-hit:%2Fabs%2Fpath%2Fto%2Ffoo\.rs:[a-z0-9]+:0$/),
      expect.stringMatching(/^keyword-search-hit:%2Fabs%2Fpath%2Fto%2Fbar\.rs:[a-z0-9]+:0$/),
    ]);
    expect(built.fragments.every((fragment) => fragment.revealTarget.kind === 'tool-result-keyword-search')).toBe(true);
    expect(built.fragments.every((fragment) => fragment.revealTarget.kind === 'tool-result-keyword-search'
      && fragment.revealTarget.key === 'keyword-search:tool-1')).toBe(true);
  });

  it('builds a searchable fallback fragment for raw output', () => {
    const raw = [
      'src/foo.rs:1:hit one',
      'src/foo.rs-2-ctx',
      '--',
      'src/bar.rs:3:hit two',
      'src/bar.rs-4-ctx',
    ].join('\n');
    const built = buildKeywordSearchOutputProjection(raw, { toolUseId: 'tool-2' });
    expect(built.rawFallback).toBe(true);
    expect(built.fragments).toHaveLength(2);
    expect(built.fragments[0]?.semanticText).toBe('Raw ripgrep results — LLM filter unavailable');
    expect(built.fragments[0]?.fragmentId).toBe('keyword-search-fallback-title');
    expect(built.fragments[1]?.semanticText).toBe(raw);
    expect(built.fragments[1]?.fragmentId).toBe('keyword-search-fallback-body');
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
    expect(parsed.hits[0]).toMatchObject({
      path: '/abs/path/to/foo.rs',
      explanation: 'implements the foo state machine, primary hit',
    });
    expect(parsed.hits[0]?.fragment.fragmentId)
      .toMatch(/^keyword-search-hit:%2Fabs%2Fpath%2Fto%2Ffoo\.rs:[a-z0-9]+:0$/);
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

  it('builds stable searchable fragments for coverage notes', () => {
    const text = [
      '/abs/path/to/foo.rs: primary hit',
      '[results truncated: use more specific terms]',
    ].join('\n');
    const projection = buildKeywordSearchOutputProjection(text, { toolUseId: 'keyword-notes' });
    expect(projection.notes).toHaveLength(1);
    expect(projection.notes[0]?.fragment).toMatchObject({
      semanticText: 'results truncated: use more specific terms',
      kind: 'note',
      revealTarget: {
        kind: 'tool-result-keyword-search',
        key: 'keyword-search:keyword-notes',
        toolUseId: 'keyword-notes',
      },
    });
    expect(projection.fragments).toContain(projection.notes[0]?.fragment);
  });

  it('orders structured keyword fragments as rendered: notes before hits', () => {
    const projection = buildKeywordSearchOutputProjection([
      '/abs/path/to/foo.rs: primary hit',
      '[results truncated: use more specific terms]',
    ].join('\n'), { toolUseId: 'keyword-order' });

    expect(projection.fragments.map((fragment) => fragment.kind)).toEqual(['note', 'hit']);
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

describe('buildReadFileOutputProjection parity', () => {
  it('builds typed path and line fragments from the canonical renderer text', () => {
    const text = [
      '     7\tconst alpha = 1;',
      '     8\tsecond alpha line',
    ].join('\n');
    const built = buildReadFileOutputProjection(text, { path: 'src/foo.ts', offset: 7, limit: 2 }, { toolUseId: 'read-1' });
    expect(built.fullText).toBe('src/foo.ts:7-8\n7\tconst alpha = 1;\n8\tsecond alpha line');
    expect(built.fragments.map((fragment) => fragment.fragmentId)).toEqual([
      'read-file-path',
      expect.stringMatching(/^read-file-line:[a-z0-9]+:0$/),
      expect.stringMatching(/^read-file-line:[a-z0-9]+:0$/),
    ]);
    expect(built.fragments[1]?.revealTarget).toMatchObject({
      kind: 'tool-result-read-file',
      toolUseId: 'read-1',
      lineNumber: 7,
      startLineNumber: 7,
      endLineNumber: 7,
    });
  });
});

describe('buildPatchOutputProjection parity', () => {
  const diff = '--- a/src/foo.ts\n+++ b/src/foo.ts\n@@ -1 +1 @@\n-old alpha\n+new alpha';

  it('builds one revealable fragment from the canonical display diff', () => {
    expect(buildPatchOutputProjection(diff, { toolUseId: 'patch-1' })).toEqual({
      fragments: [{
        fragmentId: 'patch-diff',
        semanticText: diff,
        display: { diff },
        revealTarget: {
          kind: 'tool-result-patch',
          toolUseId: 'patch-1',
          fragmentId: 'patch-diff',
        },
        kind: 'diff',
      }],
      fullText: diff,
    });
  });

  it('highlights the exact occurrence without changing diff layout or duplicating text', () => {
    const start = diff.lastIndexOf('alpha');
    const { container } = render(
      <PatchResultView
        diff={diff}
        toolUseId="patch-1"
        activeHighlight={{ fragmentId: 'patch-diff', start, end: start + 5 }}
      />,
    );
    expect(container.querySelector('.viewer-find-inline-match--active')?.textContent).toBe('alpha');
    expect(container.querySelector('[data-fragment-id="patch-diff"]')?.textContent).toBe(diff);
    expect(container.querySelectorAll('[data-fragment-id="patch-diff"]')).toHaveLength(1);
  });

  it('bounds large active diffs around the selected match', () => {
    const largeDiff = `${'a'.repeat(8_000)}needle${'b'.repeat(8_000)}`;
    const start = largeDiff.indexOf('needle');
    const { container } = render(
      <PatchResultView
        diff={largeDiff}
        toolUseId="patch-large"
        activeHighlight={{ fragmentId: 'patch-diff', start, end: start + 6 }}
      />,
    );

    expect(container.querySelector('[data-fragment-id="patch-diff"]')?.textContent?.length).toBeLessThan(5_100);
    expect(container.querySelector('.viewer-find-inline-match--active')).toHaveTextContent('needle');
  });
});

describe('ReadFileResultView', () => {
  const readText = [
    '     7\tconst alpha = 1;',
    '     8\tsecond alpha line',
  ].join('\n');

  it('renders the input path/window and numbered lines via the shared builder', () => {
    render(
      <ReadFileResultView
        rawText={readText}
        input={{ path: 'src/foo.ts', offset: 7, limit: 2 }}
        onOpenFile={undefined}
        toolUseId="read-1"
      />,
    );
    expect(screen.getByText('src/foo.ts:7-8')).toBeInTheDocument();
    expect(screen.getByText('7')).toBeInTheDocument();
    expect(screen.getByText('const alpha = 1;')).toBeInTheDocument();
    expect(screen.getByText('8')).toBeInTheDocument();
    expect(screen.getByText('second alpha line')).toBeInTheDocument();
  });

  it('renders and marks the path when it is the active read_file occurrence', () => {
    const projection = buildReadFileOutputProjection(readText, { path: 'src/foo.ts', offset: 7, limit: 2 }, { toolUseId: 'read-1' });
    const pathFragment = projection.fragments.find((fragment) => fragment.kind === 'path')!;
    const { container } = render(
      <ReadFileResultView
        rawText={readText}
        input={{ path: 'src/foo.ts', offset: 7, limit: 2 }}
        onOpenFile={undefined}
        toolUseId="read-1"
        showPath
        activeHighlight={{ fragmentId: pathFragment.fragmentId, start: 0, end: 'src/foo.ts'.length }}
      />,
    );
    expect(container.querySelector('[data-fragment-id="read-file-path"] .viewer-find-inline-match--active')?.textContent).toBe('src/foo.ts');
  });

  it('keeps a capped preview when the active match is the path', () => {
    const manyLines = Array.from({ length: 25 }, (_, index) => `${String(index + 1).padStart(6)}\tline ${index + 1}`).join('\n');
    const projection = buildReadFileOutputProjection(manyLines, { path: 'src/large.ts' }, { toolUseId: 'read-large' });
    const pathFragment = projection.fragments.find((fragment) => fragment.kind === 'path')!;
    render(
      <ReadFileResultView
        rawText={manyLines}
        input={{ path: 'src/large.ts' }}
        onOpenFile={undefined}
        toolUseId="read-large"
        metadata={{
          type: 'read_file', path: 'src/large.ts',
          total_line_count: 25, returned_start_line: 1, returned_end_line: 25,
          returned_line_count: 25, remaining_line_count: 0, requested_offset: 1,
          requested_limit: 25, viewer_available: false,
        }}
        showPath
        activeHighlight={{ fragmentId: pathFragment.fragmentId, start: 0, end: 'src/large.ts'.length }}
      />,
    );

    expect(screen.getByText('line 20')).toBeInTheDocument();
    expect(screen.queryByText('line 21')).toBeNull();
    expect(screen.getByRole('button', { name: 'Show all returned lines' })).toBeInTheDocument();
  });

  it('highlights the exact active occurrence without duplicating content', () => {
    const projection = buildReadFileOutputProjection(readText, { path: 'src/foo.ts', offset: 7, limit: 2 }, { toolUseId: 'read-1' });
    const targetFragment = projection.fragments.find((fragment) => fragment.kind === 'line'
      && fragment.display.lineNumber === 8);
    expect(targetFragment).toBeTruthy();
    const start = targetFragment!.semanticText.indexOf('alpha');
    const { container } = render(
      <ReadFileResultView
        rawText={readText}
        input={{ path: 'src/foo.ts', offset: 7, limit: 2 }}
        onOpenFile={undefined}
        toolUseId="read-1"
        activeHighlight={{
          fragmentId: targetFragment!.fragmentId,
          start,
          end: start + 'alpha'.length,
        }}
      />,
    );
    const activeMark = container.querySelector('.viewer-find-inline-match--active');
    expect(activeMark?.textContent).toBe('alpha');
    expect(container.querySelectorAll(`[data-fragment-id="${targetFragment!.fragmentId}"]`)).toHaveLength(1);
    expect(container.querySelector(`.search-result-line[data-fragment-id="${targetFragment!.fragmentId}"]`)?.textContent)
      .toBe('8second alpha line');
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
    render(<SearchResultsView rawText={text} onOpenFile={undefined} toolUseId="search-1" />);
    expect(screen.getByText(/3 matches in 2 files/)).toBeTruthy();
    expect(screen.getByText('2 hits')).toBeTruthy();
    expect(screen.getByText('1 hit')).toBeTruthy();
    expect(screen.getByText('Results limited to 50 matches.')).toBeTruthy();
  });

  it('invokes onOpenFile with the right line on hit click', () => {
    const onOpenFile = vi.fn();
    const { container } = render(<SearchResultsView rawText={text} onOpenFile={onOpenFile} toolUseId="search-1" />);
    const lines = container.querySelectorAll('.search-result-line-clickable');
    expect(lines.length).toBe(3);
    fireEvent.click(lines[1]!);
    expect(onOpenFile).toHaveBeenCalledWith('src/foo.rs', new Set([34]), 34);
  });

  it('renders no-matches sentinel as friendly empty state', () => {
    render(<SearchResultsView rawText="No matches found." onOpenFile={undefined} toolUseId="search-1" />);
    expect(screen.getByText('No matches found.')).toBeTruthy();
  });
  it('marks the active occurrence in the no-matches state', () => {
    const { container } = render(
      <SearchResultsView
        rawText="No matches found."
        onOpenFile={undefined}
        toolUseId="search-1"
        activeHighlight={{ fragmentId: 'search-empty', start: 3, end: 10 }}
      />,
    );
    expect(container.querySelector('.viewer-find-inline-match--active')?.textContent).toBe('matches');
  });


  it('omits the clickable affordance when onOpenFile is undefined', () => {
    const { container } = render(<SearchResultsView rawText={text} onOpenFile={undefined} toolUseId="search-1" />);
    expect(container.querySelector('.search-result-line-clickable')).toBeNull();
    expect(container.querySelector('button.search-results-filepath')).toBeNull();
  });

  it('marks exact active search hit without duplicating line content', () => {
    const projection = buildSearchOutputProjection(text, { toolUseId: 'search-1' });
    const targetFragment = projection.hits[1]?.fragment;
    expect(targetFragment).toBeTruthy();
    const { container } = render(
      <SearchResultsView
        rawText={text}
        onOpenFile={undefined}
        toolUseId="search-1"
        activeHighlight={{
          fragmentId: targetFragment!.fragmentId,
          start: targetFragment!.semanticText.indexOf('println!'),
          end: targetFragment!.semanticText.indexOf('println!') + 'println!'.length,
        }}
      />
    );
    const activeMark = container.querySelector('.viewer-find-inline-match--active');
    expect(activeMark?.textContent).toBe('println!');
    expect(screen.getAllByText(/println!/)).toHaveLength(1);
  });

  it('projects a grouped path once and excludes it from per-hit text', () => {
    const projection = buildSearchOutputProjection(text, { toolUseId: 'search-1' });
    expect(projection.groups[0]?.fragment.semanticText).toBe('src/foo.rs');
    expect(projection.fragments.filter((fragment) => fragment.semanticText.includes('src/foo.rs'))).toHaveLength(1);
    expect(projection.groups[0]?.hits.every((hit) => !hit.fragment.semanticText.includes('src/foo.rs'))).toBe(true);
  });

  it('marks a path-only active occurrence in the grouped path field', () => {
    const projection = buildSearchOutputProjection(text, { toolUseId: 'search-1' });
    const targetFragment = projection.groups[0]!.fragment;
    const { container } = render(
      <SearchResultsView
        rawText={text}
        onOpenFile={undefined}
        toolUseId="search-1"
        activeHighlight={{
          fragmentId: targetFragment.fragmentId,
          start: 0,
          end: 'src/foo.rs'.length,
        }}
      />,
    );
    expect(container.querySelector('.search-results-filepath .viewer-find-inline-match--active')?.textContent).toBe('src/foo.rs');
  });
});

describe('KeywordSearchView', () => {
  const llm = [
    '/abs/path/to/foo.rs: implements the foo state machine',
    '/abs/path/to/bar.rs: helper utilities for foo',
  ].join('\n');

  it('marks exact active fragment occurrence without duplicating explanation text', () => {
    const fragmentId = buildKeywordSearchOutputProjection(llm, { toolUseId: 'tool-1' }).hits[1]!.fragment.fragmentId;
    const { container } = render(
      <KeywordSearchView
        rawText={llm}
        onOpenFile={undefined}
        toolUseId="tool-1"
        activeHighlight={{
          fragmentId,
          start: 0,
          end: 19,
        }}
      />
    );
    const activeMark = container.querySelector('.viewer-find-inline-match--active');
    expect(activeMark?.textContent).toBe('/abs/path/to/bar.rs');
    expect(screen.getByText('helper utilities for foo')).toBeInTheDocument();
  });

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

  it('searches and marks the raw-fallback notice separately from body text', () => {
    const raw = [
      'src/a.ts:1:alpha',
      'src/b.ts:2:beta',
      '--',
      'src/c.ts:3:gamma',
    ].join('\n');
    const projection = buildKeywordSearchOutputProjection(raw, { toolUseId: 'keyword-1' });
    const title = projection.fragments.find((fragment) => fragment.fragmentId === 'keyword-search-fallback-title')!;
    const { container } = render(
      <KeywordSearchView
        rawText={raw}
        onOpenFile={undefined}
        toolUseId="keyword-1"
        activeHighlight={{ fragmentId: title.fragmentId, start: 4, end: 11 }}
      />,
    );
    expect(container.querySelector('.keyword-search-fallback-note .viewer-find-inline-match--active')?.textContent).toBe('ripgrep');
    expect(container.querySelector('.keyword-search-raw-text .viewer-find-inline-match--active')).toBeNull();
  });

  it('marks an active coverage note without changing its visible text', () => {
    const rawText = [
      '/abs/path/to/foo.rs: primary hit',
      '[results truncated: use more specific terms]',
    ].join('\n');
    const projection = buildKeywordSearchOutputProjection(rawText, { toolUseId: 'keyword-1' });
    const note = projection.notes[0]!.fragment;
    const start = note.semanticText.indexOf('truncated');
    const { container } = render(
      <KeywordSearchView
        rawText={rawText}
        onOpenFile={undefined}
        toolUseId="keyword-1"
        activeHighlight={{ fragmentId: note.fragmentId, start, end: start + 'truncated'.length }}
      />,
    );
    expect(container.querySelector('.search-results-note .viewer-find-inline-match--active')?.textContent).toBe('truncated');
    expect(container.querySelector('.search-results-note')?.textContent).toBe(note.semanticText);
  });

  it('renders the empty/none-found state', () => {
    render(<KeywordSearchView rawText="No relevant files found" onOpenFile={undefined} />);
    expect(screen.getByText('No relevant files found.')).toBeTruthy();
  });
  it('marks the active occurrence in the empty results state', () => {
    const { container } = render(
      <KeywordSearchView
        rawText="No relevant files found"
        onOpenFile={undefined}
        toolUseId="keyword-1"
        activeHighlight={{ fragmentId: 'keyword-search-empty', start: 3, end: 11 }}
      />,
    );
    expect(container.querySelector('.viewer-find-inline-match--active')?.textContent).toBe('relevant');
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



describe('parseReadFileOutput', () => {
  it('parses numbered read_file lines with tabs', () => {
    const padded = __readFileResultTestables.parseOutput('     7\tproduction format');
    expect(padded.malformed).toBe(false);
    expect(padded.lines).toEqual([{ lineNumber: 7, content: 'production format' }]);

    const parsed = __readFileResultTestables.parseOutput('12\talpha\n13\tbeta');
    expect(parsed.malformed).toBe(false);
    expect(parsed.notes).toEqual([]);
    expect(parsed.lines).toEqual([
      { lineNumber: 12, content: 'alpha' },
      { lineNumber: 13, content: 'beta' },
    ]);
  });

  it('marks non-numbered output as malformed fallback data', () => {
    const parsed = __readFileResultTestables.parseOutput('not numbered\n12\tstill parsed');
    expect(parsed.malformed).toBe(true);
    expect(parsed.notes).toEqual(['not numbered']);
    expect(parsed.lines).toEqual([{ lineNumber: 12, content: 'still parsed' }]);
  });
});
