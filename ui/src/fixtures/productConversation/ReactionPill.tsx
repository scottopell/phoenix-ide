import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ArrowUpRight, ListPlus, X } from 'lucide-react';
import type { BubbleProps } from '../../components/InlineMessageReaction';
import { restoreReactionRange } from './reactionRange';
import { useFocusScope, useKeyboardRouterShortcut, useRegisterFocusScope } from '../../hooks/useFocusScope';
import './ReactionPill.css';

export function ReactionPill({ source, bubbleRef, scopeId, body, available, onChange, onAdd, onClose, returnToSource }: BubbleProps) {
  useRegisterFocusScope(scopeId);
  const { activeScope } = useFocusScope();
  const [docked, setDocked] = useState(false);
  const [discard, setDiscard] = useState(false);
  const [error, setError] = useState('');
  const returning = useRef(false);
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
      let rect = range?.getBoundingClientRect();
      if (returning.current && rect && rect.height > 0) {
        returning.current = false;
        scroller.scrollTop += rect.top - Math.max(top, transcript.top) - 72;
        window.getSelection()?.removeAllRanges();
        window.getSelection()?.addRange(range!);
        rect = range!.getBoundingClientRect();
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

  const returnToPassage = () => {
    returning.current = true;
    if (!returnToSource?.(source)) {
      returning.current = false;
      setError('Passage unavailable. Your reaction is saved here.');
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
            <button type="button" className="reaction-pill-return" onClick={returnToPassage} title={error || source.quote}>
              <ArrowUpRight size={18} aria-hidden="true" />
              <span>{error || `Return to passage · ${body || source.quote}`}</span>
            </button>
          ) : (
            <>
              <input
                type="text"
                aria-label="Your reaction"
                placeholder="Your reaction…"
                title={available ? source.quote : 'Draft unavailable. Your reaction is retained.'}
                value={body}
                onChange={(event) => onChange(event.target.value)}
                onKeyDown={(event) => {
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
      <span className="reaction-pill-sr" role="status">{error || (!available ? 'Draft unavailable. Your reaction is retained.' : '')}</span>
    </div>
  );
}
