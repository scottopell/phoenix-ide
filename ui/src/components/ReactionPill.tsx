import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ArrowUpRight, ListPlus, X } from 'lucide-react';
import type { ReactionPillProps } from './InlineMessageReaction';
import { restoreReactionRange } from './reactionRange';
import { useFocusScope, useKeyboardRouterShortcut, useRegisterFocusScope } from '../hooks/useFocusScope';
import './ReactionPill.css';

export function ReactionPill({ source, bubbleRef, scopeId, body, available, onChange, onAdd, onClose, returnToSource }: ReactionPillProps) {
  useRegisterFocusScope(scopeId);
  const { activeScope } = useFocusScope();
  const [docked, setDocked] = useState(false);
  const [discard, setDiscard] = useState(false);
  const [error, setError] = useState('');
  const returning = useRef(false);
  const returnRequest = useRef<AbortController | null>(null);
  const [returnPending, setReturnPending] = useState(false);
  useEffect(() => {
    setReturnPending(false);
    setError('');
    return () => {
      returnRequest.current?.abort();
      returning.current = false;
    };
  }, [source]);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (docked || discard || (activeScope && activeScope !== scopeId)) return;
    const focusFromSelection = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.key !== 'Enter' || event.isComposing || event.keyCode === 229
        || event.repeat || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
      const target = event.target instanceof Element ? event.target : document.activeElement;
      const control = target?.closest('input, textarea, select, button, summary, a[href], audio, video, [contenteditable], [tabindex], [role="button"], [role="textbox"], [role="combobox"]');
      if (control && control !== document.getElementById('messages')) return;
      if (!inputRef.current || bubbleRef.current?.hidden) return;
      event.preventDefault();
      inputRef.current.focus({ preventScroll: true });
    };
    document.addEventListener('keydown', focusFromSelection);
    return () => document.removeEventListener('keydown', focusFromSelection);
  }, [activeScope, bubbleRef, discard, docked, scopeId]);
  const requestClose = () => body ? setDiscard(true) : onClose();
  useKeyboardRouterShortcut({
    id: `${scopeId}:escape`, scopeId, key: 'Escape', layer: 'passive-content',
    handler: (event) => {
      event.preventDefault();
      event.stopImmediatePropagation();
      if (discard) setDiscard(false);
      else requestClose();
    },
  });

  useLayoutEffect(() => {
    const el = bubbleRef.current;
    const scroller = document.getElementById('messages');
    if (!el || !scroller) return;
    let frame = 0;
    const position = () => {
      frame = 0;
      const viewport = window.visualViewport;
      const left = viewport?.offsetLeft ?? 0;
      const top = viewport?.offsetTop ?? 0;
      const width = viewport?.width ?? window.innerWidth;
      const bottom = top + (viewport?.height ?? window.innerHeight);
      const transcript = scroller.getBoundingClientRect();
      const range = restoreReactionRange(source);
      const rect = range?.getBoundingClientRect();
      if (returning.current && rect && rect.height > 0) {
        returning.current = false;
        window.getSelection()?.removeAllRanges();
        window.getSelection()?.addRange(range!);
      }
      const visibleTop = Math.max(top, transcript.top);
      const visibleBottom = Math.min(bottom, transcript.bottom);
      const visible = Boolean(rect && rect.height > 0 && rect.bottom > visibleTop && rect.top < visibleBottom);
      setDocked(!visible);
      el.style.width = `${Math.min(420, width - 24)}px`;
      const height = el.getBoundingClientRect().height || 46;
      let y = Math.min(visibleBottom, bottom) - height - 12;
      let x = transcript.right - Math.min(420, width - 24) - 12;
      if (visible && rect) {
        x = rect.left;
        y = rect.bottom + 16;
        if (y + height > visibleBottom - 8) y = rect.top - height - 16;
      }
      el.style.left = `${Math.max(left + 12, Math.min(x, left + width - Math.min(420, width - 24) - 12))}px`;
      el.style.top = `${Math.max(top + 12, Math.min(y, bottom - height - 12))}px`;
    };
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(position); };
    position();
    const mutations = new MutationObserver(schedule);
    mutations.observe(scroller, { childList: true, subtree: true });
    const resize = new ResizeObserver(schedule);
    resize.observe(scroller);
    resize.observe(el);
    window.addEventListener('scroll', schedule, true);
    window.addEventListener('resize', schedule);
    window.visualViewport?.addEventListener('resize', schedule);
    window.visualViewport?.addEventListener('scroll', schedule);
    return () => {
      cancelAnimationFrame(frame);
      mutations.disconnect();
      resize.disconnect();
      window.removeEventListener('scroll', schedule, true);
      window.removeEventListener('resize', schedule);
      window.visualViewport?.removeEventListener('resize', schedule);
      window.visualViewport?.removeEventListener('scroll', schedule);
    };
  }, [source, bubbleRef]);

  useEffect(() => {
    if (!docked) setError('');
  }, [docked]);

  const returnToPassage = async () => {
    returnRequest.current?.abort();
    const request = new AbortController();
    returnRequest.current = request;
    returning.current = true;
    setReturnPending(true);
    setError('');
    try {
      const found = await returnToSource?.(source, request.signal);
      if (request.signal.aborted) return;
      if (!found) {
        returning.current = false;
        setError('Passage unavailable. Your reaction is saved here.');
      }
    } catch {
      if (!request.signal.aborted) {
        returning.current = false;
        setError('Passage unavailable. Your reaction is saved here.');
      }
    } finally {
      if (!request.signal.aborted) setReturnPending(false);
    }
  };

  return (
    <div ref={bubbleRef} className="reaction-pill" role="region" aria-label={docked ? 'Docked reaction' : 'React to selected text'} hidden={Boolean(activeScope && activeScope !== scopeId)}>
      {discard ? (
        <>
          <span className="reaction-pill-label">Discard reaction?</span>
          <button type="button" onClick={() => setDiscard(false)}>Keep</button>
          <button type="button" onClick={onClose}>Discard</button>
        </>
      ) : (
        <>
          {docked ? (
            <button type="button" className="reaction-pill-return" onClick={returnToPassage} disabled={returnPending} title={error || source.quote}>
              <ArrowUpRight size={18} aria-hidden="true" />
              <span>{returnPending ? 'Returning to passage…' : error || `Return to passage · ${body || source.quote}`}</span>
            </button>
          ) : (
            <>
              <input
                ref={inputRef}
                type="text"
                aria-label="Your reaction"
                placeholder="Your reaction… (Enter to focus)"
                title={available ? source.quote : 'Draft unavailable. Your reaction is retained.'}
                value={body}
                onChange={(event) => onChange(event.target.value)}
                onKeyDown={(event) => {
                  if (event.nativeEvent.isComposing || event.keyCode === 229) return;
                  if (event.key === 'Enter') {
                    event.stopPropagation();
                    event.preventDefault();
                    if ((event.metaKey || event.ctrlKey) && !event.nativeEvent.isComposing) onAdd();
                  }
                }}
              />
              <button type="button" aria-label="Add to draft" title={available ? 'Add to draft (Cmd/Ctrl+Enter)' : 'Draft unavailable. Your reaction is retained.'} disabled={!available || !body.trim()} onClick={onAdd}>
                <ListPlus size={20} aria-hidden="true" />
              </button>
            </>
          )}
          <button type="button" aria-label="Dismiss reaction" title="Dismiss reaction" onClick={requestClose}><X size={16} aria-hidden="true" /></button>
        </>
      )}
      {(error || !available) && <span className="reaction-pill-sr" role="status">{error || 'Draft unavailable. Your reaction is retained.'}</span>}
    </div>
  );
}
