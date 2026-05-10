// Unit tests for the per-conversation last-viewer storage helper
// (REQ-VS-014). The helper is a thin localStorage wrapper; the exhaustive
// behavioural coverage (write-on-open, restore-on-in-app-entry,
// no-restore-on-cold-reload, etc.) lives in FileExplorerContext.test.tsx.

import { describe, it, expect, beforeEach } from 'vitest';
import {
  clearLastViewer,
  getLastViewer,
  setLastViewer,
} from './lastViewerStorage';

describe('lastViewerStorage', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('round-trips set/get for a slug', () => {
    setLastViewer('conv-A', 'file=%2Ffoo&root=%2Frepo');
    expect(getLastViewer('conv-A')).toBe('file=%2Ffoo&root=%2Frepo');
  });

  it('returns null for an unknown slug', () => {
    expect(getLastViewer('never-set')).toBeNull();
  });

  it('clear removes the entry', () => {
    setLastViewer('conv-A', 'file=%2Ffoo&root=%2Frepo');
    clearLastViewer('conv-A');
    expect(getLastViewer('conv-A')).toBeNull();
  });

  it('keys are namespaced per slug (no cross-talk)', () => {
    setLastViewer('conv-A', 'file=%2Fa');
    setLastViewer('conv-B', 'file=%2Fb');
    expect(getLastViewer('conv-A')).toBe('file=%2Fa');
    expect(getLastViewer('conv-B')).toBe('file=%2Fb');
  });

  it('uses the spec-pinned key prefix', () => {
    setLastViewer('conv-A', 'file=%2Fa');
    expect(localStorage.getItem('phoenix:lastviewer:conv-A')).toBe('file=%2Fa');
  });

  // Note: the Safari-private-mode throw-on-access path is covered by the
  // try/catch in each helper. happy-dom installs the Storage methods such
  // that replacing them in-test is awkward (own-property assignments don't
  // intercept the helpers' calls), so we rely on code review for that
  // branch — the failure mode (helper throws → consumer crashes) would
  // surface immediately on Safari private mode.
});
