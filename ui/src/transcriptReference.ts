export function isOpaqueReferenceId(value: string): boolean {
  if (!value || /[\s#\p{White_Space}]/u.test(value)) return false;
  try {
    encodeURIComponent(value);
    return true;
  } catch {
    return false;
  }
}

export type TranscriptSelector =
  | { kind: 'absent' }
  | { kind: 'invalid' }
  | { kind: 'transcript'; id: string };

export function transcriptSelector(search: string): TranscriptSelector {
  const query = new URLSearchParams(search);
  const pins = query.getAll('source_transcript');
  if (pins.length === 0) return { kind: 'absent' };
  const id = pins[0];
  if (pins.length !== 1 || !id || !isOpaqueReferenceId(id)) return { kind: 'invalid' };
  // URLSearchParams replaces malformed UTF-8; exact evidence must not inherit that replacement.
  try {
    for (const pair of search.replace(/^\?/, '').split('&')) {
      const equal = pair.indexOf('=');
      if (new URLSearchParams(pair).has('source_transcript')) {
        decodeURIComponent((equal < 0 ? '' : pair.slice(equal + 1)).replace(/\+/g, ' '));
      }
    }
  } catch {
    return { kind: 'invalid' };
  }
  return { kind: 'transcript', id };
}

export function transcriptReferenceHref(reference: string): string | undefined {
  const match = /^@transcript:([^\s#]+)(?:#message-([^\s#]+))?$/u.exec(reference);
  if (!match?.[1] || match[0] !== reference || !isOpaqueReferenceId(match[1]) || (match[2] !== undefined && !isOpaqueReferenceId(match[2]))) return undefined;
  return `/transcript?source_transcript=${encodeURIComponent(match[1])}${match[2] === undefined ? '' : `#message-${encodeURIComponent(match[2])}`}`;
}

export function decodedRouteReference(pathname: string): string | undefined {
  const component = pathname.replace(/\/$/, '').split('/').at(-1);
  try {
    return component ? decodeURIComponent(component) : undefined;
  } catch {
    return undefined;
  }
}
