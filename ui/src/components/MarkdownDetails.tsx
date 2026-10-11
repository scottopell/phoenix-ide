import type { ComponentPropsWithoutRef } from 'react';
import './MarkdownDetails.css';

export function MarkdownDetails({ open, children }: ComponentPropsWithoutRef<'details'> & { node?: unknown }) {
  return <details className="markdown-details" open={open}>{children}</details>;
}

export function MarkdownSummary({ children }: ComponentPropsWithoutRef<'summary'> & { node?: unknown }) {
  return <summary className="markdown-summary">{children}</summary>;
}
