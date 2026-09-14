import { useState, useEffect, useLayoutEffect, useRef, useId, type KeyboardEvent, type CSSProperties, type ReactNode } from 'react';
import ReactMarkdown from 'react-markdown';
import { api, QuestionMutationError, type UserQuestion, type ConversationState } from '../api';
import { useRegisterFocusScope, useFocusScope } from '../hooks/useFocusScope';
import { formatShortcut, parseConversationState } from '../utils';
import { createQuestionDraft, choose, selected, isAnswered, answerPayload, type QuestionDraft } from './questionDraft';
import './QuestionPanel.css';

export interface QuestionPanelProps {
  questions: UserQuestion[];
  conversationId: string;
  requestId: string;
  showToast: (message: string, duration?: number) => void;
  onResolved: (state: ConversationState) => void;
  readOnly?: boolean;
}
type Operation = { kind: 'answer'; payload: ReturnType<typeof answerPayload> } | { kind: 'dismiss' };
type Submission = { kind: 'editing' } | { kind: 'sending'; operation: Operation }
  | { kind: 'uncertain'; operation: Operation; checked: boolean; message: string }
  | { kind: 'resolved'; operation: Operation; message: string };
const previewComponents = {
  pre: ({children}: {children?: ReactNode}) => <pre tabIndex={0} role="region" aria-label="Preview code" onKeyDown={event => {
    if (event.altKey || event.ctrlKey || event.metaKey || event.currentTarget.scrollWidth <= event.currentTarget.clientWidth) return;
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault(); event.stopPropagation();
    const element = event.currentTarget;
    if (event.key === 'Home' || event.key === 'End') element.scrollTo({left: event.key === 'Home' ? 0 : element.scrollWidth});
    else element.scrollBy({left: event.key === 'ArrowRight' ? 40 : -40});
  }}>{children}</pre>,
};

export function QuestionPanel(props: QuestionPanelProps) {
  if (props.readOnly) return <section className="question-panel question-panel--readonly" aria-label="Questions (read only)">
    {props.questions.map(question => <section key={question.question}><h3>{question.header}</h3>
      <ReactMarkdown>{question.question}</ReactMarkdown><ul>{question.options.map(option => <li key={option.label}>
        <strong>{option.label}</strong>{option.description && <p>{option.description}</p>}{option.preview && !question.multiSelect && <pre>{option.preview}</pre>}
      </li>)}</ul></section>)}
  </section>;
  return <ActiveQuestionPanel key={`${props.conversationId}:${props.requestId}`} {...props} />;
}

function ActiveQuestionPanel({ questions, conversationId, requestId, showToast, onResolved }: QuestionPanelProps) {
  useRegisterFocusScope('question-panel');
  const { activeScope } = useFocusScope();
  const id = useId();
  const root = useRef<HTMLElement>(null);
  const body = useRef<HTMLDivElement>(null);
  const previewRef = useRef<HTMLElement>(null);
  const otherChoiceRef = useRef<HTMLInputElement>(null);
  const previewTextRef = useRef<HTMLDivElement>(null);
  const [previewOverflow, setPreviewOverflow] = useState(false);
  const otherRef = useRef<HTMLTextAreaElement>(null);
  const notesRef = useRef<HTMLTextAreaElement>(null);
  const notesButton = useRef<HTMLButtonElement>(null);
  const dismissButton = useRef<HTMLButtonElement>(null);
  const mounted = useRef(true);
  const inFlight = useRef(false);
  const focusIntent = useRef<'notes' | 'other' | null>(null);
  const uncertain = useRef(false);
  const [drafts, setDrafts] = useState(() => questions.map(createQuestionDraft));
  const [step, setStep] = useState(0);
  const [submission, setSubmission] = useState<Submission>({ kind: 'editing' });
  const [error, setError] = useState('');
  const [confirmDismiss, setConfirmDismiss] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [bounds, setBounds] = useState({ width: 1000, height: 700, previewFits: true });
  const question = questions[step];
  const draft = drafts[step];
  const locked = submission.kind !== 'editing';
  const allAnswered = questions.every((q, i) => isAnswered(q, drafts[i]!));
  const last = step === questions.length - 1;
  const update = (change: (value: QuestionDraft) => QuestionDraft) => {
    if (locked) return;
    setDrafts(values => values.map((value, i) => i === step ? change(value) : value));
    setError('');
  };
  const focusChoice = () => {
    const target = root.current?.querySelector<HTMLInputElement>('input:checked') ?? root.current?.querySelector<HTMLInputElement>('input[type="radio"], input[type="checkbox"]');
    target?.focus();
    target?.scrollIntoView?.({ block: 'nearest' });
  };
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useLayoutEffect(focusChoice, [step]);
  useLayoutEffect(() => {
    if (focusIntent.current === 'notes') notesRef.current?.focus();
    if (focusIntent.current === 'other') otherRef.current?.focus();
    focusIntent.current = null;
  });
  useEffect(() => {
    const panel = root.current;
    const container = panel?.closest('.product-conversation-page') ?? panel?.closest('.conversation-column') ?? panel?.parentElement;
    if (!panel || !container) return;
    const measure = () => {
      const viewport = window.visualViewport;
      const rect = container.getBoundingClientRect();
      const viewportTop = viewport?.offsetTop ?? 0;
      let chrome = 0;
      let child: Element = panel;
      while (child !== container && child.parentElement) {
        for (const sibling of child.parentElement.children) {
          if (sibling === child || sibling.matches('.view, .question-fixture-transcript')) continue;
          const style = getComputedStyle(sibling);
          if (style.position === 'absolute' || style.position === 'fixed') continue;
          chrome += sibling.getBoundingClientRect().height;
        }
        child = child.parentElement;
      }
      const height = Math.max(120, Math.min(rect.bottom, viewportTop + (viewport?.height ?? window.innerHeight)) - Math.max(rect.top, viewportTop) - chrome);
      setBounds({ width: panel.clientWidth, height,
        previewFits: (previewRef.current?.offsetHeight ?? 0) < (body.current?.clientHeight ?? height) - 16 });
    };
    const observer = new ResizeObserver(measure);
    const observeChrome = () => {
      let child: Element = panel;
      while (child !== container && child.parentElement) {
        for (const sibling of child.parentElement.children) {
          if (sibling !== child && !sibling.matches('.view, .question-fixture-transcript')) observer.observe(sibling);
        }
        child = child.parentElement;
      }
    };
    observer.observe(container); observer.observe(panel); observeChrome();
    const mutations = new MutationObserver(() => { observeChrome(); measure(); });
    mutations.observe(container, { childList: true, subtree: true });
    if (previewRef.current) observer.observe(previewRef.current);
    window.visualViewport?.addEventListener('resize', measure);
    window.visualViewport?.addEventListener('scroll', measure);
    measure();
    return () => { observer.disconnect(); mutations.disconnect(); window.visualViewport?.removeEventListener('resize', measure); window.visualViewport?.removeEventListener('scroll', measure); };
  }, [step]);

  useLayoutEffect(() => {
    const active = document.activeElement;
    if (active instanceof HTMLElement && root.current?.contains(active)) active.scrollIntoView?.({ block: 'nearest' });
  }, [bounds.width, bounds.height]);

  const reconcile = async (operation: Operation, knowledge: 'unknown' | 'consumed' = 'unknown') => {
    if (!mounted.current || inFlight.current) return;
    inFlight.current = true;
    try {
      const result = await api.getConversation(conversationId);
      if (!mounted.current) return;
      const rawState = result.conversation.state;
      const state = parseConversationState(rawState);
      if (!rawState || rawState.type !== state.type) throw new Error('Question status unavailable');
      if (state.type !== 'awaiting_user_response' || state.request_id !== requestId) {
        if (knowledge === 'unknown') showToast('This question is no longer awaiting an answer');
        onResolved(state);
      } else if (knowledge === 'consumed') setSubmission({ kind: 'resolved', operation, message: 'This question has closed. Waiting for updated conversation status.' });
      else setSubmission({ kind: 'uncertain', operation, checked: true,
        message: operation.kind === 'answer' ? 'Your original answer may still be processing. Retry sends the same answer.' : 'The dismissal may still be processing. Retry dismisses the same question.' });
    } catch {
      if (mounted.current) setSubmission(knowledge === 'consumed'
        ? { kind: 'resolved', operation, message: 'This question has closed, but the conversation status could not be refreshed. Check status again.' }
        : { kind: 'uncertain', operation, checked: false, message: 'Could not check question status. Your original response remains unchanged.' });
    } finally { inFlight.current = false; }
  };
  const perform = async (operation: Operation) => {
    if (inFlight.current || !mounted.current) return;
    inFlight.current = true;
    setSubmission({ kind: 'sending', operation }); setError('');
    try {
      if (operation.kind === 'answer') await api.respondToQuestion(conversationId, requestId, operation.payload.answers, operation.payload.annotations);
      else await api.dismissQuestion(conversationId, requestId);
      if (!mounted.current) return;
      showToast(operation.kind === 'answer' ? 'Answers sent' : 'Questions dismissed. Send a message to continue.');
      setSubmission({ kind: 'resolved', operation, message: 'Updating conversation status…' });
      inFlight.current = false;
      await reconcile(operation, 'consumed');
    } catch (err) {
      if (!mounted.current) return;
      if (err instanceof QuestionMutationError && err.code === 'question_request_stale') {
        showToast('This question is no longer awaiting an answer');
        setSubmission({ kind: 'resolved', operation, message: 'Updating conversation status…' });
        inFlight.current = false;
        await reconcile(operation, 'consumed');
      } else if (err instanceof QuestionMutationError && err.code === 'question_request_invalid' && !uncertain.current) {
        setSubmission({ kind: 'editing' }); setError(err.message);
      } else {
        uncertain.current = true;
        setSubmission({ kind: 'uncertain', operation, checked: false, message: 'Could not confirm the response. Checking status…' });
        inFlight.current = false;
        await reconcile(operation);
      }
    } finally { inFlight.current = false; }
  };
  const send = () => {
    if (locked) return;
    if (!allAnswered) {
      const missing = questions.findIndex((q, i) => !isAnswered(q, drafts[i]!));
      setStep(missing); setError(`Answer ${questions[missing]!.header} before sending.`);
      if (missing === step) focusChoice();
      return;
    }
    void perform({ kind: 'answer', payload: answerPayload(questions, drafts) });
  };
  const openNotes = () => {
    if (locked) return;
    focusIntent.current = 'notes';
    update(value => ({ ...value, notesOpen: true }));
  };
  const keyboard = (event: KeyboardEvent<HTMLElement>) => {
    if (confirmDismiss || (activeScope !== null && activeScope !== 'question-panel')) return;
    if (event.nativeEvent.isComposing) return;
    const editor = event.target instanceof HTMLTextAreaElement || event.target instanceof HTMLInputElement && !['radio', 'checkbox'].includes(event.target.type);
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
      event.preventDefault(); event.stopPropagation(); send(); return;
    }
    if (event.key === 'Escape') {
      event.preventDefault(); event.stopPropagation();
      if (editor) {
        if (event.target === notesRef.current) notesButton.current?.focus(); else otherChoiceRef.current?.focus();
      } else if (!locked) {
        if (draft?.notesOpen) { update(value => ({ ...value, notesOpen: false })); notesButton.current?.focus(); }
        else if (draft?.previewOpen) update(value => ({ ...value, previewOpen: false }));
        else setConfirmDismiss(true);
      }
      return;
    }
    if (event.key === 'n' && !editor && !event.ctrlKey && !event.metaKey && !event.altKey) { event.preventDefault(); event.stopPropagation(); openNotes(); return; }
    if (['Tab', 'ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight', ' ', 'Enter'].includes(event.key)) event.stopPropagation();
  };
  useEffect(() => {
    const text = previewTextRef.current;
    if (!text) { setPreviewOverflow(false); return; }
    const measure = () => setPreviewOverflow(text.scrollHeight > 8 * 21 + 1);
    const observer = new ResizeObserver(measure);
    observer.observe(text); measure();
    return () => observer.disconnect();
  }, [step, draft]);
  if (!question || !draft) return null;
  const otherIndex = question.options.length;
  const otherSelected = selected(draft, otherIndex);
  const previewMode = !question.multiSelect && question.options.some(option => option.preview);
  const choice = draft.selection.kind === 'single' && draft.selection.value !== null ? question.options[draft.selection.value] : undefined;
  const preview = choice?.preview;

  const hasDraft = drafts.some(value => value.other || value.notes || (value.selection.kind === 'single' ? value.selection.value !== null : value.selection.values.length > 0));
  const narrow = bounds.width < 840;
  const short = bounds.height < 360;
  const expand = expanded || bounds.width < 480;
  const onOtherEntry = () => {
    if (otherRef.current) otherRef.current.focus();
    else focusIntent.current = 'other';
  };
  const idFor = (suffix: string) => `${id}-${step}-${suffix}`;

  if (submission.kind === 'resolved') return <section className="question-panel" aria-label="Question response status">
    <p role="status">{submission.message}</p>
    <button type="button" onClick={() => void reconcile(submission.operation, 'consumed')}>Check status again</button>
  </section>;

  return <section ref={root} className={`question-panel${expand ? ' question-panel--expanded' : ''}${short ? ' question-panel--short' : ''}`}
    style={{ '--question-available-height': `${bounds.height}px` } as CSSProperties}
    aria-label="Answer agent questions" onKeyDown={keyboard} onFocus={event => {
      if (event.target instanceof HTMLElement) event.target.scrollIntoView?.({ block: 'nearest' });
    }}>
    <header className="question-context"><strong>{questions.length > 1 ? `Question ${step + 1} of ${questions.length} · ` : ''}{question.header}</strong>
      {bounds.width >= 480 && <button type="button" disabled={locked} onClick={() => setExpanded(value => !value)}>{expanded ? 'Restore conversation' : 'Expand questions'}</button>}
      {questions.length > 1 && !short && <nav aria-label="Questions">{questions.map((q, i) => <button type="button" key={q.question} disabled={locked}
        aria-current={step === i ? 'step' : undefined} aria-label={`${q.header}, ${isAnswered(q, drafts[i]!) ? 'answered' : 'unanswered'}`} onClick={() => setStep(i)}>
        {isAnswered(q, drafts[i]!) ? '✓ ' : ''}{q.header}</button>)}</nav>}
    </header>
    <div className="question-scroll" ref={body}>
      <div className="question-content">
        <div className="question-text" id={idFor('question')}><ReactMarkdown>{question.question}</ReactMarkdown></div>
        <p className="question-instruction">{question.multiSelect ? 'Choose one or more.' : 'Choose one.'} Answers are sent only when you send the completed form.</p>
        <div className={previewMode ? 'question-preview-layout' : 'question-standard-layout'}>
          <fieldset disabled={locked} aria-labelledby={idFor('question')}><legend className="sr-only">{question.header}</legend>
            {question.options.map((option, i) => <div className="question-choice-block" key={option.label}>
              <label className={`question-option${selected(draft, i) ? ' selected' : ''}`}>
                <input type={question.multiSelect ? 'checkbox' : 'radio'} name={idFor('choice')} checked={selected(draft, i)}
                  aria-labelledby={idFor(`label-${i}`)} aria-describedby={option.description ? idFor(`description-${i}`) : undefined}
                  onChange={event => update(value => choose(value, i, event.target.checked))} />
                <span className="question-option-content"><span id={idFor(`label-${i}`)} className="question-option-label">{option.label}</span>
                  {option.description && <span id={idFor(`description-${i}`)} className="question-option-description">{option.description}</span>}</span>
              </label>

            </div>)}
            <label className={`question-option${otherSelected ? ' selected' : ''}`} onClick={event => {
              if (event.detail > 0) {
                if (question.multiSelect && otherSelected) otherChoiceRef.current?.focus();
                else onOtherEntry();
              }
            }}>
              <input type={question.multiSelect ? 'checkbox' : 'radio'} ref={otherChoiceRef} name={idFor('choice')} checked={otherSelected}
                onChange={event => update(value => choose(value, otherIndex, event.target.checked))} />
              <span className="question-option-label">Other</span>
            </label>
            {otherSelected ? <label className="question-editor">Custom answer<textarea ref={otherRef} value={draft.other} rows={3}
              aria-invalid={!draft.other.trim()} aria-describedby={!draft.other.trim() ? idFor('other-help') : undefined}
              onChange={event => update(value => ({ ...choose(value, otherIndex, true), other: event.target.value }))} /></label> : draft.other && <p className="question-draft-note">
                {question.multiSelect ? 'Custom answer not included.' : 'Saved custom draft — not included.'} <button type="button" onClick={() => { update(value => choose(value, otherIndex)); onOtherEntry(); }}>Edit custom answer</button></p>}
            {otherSelected && !draft.other.trim() && <p id={idFor('other-help')}>Write a custom answer to include Other.</p>}
            {narrow && previewMode && <button className="question-preview-link" type="button" disabled={!choice && !otherSelected} onClick={() => {
              previewRef.current?.querySelector<HTMLElement>('h3')?.focus(); previewRef.current?.scrollIntoView?.({ block: 'nearest' });
            }}>Preview below</button>}
          </fieldset>
          {previewMode && <section ref={previewRef} className={`question-preview-pane${bounds.previewFits ? ' question-preview-pane--sticky' : ''}`} aria-label="Selected option preview">
            <h3 tabIndex={-1}>Preview{choice ? ` — ${choice.label}` : otherSelected ? ' — Other' : ''}</h3>
            {preview ? <><div ref={previewTextRef} className={`question-preview-text${draft.previewOpen ? '' : ' question-preview-text--collapsed'}`}><ReactMarkdown components={previewComponents}>{preview}</ReactMarkdown></div>
              {previewOverflow && <button type="button" disabled={locked} onClick={() => update(value => ({ ...value, previewOpen: !value.previewOpen }))} aria-expanded={draft.previewOpen}>{draft.previewOpen ? 'Show less' : 'Show full preview'}</button>}</>
              : <p>{otherSelected ? 'Your custom answer will be sent.' : choice ? 'No preview for this option.' : 'Choose an option to view its preview.'}</p>}
          </section>}
        </div>
        <div className="question-notes"><button ref={notesButton} type="button" disabled={locked} aria-expanded={draft.notesOpen} aria-controls={idFor('notes')}
          title={`Notes (${formatShortcut('n')})`} onClick={() => { if (draft.notesOpen) update(value => ({ ...value, notesOpen: false })); else openNotes(); }}>
          {draft.notes ? 'Edit notes · included' : 'Add notes (optional)'}</button>
          {draft.notesOpen && <label className="question-editor" id={idFor('notes')}>Notes for the agent<textarea ref={notesRef} disabled={locked} rows={3} value={draft.notes} onChange={event => update(value => ({ ...value, notes: event.target.value }))} /></label>}
        </div>
        {hasDraft && <p className="question-draft-note">Unsent answers stay here while you answer. Refreshing or leaving this conversation may discard it.</p>}
        {error && <p role="alert" className="question-feedback--error">{error}</p>}
        <p className="question-status" role="status">{submission.kind === 'sending' ? (submission.operation.kind === 'answer' ? 'Sending…' : 'Dismissing…') : submission.kind === 'uncertain' ? submission.message : ''}</p>
      </div>
      {short && actions()}
    </div>
    {!short && actions()}
    {confirmDismiss && <DismissDialog onCancel={() => { setConfirmDismiss(false); dismissButton.current?.focus(); }} onConfirm={() => { setConfirmDismiss(false); void perform({ kind: 'dismiss' }); }} />}
  </section>;

  function actions() {
    return <footer className="question-actions">
      <button ref={dismissButton} className="question-dismiss" type="button" disabled={locked} onClick={() => setConfirmDismiss(true)}>Use chat instead</button>
      {submission.kind === 'uncertain' ? <div className="question-actions-right">
        <button type="button" onClick={() => void reconcile(submission.operation)}>Check status again</button>
        {submission.checked && <button type="button" className="question-primary" onClick={() => void perform(submission.operation)}>{submission.operation.kind === 'answer' ? 'Retry same answer' : 'Retry dismissal'}</button>}
      </div> : <div className="question-actions-right">
        {!last && !isAnswered(question!, draft!) && <span>Choose an answer to continue.</span>}
        {last && !allAnswered && <span>Still needed: {questions.filter((q, i) => !isAnswered(q, drafts[i]!)).map(q => q.header).join(', ')}</span>}
        {step > 0 && <button type="button" disabled={locked} onClick={() => setStep(value => value - 1)}>Back</button>}
        {last ? <button type="button" className="question-primary" title={formatShortcut('Ctrl+Enter')} disabled={locked || !allAnswered} onClick={send}>{questions.length > 1 ? 'Send answers' : 'Send answer'}</button>
          : <button type="button" className="question-primary" disabled={locked || !isAnswered(question!, draft!)} onClick={() => setStep(value => value + 1)}>Next</button>}
      </div>}
    </footer>;
  }
}

function DismissDialog({ onCancel, onConfirm }: { onCancel: () => void; onConfirm: () => void }) {
  useRegisterFocusScope('question-dismiss');
  const dialog = useRef<HTMLDialogElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const title = useId();
  useEffect(() => { dialog.current?.showModal(); cancel.current?.focus(); }, []);
  useEffect(() => {
    window.addEventListener('command-palette-opening', onCancel);
    return () => window.removeEventListener('command-palette-opening', onCancel);
  }, [onCancel]);
  return <dialog ref={dialog} aria-labelledby={title} className="question-dismiss-dialog" onCancel={event => { event.preventDefault(); onCancel(); }}
    onKeyDown={event => { if (event.key === '?' && !event.ctrlKey && !event.metaKey) { event.preventDefault(); event.stopPropagation(); onCancel(); requestAnimationFrame(() => window.dispatchEvent(new CustomEvent('toggle-shortcut-help'))); } else if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onCancel(); } }}>
    <h3 id={title}>Use chat instead?</h3><p>No answer will be sent. The agent will wait for your message.</p><div className="question-dialog-actions">
      <button type="button" ref={cancel} onClick={onCancel}>Keep answering</button><button type="button" onClick={onConfirm}>Use chat instead</button>
    </div>
  </dialog>;
}
