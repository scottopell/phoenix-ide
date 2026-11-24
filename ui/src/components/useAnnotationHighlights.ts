import { useEffect, useSyncExternalStore } from 'react';
import type { AnnotationDraft, DraftAnnotation } from '../conversation/AnnotationDraftStore';
import type { ReactionSource } from '../conversation/InlineReactionStore';
import { restoreReactionRange } from './reactionRange';

const EMPTY: readonly DraftAnnotation[] = [];
const noSubscription = () => () => {};
const emptySnapshot = () => EMPTY;
const NAME = 'phoenix-pending-annotations';

export function useAnnotationHighlights(active: ReactionSource | undefined, draft: AnnotationDraft | undefined) {
  const annotations = useSyncExternalStore(draft?.subscribe ?? noSubscription, draft?.getSnapshot ?? emptySnapshot);
  useEffect(() => {
    if (typeof CSS === 'undefined' || !CSS.highlights || typeof Highlight === 'undefined') {
      if (active || annotations.length) console.debug('Annotation highlights are unavailable in this browser');
      return;
    }
    const registry = CSS.highlights;
    const highlight = new Highlight();
    registry.set(NAME, highlight);
    let frame = 0;
    const paint = () => {
      frame = 0;
      highlight.clear();
      for (const source of [...annotations.map(annotation => annotation.source), ...(active ? [active] : [])]) {
        const range = restoreReactionRange(source);
        if (range) highlight.add(range);
      }
    };
    // Coalesce rather than debounce: a change during a pending frame is covered by
    // that frame, and paint() clears `frame` first, so later changes schedule anew.
    const observer = new MutationObserver(() => {
      if (!frame) frame = requestAnimationFrame(paint);
    });
    // document.body, not #messages: a remounted transcript must not escape observation.
    observer.observe(document.body, { childList: true, subtree: true, characterData: true });
    paint();
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      if (registry.get(NAME) === highlight) registry.delete(NAME);
    };
  }, [active, annotations]);
}
