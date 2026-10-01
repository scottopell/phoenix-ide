import React from 'react';

export type TableColumnKind = 'numeric' | 'atomic' | 'compact' | 'prose' | 'label';

const NUMERIC_VALUE = /^[+−-]?(?:\d[\d,.]*)(?:\s*[–—-]\s*\d[\d,.]*)?\s*(?:%|ms|s|KiB|MiB|GiB|rows?)?$/i;
const SHORT_ATOMIC_CODE = /^\S{1,12}$/;

export function classifyTableColumn(values: string[]): TableColumnKind {
  const normalized = values.map((value) => value.trim()).filter(Boolean);
  if (normalized.length === 0) return 'label';
  if (normalized.every((value) => NUMERIC_VALUE.test(value))) return 'numeric';
  if (normalized.every((value) => !/\s/.test(value) && value.length <= 24)) return 'atomic';
  if (normalized.every((value) => value.split(/\s+/).length <= 4 && value.length <= 28)) return 'compact';
  if (normalized.some((value) => value.split(/\s+/).length >= 8)) return 'prose';
  return 'label';
}

export function reactNodeText(node: React.ReactNode): string {
  return React.Children.toArray(node).map((child) => {
    if (typeof child === 'string' || typeof child === 'number') return String(child);
    return React.isValidElement<{ children?: React.ReactNode }>(child)
      ? reactNodeText(child.props.children)
      : '';
  }).join('');
}

export function inlineCodeTokenKind(children: React.ReactNode): 'short-atomic' | 'breakable' {
  return SHORT_ATOMIC_CODE.test(reactNodeText(children).trim()) ? 'short-atomic' : 'breakable';
}
