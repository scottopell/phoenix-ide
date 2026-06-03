// Slot-level transition obligations for the unified viewer slot
// (specs/viewer_slot). Prose persistence/restoration is covered through the
// adapter in FileExplorerContext.test.tsx; this file covers the discriminated
// union: open/close transitions, the structural single-slot mutex, malformed
// URL normalization, and the browser-session edges.

import { describe, it, expect, beforeEach } from 'vitest';
import { render, act, waitFor } from '@testing-library/react';
import { useEffect, useState } from 'react';
import { MemoryRouter, Routes, Route, useLocation } from 'react-router-dom';
import { ViewerSlotProvider, useViewerSlot } from './ViewerSlotContext';
import type { ViewerSlotValue } from './ViewerSlotContext';

beforeEach(() => { localStorage.clear(); });

function Capture({ onCtx }: { onCtx: (ctx: ViewerSlotValue) => void }) {
  const ctx = useViewerSlot();
  useEffect(() => { onCtx(ctx); }, [ctx, onCtx]);
  return null;
}

function LocationCapture({ onSearch }: { onSearch: (s: string) => void }) {
  const location = useLocation();
  useEffect(() => { onSearch(location.search); }, [location.search, onSearch]);
  return null;
}

function renderSlot(initialEntry = '/c/conv-A', browserSessionActive = false) {
  let latest: ViewerSlotValue | null = null;
  let search = '';
  render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <Routes>
        <Route
          path="/c/:slug"
          element={
            <ViewerSlotProvider scopeKey="conv-A" browserSessionActive={browserSessionActive}>
              <Capture onCtx={(c) => { latest = c; }} />
              <LocationCapture onSearch={(s) => { search = s; }} />
            </ViewerSlotProvider>
          }
        />
      </Routes>
    </MemoryRouter>,
  );
  return { get: () => latest!, search: () => search };
}

describe('ViewerSlot — open/close transitions', () => {
  it('opens prose, diff, and browser, and closes back to none', () => {
    const h = renderSlot();
    expect(h.get().slot.kind).toBe('none');

    act(() => { h.get().openProse('/repo/a.ts', '/repo'); });
    expect(h.get().slot.kind).toBe('prose');
    expect(h.search()).toContain('viewer=prose');

    act(() => { h.get().openDiff('pane'); });
    expect(h.get().slot).toEqual({ kind: 'diff', presentation: 'pane' });
    expect(h.search()).toContain('viewer=diff');
    expect(h.search()).toContain('presentation=pane');

    act(() => { h.get().openBrowser(); });
    expect(h.get().slot.kind).toBe('browser');
    expect(h.search()).toContain('viewer=browser');

    act(() => { h.get().close(); });
    expect(h.get().slot.kind).toBe('none');
    expect(h.search()).not.toContain('viewer=');
  });
});

describe('ViewerSlot — structural single-slot mutex', () => {
  it('opening diff while prose is open removes the file/root params', () => {
    const h = renderSlot();
    act(() => { h.get().openProse('/repo/a.ts', '/repo'); });
    expect(h.search()).toContain('file=');

    act(() => { h.get().openDiffFullscreen(); });
    expect(h.get().slot).toEqual({ kind: 'diff', presentation: 'fullscreen' });
    expect(h.search()).toContain('viewer=diff');
    expect(h.search()).toContain('presentation=fullscreen');
    expect(h.search()).not.toContain('file=');
    expect(h.search()).not.toContain('root=');
  });

  it('opening prose while diff is open sets the file/root params', () => {
    const h = renderSlot();
    act(() => { h.get().openDiff('pane'); });
    act(() => { h.get().openProse('/repo/b.ts', '/repo'); });
    expect(h.get().slot.kind).toBe('prose');
    expect(h.search()).toContain('viewer=prose');
    expect(h.search()).toContain('file=%2Frepo%2Fb.ts');
  });
});

describe('ViewerSlot — malformed URL normalization (REQ-VS-012)', () => {
  it('normalizes viewer=prose without a file to none', () => {
    const h = renderSlot('/c/conv-A?viewer=prose');
    expect(h.get().slot.kind).toBe('none');
    expect(h.search()).not.toContain('viewer=');
  });

  it('normalizes an unknown viewer value to none', () => {
    const h = renderSlot('/c/conv-A?viewer=bogus');
    expect(h.get().slot.kind).toBe('none');
    expect(h.search()).not.toContain('viewer=');
  });

  it('parses fullscreen diff URLs and normalizes missing diff presentation to none', async () => {
    const fullscreen = renderSlot('/c/conv-A?viewer=diff&presentation=fullscreen');
    expect(fullscreen.get().slot).toEqual({ kind: 'diff', presentation: 'fullscreen' });

    const malformed = renderSlot('/c/conv-A?viewer=diff');
    expect(malformed.get().slot.kind).toBe('none');
    await waitFor(() => {
      expect(malformed.search()).not.toContain('viewer=');
      expect(malformed.search()).not.toContain('presentation=');
    });
  });

  it('removes the diff presentation param when opening prose or browser', () => {
    const h = renderSlot();
    act(() => { h.get().openDiffFullscreen(); });
    expect(h.search()).toContain('presentation=fullscreen');

    act(() => { h.get().openProse('/repo/a.ts', '/repo'); });
    expect(h.search()).toContain('viewer=prose');
    expect(h.search()).not.toContain('presentation=');

    act(() => { h.get().openDiffFullscreen(); });
    act(() => { h.get().openBrowser(); });
    expect(h.search()).toContain('viewer=browser');
    expect(h.search()).not.toContain('presentation=');
  });
});

describe('ViewerSlot — browser-session edges (REQ-VS-008/009)', () => {
  function renderWithFlag() {
    let latest: ViewerSlotValue | null = null;
    let setActive: ((v: boolean) => void) | null = null;
    function Harness() {
      const [active, setActiveState] = useState(false);
      setActive = setActiveState;
      return (
        <ViewerSlotProvider scopeKey="conv-A" browserSessionActive={active}>
          <Capture onCtx={(c) => { latest = c; }} />
        </ViewerSlotProvider>
      );
    }
    render(
      <MemoryRouter initialEntries={['/c/conv-A']}>
        <Routes>
          <Route path="/c/:slug" element={<Harness />} />
        </Routes>
      </MemoryRouter>,
    );
    return { get: () => latest!, setActive: (v: boolean) => setActive!(v) };
  }

  it('rising edge auto-opens the browser viewer when the slot is empty', () => {
    const h = renderWithFlag();
    expect(h.get().slot.kind).toBe('none');
    act(() => { h.setActive(true); });
    expect(h.get().slot.kind).toBe('browser');
  });

  it('rising edge does not steal an open prose viewer', () => {
    const h = renderWithFlag();
    act(() => { h.get().openProse('/repo/a.ts', '/repo'); });
    act(() => { h.setActive(true); });
    expect(h.get().slot.kind).toBe('prose');
  });

  it('falling edge auto-closes the browser viewer', () => {
    const h = renderWithFlag();
    act(() => { h.setActive(true); });
    expect(h.get().slot.kind).toBe('browser');
    act(() => { h.setActive(false); });
    expect(h.get().slot.kind).toBe('none');
  });

  it('entering a conversation whose session is already active is NOT a rising edge', () => {
    // The provider stays mounted across conversation switches; only scopeKey +
    // browserSessionActive change. A scope change must reseed the edge tracker,
    // so entering a conversation that already had an active session does not
    // auto-open the browser (only a session that *just* started does).
    let latest: ViewerSlotValue | null = null;
    let setScope: ((s: string) => void) | null = null;
    let setActive: ((v: boolean) => void) | null = null;
    function Harness() {
      const [scope, setScopeState] = useState('conv-A');
      const [active, setActiveState] = useState(false);
      setScope = setScopeState;
      setActive = setActiveState;
      return (
        <ViewerSlotProvider scopeKey={scope} browserSessionActive={active}>
          <Capture onCtx={(c) => { latest = c; }} />
        </ViewerSlotProvider>
      );
    }
    render(
      <MemoryRouter initialEntries={['/c/conv-A']}>
        <Routes>
          <Route path="/c/:slug" element={<Harness />} />
        </Routes>
      </MemoryRouter>,
    );
    expect(latest!.slot.kind).toBe('none');
    // Switch to a different conversation that already has an active session.
    act(() => { setScope!('conv-B'); setActive!(true); });
    expect(latest!.slot.kind).toBe('none');
  });
});
