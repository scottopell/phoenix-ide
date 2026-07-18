import { useEffect, useState } from 'react';
import './ContextExhaustedHandoff.css';

const AlertTriangle = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
    <line x1="12" y1="9" x2="12" y2="13" />
    <line x1="12" y1="17" x2="12.01" y2="17" />
  </svg>
);

export type ContinueHandoffResult = 'accepted' | 'dispatch_failed' | 'already_exists';

export interface ContextExhaustedHandoffProps {
  parentId: string;
  generatedHandoff: string;
  continuedInConvId: string | null | undefined;
  disabled?: boolean;
  onOpenExisting: () => void | Promise<void>;
  onContinue: (handoff: string) => Promise<ContinueHandoffResult>;
  onCopy: (handoff: string) => void | Promise<void>;
}

interface StoredEditDraft {
  sourceFingerprint: string;
  text: string;
}

const storageKey = (parentId: string) => `handoff-edit-draft:${parentId}`;

function fingerprint(value: string): string {
  let hash = 2166136261;
  for (let i = 0; i < value.length; i += 1) {
    hash ^= value.charCodeAt(i);
    hash = Math.imul(hash, 16777619);
  }
  return `${value.length}:${hash >>> 0}`;
}

function loadEditDraft(parentId: string, generatedHandoff: string): string | null {
  try {
    const raw = localStorage.getItem(storageKey(parentId));
    if (!raw) return null;
    const stored = JSON.parse(raw) as StoredEditDraft;
    if (stored.sourceFingerprint !== fingerprint(generatedHandoff) || typeof stored.text !== 'string') {
      localStorage.removeItem(storageKey(parentId));
      return null;
    }
    return stored.text;
  } catch {
    return null;
  }
}

function saveEditDraft(parentId: string, generatedHandoff: string, text: string): boolean {
  try {
    if (text === generatedHandoff) {
      localStorage.removeItem(storageKey(parentId));
    } else {
      const stored: StoredEditDraft = {
        sourceFingerprint: fingerprint(generatedHandoff),
        text,
      };
      localStorage.setItem(storageKey(parentId), JSON.stringify(stored));
    }
    return true;
  } catch {
    return false;
  }
}

function clearEditDraft(parentId: string): boolean {
  try {
    localStorage.removeItem(storageKey(parentId));
    return true;
  } catch {
    return false;
  }
}

export function ContextExhaustedHandoff({
  parentId,
  generatedHandoff,
  continuedInConvId,
  disabled = false,
  onOpenExisting,
  onContinue,
  onCopy,
}: ContextExhaustedHandoffProps) {
  const [mode, setMode] = useState<'reviewing' | 'editing'>('reviewing');
  const [editDraft, setEditDraft] = useState(() => loadEditDraft(parentId, generatedHandoff));
  const [submitting, setSubmitting] = useState(false);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [draftPersisted, setDraftPersisted] = useState(true);
  const hasContinuation = continuedInConvId != null;
  const editorValue = editDraft ?? generatedHandoff;

  useEffect(() => {
    setMode('reviewing');
    setEditDraft(loadEditDraft(parentId, generatedHandoff));
    setFeedback(null);
    setDraftPersisted(true);
  }, [parentId, generatedHandoff]);

  const updateDraft = (text: string) => {
    setEditDraft(text === generatedHandoff ? null : text);
    const persisted = saveEditDraft(parentId, generatedHandoff, text);
    setDraftPersisted(persisted);
    setFeedback(
      persisted ? null : 'Your edit is retained in this tab only; browser storage is unavailable.',
    );
  };

  const submit = async (handoff: string) => {
    if (!handoff.trim()) {
      setFeedback('The handoff cannot be empty.');
      return;
    }
    setSubmitting(true);
    setFeedback(null);
    try {
      const result = await onContinue(handoff);
      if (result === 'accepted') {
        clearEditDraft(parentId);
      }
    } catch (error) {
      setFeedback(error instanceof Error ? error.message : 'Failed to continue conversation.');
    } finally {
      setSubmitting(false);
    }
  };

  if (hasContinuation) {
    return (
      <section className="context-exhausted-banner" aria-labelledby="context-exhausted-title">
        <div className="context-exhausted-header context-exhausted-header--static">
          <span className="context-exhausted-icon"><AlertTriangle /></span>
          <span id="context-exhausted-title" className="context-exhausted-title">Context Window Full</span>
          <span className="context-exhausted-subtitle">This conversation has been continued</span>
        </div>
        <div className="context-exhausted-summary">
          <button type="button" className="context-exhausted-continue" data-testid="continuation-link" disabled={disabled || submitting} onClick={() => void onOpenExisting()}>
            Open continuation
          </button>
          <pre className="context-exhausted-content">{generatedHandoff}</pre>
        </div>
      </section>
    );
  }

  return (
    <section className="context-exhausted-banner context-exhausted-banner--expanded" aria-labelledby="context-exhausted-title">
      <div className="context-exhausted-header context-exhausted-header--static">
        <span className="context-exhausted-icon"><AlertTriangle /></span>
        <span id="context-exhausted-title" className="context-exhausted-title">Context Window Full</span>
        <span className="context-exhausted-subtitle">Choose how to hand off progress to a fresh conversation</span>
      </div>
      <div className="context-exhausted-summary">
        {mode === 'reviewing' ? (
          <>
            <div className="context-exhausted-actions">
              <button type="button" className="context-exhausted-continue" data-testid="continue-button" disabled={disabled || submitting} onClick={() => void submit(generatedHandoff)}>
                {submitting ? 'Continuing…' : 'Continue'}
              </button>
              <button type="button" className="context-exhausted-copy" disabled={disabled || submitting} onClick={() => setMode('editing')}>Edit first</button>
              <button type="button" className="context-exhausted-copy" disabled={submitting} onClick={() => void onCopy(generatedHandoff)}>Copy handoff</button>
              {editDraft !== null && (
                <span className="context-exhausted-draft-note">
                  {draftPersisted ? 'Local edit saved' : 'Edit retained in this tab only'}
                </span>
              )}
            </div>
            {feedback && <p className="context-exhausted-handoff-feedback" role="alert">{feedback}</p>}
            <pre className="context-exhausted-content">{generatedHandoff}</pre>
          </>
        ) : (
          <>
            <label className="context-exhausted-handoff-label" htmlFor={`context-exhausted-handoff-${parentId}`}>
              Edit handoff
              <textarea
                id={`context-exhausted-handoff-${parentId}`}
                data-testid="context-exhausted-handoff"
                className="context-exhausted-handoff-field"
                value={editorValue}
                onChange={(event) => updateDraft(event.target.value)}
                disabled={disabled || submitting}
                autoFocus
              />
            </label>
            {feedback && <p className="context-exhausted-handoff-feedback" role="alert">{feedback}</p>}
            <div className="context-exhausted-actions">
              <button type="button" className="context-exhausted-continue" disabled={disabled || submitting} onClick={() => void submit(editorValue)}>
                {submitting ? 'Continuing…' : 'Continue with edits'}
              </button>
              <button type="button" className="context-exhausted-copy" disabled={submitting} onClick={() => void onCopy(editorValue)}>Copy edited handoff</button>
              <button
                type="button"
                className="context-exhausted-copy"
                disabled={submitting || editDraft === null}
                onClick={() => {
                  if (editDraft !== null && window.confirm('Discard your local edits and restore the generated handoff?')) {
                    clearEditDraft(parentId);
                    setEditDraft(null);
                    setFeedback(null);
                    setDraftPersisted(true);
                  }
                }}
              >
                Revert to generated
              </button>
              <button type="button" className="context-exhausted-copy" disabled={submitting} onClick={() => setMode('reviewing')}>Cancel editing</button>
            </div>
          </>
        )}
      </div>
    </section>
  );
}
