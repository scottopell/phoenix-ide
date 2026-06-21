import type { MetaViewerPayload } from '../../components/viewer/metaViewerTypes';

export type MetaViewerTheme = 'dark' | 'light';

/**
 * Scripted post-mount interaction needed to reach a scenario's edge state.
 * Most payload edge states need no interaction; a few are only reachable
 * through a header/body click (toggle preview, enter fullscreen, open the
 * notes panel, open the annotation dialog).
 */
export type MetaViewerInteraction =
  | 'none'
  | 'html-preview'
  | 'image-takeover'
  | 'open-notes'
  | 'open-annotation';

/**
 * Canonical scenario list — the single source the id union, the built
 * scenarios, the stories, and (transitively, via Ladle's manifest) the
 * screenshot capture set all derive from. Add/remove a scenario here only.
 *
 * MetaViewer is payload-in: the loader has already fetched and classified the
 * content. So most scenarios hand it a resolved `MetaViewerPayload` directly
 * (no fetch mock) and exercise a *rare branch* a developer will not hit by
 * opening a normal file. The happy path (markdown renders, png shows) is owned
 * by real files + unit tests and is deliberately NOT screenshotted here. Only
 * the two loader-level states (loading / error) mount the real loader behind a
 * mocked `fetch`.
 */
export const metaViewerScenarioDefinitions = [
  { id: 'large-text-fallback-dark', title: 'Large-file plain-text fallback', theme: 'dark', interaction: 'none' },
  { id: 'large-text-fallback-light', title: 'Large-file plain-text fallback / light', theme: 'light', interaction: 'none' },
  { id: 'patch-context-dark', title: 'Patch context: changed lines + banner', theme: 'dark', interaction: 'none' },
  { id: 'long-lines-text-dark', title: 'Plain text with lines past the viewport', theme: 'dark', interaction: 'none' },
  { id: 'long-lines-code-dark', title: 'Code with lines past the viewport', theme: 'dark', interaction: 'none' },
  { id: 'html-source-dark', title: 'HTML source mode', theme: 'dark', interaction: 'none' },
  { id: 'html-preview-dark', title: 'HTML sandboxed preview', theme: 'dark', interaction: 'html-preview' },
  { id: 'image-takeover-dark', title: 'Image fullscreen takeover', theme: 'dark', interaction: 'image-takeover' },
  { id: 'notes-panel-dark', title: 'Review notes panel populated', theme: 'dark', interaction: 'open-notes' },
  { id: 'annotation-dialog-dark', title: 'Annotation dialog open', theme: 'dark', interaction: 'open-annotation' },
  { id: 'loading-dark', title: 'Loader: loading spinner', theme: 'dark', interaction: 'none' },
  { id: 'error-dark', title: 'Loader: read error (cannot render)', theme: 'dark', interaction: 'none' },
] as const satisfies readonly {
  id: string;
  title: string;
  theme: MetaViewerTheme;
  interaction: MetaViewerInteraction;
}[];

export type MetaViewerScenarioId = (typeof metaViewerScenarioDefinitions)[number]['id'];

/** A review note seeded into the notes pile before capture (notes-panel scenario). */
export interface MetaViewerSeedNote {
  lineNumber: number;
  lineContent: string;
  body: string;
}

/** Loader-level state, rendered through the real `FileViewer` behind a mocked fetch. */
export interface MetaViewerLoaderState {
  state: 'loading' | 'error';
  filePath: string;
  rootDir: string;
}

export interface MetaViewerScenario {
  id: MetaViewerScenarioId;
  title: string;
  theme: MetaViewerTheme;
  interaction: MetaViewerInteraction;
  /**
   * DOM selector that, once present, means this scenario has reached its
   * settled edge state and is safe to screenshot. Mirrors grounding-panel's
   * settled-DOM readiness — never a wall-clock timer, since the interactive
   * scenarios open their target through an async click→render chain.
   */
  settleSelector: string;
  /** Resolved payload for the payload-in scenarios. Undefined for loader scenarios. */
  payload?: MetaViewerPayload;
  /** Loader scenario state; mutually exclusive with `payload`. */
  loader?: MetaViewerLoaderState;
  /** Notes seeded before capture (notes-panel scenario only). */
  seedNotes?: MetaViewerSeedNote[];
}
