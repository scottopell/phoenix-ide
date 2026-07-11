import { createContext, useCallback, useContext, useEffect, useMemo, useRef } from 'react';
import type { ReactNode } from 'react';
import { useLocation, useSearchParams } from 'react-router-dom';
import { useScopedState } from '../hooks/useScopedState';
import type { OpenFileOptions, PatchContext } from '../components/FileExplorer/fileExplorerTypes';
import { clearLastViewer, getLastViewer, setLastViewer } from '../storage/lastViewerStorage';

/**
 * The conversation viewer slot — one mutually-exclusive surface beside the chat.
 *
 * State is a discriminated union derived from the URL search params on every
 * render; the URL is the source of truth (see specs/viewer_slot/). This makes
 * "prose open without a file" or "two viewers at once" structurally
 * unrepresentable, replacing the three independent React contexts plus the
 * coordinating effects in ConversationPage with one type.
 *
 *   ?viewer=prose&file=<abs>&root=<abs>   → prose
 *   ?viewer=diff&presentation=<mode>      → diff   (payload fetched on mount,
 *                                                    keyed by conversation id)
 *   ?viewer=browser                       → browser
 *   (no ?viewer=)                          → none
 *
 * `patchContext` (modified-line highlights) rides in conversation-scoped state,
 * not the URL — it is patch provenance, not view identity, and a Set<number>
 * cannot be URL-encoded sensibly. It is lost on cold reload, which is the
 * correct trade: the file still opens, just without highlights.
 */

export type DiffPresentation = 'fullscreen' | 'pane';

export interface ProseFile {
  path: string;
  rootDir: string;
  focusLine?: number | undefined;
  focusEndLine?: number | undefined;
}

export type ViewerSlot =
  | { kind: 'none' }
  | { kind: 'prose'; file: ProseFile; patchContext: PatchContext | null }
  | { kind: 'diff'; presentation: DiffPresentation }
  | { kind: 'browser' }
  | { kind: 'inspect'; scopeKey: string; handleId: string }
  | { kind: 'message'; sequenceId: number };

/** The slot's imperative surface. Identity-stable across slot / browser-session
 *  changes, so a command-only consumer (a button that opens a viewer) does not
 *  re-render when the open viewer or the session flag changes. */
export interface ViewerSlotCommands {
  openProse: (path: string, rootDir: string, options?: OpenFileOptions) => void;
  openDiff: (presentation: DiffPresentation) => void;
  openDiffFullscreen: () => void;
  openBrowser: () => void;
  /** Open the process inspector on a single bash handle, addressed by its
   *  `(scope_key, handle_id)` pair (REQ-PINSP-007). */
  openInspect: (scopeKey: string, handleId: string) => void;
  /** Open a finalized chat message in the annotatable markdown viewer. */
  openMessage: (sequenceId: number) => void;
  close: () => void;
}

export interface ViewerSlotValue extends ViewerSlotCommands {
  slot: ViewerSlot;
  /** Server-authoritative live browser-session flag (from the atom). Gates the
   *  manual browser-open affordance and drives the auto-open/close edges. */
  browserSessionActive: boolean;
}

// Three contexts, not one, so a consumer subscribes only to the slice it reads:
// commands (stable), the slot union, and the browser-session flag are
// independent. A button that only calls `openDiffFullscreen` no longer
// re-renders when the open viewer or the session flag changes.
const ViewerSlotCommandsContext = createContext<ViewerSlotCommands | null>(null);
const ViewerSlotDataContext = createContext<ViewerSlot | null>(null);
const ViewerSlotBrowserActiveContext = createContext<boolean | null>(null);

const VIEWER_PARAM = 'viewer';
const DIFF_PRESENTATION_PARAM = 'presentation';
const FILE_PARAM = 'file';
const ROOT_PARAM = 'root';
const LINE_PARAM = 'line';
const END_LINE_PARAM = 'endLine';
const SCOPE_PARAM = 'scope';
const HANDLE_PARAM = 'handle';
const MESSAGE_PARAM = 'message';

/** The full set of slot-owned search params. Every transition clears all of
 *  them and writes only the ones its kind needs, so a stale param from a prior
 *  kind can never linger and produce a malformed cross-kind URL. */
const SLOT_PARAMS = [
  VIEWER_PARAM,
  DIFF_PRESENTATION_PARAM,
  FILE_PARAM,
  ROOT_PARAM,
  LINE_PARAM,
  END_LINE_PARAM,
  SCOPE_PARAM,
  HANDLE_PARAM,
  MESSAGE_PARAM,
] as const;

function clearSlotParams(next: URLSearchParams) {
  for (const p of SLOT_PARAMS) next.delete(p);
}

/** Sentinel for "no conversation entered yet" — distinct from any slug and from
 *  `undefined` (which is itself a valid scopeKey on the no-conversation route). */
const UNSET_SCOPE = Symbol('unset-scope');

interface DerivedSlot {
  slot: ViewerSlot;
  /** The URL describes an impossible slot (e.g. viewer=prose with no file).
   *  Normalized to none, with a corrective setSearchParams scheduled. */
  malformed: boolean;
}

function deriveSlot(
  searchParams: URLSearchParams,
  patchContext: PatchContext | null,
): DerivedSlot {
  const viewer = searchParams.get(VIEWER_PARAM);
  const presentation = searchParams.get(DIFF_PRESENTATION_PARAM);
  const file = searchParams.get(FILE_PARAM);
  const root = searchParams.get(ROOT_PARAM);

  // Back-compat: the prose-only URL contract (file+root, no ?viewer=) predates
  // the unified slot. Treat it as prose so stored snapshots and existing links
  // still resolve; opening prose rewrites it with an explicit ?viewer=prose.
  const effective = viewer ?? (file && root ? 'prose' : null);

  switch (effective) {
    case 'prose': {
      if (!file || !root) return { slot: { kind: 'none' }, malformed: true };
      const focusLine = parseFocusLineParam(searchParams.get(LINE_PARAM));
      const parsedEndLine = parseFocusLineParam(searchParams.get(END_LINE_PARAM));
      const focusEndLine = focusLine !== undefined && parsedEndLine !== undefined && parsedEndLine >= focusLine
        ? parsedEndLine
        : undefined;
      return {
        slot: { kind: 'prose', file: { path: file, rootDir: root, focusLine, focusEndLine }, patchContext },
        malformed: false,
      };
    }
    case 'diff':
      if (presentation !== 'fullscreen' && presentation !== 'pane') {
        return { slot: { kind: 'none' }, malformed: true };
      }
      return { slot: { kind: 'diff', presentation }, malformed: false };
    case 'browser':
      return { slot: { kind: 'browser' }, malformed: false };
    case 'inspect': {
      const scope = searchParams.get(SCOPE_PARAM);
      const handle = searchParams.get(HANDLE_PARAM);
      if (!scope || !handle) return { slot: { kind: 'none' }, malformed: true };
      return { slot: { kind: 'inspect', scopeKey: scope, handleId: handle }, malformed: false };
    }
    case 'message': {
      const sequenceId = parseMessageParam(searchParams.get(MESSAGE_PARAM));
      if (sequenceId === undefined) return { slot: { kind: 'none' }, malformed: true };
      return { slot: { kind: 'message', sequenceId }, malformed: false };
    }
    case null:
      return { slot: { kind: 'none' }, malformed: false };
    default:
      // Unknown ?viewer= value — normalize away.
      return { slot: { kind: 'none' }, malformed: true };
  }
}

function parseFocusLineParam(lineParam: string | null): number | undefined {
  if (!lineParam || !/^[1-9]\d*$/.test(lineParam)) return undefined;
  const lineNumber = Number(lineParam);
  return validFocusLine(lineNumber);
}

function validFocusLine(lineNumber: number): number | undefined {
  return Number.isSafeInteger(lineNumber) && lineNumber > 0 ? lineNumber : undefined;
}

function parseMessageParam(messageParam: string | null): number | undefined {
  if (!messageParam || !/^[1-9]\d*$/.test(messageParam)) return undefined;
  const sequenceId = Number(messageParam);
  return Number.isSafeInteger(sequenceId) && sequenceId > 0 ? sequenceId : undefined;
}

interface ViewerSlotProviderProps {
  children: ReactNode;
  /** Active conversation slug — scopes patchContext and per-conversation
   *  last-viewer storage. */
  scopeKey?: string | undefined;
  /** Server-authoritative live-session flag; pass false when no atom yet. */
  browserSessionActive: boolean;
}

export function ViewerSlotProvider({ children, scopeKey, browserSessionActive }: ViewerSlotProviderProps) {
  const [searchParams, setSearchParams] = useSearchParams();
  const location = useLocation();

  const [patchContext, setPatchContext] = useScopedState<PatchContext | null>(scopeKey, null);

  const { slot, malformed } = useMemo(
    () => deriveSlot(searchParams, patchContext),
    [searchParams, patchContext],
  );

  // Write-through helpers. All slot transitions are URL writes with replace:true
  // so a user clicking through several files doesn't accumulate history entries;
  // the slot kind is recomputed from the URL on the next render, never mutated
  // directly. Mutual exclusion is structural — one `viewer` value at a time.
  const writeUrl = useCallback(
    (mutate: (next: URLSearchParams) => void) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          mutate(next);
          return next;
        },
        { replace: true },
      );
    },
    [setSearchParams],
  );

  const openProse = useCallback(
    (path: string, rootDir: string, options?: OpenFileOptions) => {
      setPatchContext(options?.kind === 'patch' ? options.patchContext : null);
      writeUrl((next) => {
        clearSlotParams(next);
        next.set(VIEWER_PARAM, 'prose');
        next.set(FILE_PARAM, path);
        next.set(ROOT_PARAM, rootDir);
        const lineNumber = options?.kind === 'line' ? validFocusLine(options.lineNumber) : undefined;
        if (lineNumber !== undefined) next.set(LINE_PARAM, String(lineNumber));
        if (options?.kind === 'range') {
          const startLine = validFocusLine(options.startLine);
          const endLine = validFocusLine(options.endLine);
          if (startLine !== undefined && endLine !== undefined && endLine >= startLine) {
            next.set(LINE_PARAM, String(startLine));
            next.set(END_LINE_PARAM, String(endLine));
          }
        }
      });
    },
    [setPatchContext, writeUrl],
  );

  const openDiff = useCallback((presentation: DiffPresentation) => {
    setPatchContext(null);
    writeUrl((next) => {
      clearSlotParams(next);
      next.set(VIEWER_PARAM, 'diff');
      next.set(DIFF_PRESENTATION_PARAM, presentation);
    });
  }, [setPatchContext, writeUrl]);

  const openDiffFullscreen = useCallback(() => openDiff('fullscreen'), [openDiff]);

  const openBrowser = useCallback(() => {
    setPatchContext(null);
    writeUrl((next) => {
      clearSlotParams(next);
      next.set(VIEWER_PARAM, 'browser');
    });
  }, [setPatchContext, writeUrl]);

  const openInspect = useCallback((inspectScopeKey: string, handleId: string) => {
    setPatchContext(null);
    writeUrl((next) => {
      clearSlotParams(next);
      next.set(VIEWER_PARAM, 'inspect');
      next.set(SCOPE_PARAM, inspectScopeKey);
      next.set(HANDLE_PARAM, handleId);
    });
  }, [setPatchContext, writeUrl]);

  const openMessage = useCallback((sequenceId: number) => {
    const validSequenceId = Number.isSafeInteger(sequenceId) && sequenceId > 0 ? sequenceId : undefined;
    if (validSequenceId === undefined) return;
    setPatchContext(null);
    writeUrl((next) => {
      clearSlotParams(next);
      next.set(VIEWER_PARAM, 'message');
      next.set(MESSAGE_PARAM, String(validSequenceId));
    });
  }, [setPatchContext, writeUrl]);

  // Clear the URL to the empty slot. `clearStorage` distinguishes an explicit
  // user close (clears the last-viewer entry so navigating back doesn't reopen)
  // from a system-driven close like the browser-session falling edge (which
  // leaves storage intact — see REQ-VS-009 vs the user-close storage rule).
  const clearSlot = useCallback(
    (clearStorage: boolean) => {
      setPatchContext(null);
      writeUrl(clearSlotParams);
      if (clearStorage && scopeKey) clearLastViewer(scopeKey);
    },
    [setPatchContext, writeUrl, scopeKey],
  );

  const close = useCallback(() => clearSlot(true), [clearSlot]);

  // REQ-VS-012: normalize a malformed slot URL away (viewer=prose without a
  // file, or an unknown ?viewer= value).
  useEffect(() => {
    if (!malformed) return;
    writeUrl(clearSlotParams);
  }, [malformed, writeUrl]);

  // REQ-VS-014: persist the current viewer URL for this conversation whenever
  // the slot is non-empty. Depend on the serialized string (a primitive) so the
  // effect re-fires exactly when the URL changes, not on every render.
  const searchString = searchParams.toString();
  useEffect(() => {
    if (!scopeKey) return;
    if (slot.kind === 'none') return;
    setLastViewer(scopeKey, searchString);
  }, [scopeKey, slot.kind, searchString]);

  // REQ-VS-014: restore the last viewer on in-app *entry* to a conversation.
  // Entry is a scopeKey change (the user navigated to a different conversation),
  // NOT any in-conversation URL change — otherwise a programmatic URL clear
  // (malformed normalization, browser falling-edge close) would land on a bare
  // URL and immediately re-restore the stored viewer, undoing the close.
  //
  // Cold reload (location.key === 'default' on the initial SPA mount) is
  // excluded by design (D1): the URL is authoritative there. A URL that already
  // carries viewer params is left alone (browser back/forward, shared link).
  const enteredScopeRef = useRef<string | undefined | typeof UNSET_SCOPE>(UNSET_SCOPE);
  useEffect(() => {
    const isEntry = enteredScopeRef.current !== scopeKey;
    enteredScopeRef.current = scopeKey;
    if (!isEntry) return;
    if (!scopeKey) return;
    if (location.key === 'default') return;
    if (searchParams.has(VIEWER_PARAM) || searchParams.has(FILE_PARAM) || searchParams.has(ROOT_PARAM) || searchParams.has(DIFF_PRESENTATION_PARAM)) return;
    const stored = getLastViewer(scopeKey);
    if (!stored) return;
    setSearchParams(new URLSearchParams(stored), { replace: true });
  }, [scopeKey, location.key, searchParams, setSearchParams]);

  // REQ-VS-008 / REQ-VS-009: browser-session edges. Rising edge auto-opens the
  // browser viewer only when the slot is empty (never steals prose/diff);
  // falling edge auto-closes only when the browser viewer is showing, without
  // clearing storage (a system close, not a user close).
  //
  // The provider is mounted once in DesktopLayout and lives across conversation
  // switches, so the edge tracker is scoped: on a scopeKey change (conversation
  // entry) we reseed prevActiveRef to the new conversation's flag WITHOUT
  // firing. Entering a conversation whose session was already active is not a
  // rising edge (REQ-VS-008: the session must have *just* started); only a flag
  // change within the same conversation is a true edge. Without this reseed, the
  // prior conversation's flag would be misread as an edge on entry.
  const prevActiveRef = useRef(browserSessionActive);
  const edgeScopeRef = useRef(scopeKey);
  const slotKind = slot.kind;
  useEffect(() => {
    if (edgeScopeRef.current !== scopeKey) {
      edgeScopeRef.current = scopeKey;
      prevActiveRef.current = browserSessionActive;
      return;
    }
    const prev = prevActiveRef.current;
    prevActiveRef.current = browserSessionActive;
    if (!prev && browserSessionActive && slotKind === 'none') {
      openBrowser();
    } else if (prev && !browserSessionActive && slotKind === 'browser') {
      clearSlot(false);
    }
  }, [scopeKey, browserSessionActive, slotKind, openBrowser, clearSlot]);

  const commands = useMemo<ViewerSlotCommands>(
    () => ({ openProse, openDiff, openDiffFullscreen, openBrowser, openInspect, openMessage, close }),
    [openProse, openDiff, openDiffFullscreen, openBrowser, openInspect, openMessage, close],
  );

  return (
    <ViewerSlotCommandsContext.Provider value={commands}>
      <ViewerSlotDataContext.Provider value={slot}>
        <ViewerSlotBrowserActiveContext.Provider value={browserSessionActive}>
          {children}
        </ViewerSlotBrowserActiveContext.Provider>
      </ViewerSlotDataContext.Provider>
    </ViewerSlotCommandsContext.Provider>
  );
}

/* eslint-disable react-refresh/only-export-components -- hooks colocated with the provider */

/** Imperative slot commands only. Stable; never re-renders on slot/flag change. */
export function useViewerSlotCommands(): ViewerSlotCommands {
  const ctx = useContext(ViewerSlotCommandsContext);
  if (!ctx) {
    throw new Error('useViewerSlotCommands must be used inside <ViewerSlotProvider>.');
  }
  return ctx;
}

/** The current slot union. Re-renders only when the open viewer changes. */
export function useViewerSlotData(): ViewerSlot {
  const ctx = useContext(ViewerSlotDataContext);
  if (!ctx) {
    throw new Error('useViewerSlotData must be used inside <ViewerSlotProvider>.');
  }
  return ctx;
}

/** The live browser-session flag. Re-renders only when the flag changes. */
export function useBrowserSessionActive(): boolean {
  const ctx = useContext(ViewerSlotBrowserActiveContext);
  if (ctx === null) {
    throw new Error('useBrowserSessionActive must be used inside <ViewerSlotProvider>.');
  }
  return ctx;
}

/** Back-compat combiner for consumers that genuinely need the whole surface
 *  (e.g. `ConversationPage`). Subscribes to all three slices — prefer the
 *  narrow hooks above when a consumer reads only one. */
export function useViewerSlot(): ViewerSlotValue {
  const commands = useViewerSlotCommands();
  const slot = useViewerSlotData();
  const browserSessionActive = useBrowserSessionActive();
  return useMemo(
    () => ({ slot, browserSessionActive, ...commands }),
    [slot, browserSessionActive, commands],
  );
}
