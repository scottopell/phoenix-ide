import mermaid from 'mermaid';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { MetaViewer } from './MetaViewer';
import { ReviewNotesProvider } from '../../contexts/ReviewNotesContext';
import type { MetaViewerPayload } from './metaViewerTypes';
import { resetCodeViewMock } from './__testutils__/codeViewMock';

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
describe('MetaViewer payload routing', () => {
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

  it('lets a large HTML file still toggle to the sandboxed preview (fallback only gates source)', () => {
    const largeHtml = `${'<p>line</p>\n'.repeat(2_001)}<p>tail</p>`;
    const { container } = renderViewer({
      ...textCommon,
      kind: 'html',
      language: 'html',
      content: largeHtml,
      renderMode: 'plainLargeText',
      previewUrl: '/preview/tmp/project/thing',
    });

    // Source view falls back to plain text for the large file...
    expect(screen.getByTestId('viewer-large-text-fallback')).toBeInTheDocument();
    // ...but switching to Preview must reach the iframe, not stay stranded on <pre>.
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
    expect(container.querySelector('iframe')).not.toBeNull();
    expect(screen.queryByTestId('viewer-large-text-fallback')).toBeNull();
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

  it('keeps HTML source and markdown find ineligible so unrevealable matches are not counted', () => {
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

    expect(screen.queryByRole('button', { name: 'Find in file' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }));
    expect(screen.queryByRole('button', { name: 'Find in file' })).toBeNull();

    unmount();
    const { rerender } = render(
      <ReviewNotesProvider>
        <MetaViewer payload={{ ...textCommon, kind: 'markdown', content: '# alpha' }} />
      </ReviewNotesProvider>,
    );
    expect(screen.queryByRole('button', { name: 'Find in file' })).toBeNull();
    rerender(<ReviewNotesProvider><MetaViewer payload={{ ...textCommon, kind: 'text', content: 'alpha' }} /></ReviewNotesProvider>);
    expect(screen.getByRole('button', { name: 'Find in file' })).toBeInTheDocument();
  });
});

