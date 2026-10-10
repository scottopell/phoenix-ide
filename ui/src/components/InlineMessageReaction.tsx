import { useCallback, useContext, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { createPortal } from 'react-dom';
import type { Message } from '../api';
import { InlineReactionContext, InlineReactionStore, formatInlineReaction, type ReactionSource } from '../conversation/InlineReactionStore';
import { useFocusScope } from '../hooks/useFocusScope';
import { readReactionSelection } from './inlineReactionSelection';
import { restoreReactionRange } from './reactionRange';
import { ReactionPill } from './ReactionPill';
import './InlineMessageReaction.css';

export interface ReactionDraftDestination {
  append: (text: string) => void;
}

interface Props {
  scopeKey: string;
  messages: Message[];
  destination?: ReactionDraftDestination | undefined;
  returnToSource?: ((source: ReactionSource, signal: AbortSignal) => boolean | Promise<boolean>) | undefined;
  scrollTranscriptBy?: ((delta: number) => void) | undefined;
}

function sameReactionSource(a: ReactionSource | undefined, b: ReactionSource): boolean {
  return a?.messageId === b.messageId
    && a.occurrenceToken === b.occurrenceToken
    && a.quote === b.quote
    && a.textAnchor?.start.fragmentId === b.textAnchor?.start.fragmentId
    && a.textAnchor?.start.offset === b.textAnchor?.start.offset
    && a.textAnchor?.end.fragmentId === b.textAnchor?.end.fragmentId
    && a.textAnchor?.end.offset === b.textAnchor?.end.offset;
}

export function InlineMessageReaction(props: Props) {
  const store = useContext(InlineReactionContext);
  return store ? <ReactionSession key={props.scopeKey} {...props} store={store} /> : null;
}

function ReactionSession({ scopeKey, messages, destination, returnToSource, scrollTranscriptBy, store }: Props & { store: InlineReactionStore }) {
  const subscribe = useCallback((listener: () => void) => store.subscribe(scopeKey, listener), [scopeKey, store]);
  const getSnapshot = useCallback(() => store.getSnapshot(scopeKey), [scopeKey, store]);
  const reaction = useSyncExternalStore(subscribe, getSnapshot);
  const bubbleRef = useRef<HTMLDivElement>(null);
  const selectedRange = useRef<Range | null>(null);
  const pillFocusPending = useRef(false);
  const selecting = useRef(false);
  const selectionInput = useRef<'touch' | 'fine' | null>(null);
  const selectionChangedDuringGesture = useRef(false);
  const { activeScope } = useFocusScope();
  const focusScope = `inline-reaction:${scopeKey}`;
  const [notice, setNotice] = useState('');

  useEffect(() => {
    let frame = 0;
    if (activeScope && activeScope !== focusScope) {
      selectionInput.current = null;
      selectionChangedDuringGesture.current = false;
      selecting.current = false;
      return;
    }
    const read = () => {
      frame = 0;
      if (selecting.current || pillFocusPending.current || !destination || (activeScope && activeScope !== focusScope)) return;
      if (bubbleRef.current?.contains(document.activeElement)) return;
      const current = store.getSnapshot(scopeKey);
      if (current?.body) return;
      const nativeSelection = window.getSelection();
      const selected = readReactionSelection(nativeSelection, messages);
      if (selected) {
        selectedRange.current = selected.range.cloneRange();
        const sameSource = sameReactionSource(current?.source, selected.source);
        const gesturePresentation = selectionChangedDuringGesture.current && selectionInput.current !== null
          ? selectionInput.current === 'touch'
          : null;
        const touchDocked = gesturePresentation ?? (sameSource
          ? current?.presentation === 'touch-docked'
          : window.matchMedia?.('(any-pointer: coarse)').matches ?? false);
        const presentation = touchDocked ? 'touch-docked' : 'floating';
        if (!sameSource || current?.presentation !== presentation) {
          store.dispatch(scopeKey, { type: 'select', source: selected.source, presentation });
        }
        setNotice('');
      } else if (current && (current.presentation === 'floating'
        || Boolean(nativeSelection && nativeSelection.rangeCount > 0 && !nativeSelection.isCollapsed)
        || Boolean(restoreReactionRange(current.source)))) {
        store.dispatch(scopeKey, { type: 'clear' });
      }
      selectionInput.current = null;
      selectionChangedDuringGesture.current = false;
    };
    const schedule = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(read);
    };
    const down = (event: PointerEvent) => {
      if (bubbleRef.current?.contains(event.target as Node)) return;
      selectionInput.current = event.pointerType === 'touch' ? 'touch' : 'fine';
      selectionChangedDuringGesture.current = false;
      selecting.current = event.pointerType !== 'touch';
    };
    const up = () => { selecting.current = false; schedule(); };
    const keydown = (event: KeyboardEvent) => {
      if (event.shiftKey || ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'a')) selectionInput.current = 'fine';
    };
    const selectionChange = () => {
      if (selectionInput.current !== null) selectionChangedDuringGesture.current = true;
      const nativeSelection = window.getSelection();
      if (selectionInput.current === null && (!nativeSelection || nativeSelection.rangeCount === 0 || nativeSelection.isCollapsed)) {
        cancelAnimationFrame(frame);
        read();
        return;
      }
      schedule();
    };
    document.addEventListener('selectionchange', selectionChange);
    document.addEventListener('keydown', keydown, true);
    document.addEventListener('pointerdown', down, { passive: true });
    document.addEventListener('pointerup', up, { passive: true });
    document.addEventListener('pointercancel', up, { passive: true });
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener('selectionchange', selectionChange);
      document.removeEventListener('keydown', keydown, true);
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
    selectedRange.current = null;
    store.dispatch(scopeKey, { type: 'clear' });
    window.getSelection()?.removeAllRanges();
  };
  const add = () => {
    const current = store.getSnapshot(scopeKey);
    if (!destination || !current?.body.trim()) return;
    try {
      destination.append(formatInlineReaction(current));
      clear();
      setNotice('Added to draft');
    } catch {
      setNotice('Could not add to draft. Your reaction is still here.');
    }
  };

  return createPortal(
    <>
      {reaction && (
        <ReactionPill
          bubbleRef={bubbleRef}
          scopeId={focusScope}
          source={reaction.source}
          sourceRange={selectedRange.current}
          touchDocked={reaction.presentation === 'touch-docked'}
          captureSource={() => {
            pillFocusPending.current = true;
            requestAnimationFrame(() => requestAnimationFrame(() => { pillFocusPending.current = false; }));
            if (reaction.body) return;
            const selected = readReactionSelection(window.getSelection(), messages);
            if (!selected) return;
            selectedRange.current = selected.range.cloneRange();
            if (!sameReactionSource(reaction.source, selected.source)) {
              store.dispatch(scopeKey, { type: 'select', source: selected.source, presentation: reaction.presentation });
            }
          }}
          returnToSource={returnToSource}
          scrollTranscriptBy={scrollTranscriptBy}
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

export interface ReactionPillProps {
  source: ReactionSource;
  sourceRange?: Range | null;
  touchDocked?: boolean;
  captureSource?: () => void;
  scrollTranscriptBy?: ((delta: number) => void) | undefined;
  returnToSource?: ((source: ReactionSource, signal: AbortSignal) => boolean | Promise<boolean>) | undefined;
  bubbleRef: React.RefObject<HTMLDivElement>;
  scopeId: string;
  body: string;
  available: boolean;
  onChange: (body: string) => void;
  onAdd: () => void;
  onClose: () => void;
}
