import { useCallback, useContext, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from 'react';
import { createPortal } from 'react-dom';
import { ListPlus, X } from 'lucide-react';
import type { Message } from '../api';
import { InlineReactionContext, InlineReactionStore, formatInlineReaction } from '../conversation/InlineReactionStore';
import { useFocusScope, useKeyboardRouterShortcut, useRegisterFocusScope } from '../hooks/useFocusScope';
import { readReactionSelection } from './inlineReactionSelection';
import './InlineMessageReaction.css';

export interface ReactionDraftDestination {
  append: (text: string) => void;
}

interface Props {
  scopeKey: string;
  messages: Message[];
  destination?: ReactionDraftDestination | undefined;
}

export function InlineMessageReaction(props: Props) {
  const store = useContext(InlineReactionContext);
  return store ? <ReactionSession key={props.scopeKey} {...props} store={store} /> : null;
}

function ReactionSession({ scopeKey, messages, destination, store }: Props & { store: InlineReactionStore }) {
  const subscribe = useCallback((listener: () => void) => store.subscribe(scopeKey, listener), [scopeKey, store]);
  const getSnapshot = useCallback(() => store.getSnapshot(scopeKey), [scopeKey, store]);
  const reaction = useSyncExternalStore(subscribe, getSnapshot);
  const anchor = useRef<Range | null>(null);
  const bubbleRef = useRef<HTMLDivElement>(null);
  const { activeScope } = useFocusScope();
  const focusScope = `inline-reaction:${scopeKey}`;
  const [notice, setNotice] = useState('');

  useEffect(() => {
    let frame = 0;
    let selecting = false;
    const read = () => {
      frame = 0;
      if (selecting || !destination || (activeScope && activeScope !== focusScope)) return;
      if (bubbleRef.current?.contains(document.activeElement)) return;
      const current = store.getSnapshot(scopeKey);
      if (current?.body) return;
      const selected = readReactionSelection(window.getSelection(), messages);
      if (selected) {
        anchor.current = selected.range;
        store.dispatch(scopeKey, { type: 'select', source: selected.source });
        setNotice('');
      } else if (current) {
        store.dispatch(scopeKey, { type: 'clear' });
      }
    };
    const schedule = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(read);
    };
    const down = (event: PointerEvent) => {
      if (bubbleRef.current?.contains(event.target as Node)) return;
      selecting = event.pointerType !== 'touch';
    };
    const up = () => { selecting = false; schedule(); };
    document.addEventListener('selectionchange', schedule);
    document.addEventListener('pointerdown', down, { passive: true });
    document.addEventListener('pointerup', up, { passive: true });
    document.addEventListener('pointercancel', up, { passive: true });
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener('selectionchange', schedule);
      document.removeEventListener('pointerdown', down);
      document.removeEventListener('pointerup', up);
      document.removeEventListener('pointercancel', up);
    };
  }, [activeScope, destination, focusScope, messages, scopeKey, store]);

  useEffect(() => () => {
    if (!store.getSnapshot(scopeKey)?.body) store.dispatch(scopeKey, { type: 'clear' });
  }, [scopeKey, store]);

  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(''), 4000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  const clear = () => {
    store.dispatch(scopeKey, { type: 'clear' });
    anchor.current = null;
  };
  const add = () => {
    const current = store.getSnapshot(scopeKey);
    if (!destination || !current?.body.trim()) return;
    try {
      destination.append(formatInlineReaction(current));
      clear();
      // Clearing the consumed selection prevents pointerup from reopening it.
      window.getSelection()?.removeAllRanges();
      setNotice('Added to draft');
    } catch {
      setNotice('Could not add to draft. Your reaction is still here.');
    }
  };

  return createPortal(
    <>
      {reaction && (
        <ReactionBubble
          bubbleRef={bubbleRef}
          anchor={anchor.current}
          scopeId={focusScope}
          quote={reaction.source.quote}
          body={reaction.body}
          available={Boolean(destination)}
          onChange={(body) => store.dispatch(scopeKey, { type: 'edit', body })}
          onAdd={add}
          onClose={clear}
        />
      )}
      <div className="inline-reaction-notice" role="status">{notice}</div>
    </>,
    document.body,
  );
}

interface BubbleProps {
  bubbleRef: React.RefObject<HTMLDivElement>;
  anchor: Range | null;
  scopeId: string;
  quote: string;
  body: string;
  available: boolean;
  onChange: (body: string) => void;
  onAdd: () => void;
  onClose: () => void;
}

function ReactionBubble({ bubbleRef, anchor, scopeId, quote, body, available, onChange, onAdd, onClose }: BubbleProps) {
  useRegisterFocusScope(scopeId);
  const { activeScope } = useFocusScope();
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const requestClose = () => { if (body) setConfirmDiscard(true); else onClose(); };
  useKeyboardRouterShortcut({
    id: `${scopeId}:escape`, scopeId, key: 'Escape', layer: 'passive-content',
    handler: (event) => {
      event.preventDefault();
      event.stopImmediatePropagation();
      if (confirmDiscard) setConfirmDiscard(false);
      else requestClose();
    },
  });

  useLayoutEffect(() => {
    const el = bubbleRef.current;
    if (!el) return;
    const position = () => {
      const viewport = window.visualViewport;
      const left = viewport?.offsetLeft ?? 0;
      const top = viewport?.offsetTop ?? 0;
      const width = viewport?.width ?? window.innerWidth;
      const height = viewport?.height ?? window.innerHeight;
      el.style.maxHeight = `${Math.max(80, height - 24)}px`;
      el.style.width = `${Math.min(360, width - 24)}px`;
      const box = el.getBoundingClientRect();
      const rect = anchor?.startContainer.isConnected ? anchor.getBoundingClientRect() : null;
      const gap = 24;
      let y = top + height - box.height - 12;
      let x = left + width - box.width - 12;
      if (rect && rect.bottom >= top && rect.top <= top + height) {
        x = rect.left;
        if (rect.bottom + gap + box.height <= top + height - 12) y = rect.bottom + gap;
        else if (rect.top - gap - box.height >= top + 12) y = rect.top - gap - box.height;
      }
      el.style.left = `${Math.max(left + 12, Math.min(x, left + width - box.width - 12))}px`;
      el.style.top = `${Math.max(top + 12, y)}px`;
    };
    position();
    const observer = new ResizeObserver(position);
    observer.observe(el);
    window.addEventListener('resize', position);
    window.addEventListener('scroll', position, true);
    window.visualViewport?.addEventListener('resize', position);
    window.visualViewport?.addEventListener('scroll', position);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', position);
      window.removeEventListener('scroll', position, true);
      window.visualViewport?.removeEventListener('resize', position);
      window.visualViewport?.removeEventListener('scroll', position);
    };
  }, [anchor, bubbleRef]);

  return (
    <div ref={bubbleRef} className="inline-reaction" role="region" aria-label="React to selected text" hidden={Boolean(activeScope && activeScope !== scopeId)}>
      <div className="inline-reaction-header">
        <span>Reaction</span>
        <button type="button" onClick={requestClose} aria-label="Dismiss reaction"><X size={16} /></button>
      </div>
      <blockquote className="inline-reaction-quote" title={quote}>{quote}</blockquote>
      <textarea
        aria-label="Your reaction"
        placeholder="Your reaction…"
        rows={3}
        value={body}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' && (event.metaKey || event.ctrlKey) && !event.nativeEvent.isComposing) {
            event.preventDefault();
            event.stopPropagation();
            onAdd();
          }
        }}
      />
      {!available && <p role="status">The current message draft is unavailable. Your reaction is retained.</p>}
      {confirmDiscard ? (
        <div className="inline-reaction-discard" role="group" aria-label="Discard this reaction?">
          <span>Discard this reaction?</span>
          <button type="button" onClick={() => setConfirmDiscard(false)}>Keep writing</button>
          <button type="button" onClick={onClose}>Discard</button>
        </div>
      ) : (
        <div className="inline-reaction-actions">
          <button type="button" disabled={!available || !body.trim()} onClick={onAdd} title="Add to draft (Cmd/Ctrl+Enter)">
            <ListPlus size={18} aria-hidden="true" /> Add to draft
          </button>
        </div>
      )}
    </div>
  );
}
