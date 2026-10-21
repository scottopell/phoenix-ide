import { describe, expect, it } from 'vitest';
import { decodedRouteReference, transcriptReferenceHref, transcriptSelector } from './transcriptReference';

describe('exact transcript transport', () => {
  it.each(['café', 'caf%C3%A9', '%', '%25', '[x]', '%5Bx%5D', '%2F', '%2f', '/', '.', '..', '../x', 'javascript:alert(1)', 'x?source_transcript=other&x=y', '\\host\\path', '日本語'])('round-trips opaque %s through WHATWG URL parsing without treating it as syntax', id => {
    for (const suffix of ['', `#message-${id}`]) {
      const href = transcriptReferenceHref(`@transcript:${id}${suffix}`)!;
      const url = new URL(href, 'https://phoenix.test/origin');
      expect(url.pathname).toBe('/transcript');
      expect(transcriptSelector(url.search)).toEqual({ kind: 'transcript', id });
      expect(url.searchParams.getAll('source_transcript')).toEqual([id]);
      if (suffix) expect(decodeURIComponent(url.hash.slice('#message-'.length))).toBe(id);
    }
  });

  it.each(['', 'row space', 'row\nspace', 'row\n', 'row\r', 'row\u2028', 'row\u0085end', 'row#other', 'row#message-', 'row#message-bad#extra', 'row#message-id\n', '\ud800'])('does not promote malformed parsed reference %j', id => {
    expect(transcriptReferenceHref(`@transcript:${id}`)).toBeUndefined();
  });

  it.each([
    '?source_transcript=', '?source_transcript=a&source_transcript=b',
    '?source_transcript=a&source%5Ftranscript=a', '?source_transcript=has+space',
    '?source_transcript=has%23hash', '?source_transcript=has%C2%85space', '?source_transcript=%', '?source_transcript=%ZZ',
    '?source_transcript=%FF', '?source_transcript=%ED%A0%80', '?source_transcript=%E0%A4',
  ])('rejects invalid selector encoding/cardinality/shape: %s', search => {
    expect(transcriptSelector(search)).toEqual({ kind: 'invalid' });
  });

  it('distinguishes absence from an invalid exact selection', () => {
    expect(transcriptSelector('?viewer=inspect')).toEqual({ kind: 'absent' });
  });

  it.each(['café', 'caf%C3%A9', 'old%2Frow', 'old%2frow', 'row/other', 'row?x=1', '%', '[x]'])('decodes a raw encoded path component exactly once: %s', id => {
    expect(decodedRouteReference(`/c/${encodeURIComponent(id)}`)).toBe(id);
  });

  it('characterizes why even percent-encoded dots cannot be path IDs', () => {
    expect(new URL('/c/%2E', 'https://phoenix.test').pathname).toBe('/c/');
    expect(new URL('/c/%2E%2E', 'https://phoenix.test').pathname).toBe('/');
    expect(decodedRouteReference('/c/%ZZ')).toBeUndefined();
  });
});
