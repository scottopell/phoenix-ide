import mermaid from 'mermaid';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { __metaViewerFindTestables, MetaViewer } from './MetaViewer';
import { ReviewNotesProvider } from '../../contexts/ReviewNotesContext';
import type { MetaViewerPayload } from './metaViewerTypes';
import { codeViewMockState, resetCodeViewMock } from './__testutils__/codeViewMock';

vi.mock('mermaid', () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn((_id: string, code: string) => Promise.resolve({
      svg: `<svg role="img" aria-label="Viewer Mermaid"><text>${code}</text></svg>`,
      bindFunctions: vi.fn(),
    })),
  },
}));

// Code payloads render through Pierre's CodeView; stub it so its async
// tokenizer doesn't run under happy-dom (see DiffView.test).
vi.mock('@pierre/diffs/react', async () => {
  const { makeCodeViewMock } = await import('./__testutils__/codeViewMock');
  return makeCodeViewMock();
});

function renderViewer(payload: MetaViewerPayload) {
  return render(
    <ReviewNotesProvider>
      <MetaViewer payload={payload} />
    </ReviewNotesProvider>,
  );
}

const common = {
  title: 'thing',
  absolutePath: '/tmp/project/thing',
  onClose: () => undefined,
  onSendNotes: () => undefined,
};
async function renderLoadedImageViewer() {
  renderViewer({
    ...common,
    kind: 'image',
    url: '/preview/tmp/project/thing.png',
    mimeType: 'image/png',
    fileName: 'thing.png',
  });

  const surface = screen.getByTestId('image-preview-surface');
  vi.spyOn(surface, 'getBoundingClientRect').mockReturnValue({
    x: 0,
    y: 0,
    top: 0,
    left: 0,
    right: 800,
    bottom: 600,
    width: 800,
    height: 600,
    toJSON: () => ({}),
  });
  fireEvent(window, new Event('resize'));

  Object.defineProperties(surface, {
    scrollLeft: { value: 0, writable: true },
    scrollTop: { value: 0, writable: true },
  });
  const img = screen.getByRole('img', { name: 'thing.png' }) as HTMLImageElement;
  Object.defineProperties(img, {
    naturalWidth: { value: 1600 },
    naturalHeight: { value: 1200 },
  });
  fireEvent.load(img);

  await waitFor(() => expect(screen.getByText('46%')).toBeInTheDocument());
  return { surface, img };
}

function fireWheel(surface: HTMLElement, deltaY: number) {
  const event = new Event('wheel', { bubbles: true, cancelable: true });
  Object.defineProperties(event, {
    ctrlKey: { value: true },
    deltaY: { value: deltaY },
    clientX: { value: 400 },
    clientY: { value: 300 },
  });
  const preventDefault = vi.spyOn(event, 'preventDefault');
  fireEvent(surface, event);
  expect(preventDefault).toHaveBeenCalled();
  return event;
}

const textCommon = { ...common, filePath: 'thing', rootDir: '/tmp/project' };
describe('MetaViewer find identities', () => {
  it('bounds match ids for arbitrarily long file lines', () => {
    const text = `prefix ${'x'.repeat(20_000)} token`;
    const source = {
      id: 'line:1',
      kind: 'line' as const,
      lineNumber: 1,
      text,
      target: { kind: 'file-line' as const, lineNumber: 1, startColumn: 0, endColumn: 0 },
    };
    const id = __metaViewerFindTestables.stableFileMatchId([source])({
      sourceId: source.id,
      sourceText: text,
      start: text.length - 5,
      end: text.length,
      target: { kind: 'file-line', lineNumber: 1, startColumn: text.length - 5, endColumn: text.length },
    });

    expect(id).not.toContain('prefix');
    expect(id.length).toBeLessThan(128);
  });
});

describe('MetaViewer payload routing', () => {
  it('drives file counts from the typed line projection', async () => {
    renderViewer({
      ...textCommon,
      kind: 'text',
      content: 'alpha\nbeta',
    });

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });

    await waitFor(() => expect(screen.getByText('1 of 1')).toBeInTheDocument());
  });

  beforeEach(() => resetCodeViewMock());

  it('routes a markdown payload to rendered markdown', () => {
    renderViewer({ ...textCommon, kind: 'markdown', content: '# Hello\n\nbody text' });
    expect(screen.getByRole('heading', { name: 'Hello' })).toBeInTheDocument();
  });

  it('routes markdown mermaid fences to the shared diagram renderer', async () => {
    renderViewer({ ...textCommon, kind: 'markdown', content: '```mermaid\nflowchart TD\n  A --> B\n```' });

    expect(await screen.findByTestId('mermaid-diagram')).toBeInTheDocument();
    expect(await screen.findByRole('img', { name: 'Viewer Mermaid' })).toBeInTheDocument();
    expect(mermaid.render).toHaveBeenCalledWith(expect.stringMatching(/^phoenix-mermaid-/), 'flowchart TD\n  A --> B');
  });

  it('routes a code payload to the Pierre file code view, not the legacy code body', () => {
    const { container } = renderViewer({ ...textCommon, kind: 'code', language: 'rust', content: 'fn main() {}' });
    expect(container.querySelector('.phoenix-file-codeview')).not.toBeNull();
    // The react-syntax-highlighter path is retired for code.
    expect(container.querySelector('.viewer-code')).toBeNull();
  });

  it('keeps large code on the Pierre view (no plain-text fallback for code)', () => {
    const largeContent = `${'line\n'.repeat(2_001)}tail`;
    const { container } = renderViewer({
      ...textCommon,
      kind: 'code',
      language: 'typescript',
      content: largeContent,
    });

    expect(container.querySelector('.phoenix-file-codeview')).not.toBeNull();
    expect(screen.queryByTestId('viewer-large-text-fallback')).toBeNull();
  });

  it('routes a plain-text payload to the Pierre file code view, not the legacy text body', () => {
    const { container } = renderViewer({ ...textCommon, kind: 'text', content: 'plain line' });
    expect(container.querySelector('.phoenix-file-codeview')).not.toBeNull();
    expect(container.querySelector('.viewer-text')).toBeNull();
  });

  it('reopens find by refocusing the existing bar after body focus leaves the input', async () => {
    renderViewer({ ...textCommon, kind: 'text', content: 'alpha\nbeta alpha' });

    const findButton = screen.getByRole('button', { name: 'Find in file' });
    fireEvent.click(findButton);

    const input = screen.getByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'alpha' } });

    const bodyLine = document.querySelector('[data-line="1"]') as HTMLElement;
    bodyLine.focus();
    expect(bodyLine).toHaveFocus();

    fireEvent.click(findButton);

    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Find in viewer' })).toHaveFocus());
    expect((screen.getByRole('textbox', { name: 'Find in viewer' }) as HTMLInputElement).value).toBe('alpha');
  });

  it('opens shared find for Pierre-backed files and restores focus to the opener on Escape', async () => {
    renderViewer({ ...textCommon, kind: 'text', content: 'alpha\nbeta alpha' });

    const findButton = screen.getByRole('button', { name: 'Find in file' });
    findButton.focus();
    fireEvent.click(findButton);

    const input = screen.getByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'alpha' } });
    fireEvent.keyDown(input, { key: 'Escape' });

    await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull());
    expect(findButton).toHaveFocus();
  });

  it('clears Pierre file decorations when shell Escape closes an open find bar after body focus', async () => {
    renderViewer({ ...textCommon, kind: 'text', content: 'alpha\nbeta alpha' });

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });

    const bodyLine = document.querySelector('[data-line="1"]') as HTMLElement;
    bodyLine.focus();
    fireEvent.keyDown(window, { key: 'Escape' });

    await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull());
    expect(document.querySelector('[data-find-occurrence]')).toBeNull();
  });

  it('restores large-text find focus to the body opener instead of the toolbar', async () => {
    renderViewer({
      ...textCommon,
      kind: 'html',
      language: 'html',
      content: '<p>alpha</p>\n<p>beta alpha</p>',
      renderMode: 'plainLargeText',
      previewUrl: '/preview/tmp/project/thing',
    });

    const largeTextBody = screen.getByTestId('viewer-large-text-fallback');
    const findButton = screen.getByRole('button', { name: 'Find in file' });
    largeTextBody.focus();
    fireEvent.click(findButton);

    const input = screen.getByRole('textbox', { name: 'Find in viewer' });
    fireEvent.change(input, { target: { value: 'alpha' } });
    fireEvent.keyDown(input, { key: 'Escape' });

    await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull());
    expect(largeTextBody).toHaveFocus();
  });

  it('clears large-text fallback marks when closing find from the toolbar', async () => {
    renderViewer({
      ...textCommon,
      kind: 'html',
      language: 'html',
      content: '<p>alpha</p>\n<p>beta alpha</p>',
      renderMode: 'plainLargeText',
      previewUrl: '/preview/tmp/project/thing',
    });

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });
    expect(document.querySelectorAll('[data-find-occurrence]').length).toBe(2);

    fireEvent.click(screen.getByRole('button', { name: 'Close' }));

    await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull());
    expect(document.querySelector('[data-find-occurrence]')).toBeNull();
  });

  it('highlights and counts exact occurrences in large text fallback DOM source', async () => {
    renderViewer({
      ...textCommon,
      kind: 'html',
      language: 'html',
      content: '<p>alpha</p>\n<p>beta alpha</p>',
      renderMode: 'plainLargeText',
      previewUrl: '/preview/tmp/project/thing',
    });

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });

    expect(screen.getByText('1 of 2')).toBeInTheDocument();
    expect(document.querySelectorAll('[data-find-occurrence]').length).toBe(2);
    expect(document.querySelector('.viewer-find-match--active')).toHaveTextContent('alpha');
  });

  it('uses the line-aware source viewer for a focused markdown range regardless of file size', () => {
    const { container } = renderViewer({
      ...textCommon,
      kind: 'markdown',
      content: '# Heading\nparagraph first line\nparagraph second line\n```ts\nconst value = 1;\n```',
      focus: { kind: 'range', startLine: 3, endLine: 5 },
    });

    expect(container.querySelector('.phoenix-file-codeview')).not.toBeNull();
    expect(screen.queryByRole('heading', { name: 'Heading' })).not.toBeInTheDocument();
    expect(screen.getByText(/focused on lines 3–5/)).toBeInTheDocument();
  });

  it('enables find when a focused markdown range renders in the line-aware source viewer', async () => {
    renderViewer({
      ...textCommon,
      kind: 'markdown',
      content: '# Heading\nparagraph alpha\nparagraph second line',
      focus: { kind: 'range', startLine: 2, endLine: 3 },
    });

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });

    await waitFor(() => expect(screen.getByText('1 of 1')).toBeInTheDocument());
  });

  it('lets a focused large HTML file still toggle from line-aware source to sandboxed preview', () => {
    const largeHtml = `${'<p>line</p>\n'.repeat(2_001)}<p>tail</p>`;
    const { container } = renderViewer({
      ...textCommon,
      kind: 'html',
      language: 'html',
      content: largeHtml,
      renderMode: 'plainLargeText',
      previewUrl: '/preview/tmp/project/thing',
      focus: { kind: 'range', startLine: 100, endLine: 110 },
    });

    expect(container.querySelector('.phoenix-file-codeview')).not.toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
    expect(container.querySelector('iframe')).not.toBeNull();
    expect(container.querySelector('.phoenix-file-codeview')).toBeNull();
  });

  it('routes an image payload to the image body', () => {
    renderViewer({
      ...common,
      kind: 'image',
      url: '/preview/tmp/project/thing.png',
      mimeType: 'image/png',
      fileName: 'thing.png',
    });
    const img = screen.getByRole('img', { name: 'thing.png' });
    expect(img).toHaveAttribute('src', '/preview/tmp/project/thing.png');
  });

  it('shows fullscreen controls only for image payloads', () => {
    const { rerender } = render(
      <ReviewNotesProvider>
        <MetaViewer
          payload={{
            ...common,
            kind: 'image',
            url: '/preview/tmp/project/thing.png',
            mimeType: 'image/png',
            fileName: 'thing.png',
          }}
        />
      </ReviewNotesProvider>,
    );

    expect(screen.getByRole('link', { name: 'Open in new tab' })).toHaveAttribute('href', '/preview/tmp/project/thing.png');
    const fullscreen = screen.getByRole('button', { name: 'Open fullscreen image viewer' });
    expect(fullscreen).toBeInTheDocument();
    fireEvent.click(fullscreen);
    expect(screen.getByRole('button', { name: 'Exit fullscreen image viewer' })).toBeInTheDocument();
    expect(document.body.querySelector('.viewer-shell--takeover')).not.toBeNull();

    rerender(
      <ReviewNotesProvider>
        <MetaViewer payload={{ ...textCommon, kind: 'text', content: 'plain line' }} />
      </ReviewNotesProvider>,
    );
    expect(screen.queryByRole('button', { name: /fullscreen image viewer/i })).toBeNull();
    expect(screen.queryByRole('link', { name: 'Open in new tab' })).toBeNull();
  });

  it('handles trackpad pinch wheel events locally and updates image zoom', async () => {
    const { surface, img } = await renderLoadedImageViewer();

    fireWheel(surface, -240);

    await waitFor(() => expect(screen.getByText(/^[1-9]\d\d%$/)).toBeInTheDocument());
    expect(img.getAttribute('width')).not.toBeNull();
    expect(surface.scrollLeft).toBeGreaterThan(0);
    expect(surface.scrollTop).toBeGreaterThan(0);
  });

  it('single-clicks from fit to 100% around the cursor and clicks again to reset', async () => {
    const { surface } = await renderLoadedImageViewer();

    fireEvent.click(surface, { clientX: 400, clientY: 300 });
    await waitFor(() => expect(screen.getByText('100%')).toBeInTheDocument());
    expect(surface.scrollLeft).toBeGreaterThan(0);
    expect(surface.scrollTop).toBeGreaterThan(0);

    fireEvent.click(surface, { clientX: 400, clientY: 300 });
    await waitFor(() => expect(screen.getByText('46%')).toBeInTheDocument());
    expect(surface.scrollLeft).toBe(0);
    expect(surface.scrollTop).toBe(0);
  });

  it('applies equal pinch-in and pinch-out deltas symmetrically', async () => {
    const { surface, img } = await renderLoadedImageViewer();

    fireWheel(surface, -120);
    const zoomedWidth = Number(img.getAttribute('width'));
    expect(zoomedWidth).toBeGreaterThan(736);

    fireWheel(surface, 120);
    await waitFor(() => expect(Number(img.getAttribute('width'))).toBeCloseTo(736, 0));
    expect(screen.getByText('46%')).toBeInTheDocument();
  });

  it('renders html as source by default and toggles to a sandboxed preview', () => {
    const { container } = renderViewer({
      ...textCommon,
      kind: 'html',
      language: 'html',
      content: '<p>hi</p>',
      previewUrl: '/preview/tmp/project/thing',
    });

    // Source mode: code body present, no iframe.
    expect(container.querySelector('.viewer-code')).not.toBeNull();
    expect(container.querySelector('iframe')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));

    const iframe = container.querySelector('iframe');
    expect(iframe).not.toBeNull();
    // Security: sandbox must stay allow-same-origin only (no allow-scripts).
    expect(iframe).toHaveAttribute('sandbox', 'allow-same-origin');
    expect(iframe).toHaveAttribute('src', '/preview/tmp/project/thing');
  });

  it('defers file search projection work until find is open with a query', async () => {
    const searchProjectionModule = await import('../viewer-find/searchProjections');
    const buildProjectionSpy = vi.spyOn(searchProjectionModule, 'buildFileSearchProjection');

    renderViewer({ ...textCommon, kind: 'text', content: 'alpha\nbeta' });
    expect(buildProjectionSpy).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    expect(buildProjectionSpy).not.toHaveBeenCalled();

    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });
    expect(buildProjectionSpy).toHaveBeenCalledWith('alpha\nbeta', 'alpha');
  });

  it('skips large-text line-fragment rendering when the active occurrence is negative', () => {
    renderViewer({
      ...textCommon,
      kind: 'html',
      language: 'html',
      content: '<p>alpha</p>\n<p>beta</p>',
      renderMode: 'plainLargeText',
      previewUrl: '/preview/tmp/project/thing',
    });

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'missing' } });

    expect(screen.getByText('0 results')).toBeInTheDocument();
    expect(document.querySelector('[data-find-line]')).toBeNull();
    expect(document.querySelector('[data-find-occurrence]')).toBeNull();
  });

  it('keeps image payloads out of file find and does not show false counts', () => {
    renderViewer({
      ...common,
      kind: 'image',
      url: '/preview/tmp/project/thing.png',
      mimeType: 'image/png',
      fileName: 'thing.png',
    });

    expect(screen.queryByRole('button', { name: 'Find in file' })).toBeNull();
    expect(screen.queryByText(/of \d+/)).toBeNull();
  });

  it('keeps HTML preview ineligible while allowing decorated HTML source and rendered Markdown find', () => {
    const { unmount } = render(
      <ReviewNotesProvider>
        <MetaViewer
          payload={{
            ...textCommon,
            kind: 'html',
            language: 'html',
            content: '<p>alpha</p>',
            previewUrl: '/preview/tmp/project/thing',
          }}
        />
      </ReviewNotesProvider>,
    );

    expect(screen.getByRole('button', { name: 'Find in file' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });
    expect(document.querySelector('.viewer-code .viewer-find-match--active')).toHaveTextContent('alpha');

    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
    expect(screen.queryByRole('button', { name: 'Find in file' })).toBeNull();

    unmount();
    render(
      <ReviewNotesProvider>
        <MetaViewer payload={{ ...textCommon, kind: 'markdown', content: '# Title\n\nparagraph alpha' }} />
      </ReviewNotesProvider>,
    );
    expect(screen.getByRole('button', { name: 'Find in file' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });
    expect(screen.getByText('1 of 1')).toBeInTheDocument();
    expect(document.querySelector('.viewer-markdown .viewer-find-match--active')).toHaveTextContent('alpha');
    expect(document.querySelector('[data-line="3"] .viewer-find-match--active')).toHaveTextContent('alpha');
  });

  it('treats rendered and source-style Markdown as different find surfaces', () => {
    const { rerender } = render(
      <ReviewNotesProvider>
        <MetaViewer payload={{ ...textCommon, kind: 'markdown', content: '# alpha' }} />
      </ReviewNotesProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });
    expect(screen.getByText('1 of 1')).toBeInTheDocument();

    rerender(
      <ReviewNotesProvider>
        <MetaViewer payload={{ ...textCommon, kind: 'markdown', content: '# alpha', renderMode: 'plainLargeText' }} />
      </ReviewNotesProvider>,
    );
    expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull();
  });

  it('marks matches in lower-level Markdown headings', () => {
    renderViewer({ ...textCommon, kind: 'markdown', content: '#### lower heading alpha' });

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });
    expect(document.querySelector('h4 .viewer-find-match--active')).toHaveTextContent('alpha');
  });

  it('does not count rendered fenced-code text without an owned inline marker', () => {
    renderViewer({ ...textCommon, kind: 'markdown', content: '# Title\n\n```ts\nconst hiddenNeedle = true;\n```' });

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'hiddenNeedle' } });
    expect(screen.getByText('0 results')).toBeInTheDocument();
  });

  it('marks a match in a later table cell with block-local offsets', () => {
    renderViewer({ ...textCommon, kind: 'markdown', content: '| first | second target |\n| --- | --- |' });

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'target' } });
    expect(screen.getByText('1 of 1')).toBeInTheDocument();
    expect(document.querySelector('.viewer-markdown .viewer-find-match--active')).toHaveTextContent('target');
  });

  it('keeps identical same-line table cells as distinct match owners', () => {
    renderViewer({ ...textCommon, kind: 'markdown', content: '| alpha | alpha |\n| --- | --- |' });
    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });

    expect(screen.getByText('1 of 2')).toBeInTheDocument();
    expect(document.querySelectorAll('th .viewer-find-match--active')).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('2 of 2')).toBeInTheDocument();
    expect(document.querySelectorAll('th .viewer-find-match--active')).toHaveLength(1);
  });

  it('keeps source-style Markdown fallbacks searchable', () => {
    renderViewer({ ...textCommon, kind: 'markdown', content: '# alpha', renderMode: 'plainLargeText' });

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });
    expect(screen.getByText('1 of 1')).toBeInTheDocument();
    expect(document.querySelector('[data-find-occurrence="0"]')).toHaveTextContent('alpha');
  });

  it('closes HTML source find when switching to preview', async () => {
    renderViewer({
      ...textCommon,
      kind: 'html',
      language: 'html',
      content: '<p>alpha</p>',
      previewUrl: '/preview/tmp/project/thing',
    });

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });
    expect(screen.getByText('1 of 1')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Find in file' })).toBeNull());
  });

  it('resets find state when the viewed absolutePath changes', async () => {
    const { rerender } = render(
      <ReviewNotesProvider>
        <MetaViewer payload={{ ...textCommon, kind: 'text', content: 'alpha\nbeta' }} />
      </ReviewNotesProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });
    expect(screen.getByText('1 of 1')).toBeInTheDocument();

    rerender(
      <ReviewNotesProvider>
        <MetaViewer payload={{ ...textCommon, absolutePath: '/tmp/project/other', kind: 'text', content: 'alpha\nbeta' }} />
      </ReviewNotesProvider>,
    );

    await waitFor(() => expect(screen.queryByRole('textbox', { name: 'Find in viewer' })).toBeNull());
  });

  it('replaces results when the viewed content changes under the same path and kind', async () => {
    const { rerender } = render(
      <ReviewNotesProvider>
        <MetaViewer payload={{ ...textCommon, kind: 'text', content: 'alpha\nbeta' }} />
      </ReviewNotesProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'alpha' } });
    expect(screen.getByText('1 of 1')).toBeInTheDocument();

    rerender(
      <ReviewNotesProvider>
        <MetaViewer payload={{ ...textCommon, kind: 'text', content: 'gamma\ndelta' }} />
      </ReviewNotesProvider>,
    );

    await waitFor(() => expect(screen.getByRole('textbox', { name: 'Find in viewer' })).toHaveValue('alpha'));
    expect(screen.getByText('0 results')).toBeInTheDocument();
  });

  it('preserves and re-reveals the semantic active match when insertions move it', async () => {
    const { rerender } = render(
      <ReviewNotesProvider>
        <MetaViewer payload={{ ...textCommon, kind: 'text', content: 'alpha\nbeta\ngamma' }} />
      </ReviewNotesProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'beta' } });

    await waitFor(() => expect(codeViewMockState.scrollToCalls).toContainEqual({
      type: 'line', id: 'file:/tmp/project/thing', lineNumber: 2, align: 'center', behavior: 'smooth',
    }));
    await waitFor(() => expect(codeViewMockState.scrollToCalls).toContainEqual({
      type: 'position', position: 0,
    }));
    codeViewMockState.scrollToCalls.length = 0;

    rerender(
      <ReviewNotesProvider>
        <MetaViewer payload={{ ...textCommon, kind: 'text', content: 'intro\nalpha\nbeta\ngamma' }} />
      </ReviewNotesProvider>,
    );

    await waitFor(() => expect(screen.getByText('1 of 1')).toBeInTheDocument());
    await waitFor(() => expect(codeViewMockState.scrollToCalls).toContainEqual({
      type: 'line', id: 'file:/tmp/project/thing', lineNumber: 3, align: 'center', behavior: 'smooth',
    }));
  });

  it('keeps repeated identical file matches distinct', async () => {
    render(
      <ReviewNotesProvider>
        <MetaViewer payload={{ ...textCommon, kind: 'text', content: 'same\nsame\nsame' }} />
      </ReviewNotesProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Find in file' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Find in viewer' }), { target: { value: 'same' } });

    await waitFor(() => expect(screen.getByText('1 of 3')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('2 of 3')).toBeInTheDocument();
  });

});

