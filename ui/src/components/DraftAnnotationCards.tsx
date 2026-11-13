import { useEffect, useId, useRef, useState } from 'react';
import { ArrowUpRight, ChevronRight } from 'lucide-react';
import type { AnnotationDraft, DraftAnnotation } from '../conversation/AnnotationDraftStore';
import { useRegisterFocusScope } from '../hooks/useFocusScope';
import { SelectionDialog } from './SelectionDialog';
import './DraftAnnotationCards.css';

function AnnotationEditor({ draft, annotation, disabled, onClose, onDelete, restoreFocusRef }: {
  draft: AnnotationDraft;
  annotation: DraftAnnotation;
  disabled: boolean;
  onClose: () => void;
  onDelete: () => void;
  restoreFocusRef: React.RefObject<HTMLElement | null>;
}) {
  useRegisterFocusScope('draft-annotation-editor');
  const navigation = useRef<AbortController | null>(null);
  const [notice, setNotice] = useState('');
  useEffect(() => () => navigation.current?.abort(), [draft]);
  const locked = disabled || annotation.submission !== null;
  return (
    <SelectionDialog title={`Annotation ${annotation.number}`} onClose={onClose} restoreFocusRef={restoreFocusRef} className="annotation-editor" footer={<>
      <button type="button" className="annotation-editor-delete" disabled={locked} onClick={onDelete}>Delete annotation</button>
      <button type="button" className="selection-dialog__apply" onClick={onClose}>Done</button>
    </>}>
      <div className="annotation-editor-source-heading">
        <span>Message #{annotation.source.sequenceId}</span>
        <button type="button" onClick={async () => {
          navigation.current?.abort();
          const request = new AbortController();
          navigation.current = request;
          setNotice('');
          try {
            const found = await draft.returnToSource(annotation.source, request.signal);
            if (request.signal.aborted) return;
            if (found) onClose();
            else setNotice('The source passage is unavailable. Your annotation is retained.');
          } catch {
            if (!request.signal.aborted) setNotice('Could not return to the source passage. Your annotation is retained.');
          }
        }}>Show in conversation <ArrowUpRight size={14} aria-hidden="true" /></button>
      </div>
      <blockquote>{annotation.source.quote}</blockquote>
      <label className="annotation-editor-label" htmlFor="draft-annotation-comment">Your comment</label>
      <textarea id="draft-annotation-comment" aria-label={`Annotation ${annotation.number} comment`} data-selection-dialog-autofocus={!locked || undefined} rows={4} value={annotation.body} disabled={locked} onChange={event => draft.edit(annotation.id, event.target.value)} />
      <p className="annotation-editor-status" role="status">{notice || (annotation.submission !== null ? 'Sending…' : 'Changes are saved to your draft as you type.')}</p>
    </SelectionDialog>
  );
}

export function DraftAnnotationCards({ draft, annotations, disabled }: {
  draft: AnnotationDraft;
  annotations: readonly DraftAnnotation[];
  disabled: boolean;
}) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const previewId = useId();
  const restoreFocus = useRef<HTMLElement | null>(null);
  const section = useRef<HTMLElement | null>(null);
  const selected = annotations.find(annotation => annotation.id === selectedId);
  if (!annotations.length) return null;
  return (
    <section ref={section} className="draft-annotations" aria-label="Annotations">
      <div className="draft-annotation-scroller">
        <div className="draft-annotation-stack">
          {annotations.map(annotation => (
            <button type="button" className="draft-annotation" key={annotation.id} aria-label={`Edit annotation ${annotation.number}`} aria-haspopup="dialog" aria-describedby={`${previewId}-${annotation.number}`} onClick={event => {
              restoreFocus.current = event.currentTarget;
              setSelectedId(annotation.id);
            }}>
              <span className="draft-annotation-number">{annotation.number}</span>
              <span className="draft-annotation-preview" id={`${previewId}-${annotation.number}`}>
                <span className="draft-annotation-source">“{annotation.source.quote}”</span>
                <span className="draft-annotation-comment">{annotation.body || 'No comment'}</span>
              </span>
              {annotation.submission !== null && <span className="draft-annotation-state">Sending…</span>}
              <ChevronRight size={14} aria-hidden="true" />
            </button>
          ))}
        </div>
      </div>
      {selected && <AnnotationEditor key={selected.id} draft={draft} annotation={selected} disabled={disabled} onClose={() => setSelectedId(null)} onDelete={() => {
        const buttons = section.current!.querySelectorAll<HTMLButtonElement>('.draft-annotation');
        const index = annotations.indexOf(selected);
        restoreFocus.current = buttons[index + 1] ?? buttons[index - 1] ?? document.getElementById('message-input');
        draft.remove(selected.id);
        setSelectedId(null);
      }} restoreFocusRef={restoreFocus} />}
    </section>
  );
}
