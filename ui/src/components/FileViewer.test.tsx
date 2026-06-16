import { render, screen, waitFor } from '@testing-library/react';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { FileViewer } from './FileViewer';
import { ReviewNotesProvider } from '../contexts/ReviewNotesContext';

function renderReader(filePath: string) {
  return render(
    <ReviewNotesProvider>
      <FileViewer
        filePath={filePath}
        rootDir="/tmp/project"
        onClose={() => undefined}
        onSendNotes={() => undefined}
      />
    </ReviewNotesProvider>,
  );
}

describe('FileViewer typed file responses', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('renders image responses using the preview URL instead of text content', async () => {
    const fetchMock = vi.mocked(fetch);
    fetchMock.mockResolvedValueOnce(new Response(JSON.stringify({
      kind: 'image',
      mime_type: 'image/png',
      url: '/preview/tmp/project/screenshot.png',
    }), { status: 200, headers: { 'Content-Type': 'application/json' } }));

    renderReader('screenshot.png');

    const image = await screen.findByRole('img', { name: 'screenshot.png' });
    expect(image).toHaveAttribute('src', '/preview/tmp/project/screenshot.png');
    expect(screen.queryByText(/File appears to be binary/)).not.toBeInTheDocument();
    expect(fetchMock).toHaveBeenCalledWith('/api/files/read?path=%2Ftmp%2Fproject%2Fscreenshot.png');
  });

  it('continues to render text responses as prose content', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(new Response(JSON.stringify({
      kind: 'text',
      content: 'hello text file',
      encoding: 'utf-8',
      category: 'plain',
    }), { status: 200, headers: { 'Content-Type': 'application/json' } }));

    renderReader('notes.txt');

    await waitFor(() => expect(screen.getByText('hello text file')).toBeInTheDocument());
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
  });

  it('routes very line-heavy text files to the large plain-text fallback', async () => {
    // The fallback guards line-per-node text/markdown rendering; code renders
    // through Pierre's virtualized view and never falls back.
    vi.mocked(fetch).mockResolvedValueOnce(new Response(JSON.stringify({
      kind: 'text',
      content: `${'line\n'.repeat(2_001)}tail`,
      encoding: 'utf-8',
      category: 'plain',
    }), { status: 200, headers: { 'Content-Type': 'application/json' } }));

    renderReader('big.txt');

    expect(await screen.findByTestId('viewer-large-text-fallback')).toBeInTheDocument();
    expect(screen.getByText(/Large file shown as plain text/)).toBeInTheDocument();
  });
});
