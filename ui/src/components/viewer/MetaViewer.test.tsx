import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect } from 'vitest';
import { MetaViewer } from './MetaViewer';
import { ReviewNotesProvider } from '../../contexts/ReviewNotesContext';
import type { MetaViewerPayload } from './metaViewerTypes';

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
const textCommon = { ...common, filePath: 'thing', rootDir: '/tmp/project' };

describe('MetaViewer payload routing', () => {

  it('routes a markdown payload to rendered markdown', () => {
    renderViewer({ ...textCommon, kind: 'markdown', content: '# Hello\n\nbody text' });
    expect(screen.getByRole('heading', { name: 'Hello' })).toBeInTheDocument();
  });

  it('routes a code payload to the syntax-highlighted code body', () => {
    const { container } = renderViewer({ ...textCommon, kind: 'code', language: 'rust', content: 'fn main() {}' });
    expect(container.querySelector('.viewer-code')).not.toBeNull();
    expect(container.querySelector('.viewer-code-line')).not.toBeNull();
  });

  it('routes a plain-text payload to line-numbered text', () => {
    const { container } = renderViewer({ ...textCommon, kind: 'text', content: 'plain line' });
    expect(screen.getByText('plain line')).toBeInTheDocument();
    expect(container.querySelector('.viewer-text')).not.toBeNull();
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
});
