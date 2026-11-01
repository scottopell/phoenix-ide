import { useEffect, useRef, useState, type ReactNode } from 'react';
import { api, ConflictError, type ContinueHandoffSource, type HandoffCapacityReview, type HandoffCandidate, type HandoffReviewResponse } from '../api';
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
  generatedSource?: Extract<ContinueHandoffSource, { kind: 'summary' }> | null;
  recoveryRevision?: string;
  onRegenerate?: () => void | Promise<void>;
  capacityConflict?: boolean;
  failureMessage?: string;
  onOpenExisting: () => void | Promise<void>;
  onContinue: (handoff: string, source: ContinueHandoffSource) => Promise<ContinueHandoffResult>;
  onCopy: (handoff: string) => void | Promise<void>;
}

interface StoredEditDraft {
  text: string;
}

const storageKey = (parentId: string) => `handoff-edit-draft:${parentId}`;

function loadEditDraft(parentId: string): string | null {
  try {
    const raw = localStorage.getItem(storageKey(parentId));
    if (!raw) return null;
    const stored = JSON.parse(raw) as StoredEditDraft;
    if (typeof stored.text !== 'string') {
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

interface CompletedContinuationBoundaryProps {
  summary: string;
  revealedSummary?: ReactNode;
  revealSummary?: boolean;
  onOpenContinuation?: () => void;
  provenance?: { predecessor: string; successor?: string; messageId: string };
}

export function CompletedContinuationBoundary({
  summary,
  revealedSummary,
  revealSummary = false,
  onOpenContinuation,
  provenance,
}: CompletedContinuationBoundaryProps) {
  return (
    <section className="conversation-history-boundary" aria-label="Conversation continuation boundary">
      <details open={revealSummary || undefined}>
        <summary>Context boundary · Review handoff</summary>
        {provenance && <dl className="conversation-history-boundary-provenance">
          <dt>From</dt><dd>{provenance.predecessor}</dd>
          {provenance.successor && <><dt>To</dt><dd>{provenance.successor}</dd></>}
          <dt>Message</dt><dd>{provenance.messageId}</dd>
        </dl>}
        <pre className="context-exhausted-content" data-fragment-id="message-text">
          {revealedSummary ?? summary}
        </pre>
      </details>
      {onOpenContinuation ? (
        <button type="button" className="context-exhausted-continue" onClick={onOpenContinuation}>
          Open continuation
        </button>
      ) : null}
    </section>
  );
}

function CapacityBreakdown({ capacity }: { capacity: HandoffCapacityReview }) {
  return <div className="handoff-capacity" role="status">
    <p>{capacity.status.kind === 'ready' ? 'Fits with successor working room.'
      : capacity.status.kind === 'required_overhead' ? 'Required instructions and output reserve leave insufficient room. Editing the handoff cannot fix this; select a different model when available.'
        : `Handoff is too large. Reduce it to at most ${capacity.status.target_tokens.toLocaleString()} estimated tokens.`}</p>
    <dl>
      <dt>Context window</dt><dd>{capacity.context_window_tokens.toLocaleString()}</dd>
      <dt>Required instructions and tools (estimate)</dt><dd>{capacity.required_overhead_tokens.toLocaleString()}</dd>
      <dt>Full output reserve</dt><dd>{capacity.output_reservation_tokens.toLocaleString()}</dd>
      <dt>Successor working room</dt><dd>{capacity.successor_working_room_tokens.toLocaleString()}</dd>
      <dt>Handoff (estimate)</dt><dd>{capacity.handoff_tokens.toLocaleString()}</dd>
      {capacity.status.kind !== 'required_overhead' && <><dt>Handoff target (estimate)</dt><dd>{capacity.status.target_tokens.toLocaleString()}</dd></>}
      <dt>Complete input (local estimate)</dt><dd>{(capacity.required_overhead_tokens + capacity.handoff_tokens).toLocaleString()} · instructions/tools + handoff</dd>
      <dt>Provider complete input</dt><dd>{capacity.provider_input_tokens === null ? 'Not available' : capacity.provider_input_tokens.toLocaleString()} · {capacity.counting_method.replaceAll('_', ' ')}</dd>
    </dl>
    <p>Component sizes are local estimates, including when complete input is provider counted. Cached input still uses context.</p>
  </div>;
}

export function ContextExhaustedHandoff({
  parentId, generatedHandoff, generatedSource = null, continuedInConvId,
  disabled = false, onOpenExisting, onContinue, onCopy, onRegenerate,
  capacityConflict = false, recoveryRevision = '', failureMessage,
}: ContextExhaustedHandoffProps) {
  const [mode, setMode] = useState<'reviewing' | 'editing'>('reviewing');
  const [editDraft, setEditDraft] = useState(() => loadEditDraft(parentId));
  const [submitting, setSubmitting] = useState(false);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [draftPersisted, setDraftPersisted] = useState(true);
  const [candidates, setCandidates] = useState<HandoffCandidate[]>([]);
  const [selected, setSelected] = useState<HandoffCandidate | null>(null);
  const [review, setReview] = useState<HandoffReviewResponse | null>(null);
  const [conflictCapacity, setConflictCapacity] = useState<HandoffCapacityReview | null>(null);
  const [reviewing, setReviewing] = useState(false);
  const epoch = useRef(0);
  const identity = `${parentId}:${recoveryRevision}:${generatedHandoff}:${JSON.stringify(generatedSource)}`;
  const identityRef = useRef(identity);
  // Invalidate synchronously during render so a response cannot land between navigation and effects.
  if (identityRef.current !== identity) {
    identityRef.current = identity;
    epoch.current += 1;
  }
  const sourceRef = useRef(identity);
  useEffect(() => {
    if (sourceRef.current === identity) return;
    sourceRef.current = identity;
    setMode('reviewing');
    setEditDraft(loadEditDraft(parentId));
    setSelected(null); setCandidates([]); setReview(null); setConflictCapacity(null);
    setFeedback(null); setDraftPersisted(true); setReviewing(false); setSubmitting(false);
  }, [identity, parentId, generatedHandoff]);
  useEffect(() => () => { epoch.current += 1; }, []);
  useEffect(() => {
    let active = true;
    void api.handoffCandidates(parentId).then((rows) => { if (active) setCandidates(rows); })
      .catch((error: unknown) => { if (active && capacityConflict) setFeedback(error instanceof Error ? error.message : 'Could not load retained handoffs.'); });
    return () => { active = false; };
  }, [parentId, recoveryRevision, capacityConflict]);
  const sourceText = selected?.text ?? generatedHandoff;
  const source: ContinueHandoffSource | null = selected
    ? selected.origin.kind === 'review_input' ? { kind: 'manual' } : { kind: 'candidate', candidate_id: selected.id }
    : generatedSource;
  const editorValue = editDraft ?? sourceText;
  const text = mode === 'editing' ? editorValue : sourceText;
  const activeSource: ContinueHandoffSource | null = mode === 'editing' ? { kind: 'manual' } : source;
  const capacity = conflictCapacity ?? review?.source;
  const sourceUnavailable = activeSource === null;
  const pending = disabled || submitting;
  const invalidate = () => { epoch.current += 1; setReview(null); setConflictCapacity(null); setReviewing(false); };
  const updateDraft = (value: string) => {
    invalidate(); setEditDraft(value);
    const persisted = saveEditDraft(parentId, generatedHandoff, value);
    setDraftPersisted(persisted);
    setFeedback(persisted ? null : 'Your edit is retained in this tab only; browser storage is unavailable.');
  };
  const check = async (shorten: boolean) => {
    if (activeSource === null) { setFeedback('The generated summary source is not loaded. Wait for history or choose Edit first to submit a manual handoff.'); return; }
    if (!text.trim()) { setFeedback('The handoff cannot be empty.'); return; }
    const version = ++epoch.current;
    setReviewing(true); setFeedback(null); setReview(null); setConflictCapacity(null);
    try {
      const result = await api.reviewHandoff(parentId, { handoff: text, source: activeSource, shorten });
      if (epoch.current !== version) return;
      setReview(result);
      if (result.shortening.kind === 'failed') setFeedback(result.shortening.error);
      // Reload history after retaining a source or generating a candidate.
      void api.handoffCandidates(parentId).then((rows) => { if (epoch.current === version) setCandidates(rows); }).catch(() => {});
    } catch (error) {
      if (epoch.current !== version) return;
      if (error instanceof ConflictError && error.detail.capacity) setConflictCapacity(error.detail.capacity);
      setFeedback(error instanceof Error ? error.message : 'Handoff review failed.');
    } finally { if (epoch.current === version) setReviewing(false); }
  };
  const submit = async (handoff: string, selectedSource: ContinueHandoffSource) => {
    if (!handoff.trim()) { setFeedback('The handoff cannot be empty.'); return; }
    const version = ++epoch.current;
    setSubmitting(true); setFeedback(null);
    try {
      const result = await onContinue(handoff, selectedSource);
      if (epoch.current === version && result === 'accepted') clearEditDraft(parentId);
    } catch (error) {
      if (epoch.current !== version) return;
      if (error instanceof ConflictError && error.detail.capacity) setConflictCapacity(error.detail.capacity);
      setFeedback(error instanceof Error ? error.message : 'Failed to continue conversation.');
    } finally { if (epoch.current === version) setSubmitting(false); }
  };
  const candidate = review?.shortening.kind === 'generated' ? review.shortening : null;
  const acceptanceBlocked = capacity?.status.kind !== undefined && capacity.status.kind !== 'ready';
  // Historical sources must pass a fresh authoritative review before acceptance.
  const needsReview = (capacityConflict || selected !== null) && review?.source.status.kind !== 'ready';
  return <section className="context-exhausted-handoff context-exhausted-banner context-exhausted-banner--expanded" aria-label="Continuation handoff recovery">
    <div className="context-exhausted-header context-exhausted-header--static">
      <span className="context-exhausted-icon"><AlertTriangle /></span>
      <span className="context-exhausted-title">{capacityConflict ? 'Continuation capacity conflict' : 'Context Window Full'}</span>
      <span className="context-exhausted-subtitle">{continuedInConvId ? 'This conversation has been continued' : 'Review a handoff for a fresh conversation'}</span>
    </div>
    <div className="context-exhausted-summary">
      {continuedInConvId ? <><button type="button" className="context-exhausted-continue" data-testid="continuation-link" disabled={pending} onClick={() => void onOpenExisting()}>Open continuation</button><pre className="context-exhausted-content">{generatedHandoff}</pre></> : <>
        {failureMessage && <p role="alert">{failureMessage}</p>}
        {capacityConflict && <p>Automatic progress stopped. Review retained text or edit a handoff, check its fit, or request one AI shortening candidate. You can also select a different model using the conversation model selector.</p>}
        {candidates.length > 0 && <label className="context-exhausted-handoff-label">Retained handoff source
          <select value={selected?.id ?? ''} disabled={pending} onChange={(event) => {
            invalidate(); setSelected(candidates.find((row) => row.id === event.target.value) ?? null); setMode('reviewing'); setFeedback(null);
          }}><option value="">{generatedHandoff ? 'Original generated handoff' : 'New manual handoff'}</option>
            {candidates.map((row, index) => <option key={row.id} value={row.id}>{row.origin.kind === 'review_input' ? 'Retained input' : row.origin.kind === 'oversized_summary' ? 'Generated summary' : 'AI shortening'} {index + 1} · {row.id}</option>)}
          </select>
        </label>}
        {mode === 'editing' ? <label className="context-exhausted-handoff-label" htmlFor={`context-exhausted-handoff-${parentId}`}>Edit handoff
          <textarea id={`context-exhausted-handoff-${parentId}`} data-testid="context-exhausted-handoff" className="context-exhausted-handoff-field" value={editorValue} onChange={(event) => updateDraft(event.target.value)} disabled={pending} autoFocus />
        </label> : <pre className="context-exhausted-content">{sourceText || 'No generated summary is ready. Choose a retained source or write a manual handoff.'}</pre>}
        {sourceUnavailable && generatedHandoff && <p role="status">The generated summary source is not loaded. Wait for history or choose Edit first to submit a manual handoff.</p>}
        {feedback && <p className="context-exhausted-handoff-feedback" role="alert">{feedback}</p>}
        {capacity && <CapacityBreakdown capacity={capacity} />}
        <div className="context-exhausted-actions">
          <button type="button" className="context-exhausted-continue" data-testid="continue-button" disabled={pending || reviewing || acceptanceBlocked || needsReview || (mode === 'reviewing' && !text.trim()) || (mode === 'reviewing' && !selected && generatedSource === null)} onClick={() => { if (activeSource) void submit(text, activeSource); }}>{submitting ? 'Continuing…' : mode === 'editing' ? 'Continue with edits' : selected ? 'Accept selected handoff' : 'Continue'}</button>
          {mode === 'reviewing' ? <button type="button" className="context-exhausted-copy" disabled={pending} onClick={() => { invalidate(); setMode('editing'); }}>Edit first</button> : <>
            <button type="button" className="context-exhausted-copy" disabled={submitting} onClick={() => void onCopy(editorValue)}>Copy edited handoff</button>
            <button type="button" className="context-exhausted-copy" disabled={pending || editDraft === null} onClick={() => {
              if (window.confirm('Discard your local edits and restore the generated handoff?')) { invalidate(); clearEditDraft(parentId); setEditDraft(null); setSelected(null); setFeedback(null); setDraftPersisted(true); }
            }}>Revert to generated</button>
            <button type="button" className="context-exhausted-copy" disabled={submitting} onClick={() => { invalidate(); setMode('reviewing'); }}>Cancel editing</button>
          </>}
          {selected && <button type="button" className="context-exhausted-copy" disabled={submitting} onClick={() => void onCopy(selected.text)}>Copy selected source</button>}
          <button type="button" className="context-exhausted-copy" disabled={submitting} onClick={() => void onCopy(generatedHandoff || sourceText)}>Copy {generatedHandoff ? 'handoff' : 'source'}</button>
          <button type="button" className="context-exhausted-copy" disabled={pending || reviewing || sourceUnavailable || !text.trim()} onClick={() => void check(false)}>{reviewing ? 'Reviewing…' : 'Check fit'}</button>
          <button type="button" className="context-exhausted-copy" disabled={pending || reviewing || sourceUnavailable || !text.trim() || capacity?.status.kind === 'required_overhead'} onClick={() => void check(true)}>Request AI shortening</button>
          {onRegenerate && !capacityConflict && <button type="button" className="context-exhausted-copy" disabled={pending || reviewing} onClick={() => { invalidate(); void onRegenerate(); }}>Regenerate handoff</button>}
          {editDraft !== null && <span className="context-exhausted-draft-note">{draftPersisted ? 'Local edit saved' : 'Edit retained in this tab only'}</span>}
        </div>
        {candidate && <section className="handoff-candidate-preview" aria-label="AI shortening preview">
          <h3>Review AI shortening · {candidate.candidate_id}</h3><p>The original remains saved. This candidate has not been accepted.</p>
          <pre className="context-exhausted-content">{candidate.text}</pre><CapacityBreakdown capacity={candidate.capacity} />
          <button type="button" className="context-exhausted-continue" disabled={pending || candidate.capacity.status.kind !== 'ready'} onClick={() => void submit(candidate.text, { kind: 'candidate', candidate_id: candidate.candidate_id })}>Accept AI shortening and continue</button>
          <button type="button" className="context-exhausted-copy" onClick={() => void onCopy(candidate.text)}>Copy candidate</button>
          <button type="button" className="context-exhausted-copy" disabled={pending} onClick={() => { invalidate(); }}>Decline candidate</button>
        </section>}
      </>}
    </div>
  </section>;
}
