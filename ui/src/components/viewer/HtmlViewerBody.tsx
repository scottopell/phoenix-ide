import { CodeViewerBody } from './CodeViewerBody';
import type { ViewerBodyProps } from './AnnotatableBlock';

export type HtmlViewMode = 'source' | 'preview';

interface HtmlViewerBodyProps extends ViewerBodyProps {
  mode: HtmlViewMode;
  language: string;
  /** `/preview<absolutePath>` — the sandboxed iframe source. */
  previewUrl: string;
}

/**
 * HTML body with two modes:
 *   - `source`: syntax-highlighted, annotatable source (delegates to
 *     CodeViewerBody so the line/annotation behaviour is identical to code).
 *   - `preview`: a sandboxed iframe. The sandbox is `allow-same-origin` only —
 *     no `allow-scripts` — so the preview renders styles/markup without
 *     executing page scripts. Script-enabled rendering is the explicit
 *     "Open in browser" action (a real navigation), never this inline frame.
 *     This attribute must not gain `allow-scripts`.
 */
export function HtmlViewerBody({ mode, previewUrl, ...bodyProps }: HtmlViewerBodyProps) {
  if (mode === 'preview') {
    return (
      <div className="viewer-html-preview">
        <iframe
          src={previewUrl}
          sandbox="allow-same-origin"
          title="HTML Preview"
          className="viewer-iframe"
        />
      </div>
    );
  }
  return <CodeViewerBody {...bodyProps} />;
}
