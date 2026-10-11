import { useLayoutEffect, useRef, type ComponentPropsWithoutRef } from 'react';
import './MarkdownDetails.css';

export function MarkdownDetails({ open, children }: ComponentPropsWithoutRef<'details'> & { node?: unknown }) {
  const detailsRef = useRef<HTMLDetailsElement>(null);

  useLayoutEffect(() => {
    const details = detailsRef.current;
    if (!details) return;
    const summary = details.querySelector(':scope > summary');
    const bodyMatches = [...details.querySelectorAll('mark.viewer-find-inline-match--active')]
      .some((mark) => !summary?.contains(mark));
    if (bodyMatches) details.open = true;
  }, [children]);

  return <details ref={detailsRef} className="markdown-details" open={open}>{children}</details>;
}

export function MarkdownSummary({ children }: ComponentPropsWithoutRef<'summary'> & { node?: unknown }) {
  return <summary className="markdown-summary">{children}</summary>;
}
