export function assertTouchStreamOwner(input: string, base: string, liveId: string): void {
  const url = new URL(input, base);
  if (!liveId || url.origin !== new URL(base).origin
    || url.pathname !== '/api/conversation-reads/stream'
    || url.searchParams.getAll('reference').length !== 1
    || url.searchParams.get('reference') !== liveId) {
    const knownPath = ['/api/conversation-reads/stream', '/api/product-conversations/events'].includes(url.pathname)
      ? url.pathname : 'other';
    const names = [...new Set(url.searchParams.keys())].slice(0, 8).map(key =>
      ['reference', 'after_event_sequence', 'transcript_generation', 'open_id'].includes(key) ? key : 'other');
    const references = url.searchParams.getAll('reference');
    const reason = !liveId ? 'missing-expected-owner' : url.origin !== new URL(base).origin
      ? 'foreign-origin' : url.pathname !== '/api/conversation-reads/stream' ? 'wrong-channel'
      : references.length !== 1 ? 'reference-count' : 'reference-mismatch';
    throw new Error(`Unexpected touch fixture stream owner ${JSON.stringify({ reason,
      sameOrigin: url.origin === new URL(base).origin, path: knownPath, queryKeys: names,
      referenceCount: Math.min(references.length, 2), referenceMatches: references.length === 1 && references[0] === liveId,
      expectedSyntheticOwner: liveId === 'touch-reading-member' ? liveId : 'other' })}`);
  }
}
