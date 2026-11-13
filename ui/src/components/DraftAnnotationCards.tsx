import { useEffect, useRef, useState } from 'react';
import { ArrowUpRight, X } from 'lucide-react';
import type { AnnotationDraft, DraftAnnotation } from '../conversation/AnnotationDraftStore';
import './DraftAnnotationCards.css';

export function DraftAnnotationCards({ draft, annotations, disabled }: {
  draft: AnnotationDraft;
  annotations: readonly DraftAnnotation[];
  disabled: boolean;
}) {
  const navigation = useRef<AbortController | null>(null);
  const [notice, setNotice] = useState('');
  useEffect(() => () => navigation.current?.abort(), [draft]);
  if (!annotations.length) return null;
  return (
    <section className="draft-annotations" aria-label="Annotations">
      {annotations.map((annotation, index) => (
        <article className="draft-annotation" key={annotation.id} aria-label={`Annotation ${index + 1}`}>
          <div className="draft-annotation-header">
            <button type="button" className="draft-annotation-source" title={annotation.source.quote} aria-label={`Return to annotation ${index + 1} source`} onClick={async () => {
              navigation.current?.abort();
              const request = new AbortController();
              navigation.current = request;
              setNotice('');
              try {
                const found = await draft.returnToSource(annotation.source, request.signal);
                if (!request.signal.aborted && !found) setNotice('The source passage is unavailable. Your annotation is retained.');
              } catch {
                if (!request.signal.aborted) setNotice('Could not return to the source passage. Your annotation is retained.');
              }
            }}>
              <span>“{annotation.source.quote}”</span>
              <ArrowUpRight size={14} aria-hidden="true" />
            </button>
            <button type="button" className="draft-annotation-remove" aria-label={`Remove annotation ${index + 1}`} disabled={disabled || annotation.submission !== null} onClick={() => draft.remove(annotation.id)}>
              <X size={16} aria-hidden="true" />
            </button>
          </div>
          <textarea aria-label={`Annotation ${index + 1} comment`} rows={1} value={annotation.body} disabled={disabled || annotation.submission !== null} onChange={event => draft.edit(annotation.id, event.target.value)} />
          {annotation.submission !== null && <span className="draft-annotation-state">Sending…</span>}
        </article>
      ))}
      {notice && <div role="status">{notice}</div>}
    </section>
  );
}
