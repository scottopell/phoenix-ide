import { useEffect, useRef, useState } from 'react';
import { ThemeContext } from '../../hooks/useTheme';
import { ReviewNotesProvider, useReviewNotes } from '../../contexts/ReviewNotesContext';
import { MetaViewer } from '../../components/viewer/MetaViewer';
import { FileViewer } from '../../components/FileViewer';
import '../../index.css';
import { installMetaViewerLoaderFetch } from './mockApi';
import type { MetaViewerInteraction, MetaViewerScenario, MetaViewerSeedNote } from './types';

interface Props {
  scenario: MetaViewerScenario;
}

const noop = () => {};

/** Pushes seeded notes into the review-notes pile once, on mount. */
function SeedReviewNotes({ notes, absolutePath }: { notes: MetaViewerSeedNote[]; absolutePath: string }) {
  const { addNote } = useReviewNotes();
  const seeded = useRef(false);
  useEffect(() => {
    if (seeded.current) return;
    seeded.current = true;
    for (const note of notes) {
      addNote({ kind: 'file', filePath: absolutePath, lineNumber: note.lineNumber }, note.lineContent, note.body);
    }
  }, [notes, absolutePath, addNote]);
  return null;
}

// Locate the element a scenario's scripted interaction must click to reach its
// edge state. Returns null until the element exists (the driver retries), which
// also covers the seed-notes → badge-appears render gap for `open-notes`.
function interactionTarget(interaction: MetaViewerInteraction): HTMLElement | null {
  switch (interaction) {
    case 'html-preview': {
      const buttons = [...document.querySelectorAll<HTMLButtonElement>('.viewer-shell-actions button')];
      return buttons.find((button) => button.textContent?.trim() === 'Preview') ?? null;
    }
    case 'image-takeover':
      return document.querySelector<HTMLElement>('button[aria-label="Open fullscreen image viewer"]');
    case 'open-notes':
      return document.querySelector<HTMLElement>('.viewer-shell-badge');
    case 'open-annotation':
      return document.querySelector<HTMLElement>('.annotatable__btn');
    case 'none':
      return null;
  }
}

export function MetaViewerFixture({ scenario }: Props) {
  const [installed, setInstalled] = useState(false);

  // Theme + (for loader scenarios) the fetch mock, set up before the viewer
  // mounts so it never sees a flash of the wrong theme or a real network call.
  useEffect(() => {
    setInstalled(false);
    delete document.documentElement.dataset['metaViewerFixtureReady'];
    document.documentElement.dataset['theme'] = scenario.theme;
    const restore = scenario.loader ? installMetaViewerLoaderFetch(scenario.loader) : noop;
    setInstalled(true);
    return restore;
  }, [scenario]);

  // Signal capture-readiness off settled DOM, never a wall-clock timer: the
  // interactive scenarios open their target through an async click→render chain.
  // Drive the scripted interaction (retrying until its target exists), then poll
  // for the settled selector. If it never arrives, mark ready anyway but warn
  // (non-fatal, so the miss is visible without failing the run).
  useEffect(() => {
    if (!installed) return undefined;
    let cancelled = false;
    let interacted = scenario.interaction === 'none';
    const deadline = Date.now() + 6000;
    let timer = 0;

    const markReady = () => {
      if (!cancelled) document.documentElement.dataset['metaViewerFixtureReady'] = scenario.id;
    };
    const tick = () => {
      if (cancelled) return;
      if (!interacted) {
        const target = interactionTarget(scenario.interaction);
        if (target) {
          target.click();
          interacted = true;
        }
      }
      if (interacted && document.querySelector(scenario.settleSelector)) return markReady();
      if (Date.now() >= deadline) {
        console.warn(`meta-viewer fixture "${scenario.id}" did not reach its settled state before deadline; capturing as-is`);
        return markReady();
      }
      timer = window.setTimeout(tick, 50);
    };
    timer = window.setTimeout(tick, 50);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      delete document.documentElement.dataset['metaViewerFixtureReady'];
    };
  }, [scenario, installed]);

  if (!installed) return null;

  return (
    <ThemeContext.Provider value={{ theme: scenario.theme, toggleTheme: noop }}>
      <ReviewNotesProvider scopeKey="meta-viewer-fixture">
        <main className="fixture-page" data-meta-viewer-fixture={scenario.id}>
          {scenario.loader ? (
            <FileViewer
              filePath={scenario.loader.filePath}
              rootDir={scenario.loader.rootDir}
              onClose={noop}
              onSendNotes={noop}
            />
          ) : (
            <>
              {scenario.seedNotes && scenario.payload && (
                <SeedReviewNotes notes={scenario.seedNotes} absolutePath={scenario.payload.absolutePath} />
              )}
              <MetaViewer payload={scenario.payload!} />
            </>
          )}
        </main>
      </ReviewNotesProvider>
    </ThemeContext.Provider>
  );
}
