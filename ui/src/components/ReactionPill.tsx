import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ArrowUpRight, ListPlus, X } from 'lucide-react';
import type { ReactionPillProps } from './InlineMessageReaction';
import { restoreReactionRange } from './reactionRange';
import { useFocusScope, useKeyboardRouterShortcut, useRegisterFocusScope } from '../hooks/useFocusScope';
import './ReactionPill.css';

export function ReactionPill({ source, sourceRange, touchDocked = false, captureSource, bubbleRef, scopeId, body, available, onChange, onAdd, onClose, returnToSource }: ReactionPillProps) {
  useRegisterFocusScope(scopeId);
  const { activeScope } = useFocusScope();
  const [sourceDocked, setSourceDocked] = useState(false);
  const docked = touchDocked || sourceDocked;
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
    const composer = document.getElementById('input-area');
    if (!el || !scroller) return;
    let frame = 0;
    const position = () => {
      frame = 0;
      const viewport = window.visualViewport;
      const styles = getComputedStyle(document.documentElement);
      const safeTop = Number.parseFloat(styles.getPropertyValue('--safe-area-top')) || 0;
      const safeRight = Number.parseFloat(styles.getPropertyValue('--safe-area-right')) || 0;
      const safeBottom = Number.parseFloat(styles.getPropertyValue('--safe-area-bottom')) || 0;
      const safeLeft = Number.parseFloat(styles.getPropertyValue('--safe-area-left')) || 0;
      const viewportLeft = viewport?.offsetLeft ?? 0;
      const viewportTop = viewport?.offsetTop ?? 0;
      const left = viewportLeft + safeLeft;
      const top = viewportTop + safeTop;
      const width = (viewport?.width ?? window.innerWidth) - safeLeft - safeRight;
      const bottom = viewportTop + (viewport?.height ?? window.innerHeight) - safeBottom;
      const transcript = scroller.getBoundingClientRect();
      const range = restoreReactionRange(source) ?? sourceRange;
      const rect = range?.getBoundingClientRect();
      if (returning.current && rect && rect.height > 0) {
        returning.current = false;
        window.getSelection()?.removeAllRanges();
        window.getSelection()?.addRange(range!);
      }
      const visibleTop = Math.max(top, transcript.top);
      const visibleBottom = Math.min(bottom, transcript.bottom);
      const visible = Boolean(rect && rect.height > 0 && rect.bottom > visibleTop && rect.top < visibleBottom);
      setSourceDocked(!visible);
      const pillWidth = Math.min(420, width - 24);
      el.style.width = `${pillWidth}px`;
      const height = el.getBoundingClientRect().height || 46;
      if (touchDocked) scroller.style.setProperty('--reaction-dock-height', `${height + 12}px`);
      let y = Math.min(visibleBottom, bottom) - height - 12;
      let x = transcript.right - pillWidth - 12;
      if (touchDocked) {
        const composerTop = composer?.getBoundingClientRect().top ?? bottom;
        y = Math.min(bottom, composerTop) - height - 12;
      } else if (visible && rect) {
        x = rect.left;
        y = rect.bottom + 16;
        if (y + height > visibleBottom - 8) y = rect.top - height - 16;
      }
      el.style.left = `${Math.max(left + 12, Math.min(x, left + width - pillWidth - 12))}px`;
      el.style.top = `${Math.max(top + 12, Math.min(y, bottom - height - 12))}px`;
    };
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(position); };
    if (touchDocked) scroller.classList.add('reaction-dock-reserved');
    position();
    const mutations = new MutationObserver(schedule);
    mutations.observe(scroller, { childList: true, subtree: true });
    const resize = new ResizeObserver(schedule);
    resize.observe(scroller);
    resize.observe(el);
    const sourceOwner = restoreReactionRange(source)?.commonAncestorContainer.parentElement?.closest('[data-inline-reaction-message]');
    if (sourceOwner) resize.observe(sourceOwner);
    if (composer) resize.observe(composer);
    window.addEventListener('scroll', schedule, true);
    window.addEventListener('resize', schedule);
    window.visualViewport?.addEventListener('resize', schedule);
    window.visualViewport?.addEventListener('scroll', schedule);
    return () => {
      cancelAnimationFrame(frame);
      scroller.classList.remove('reaction-dock-reserved');
      scroller.style.removeProperty('--reaction-dock-height');
      mutations.disconnect();
      resize.disconnect();
      window.removeEventListener('scroll', schedule, true);
      window.removeEventListener('resize', schedule);
      window.visualViewport?.removeEventListener('resize', schedule);
      window.visualViewport?.removeEventListener('scroll', schedule);
    };
  }, [source, sourceRange, touchDocked, bubbleRef]);

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
          {sourceDocked && !touchDocked ? (
            <button type="button" className="reaction-pill-return" onClick={returnToPassage} disabled={returnPending} title={error || source.quote}>
              <ArrowUpRight size={18} aria-hidden="true" />
              <span>{returnPending ? 'Returning to passage…' : error || `Return to passage · ${body || source.quote}`}</span>
            </button>
          ) : (
            <>
              {touchDocked && (sourceDocked ? (
                <button type="button" className="reaction-pill-source" aria-label={`Return to passage: ${source.quote}`} title="Return to passage" onClick={returnToPassage} disabled={returnPending}>
                  {returnPending ? 'Returning…' : error || `“${source.quote}”`}
                </button>
              ) : (
                <span className="reaction-pill-source" title={source.quote}>“{source.quote}”</span>
              ))}
              <input
                ref={inputRef}
                type="text"
                aria-label="Your reaction"
                placeholder={touchDocked ? 'React to selection…' : 'Your reaction… (Enter to focus)'}
                title={available ? source.quote : 'Draft unavailable. Your reaction is retained.'}
                value={body}
                onPointerDown={captureSource}
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
