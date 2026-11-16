import { useLayoutEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import type { DraftAnnotation } from '../conversation/AnnotationDraftStore';
import { restoreReactionRange } from './reactionRange';

interface Marker {
  numbers: number[];
  left: number;
  top: number;
}

export function AnnotationSourceMarkers({ annotations, emphasizedId }: {
  annotations: readonly DraftAnnotation[];
  emphasizedId: string | null;
}) {
  const [markers, setMarkers] = useState<Marker[]>([]);
  const emphasized = annotations.find(annotation => annotation.id === emphasizedId);
  useLayoutEffect(() => {
    const registry = typeof CSS !== 'undefined' ? CSS.highlights : undefined;
    const highlight = registry && typeof Highlight !== 'undefined' ? new Highlight() : null;
    if (highlight) {
      highlight.priority = 1;
      registry!.set('phoenix-focused-annotation', highlight);
    }
    let frame = 0;
    let observedTranscript: HTMLElement | null = null;
    const paint = () => {
      frame = 0;
      highlight?.clear();
      const transcript = document.getElementById('messages');
      if (transcript !== observedTranscript) {
        if (observedTranscript) resize.unobserve(observedTranscript);
        if (transcript) resize.observe(transcript);
        observedTranscript = transcript;
      }
      const bounds = transcript?.getBoundingClientRect();
      const next: Marker[] = [];
      for (const annotation of annotations) {
        const range = restoreReactionRange(annotation.source);
        if (!range) continue;
        if (annotation.id === emphasizedId) highlight?.add(range);
        if (!bounds || typeof range.getClientRects !== 'function') continue;
        const rect = Array.from(range.getClientRects()).find(rect => rect.width > 0 && rect.height > 0 && rect.top >= bounds.top && rect.bottom <= bounds.bottom);
        if (!rect) continue;
        const left = Math.max(1, rect.left - 21);
        const top = rect.top + (rect.height - 18) / 2;
        const shared = next.find(marker => Math.abs(marker.top - top) < 2 && Math.abs(marker.left - left) < 2);
        if (shared) shared.numbers.push(annotation.number);
        else next.push({ numbers: [annotation.number], left, top });
      }
      setMarkers(previous => JSON.stringify(previous) === JSON.stringify(next) ? previous : next);
    };
    const schedule = () => { if (!frame) frame = requestAnimationFrame(paint); };
    const observer = new MutationObserver(schedule);
    observer.observe(document.body, { subtree: true, childList: true, characterData: true });
    const resize = new ResizeObserver(schedule);
    resize.observe(document.body);
    document.addEventListener('scroll', schedule, true);
    window.addEventListener('resize', schedule);
    window.visualViewport?.addEventListener('resize', schedule);
    window.visualViewport?.addEventListener('scroll', schedule);
    paint();
    return () => {
      observer.disconnect();
      resize.disconnect();
      cancelAnimationFrame(frame);
      document.removeEventListener('scroll', schedule, true);
      window.removeEventListener('resize', schedule);
      window.visualViewport?.removeEventListener('resize', schedule);
      window.visualViewport?.removeEventListener('scroll', schedule);
      if (highlight && registry?.get('phoenix-focused-annotation') === highlight) registry.delete('phoenix-focused-annotation');
    };
  }, [annotations, emphasizedId]);

  return createPortal(<div className="annotation-source-markers" aria-hidden="true">
    {markers.map(marker => {
      const active = emphasized && marker.numbers.includes(emphasized.number);
      return <span key={marker.numbers.join(',')} className={`annotation-source-marker${active ? ' is-emphasized' : ''}`} style={{ left: marker.left, top: marker.top }} title={`Annotation${marker.numbers.length > 1 ? 's' : ''} ${marker.numbers.join(', ')}`}>
        {active ? emphasized.number : marker.numbers[0]}{marker.numbers.length > 1 && <sup>+</sup>}
      </span>;
    })}
  </div>, document.body);
}
