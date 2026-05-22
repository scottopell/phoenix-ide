// Per-conversation last-viewer URL snapshot storage (REQ-VS-014).
//
// Stores the URL search-params string for each conversation slug so that
// in-app navigation back to a conversation can restore the viewer the user
// had open. Keyed by slug because the restore site has the slug from
// useParams without an extra slug→id lookup. Slugs are stable identifiers.
//
// Cold reload deliberately does NOT consult this storage (REQ-VS-014 D1) --
// the URL is authoritative on cold reload. The provider gates restore on
// useLocation().key !== 'default'.
//
// Designed string-in / string-out so future ?viewer=diff or ?viewer=browser
// shapes round-trip without changes here -- the helper does not mirror the
// URL grammar.

const KEY_PREFIX = 'phoenix:lastviewer:';

export function getLastViewer(slug: string): string | null {
  try {
    return localStorage.getItem(KEY_PREFIX + slug);
  } catch {
    return null;
  }
}

export function setLastViewer(slug: string, params: string): void {
  try {
    localStorage.setItem(KEY_PREFIX + slug, params);
  } catch {
    // Safari private mode / quota: silent failure matches useDraft.ts.
  }
}

export function clearLastViewer(slug: string): void {
  try {
    localStorage.removeItem(KEY_PREFIX + slug);
  } catch {
    // Same Safari/quota story.
  }
}
