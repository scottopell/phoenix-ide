import type { ViewerBodyProps } from './AnnotatableBlock';

/** Plain <pre> body for large source-like payloads that bypass rich rendering. */
export function TextViewerBody({ content }: ViewerBodyProps) {
  return (
    <div className="viewer-text" data-testid="viewer-large-text-fallback">
      <pre className="viewer-large-text-pre">{content}</pre>
    </div>
  );
}
