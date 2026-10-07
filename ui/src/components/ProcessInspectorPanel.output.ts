/** UI scrollback bound on accumulated output entries (REQ-PINSP-003). */
export const MAX_OUTPUT_ENTRIES = 5000;

/** A rendered output entry: a ring line or a synthetic truncation marker. */
export type OutputEntry =
  | { kind: 'line'; offset: number; text: string }
  | { kind: 'gap'; id: number };

/** Append newly observed output and retain only the newest scrollback entries. */
export function accumulateOutputEntries(previous: OutputEntry[], incoming: OutputEntry[]): OutputEntry[] {
  if (incoming.length === 0) return previous;
  const merged = previous.length === 0 ? incoming : [...previous, ...incoming];
  return merged.length > MAX_OUTPUT_ENTRIES ? merged.slice(merged.length - MAX_OUTPUT_ENTRIES) : merged;
}
