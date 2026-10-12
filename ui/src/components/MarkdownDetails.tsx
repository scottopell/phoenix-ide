import { useLayoutEffect, useRef, type ComponentPropsWithoutRef } from 'react';
import './MarkdownDetails.css';

export function MarkdownDetails({ open, children }: ComponentPropsWithoutRef<'details'> & { node?: unknown }) {
  const detailsRef = useRef<HTMLDetailsElement>(null);
  const previousTarget = useRef<string | null>(null);

  useLayoutEffect(() => {
    const details = detailsRef.current;
    if (!details) return;
    const summary = details.querySelector(':scope > summary');
    const bodyTarget = [...details.querySelectorAll('[data-viewer-find-active-target]')]
      .find((target) => !summary?.contains(target));
    const targetId = bodyTarget?.getAttribute('data-viewer-find-active-target') ?? null;
    if (targetId !== null && targetId !== previousTarget.current) details.open = true;
    previousTarget.current = targetId;
  }, [children]);

  return <details ref={detailsRef} className="markdown-details" open={open}>{children}</details>;
}

export function MarkdownSummary({ children }: ComponentPropsWithoutRef<'summary'> & { node?: unknown }) {
  return <summary className="markdown-summary">{children}</summary>;
}
